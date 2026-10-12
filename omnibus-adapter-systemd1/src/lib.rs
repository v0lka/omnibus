//! systemd1 adapter: service list, status, start/stop/restart, enable/disable.
//!
//! Native backend: org.freedesktop.systemd1 over the system D-Bus (zbus 5, tokio).
//! Native types never leave this crate — mapping to DTOs happens inside the adapter
//! (specs/architecture/spi-registry.md "DTO Discipline").
//!
//! Error mapping: D-Bus failures become [`ContractError::NativeFailure`] with the
//! original message preserved in the payload (specs/architecture/error-taxonomy.md).
//! The typed `code` is numeric by contract; the symbolic name is kept visible by
//! prefixing the payload: `unit-not-found`, `polkit`, `dbus`, `job-failed`,
//! `job-pending`, `state-read-timeout`. A transport-level D-Bus timeout maps to
//! [`ContractError::TimeoutError`], never to `NativeFailure`.
//!
//! Write-path timeout semantics: a lifecycle action dispatched to systemd is not
//! recalled by dropping the call. `apply` therefore waits for the job's
//! `JobRemoved` result; a job whose outcome never arrives within the settle
//! budget is cancelled best-effort and answered
//! [`ContractError::NativeFailure`] `job-pending` — the job may still be
//! running or have completed, so the result never claims success. Once the
//! `JobRemoved` result is observed, the job's outcome is known; if the unit
//! then still fails to leave its transient states within the settle budget,
//! the answer is again `job-pending` ("job done but unit still settling") —
//! the action provably succeeded and only the state read timed out. For
//! lifecycle actions a bare [`ContractError::TimeoutError`] from `apply`
//! therefore only ever means the dispatch round trip itself hit the transport
//! timeout: whether the request reached systemd is then unknown. Enable and
//! disable create no job; once the file action has returned, the change
//! provably happened, so a trailing state-read timeout there is answered
//! [`ContractError::NativeFailure`] `state-read-timeout` (the enable/disable
//! succeeded; only the resulting unit state is unknown) — a bare `TimeoutError`
//! from `apply` never describes an observed success. In all of these cases the
//! caller must re-check with `service status` before retrying.
//!
//! State honesty: `ServiceUnit.enabled` is the DTO tri-state — `Some(true)`
//! enabled, `Some(false)` disabled, `None` when the unit-file state does not
//! decide enablement (static, masked, indirect, generated, transient, linked).
//! `ServiceActionResult.active_state` is `"unknown"` when a successful
//! enable/disable left no loadable unit behind (no runtime state exists).
//!
//! R2 failure-injection binding: the suite sentinels from
//! `omnibus_contracts::suite` are bound here — a status call on
//! [`SENTINEL_SLOW`] surfaces as `TimeoutError`, an action on [`SENTINEL_POISON`]
//! surfaces as the `polkit` native failure, exactly as `suite::families::services`
//! asserts.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use omnibus_contracts::capabilities::{SERVICE_READ, SERVICE_WRITE};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::dto::services::{
    ServiceAction, ServiceActionRequest, ServiceActionResult, ServiceFilter, ServiceStatus,
    ServiceUnit,
};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::services::ServiceManager;
use omnibus_contracts::suite::families::{SENTINEL_POISON, SENTINEL_SLOW};
use omnibus_contracts::version::CONTRACT_VERSION;
use zbus::connection::Builder;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, proxy};
// zbus does not re-export a StreamExt; SignalStream is a plain futures Stream
// (same import as the xdg-portals adapter).
use futures_util::StreamExt;

/// `NativeFailure` code: the referenced unit does not exist on this system.
pub const NF_UNIT_NOT_FOUND: i32 = 1;
/// `NativeFailure` code: OS authorization (polkit) denied the action.
pub const NF_POLKIT: i32 = 2;
/// `NativeFailure` code: the D-Bus conversation with systemd1 itself failed.
pub const NF_DBUS: i32 = 3;
/// `NativeFailure` code: the job ran but the unit settled into `failed`.
pub const NF_JOB_FAILED: i32 = 4;
/// `NativeFailure` code: the lifecycle job's settled outcome could not be
/// reported — either the dispatched job had not settled when its budget
/// elapsed (outcome unknown: it may still be running or have completed), or
/// the job provably finished `done` but the unit was still settling when the
/// state-read budget elapsed.
pub const NF_JOB_PENDING: i32 = 5;

/// Per-call D-Bus timeout; also the harness budget for one status call. It
/// matches the normative 2 s D-Bus limit (topology-and-lifecycle.md, "Limits
/// and Retries").
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// Overall budget for `list` (a full unit listing plus one unit-file pass). It
/// sits below the core's `TOOL_CALL_BUDGET` (10 s, `omnibus-core/src/limits.rs`)
/// so the adapter's typed answer is reachable inside the tool budget instead of
/// the core dropping the future (quality-gates.md, "Security Tests" ladder).
const LIST_TIMEOUT: Duration = Duration::from_secs(8);
/// How long a lifecycle job may take to settle before we stop waiting, cancel
/// it best-effort, and report the outcome as indeterminate. The post-`done`
/// settle wait rides the remaining [`APPLY_TIMEOUT`] budget instead (see
/// `run_lifecycle_job`), so a job that settles late still gets its honest
/// inner answer inside the tool budget; this constant bounds the job-wait
/// phase alone.
const JOB_WAIT: Duration = Duration::from_secs(4);
/// Kept between the inner settle deadline and the apply backstop, so the
/// inner answer always wins the race against the wrapper by a strictly
/// positive margin.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);
/// Budget for one best-effort cleanup round trip (job cancellation, last-state
/// read) after the settle deadline.
const CANCEL_BUDGET: Duration = Duration::from_secs(1);
/// Poll interval while a job settles.
const JOB_POLL: Duration = Duration::from_millis(250);
/// Overall budget for `apply` (dispatch + settle + cleanup). Every inner phase
/// is budgeted from the ladder start: dispatch ≤ [`CALL_TIMEOUT`], job wait ≤
/// [`JOB_WAIT`], cleanup ≤ 2 x [`CANCEL_BUDGET`] (2 + 4 + 1 + 1 s), and the
/// post-`done` settle wait rides the remaining budget minus [`SETTLE_MARGIN`]
/// — so this wrapper stays a backstop that only fires when an inner phase
/// overran its own bound, and like every adapter budget it stays below the
/// core's `TOOL_CALL_BUDGET` (10 s): through the tool path the typed
/// `job-pending` answers are reachable, never the bare core timeout.
const APPLY_TIMEOUT: Duration = Duration::from_secs(9);

#[derive(Debug, Clone)]
pub struct Systemd1Adapter {
    inner: Arc<Systemd1Inner>,
}

#[derive(Debug)]
struct Systemd1Inner {
    conn: Connection,
}

/// The outcome of the settle wait: either the unit left its transient states
/// (the string is the settled active state) or the settle budget elapsed while
/// it was still transient (the string is the last observed transient state,
/// `"unknown"` when no poll ever answered).
enum Settle {
    Settled(String),
    StillSettling(String),
}

impl Systemd1Adapter {
    /// Connects to the system D-Bus and pins the systemd1 manager connection.
    /// Fails with a contract error when no system bus is reachable (headless or
    /// non-systemd machines) — callers treat that as "adapter absent".
    pub async fn connect() -> Result<Self, ContractError> {
        // Connection establishment is adapter presence: no system bus here is
        // "adapter absent" ([`ContractError::AdapterUnavailable`], like the
        // atspi/xdg-portals siblings); `native_from_dbus` stays for failures of
        // the systemd1 conversation itself.
        let conn = Builder::system()
            .map_err(|_| ContractError::AdapterUnavailable)?
            .method_timeout(CALL_TIMEOUT)
            .build()
            .await
            .map_err(|_| ContractError::AdapterUnavailable)?;
        Ok(Self {
            inner: Arc::new(Systemd1Inner { conn }),
        })
    }

    async fn manager(&self) -> Result<ManagerProxy<'_>, ContractError> {
        ManagerProxy::builder(&self.inner.conn)
            .build()
            .await
            .map_err(native_from_dbus)
    }

    /// The active state of one unit, fetched through the unit properties.
    async fn active_state_of(&self, unit: &str) -> Result<String, ContractError> {
        let path = self
            .manager()
            .await?
            .get_unit(unit.to_string())
            .await
            .map_err(native_from_dbus)?;
        let unit_proxy = UnitPropsProxy::builder(&self.inner.conn)
            .path(path.as_str())
            .map_err(native_from_dbus)?
            .build()
            .await
            .map_err(native_from_dbus)?;
        unit_proxy.active_state().await.map_err(native_from_dbus)
    }

    /// Maps a unit-file state ("enabled", "static", ...) to the DTO tri-state:
    /// `Some(true)` enabled, `Some(false)` disabled, `None` when the state does
    /// not decide enablement. Static, masked, indirect, generated, transient or
    /// linked units are not "disabled" — collapsing them into `false` would
    /// tell the agent a static unit is merely switchable off and a masked unit
    /// is merely switched off, which is exactly what `systemctl is-enabled`
    /// distinguishes.
    fn enabled_from_file_state(state: &str) -> Option<bool> {
        match state {
            "enabled" | "enabled-runtime" => Some(true),
            "disabled" => Some(false),
            _ => None,
        }
    }

    /// Waits until the unit leaves the transient states after a lifecycle job.
    /// A unit that systemd unloads while we poll (transient units vanish the
    /// moment they deactivate) reads as settled "inactive": the job already
    /// reported "done", so a missing unit is the goal state, not a failure.
    /// Every poll is bounded by the caller-provided deadline (the remaining
    /// apply budget on the post-`done` path), so the phase respects its
    /// arithmetic instead of overrunning to `JOB_WAIT + CALL_TIMEOUT` on one
    /// wedged read; a budget that elapses answers
    /// [`Settle::StillSettling`], never `TimeoutError` — the caller decides
    /// what a still-settling unit means (an unobserved job is indeterminate,
    /// a job observed `done` merely has an unread state).
    async fn wait_job_settled(
        &self,
        unit: &str,
        deadline: tokio::time::Instant,
    ) -> Result<Settle, ContractError> {
        let mut last = String::from("unknown");
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(Settle::StillSettling(last));
            }
            match tokio::time::timeout(deadline - now, self.active_state_of(unit)).await {
                // The settle budget fired mid-poll (the connection method
                // timer alone would have let this single read overrun it).
                Err(_elapsed) => return Ok(Settle::StillSettling(last)),
                Ok(Ok(state)) => {
                    if !matches!(state.as_str(), "activating" | "deactivating" | "reloading") {
                        return Ok(Settle::Settled(state));
                    }
                    last = state;
                }
                Ok(Err(ContractError::NativeFailure {
                    code: NF_UNIT_NOT_FOUND,
                    ..
                })) => return Ok(Settle::Settled(String::from("inactive"))),
                Ok(Err(e)) => return Err(e),
            }
            // The sleep is clipped to the deadline: a full unclipped JOB_POLL
            // taken in the last 250 ms before it would wake at or past the
            // apply backstop, so a job whose result was already observed
            // `done` would race the wrapper and lose the honest
            // `job done, unit still settling` payload to the indeterminate
            // backstop answer. Zero remaining fires immediately and the loop
            // head returns StillSettling.
            let now = tokio::time::Instant::now();
            let remaining = if deadline > now {
                deadline - now
            } else {
                Duration::ZERO
            };
            tokio::time::sleep(remaining.min(JOB_POLL)).await;
        }
    }

    /// Runs one lifecycle job (Start/Stop/Restart) to an honest outcome.
    ///
    /// The completion of the job — not the unit's momentary state alone —
    /// decides the result: the `JobRemoved` signal carries the job result, and
    /// only `done` counts as success. A job that outlives the settle budget is
    /// cancelled best-effort and answered `NativeFailure{job-pending}`: the job
    /// may still be running or have completed, so neither success nor a job
    /// failure may be claimed (a dropped future does not recall the call). A
    /// job observed `done` whose unit still fails to settle within the budget
    /// is answered `job-pending` as well ("job done but unit still settling")
    /// — no cancellation is attempted because the job already finished; only
    /// the settled state stayed unread.
    async fn run_lifecycle_job(
        &self,
        manager: &ManagerProxy<'_>,
        action: ServiceAction,
        unit: &str,
    ) -> Result<String, ContractError> {
        // The whole ladder is budgeted from this instant, so every phase
        // (dispatch, job wait, post-done settle) provably fits inside the
        // apply backstop and the honest inner handling decides the answer.
        let started = tokio::time::Instant::now();
        // Subscribe BEFORE the dispatch so the job's completion signal cannot
        // slip through between call and wait (same dance as the portals
        // adapter's request/response).
        let mut removed = manager
            .inner()
            .receive_signal("JobRemoved")
            .await
            .map_err(native_from_dbus)?;
        let (verb, job) = match action {
            ServiceAction::Start => (
                "start",
                manager
                    .start_unit(String::from(unit), String::from("replace"))
                    .await,
            ),
            ServiceAction::Stop => (
                "stop",
                manager
                    .stop_unit(String::from(unit), String::from("replace"))
                    .await,
            ),
            _ => (
                "restart",
                manager
                    .restart_unit(String::from(unit), String::from("replace"))
                    .await,
            ),
        };
        let job = job.map_err(native_from_dbus)?;
        let deadline = started + JOB_WAIT;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            match tokio::time::timeout(deadline - now, removed.next()).await {
                // The settle deadline fired: fall through to the pending path.
                Err(_elapsed) => break,
                Ok(None) => {
                    return Err(ContractError::NativeFailure {
                        code: NF_DBUS,
                        payload: String::from("dbus: the systemd1 JobRemoved signal stream closed"),
                    });
                }
                Ok(Some(msg)) => {
                    // JobRemoved(u id, o path, s unit, s result); jobs of other
                    // units are ignored.
                    let (_id, path, _job_unit, result): (u32, OwnedObjectPath, String, String) =
                        msg.body().deserialize().map_err(native_from_dbus)?;
                    if path.as_str() != job.as_str() {
                        continue;
                    }
                    if result != "done" {
                        return Err(ContractError::NativeFailure {
                            code: NF_JOB_FAILED,
                            payload: format!(
                                "job-failed: {verb} job for {unit} finished with result \"{result}\""
                            ),
                        });
                    }
                    // The job outcome is observed (`done`), so from here on
                    // nothing can honestly surface as bare `TimeoutError`:
                    // its documented meaning — "whether the request reached
                    // systemd is unknown" — is false once the result is in.
                    // A settle wait that elapses is a state-read miss on a
                    // provably succeeded action. The settle wait rides the
                    // remaining apply budget (not a fresh `JOB_WAIT`), so it
                    // answers strictly inside the backstop and this honest
                    // inner shape stays reachable even when the dispatch and
                    // job wait together consumed most of the budget.
                    let settle_deadline = started + APPLY_TIMEOUT - SETTLE_MARGIN;
                    let state = match self.wait_job_settled(unit, settle_deadline).await {
                        Ok(Settle::Settled(state)) => state,
                        Ok(Settle::StillSettling(last)) => {
                            return Err(ContractError::NativeFailure {
                                code: NF_JOB_PENDING,
                                payload: job_done_still_settling_payload(unit, &last),
                            });
                        }
                        // Defensive: with every poll bounded by the settle
                        // budget a transport timeout here belongs to a state
                        // read only; keep the same honest shape anyway. Any
                        // other state-read failure gets the same shape: with
                        // the job outcome observed `done` the action provably
                        // succeeded, so surfacing a bare contract error from
                        // the settle read would misreport a succeeded action
                        // as a failed request.
                        Err(_) => {
                            return Err(ContractError::NativeFailure {
                                code: NF_JOB_PENDING,
                                payload: job_done_still_settling_payload(unit, "unknown"),
                            });
                        }
                    };
                    if state == "failed" {
                        return Err(ContractError::NativeFailure {
                            code: NF_JOB_FAILED,
                            payload: format!("job-failed: unit {unit} settled into failed"),
                        });
                    }
                    return Ok(state);
                }
            }
        }
        // The job outlived its budget: cancel it best-effort and report the
        // outcome as unknown.
        let canceled =
            tokio::time::timeout(CANCEL_BUDGET, cancel_job(&self.inner.conn, job.as_str()))
                .await
                .unwrap_or(false);
        let last_state = tokio::time::timeout(CANCEL_BUDGET, self.active_state_of(unit))
            .await
            .ok()
            .and_then(|read| read.ok())
            .unwrap_or_else(|| String::from("unknown"));
        Err(ContractError::NativeFailure {
            code: NF_JOB_PENDING,
            payload: job_pending_payload(verb, unit, canceled, &last_state),
        })
    }
}

#[async_trait]
impl ServiceManager for Systemd1Adapter {
    fn report(&self) -> AdapterReport {
        AdapterReport {
            adapter: String::from("systemd1"),
            subsystem: String::from("services"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![fact(SERVICE_READ), fact(SERVICE_WRITE)],
            notes: Vec::new(),
        }
    }

    async fn list(&self, filter: ServiceFilter) -> Result<Vec<ServiceUnit>, ContractError> {
        let fut = async {
            let manager = self.manager().await?;
            let rows = manager.list_units().await.map_err(native_from_dbus)?;
            let enabled = enabled_map(&manager).await?;
            let mut units: Vec<ServiceUnit> = rows
                .into_iter()
                .map(|row| {
                    let enabled = enabled.get(&row.0).copied().flatten();
                    let unit_type = unit_type_of(&row.0);
                    ServiceUnit {
                        name: row.0,
                        unit_type,
                        description: row.1,
                        active_state: row.3,
                        sub_state: row.4,
                        enabled,
                    }
                })
                .collect();
            if filter.active_only == Some(true) {
                units.retain(|u| u.active_state == "active");
            }
            if let Some(pattern) = &filter.name_pattern {
                let needle = pattern.to_lowercase();
                units.retain(|u| u.name.to_lowercase().contains(&needle));
            }
            if let Some(limit) = filter.limit {
                units.truncate(limit as usize);
            }
            Ok(units)
        };
        tokio::time::timeout(LIST_TIMEOUT, fut)
            .await
            .unwrap_or(Err(ContractError::TimeoutError))
    }

    async fn status(&self, unit: String) -> Result<ServiceStatus, ContractError> {
        validate_unit_name(&unit)?;
        let name = normalize_unit(&unit);
        // R2 sentinel binding: the configured slow unit surfaces as TimeoutError.
        if name.starts_with(SENTINEL_SLOW) {
            return Err(ContractError::TimeoutError);
        }
        let fut = async {
            let manager = self.manager().await?;
            let path = manager
                .get_unit(name.clone())
                .await
                .map_err(native_from_dbus)?;
            let unit_proxy = UnitPropsProxy::builder(&self.inner.conn)
                .path(path.as_str())
                .map_err(native_from_dbus)?
                .build()
                .await
                .map_err(native_from_dbus)?;
            let load_state = unit_proxy.load_state().await.map_err(native_from_dbus)?;
            if load_state == "not-found" {
                return Err(ContractError::NativeFailure {
                    code: NF_UNIT_NOT_FOUND,
                    payload: format!("unit-not-found: unit {name} is not loaded"),
                });
            }
            let active_state = unit_proxy.active_state().await.map_err(native_from_dbus)?;
            let sub_state = unit_proxy.sub_state().await.map_err(native_from_dbus)?;
            let description = unit_proxy.description().await.map_err(native_from_dbus)?;
            let file_state = unit_proxy.unit_file_state().await.ok();
            let docs = unit_proxy.documentation().await.unwrap_or_default();
            let main_pid = {
                let service_proxy = ServicePropsProxy::builder(&self.inner.conn)
                    .path(path.as_str())
                    .map_err(native_from_dbus)?
                    .build()
                    .await
                    .map_err(native_from_dbus)?;
                service_proxy.main_pid().await.ok().filter(|pid| *pid > 0)
            };
            let since_epoch_ms = unit_proxy
                .active_enter_timestamp()
                .await
                .ok()
                .filter(|ts| *ts > 0)
                .map(|ts| (ts / 1_000) as i64);
            let canonical = unit_proxy.id().await.unwrap_or_else(|_| name.clone());
            Ok(ServiceStatus {
                unit: ServiceUnit {
                    name: canonical.clone(),
                    // The type follows the canonical name: the raw input may
                    // lack a suffix (status("sshd") resolves sshd.service), and
                    // the unit_type of the raw argument would then be empty.
                    unit_type: unit_type_of(&canonical),
                    description,
                    active_state,
                    sub_state,
                    enabled: file_state
                        .as_deref()
                        .and_then(Self::enabled_from_file_state),
                },
                main_pid,
                since_epoch_ms,
                docs,
            })
        };
        tokio::time::timeout(CALL_TIMEOUT, fut)
            .await
            .unwrap_or(Err(ContractError::TimeoutError))
    }

    async fn apply(
        &self,
        request: ServiceActionRequest,
    ) -> Result<ServiceActionResult, ContractError> {
        validate_unit_name(&request.unit)?;
        let name = normalize_unit(&request.unit);
        // R2 sentinel binding: the poison unit stands in for a polkit denial.
        if name.starts_with(SENTINEL_POISON) {
            return Err(ContractError::NativeFailure {
                code: NF_POLKIT,
                payload: String::from("polkit: action denied by suite poison sentinel"),
            });
        }
        let unit_note = name.clone();
        let action_note = request.action;
        let fut = async {
            let manager = self.manager().await?;
            let active_state = match request.action {
                ServiceAction::Enable | ServiceAction::Disable => {
                    if request.action == ServiceAction::Enable {
                        manager
                            .enable_unit_files(&[name.as_str()], false, false)
                            .await
                            .map_err(native_from_dbus)?;
                    } else {
                        manager
                            .disable_unit_files(&[name.as_str()], false)
                            .await
                            .map_err(native_from_dbus)?;
                    }
                    // The file action succeeded. The unit itself is usually not
                    // loaded yet (a bare `GetUnit` would answer NoSuchUnit), so
                    // `load_unit` makes the state read below real whenever a
                    // unit file exists at all. From here the request's fate is
                    // known — the change provably happened — so a transport
                    // timeout on these trailing reads must never surface as
                    // bare `TimeoutError` (its documented meaning, "whether
                    // the request reached systemd is unknown", would be
                    // false); it folds into a native failure that says the
                    // action succeeded and only the state read timed out.
                    let verb = format!("{:?}", request.action);
                    match manager
                        .load_unit(name.clone())
                        .await
                        .map_err(native_from_dbus)
                    {
                        Ok(_) => match self.active_state_of(&name).await {
                            Ok(active_state) => active_state,
                            Err(ContractError::TimeoutError) => {
                                return Err(ContractError::NativeFailure {
                                    code: NF_DBUS,
                                    payload: post_action_read_timed_out_payload(&verb, &name),
                                });
                            }
                            Err(e) => return Err(e),
                        },
                        Err(ContractError::TimeoutError) => {
                            return Err(ContractError::NativeFailure {
                                code: NF_DBUS,
                                payload: post_action_read_timed_out_payload(&verb, &name),
                            });
                        }
                        // No unit file behind the name (e.g. a disable of a
                        // file-less unit): the action succeeded and there is no
                        // runtime state to read — "unknown" is the documented
                        // marker, not a swallowed error.
                        Err(ContractError::NativeFailure {
                            code: NF_UNIT_NOT_FOUND,
                            ..
                        }) => String::from("unknown"),
                        Err(e) => return Err(e),
                    }
                }
                ServiceAction::Start | ServiceAction::Stop | ServiceAction::Restart => {
                    self.run_lifecycle_job(&manager, request.action, &name)
                        .await?
                }
            };
            Ok(ServiceActionResult {
                unit: name,
                action: request.action,
                active_state,
            })
        };
        tokio::time::timeout(APPLY_TIMEOUT, fut)
            .await
            .unwrap_or_else(|_| {
                // Backstop: the inner phases (dispatch, settle wait, cleanup)
                // each carry their own budgets and every poll is bounded by
                // its phase's remaining budget, so reaching this means the
                // call itself wedged past all of them. The outcome is unknown
                // — the request may have reached systemd — so for a lifecycle
                // action cancel whatever job it left behind, best-effort and
                // detached: blocking this answer on the very bus that just
                // wedged is what we cannot afford. Enable and disable create
                // no job, so nothing is cancelled there and the payload must
                // not claim otherwise.
                let lifecycle = matches!(
                    action_note,
                    ServiceAction::Start | ServiceAction::Stop | ServiceAction::Restart
                );
                if lifecycle {
                    tokio::spawn(best_effort_cancel_unit_job(
                        self.inner.conn.clone(),
                        unit_note.clone(),
                    ));
                }
                Err(ContractError::NativeFailure {
                    code: NF_JOB_PENDING,
                    payload: apply_backstop_payload(
                        &format!("{action_note:?}"),
                        &unit_note,
                        lifecycle,
                    ),
                })
            })
    }
}

fn fact(name: &str) -> CapabilityFact {
    CapabilityFact {
        name: String::from(name),
        ver: String::from(CONTRACT_VERSION),
        status: String::from("present"),
        value: None,
        note: None,
        remedy: None,
    }
}

/// Pure: the type suffix of a unit name ("sshd.service" -> "service").
fn unit_type_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| String::from(ext))
        .unwrap_or_default()
}

/// Pure: reject names that can never be valid systemd units; the rest is
/// normalized to carry a unit type suffix.
fn validate_unit_name(name: &str) -> Result<(), ContractError> {
    if name.is_empty()
        || name.contains('/')
        || name.chars().any(char::is_whitespace)
        || name.starts_with('.')
    {
        return Err(ContractError::Unsupported {
            reason: format!("invalid unit name: {name}"),
        });
    }
    Ok(())
}

/// Pure: units without a type suffix default to `.service` (systemd semantics).
fn normalize_unit(name: &str) -> String {
    if name.contains('.') {
        String::from(name)
    } else {
        format!("{name}.service")
    }
}

/// One call for the enabled/disabled map: `ListUnitFiles` returns every unit file
/// on disk with its state; we key by the file's base name.
async fn enabled_map(
    manager: &ManagerProxy<'_>,
) -> Result<HashMap<String, Option<bool>>, ContractError> {
    let files = manager.list_unit_files().await.map_err(native_from_dbus)?;
    Ok(files
        .into_iter()
        .map(|(path, state)| {
            let name = path.rsplit('/').next().unwrap_or(path.as_str()).to_string();
            (name, Systemd1Adapter::enabled_from_file_state(&state))
        })
        .collect())
}

/// Maps a D-Bus error onto the closed taxonomy, preserving the original text:
/// transport timeouts -> TimeoutError, typed method-error replies by their
/// D-Bus error NAME (NoSuchUnit -> unit-not-found, AccessDenied/PolicyKit ->
/// polkit), name-less variants by the message text as a last resort, rest ->
/// dbus.
fn native_from_dbus(err: zbus::Error) -> ContractError {
    if let zbus::Error::InputOutput(io) = &err
        && io.kind() == std::io::ErrorKind::TimedOut
    {
        // The zbus-level method timer (CALL_TIMEOUT) fired. Budget
        // exhaustion has its own taxonomy code with different retry
        // semantics (reads retry once on TimeoutError, never on
        // NativeFailure), so it must not masquerade as `NativeFailure`.
        return ContractError::TimeoutError;
    }
    // The D-Bus error NAME is the typed, stable identity of a method-error
    // reply; Display text is not a stable interface and is consulted only
    // when the variant carries no name at all (mirrors the atspi adapter).
    let (code, symbolic) = match &err {
        zbus::Error::MethodError(name, ..) => classify_error_name(name.as_str()),
        _ => {
            let text = err.to_string();
            if text.contains("org.freedesktop.systemd1.NoSuchUnit") {
                (NF_UNIT_NOT_FOUND, "unit-not-found")
            } else if text.contains("org.freedesktop.DBus.Error.AccessDenied")
                || text.contains("org.freedesktop.PolicyKit1")
            {
                (NF_POLKIT, "polkit")
            } else {
                (NF_DBUS, "dbus")
            }
        }
    };
    let text = err.to_string();
    ContractError::NativeFailure {
        code,
        payload: format!("{symbolic}: {text}"),
    }
}

/// Classifies a systemd1 method-error reply by its D-Bus error NAME (pure;
/// offline-tested). The NAME is the typed identity: a `NoSuchUnit` suffix
/// means the unit is gone; `AccessDenied` or any `PolicyKit1` name means the
/// policy layer refused. Everything else is plain `dbus`.
fn classify_error_name(name: &str) -> (i32, &'static str) {
    if name.ends_with("NoSuchUnit") {
        (NF_UNIT_NOT_FOUND, "unit-not-found")
    } else if name.contains("AccessDenied") || name.contains("PolicyKit1") {
        (NF_POLKIT, "polkit")
    } else {
        (NF_DBUS, "dbus")
    }
}

/// Best-effort cancellation of a still-pending systemd job. A job that finished
/// in the meantime answers with a D-Bus error — that race is exactly why the
/// outcome stays indeterminate, so the caller only learns whether cancellation
/// was requested, never whether the job succeeded.
async fn cancel_job(conn: &Connection, job_path: &str) -> bool {
    let Ok(builder) = JobProxy::builder(conn).path(job_path) else {
        return false;
    };
    match builder.build().await {
        Ok(job) => job.cancel().await.is_ok(),
        Err(_) => false,
    }
}

/// Best-effort cancel of whatever job a lifecycle action left pending on a
/// unit, fired detached from the `apply` backstop once that budget fired (at
/// that point the job path, if any was created, is no longer known locally).
/// Swallows every failure — the action's outcome is reported as unknown
/// either way — and bounds the whole attempt with [`CANCEL_BUDGET`] so the
/// detached task cannot linger on the wedged bus.
async fn best_effort_cancel_unit_job(conn: Connection, unit: String) {
    let attempt = async {
        let manager = ManagerProxy::builder(&conn).build().await.ok()?;
        // A unit systemd has not loaded carries no job; every other failure
        // here is equally terminal for a best-effort attempt.
        let path = manager.get_unit(unit).await.ok()?;
        let unit_proxy = UnitPropsProxy::builder(&conn)
            .path(path.as_str())
            .ok()?
            .build()
            .await
            .ok()?;
        // "/" is systemd's "no job pending" marker on Unit.Job.
        let job = unit_proxy.job().await.ok()?;
        if job.as_str() == "/" {
            return None;
        }
        let job_proxy = JobProxy::builder(&conn)
            .path(job.as_str())
            .ok()?
            .build()
            .await
            .ok()?;
        job_proxy.cancel().await.ok()
    };
    let _ = tokio::time::timeout(CANCEL_BUDGET, attempt).await;
}

/// The payload of an indeterminate job outcome: honest about what is known —
/// the job was dispatched, did not settle in time, cancellation was requested —
/// and about what is not: whether it will still complete. A caller that retries
/// blindly may double-apply the action.
fn job_pending_payload(verb: &str, unit: &str, cancel_requested: bool, last_state: &str) -> String {
    let cancel_note = if cancel_requested {
        "cancellation requested"
    } else {
        "the job already finished or could not be canceled"
    };
    format!(
        "job-pending: {verb} job for {unit} did not settle within {}s ({cancel_note}); \
         last observed state \"{last_state}\"; the job may still be running or have \
         completed — re-check with service status before retrying",
        JOB_WAIT.as_secs()
    )
}

/// The payload for a job whose `JobRemoved` result was observed `done` but
/// whose unit still failed to leave the transient states within the settle
/// budget: the action provably succeeded, only the settled state stayed
/// unread — never claim the request's fate is unknown, and never answer bare
/// `TimeoutError` here.
fn job_done_still_settling_payload(unit: &str, last_state: &str) -> String {
    format!("job-pending: job done but unit {unit} still settling (state: {last_state})")
}

/// The payload of the `apply` backstop (the whole call outlived every inner
/// budget, so the outcome is unknown), honest per action family: lifecycle
/// actions leave a possibly-pending job behind and have fired the detached
/// best-effort cancellation — only they may claim one. Enable/disable create
/// no job at all, so nothing was requested there and the payload must not say
/// otherwise.
fn apply_backstop_payload(verb: &str, unit: &str, cancel_requested: bool) -> String {
    let fate = if cancel_requested {
        "the request may have reached systemd and the job may still be running or have \
         completed; a best-effort cancellation has been requested"
    } else {
        "the request may have reached systemd and the file change may already be applied; \
         no cancellation was requested (enable and disable create no systemd job)"
    };
    format!(
        "job-pending: the {verb} request for {unit} exceeded its {}s budget; its outcome is \
         unknown — {fate}; re-check with service status before retrying",
        APPLY_TIMEOUT.as_secs()
    )
}

/// The payload for an enable/disable whose file action provably succeeded but
/// whose trailing state read hit the transport timeout: bare `TimeoutError` is
/// documented as "request fate unknown", which would be false here — the fate
/// is known (the change is applied), only the resulting unit state is not.
fn post_action_read_timed_out_payload(verb: &str, unit: &str) -> String {
    format!(
        "state-read-timeout: the {verb} of {unit} succeeded, but the post-action state read \
         hit the transport timeout — the change is applied and only the resulting unit state \
         is unknown; re-check with service status"
    )
}

// --- D-Bus proxies (zbus 5). D-Bus names are literals here, not capabilities;
// the R4-CON lint only flags capability-domain-shaped strings. ---

#[proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1",
    assume_defaults = true
)]
trait Manager {
    /// a(ssssssusao): name, description, load, active, sub, following, path, job id, job type, job path.
    fn list_units(&self) -> zbus::Result<Vec<UnitRow>>;
    /// a(ss): unit file path, file state.
    fn list_unit_files(&self) -> zbus::Result<Vec<(String, String)>>;
    fn get_unit(&self, name: String) -> zbus::Result<OwnedObjectPath>;
    /// Loads the unit config (no-op when already loaded) so a state read is
    /// possible for a unit that is not running yet.
    fn load_unit(&self, name: String) -> zbus::Result<OwnedObjectPath>;
    fn start_unit(&self, name: String, mode: String) -> zbus::Result<OwnedObjectPath>;
    fn stop_unit(&self, name: String, mode: String) -> zbus::Result<OwnedObjectPath>;
    fn restart_unit(&self, name: String, mode: String) -> zbus::Result<OwnedObjectPath>;
    /// Returns (a(sss) carried, a(sss) changed).
    fn enable_unit_files(
        &self,
        files: &[&str],
        runtime: bool,
        force: bool,
    ) -> zbus::Result<(Vec<(String, String, String)>, Vec<(String, String, String)>)>;
    fn disable_unit_files(
        &self,
        files: &[&str],
        runtime: bool,
    ) -> zbus::Result<Vec<(String, String, String)>>;
}

/// The row of `ListUnits`, positional per the systemd1 D-Bus interface.
type UnitRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    OwnedObjectPath,
    u32,
    String,
    OwnedObjectPath,
);

#[proxy(
    interface = "org.freedesktop.systemd1.Unit",
    default_service = "org.freedesktop.systemd1",
    assume_defaults = true
)]
trait UnitProps {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn load_state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn active_state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn sub_state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn description(&self) -> zbus::Result<String>;
    /// Microseconds since the epoch; 0 when the unit never activated.
    #[zbus(property)]
    fn active_enter_timestamp(&self) -> zbus::Result<u64>;
    #[zbus(property)]
    fn documentation(&self) -> zbus::Result<Vec<String>>;
    /// Absent for non-file units (transient, generated); callers tolerate errors.
    #[zbus(property)]
    fn unit_file_state(&self) -> zbus::Result<String>;
    /// The job currently pending on this unit; "/" when none. Read by the
    /// backstop's detached best-effort cancel.
    #[zbus(property)]
    fn job(&self) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.freedesktop.systemd1.Job",
    default_service = "org.freedesktop.systemd1",
    assume_defaults = true
)]
trait Job {
    /// Cancels this job; on an already-finished job the call errors, which the
    /// best-effort caller ignores.
    fn cancel(&self) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.systemd1.Service",
    default_service = "org.freedesktop.systemd1",
    assume_defaults = true
)]
trait ServiceProps {
    #[zbus(property)]
    fn main_pid(&self) -> zbus::Result<u32>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_name_validation_rejects_bad_names() {
        assert!(validate_unit_name("sshd.service").is_ok());
        assert!(validate_unit_name("sshd").is_ok());
        assert!(validate_unit_name("").is_err());
        assert!(validate_unit_name("a/b.service").is_err());
        assert!(validate_unit_name("two words.service").is_err());
        assert!(validate_unit_name(".hidden.service").is_err());
    }

    #[test]
    fn unit_normalization_defaults_to_service() {
        assert_eq!(normalize_unit("sshd"), "sshd.service");
        assert_eq!(normalize_unit("sshd.service"), "sshd.service");
        assert_eq!(normalize_unit("user@1000.timer"), "user@1000.timer");
    }

    #[test]
    fn unit_type_extraction() {
        assert_eq!(unit_type_of("sshd.service"), "service");
        assert_eq!(unit_type_of("fstrim.timer"), "timer");
        assert_eq!(unit_type_of("nosuffix"), "");
    }

    #[test]
    fn dbus_error_classification_keeps_the_original_text() {
        let not_found = native_from_dbus(zbus::Error::Unsupported);
        match not_found {
            ContractError::NativeFailure { code, payload } => {
                assert_eq!(code, NF_DBUS);
                assert!(payload.contains("dbus:"), "{payload}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// A fixture method-error reply carrying `name`: the offline stand-in for
    /// what the bus produces on the wire (same construction pattern as the
    /// atspi adapter's tests).
    fn method_error(name: &'static str) -> zbus::Error {
        let Ok(name) = zbus::names::OwnedErrorName::try_from(name) else {
            panic!("fixture error name must be a valid D-Bus error name");
        };
        let Ok(reply) = zbus::Message::method_call("/org/omnibus/systemd1/test", "Ping")
            .and_then(|builder| builder.build(&()))
        else {
            panic!("fixture reply message must build");
        };
        zbus::Error::MethodError(name, None, reply)
    }

    fn assert_native_code(err: ContractError, expected: i32) {
        match err {
            ContractError::NativeFailure { code, payload } => {
                assert_eq!(code, expected);
                assert!(!payload.is_empty());
            }
            other => panic!("expected NativeFailure, got {other:?}"),
        }
    }

    #[test]
    fn method_error_name_no_such_unit_maps_to_unit_not_found() {
        // The typed D-Bus error NAME is the identity: no reliance on the
        // Display text, which is not a stable interface.
        let err = native_from_dbus(method_error("org.freedesktop.systemd1.NoSuchUnit"));
        match err {
            ContractError::NativeFailure { code, payload } => {
                assert_eq!(code, NF_UNIT_NOT_FOUND);
                assert!(payload.contains("unit-not-found: "), "{payload}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn method_error_name_access_denied_maps_to_polkit() {
        let err = native_from_dbus(method_error("org.freedesktop.DBus.Error.AccessDenied"));
        assert_native_code(err, NF_POLKIT);
    }

    #[test]
    fn method_error_name_policykit1_maps_to_polkit() {
        let err = native_from_dbus(method_error("org.freedesktop.PolicyKit1.Error.Failed"));
        assert_native_code(err, NF_POLKIT);
    }

    #[test]
    fn method_error_name_unrecognized_maps_to_dbus() {
        // A method-error reply whose NAME matches nothing known stays plain
        // `dbus` — the name decides, not the surrounding text.
        let err = native_from_dbus(method_error("org.freedesktop.DBus.Error.ServiceUnknown"));
        assert_native_code(err, NF_DBUS);
    }

    #[test]
    fn nameless_variant_falls_back_to_the_text() {
        // Variants that carry no D-Bus error NAME keep the old text-contains
        // classification as a last resort (`Failure`'s Display is its payload).
        let err = native_from_dbus(zbus::Error::Failure(String::from(
            "unit org.freedesktop.systemd1.NoSuchUnit ...",
        )));
        assert_native_code(err, NF_UNIT_NOT_FOUND);
    }

    #[test]
    fn unit_file_state_maps_to_the_honest_tri_state() {
        assert_eq!(
            Systemd1Adapter::enabled_from_file_state("enabled"),
            Some(true)
        );
        assert_eq!(
            Systemd1Adapter::enabled_from_file_state("enabled-runtime"),
            Some(true)
        );
        assert_eq!(
            Systemd1Adapter::enabled_from_file_state("disabled"),
            Some(false)
        );
        // Static/masked/indirect/generated units are not "disabled": the file
        // state does not decide enablement, and the DTO's `None` says so.
        for undecided in [
            "static",
            "masked",
            "masked-runtime",
            "indirect",
            "generated",
            "transient",
            "alias",
        ] {
            assert_eq!(
                Systemd1Adapter::enabled_from_file_state(undecided),
                None,
                "{undecided}"
            );
        }
    }

    #[test]
    fn dbus_transport_timeout_maps_to_timeout_error() {
        // zbus surfaces its own method timer as an I/O error with TimedOut
        // kind; the taxonomy requires TimeoutError (retry semantics differ
        // from NativeFailure).
        let timeout = native_from_dbus(zbus::Error::InputOutput(std::sync::Arc::new(
            std::io::Error::from(std::io::ErrorKind::TimedOut),
        )));
        assert_eq!(timeout, ContractError::TimeoutError);
    }

    #[test]
    fn job_pending_payload_is_honest_about_the_unknown_outcome() {
        let payload = job_pending_payload("start", "demo.service", true, "activating");
        assert!(payload.contains("job-pending"), "{payload}");
        assert!(payload.contains("start job for demo.service"), "{payload}");
        assert!(payload.contains("cancellation requested"), "{payload}");
        assert!(payload.contains("may still be running"), "{payload}");
        assert!(
            payload.contains("re-check with service status"),
            "{payload}"
        );
        let uncancelled = job_pending_payload("stop", "demo.service", false, "unknown");
        assert!(
            uncancelled.contains("could not be canceled"),
            "{uncancelled}"
        );
        assert!(
            uncancelled.contains("may still be running"),
            "{uncancelled}"
        );
    }

    #[test]
    fn job_done_still_settling_payload_claims_the_action_succeeded() {
        let payload = job_done_still_settling_payload("demo.service", "activating");
        assert!(payload.contains("job-pending"), "{payload}");
        assert!(payload.contains("job done"), "{payload}");
        assert!(payload.contains("still settling"), "{payload}");
        assert!(payload.contains("(state: activating)"), "{payload}");
        // The job outcome was observed, so the unknown-outcome wording of the
        // unobserved path must not appear here.
        assert!(!payload.contains("unknown"), "{payload}");
    }

    #[test]
    fn apply_backstop_payload_is_family_specific() {
        // Lifecycle actions fire the detached best-effort cancellation, so
        // the payload may claim it was requested.
        let lifecycle = apply_backstop_payload("Start", "demo.service", true);
        assert!(lifecycle.contains("job-pending"), "{lifecycle}");
        assert!(
            lifecycle.contains("the Start request for demo.service"),
            "{lifecycle}"
        );
        assert!(
            lifecycle.contains("a best-effort cancellation has been requested"),
            "{lifecycle}"
        );
        assert!(lifecycle.contains("may still be running"), "{lifecycle}");
        assert!(
            lifecycle.contains("re-check with service status"),
            "{lifecycle}"
        );

        // Enable/disable create no job: no cancellation claim may appear,
        // and the payload must say so explicitly.
        let file_action = apply_backstop_payload("Enable", "demo.service", false);
        assert!(file_action.contains("job-pending"), "{file_action}");
        assert!(
            file_action.contains("the Enable request for demo.service"),
            "{file_action}"
        );
        assert!(
            !file_action.contains("cancellation has been requested"),
            "{file_action}"
        );
        assert!(
            file_action.contains("no cancellation was requested"),
            "{file_action}"
        );
        assert!(
            file_action.contains("may already be applied"),
            "{file_action}"
        );
        assert!(
            file_action.contains("re-check with service status"),
            "{file_action}"
        );
    }

    #[test]
    fn post_action_read_timeout_claims_the_action_succeeded() {
        let payload = post_action_read_timed_out_payload("Enable", "demo.service");
        assert!(payload.contains("state-read-timeout"), "{payload}");
        assert!(
            payload.contains("the Enable of demo.service succeeded"),
            "{payload}"
        );
        assert!(payload.contains("post-action state read"), "{payload}");
        // Bare `TimeoutError` means "request fate unknown"; an observed
        // success must never wear that meaning.
        assert!(payload.contains("change is applied"), "{payload}");
        assert!(
            payload.contains("re-check with service status"),
            "{payload}"
        );
    }

    /// Harness: connect or skip headless (no system bus) machines.
    async fn connect_or_skip() -> Option<Systemd1Adapter> {
        match Systemd1Adapter::connect().await {
            Ok(adapter) => Some(adapter),
            Err(e) => {
                eprintln!("skipping systemd1 suite: system bus unavailable ({e})");
                None
            }
        }
    }

    #[tokio::test]
    async fn contract_suite_on_the_live_backend() {
        let Some(adapter) = connect_or_skip().await else {
            return;
        };
        let report =
            omnibus_contracts::suite::services::services_suite(|| Box::new(adapter.clone())).await;
        assert!(report.is_clean(), "{report}");

        // Fixture gate: derive a known unit from this machine's own `list`
        // (journald first — present on every systemd machine — else any
        // active unit) so `services_suite_with_fixture` verifies real state.
        // Nothing derivable means this leg is skipped with a note, never
        // silently; the fixture-free suite above always runs.
        let known = match adapter
            .list(ServiceFilter {
                active_only: None,
                name_pattern: None,
                limit: None,
            })
            .await
        {
            Ok(units) => units
                .iter()
                .find(|u| u.name == "systemd-journald.service")
                .or_else(|| units.iter().find(|u| u.active_state == "active"))
                .map(|u| u.name.clone()),
            Err(e) => {
                eprintln!("skipping services_suite_with_fixture: list failed ({e})");
                None
            }
        };
        match known {
            Some(unit) => {
                let report = omnibus_contracts::suite::services::services_suite_with_fixture(
                    || Box::new(adapter.clone()),
                    omnibus_contracts::suite::services::ServicesFixture { known_unit: unit },
                )
                .await;
                assert!(report.is_clean(), "{report}");
            }
            None => eprintln!(
                "skipping services_suite_with_fixture: no active unit to derive a known \
                 unit from on this machine"
            ),
        }
    }

    #[tokio::test]
    async fn list_returns_a_real_fleet_on_this_machine() {
        let Some(adapter) = connect_or_skip().await else {
            return;
        };
        let units = adapter
            .list(ServiceFilter {
                active_only: None,
                name_pattern: None,
                limit: None,
            })
            .await
            .expect("list must succeed on the live backend");
        assert!(
            units.len() >= 100,
            "expected 100+ units on a real machine, got {}",
            units.len()
        );
        let limited = adapter
            .list(ServiceFilter {
                active_only: None,
                name_pattern: None,
                limit: Some(5),
            })
            .await
            .expect("limited list must succeed");
        assert_eq!(limited.len(), 5);
    }

    #[tokio::test]
    async fn status_suffix_less_input_still_reports_the_unit_type() {
        let Some(adapter) = connect_or_skip().await else {
            return;
        };
        // status("systemd-journald") resolves systemd-journald.service on the
        // bus; the DTO's unit_type must follow that canonical name, not the
        // suffix-less raw input (which would yield "").
        let st = adapter
            .status(String::from("systemd-journald"))
            .await
            .expect("journald exists on any systemd machine");
        assert_eq!(st.unit.name, "systemd-journald.service");
        assert_eq!(st.unit.unit_type, "service");
    }
}

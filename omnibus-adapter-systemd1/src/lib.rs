//! systemd1 adapter: service list, status, start/stop/restart, enable/disable.
//!
//! Native backend: org.freedesktop.systemd1 over the system D-Bus (zbus 5, tokio).
//! Native types never leave this crate — mapping to DTOs happens inside the adapter
//! (specs/architecture/spi-registry.md "DTO Discipline").
//!
//! Error mapping: D-Bus failures become [`ContractError::NativeFailure`] with the
//! original message preserved in the payload (specs/architecture/error-taxonomy.md).
//! The typed `code` is numeric by contract; the symbolic name is kept visible by
//! prefixing the payload: `unit-not-found`, `polkit`, `dbus`, `job-failed`.
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

/// `NativeFailure` code: the referenced unit does not exist on this system.
pub const NF_UNIT_NOT_FOUND: i32 = 1;
/// `NativeFailure` code: OS authorization (polkit) denied the action.
pub const NF_POLKIT: i32 = 2;
/// `NativeFailure` code: the D-Bus conversation with systemd1 itself failed.
pub const NF_DBUS: i32 = 3;
/// `NativeFailure` code: the job ran but the unit settled into `failed`.
pub const NF_JOB_FAILED: i32 = 4;

/// Per-call D-Bus timeout; also the harness budget for one status call.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Overall budget for `list` (a full unit listing plus one unit-file pass).
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a lifecycle job may take to settle before we surface `TimeoutError`.
const JOB_WAIT: Duration = Duration::from_secs(15);
/// Poll interval while a job settles.
const JOB_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct Systemd1Adapter {
    inner: Arc<Systemd1Inner>,
}

#[derive(Debug)]
struct Systemd1Inner {
    conn: Connection,
}

impl Systemd1Adapter {
    /// Connects to the system D-Bus and pins the systemd1 manager connection.
    /// Fails with a contract error when no system bus is reachable (headless or
    /// non-systemd machines) — callers treat that as "adapter absent".
    pub async fn connect() -> Result<Self, ContractError> {
        let conn = Builder::system()
            .map_err(native_from_dbus)?
            .method_timeout(CALL_TIMEOUT)
            .build()
            .await
            .map_err(native_from_dbus)?;
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

    /// Maps a unit-file state ("enabled", "static", ...) to the DTO tri-state.
    fn enabled_from_file_state(state: &str) -> Option<bool> {
        Some(state == "enabled" || state == "enabled-runtime")
    }

    /// Waits until the unit leaves the transient states after a lifecycle job.
    async fn wait_job_settled(&self, unit: &str) -> Result<String, ContractError> {
        let deadline = tokio::time::Instant::now() + JOB_WAIT;
        loop {
            let state = self.active_state_of(unit).await?;
            if !matches!(state.as_str(), "activating" | "deactivating" | "reloading") {
                return Ok(state);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ContractError::TimeoutError);
            }
            tokio::time::sleep(JOB_POLL).await;
        }
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
            Ok(ServiceStatus {
                unit: ServiceUnit {
                    name: unit_proxy.id().await.unwrap_or(name),
                    unit_type: unit_type_of(&unit),
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
        let fut = async {
            let manager = self.manager().await?;
            match request.action {
                ServiceAction::Start => {
                    manager
                        .start_unit(name.clone(), String::from("replace"))
                        .await
                        .map_err(native_from_dbus)?;
                }
                ServiceAction::Stop => {
                    manager
                        .stop_unit(name.clone(), String::from("replace"))
                        .await
                        .map_err(native_from_dbus)?;
                }
                ServiceAction::Restart => {
                    manager
                        .restart_unit(name.clone(), String::from("replace"))
                        .await
                        .map_err(native_from_dbus)?;
                }
                ServiceAction::Enable => {
                    manager
                        .enable_unit_files(&[name.as_str()], false, false)
                        .await
                        .map_err(native_from_dbus)?;
                }
                ServiceAction::Disable => {
                    manager
                        .disable_unit_files(&[name.as_str()], false)
                        .await
                        .map_err(native_from_dbus)?;
                }
            }
            let active_state = if matches!(
                request.action,
                ServiceAction::Enable | ServiceAction::Disable
            ) {
                self.active_state_of(&name).await.unwrap_or_default()
            } else {
                let state = self.wait_job_settled(&name).await?;
                if state == "failed" {
                    return Err(ContractError::NativeFailure {
                        code: NF_JOB_FAILED,
                        payload: format!("job-failed: unit {name} settled into failed"),
                    });
                }
                state
            };
            Ok(ServiceActionResult {
                unit: name,
                action: request.action,
                active_state,
            })
        };
        tokio::time::timeout(LIST_TIMEOUT, fut)
            .await
            .unwrap_or(Err(ContractError::TimeoutError))
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
/// NoSuchUnit -> unit-not-found, AccessDenied/PolicyKit -> polkit, rest -> dbus.
fn native_from_dbus(err: zbus::Error) -> ContractError {
    let text = err.to_string();
    let (code, symbolic) = if text.contains("org.freedesktop.systemd1.NoSuchUnit") {
        (NF_UNIT_NOT_FOUND, "unit-not-found")
    } else if text.contains("org.freedesktop.DBus.Error.AccessDenied")
        || text.contains("org.freedesktop.PolicyKit1")
    {
        (NF_POLKIT, "polkit")
    } else {
        (NF_DBUS, "dbus")
    };
    ContractError::NativeFailure {
        code,
        payload: format!("{symbolic}: {text}"),
    }
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
}

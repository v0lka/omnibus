//! The MCP tool registry (specs/architecture/tool-registry.md): 24 domain tools
//! plus the three core tools. Every tool lives in the core and calls the trait,
//! never the implementation. The list reflects the capability document of the
//! current session's fresh probe; a hidden tool called by name returns
//! `CapabilityRequired` with a doctor hint.

use omnibus_contracts::capabilities as cap;
use omnibus_contracts::dto::desktop::{InvokeRequest, SetValueRequest, TreeSelector};
use omnibus_contracts::dto::files::{FileReadRequest, FileWriteRequest, PathQuery};
use omnibus_contracts::dto::journal::{BootSelector, JournalQuery};
use omnibus_contracts::dto::kb::{KbPageRequest, KbQuery};
use omnibus_contracts::dto::packages::{PackageListRequest, PackageNameRequest};
use omnibus_contracts::dto::portals::{ClipboardContent, NotifyRequest, ScreenshotRequest};
use omnibus_contracts::dto::services::{ServiceActionRequest, ServiceFilter};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::schema::{DtoSchema, SchemaNode, json_schema};

use crate::error_map::{capability_for_tool, join_tool};
use crate::ladder::LadderState;
use crate::limits;
use crate::session::ELEMENT_TOKEN_MAP_CAP;

/// One registry row: name, description, generated schema, and the gating facts.
#[derive(Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: &'static str,
    pub args_schema: SchemaNode,
    /// Write tool: needs a grant on `grant_key` (and the write gate).
    pub grant_key: Option<&'static str>,
    /// Parameterized-scope tool: the argument carrying the canonical path.
    pub path_arg: Option<&'static str>,
}

impl ToolSpec {
    fn read(name: String, description: &'static str, args: SchemaNode) -> Self {
        Self {
            name,
            description,
            args_schema: args,
            grant_key: None,
            path_arg: None,
        }
    }

    fn write(
        name: String,
        description: &'static str,
        args: SchemaNode,
        grant_key: &'static str,
        path_arg: Option<&'static str>,
    ) -> Self {
        Self {
            name,
            description,
            args_schema: args,
            grant_key: Some(grant_key),
            path_arg,
        }
    }
}

/// The full normative registry in tool-registry.md order. Built once and cached
/// in a `OnceLock` (schemas included): a dispatch consults it several times per
/// call (gate, write classification, `scope_ref`), and rebuilding it per lookup
/// re-allocated the whole vector every time — the static copy makes lookups
/// allocation-free with identical behavior.
pub fn all_specs() -> &'static [ToolSpec] {
    static SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
    SPECS.get_or_init(build_specs)
}

/// The registry builder behind [`all_specs`] (runs exactly once per process).
fn build_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::read(
            join_tool("desktop", "tree"),
            "a11y tree of one application, role+name addressing",
            TreeSelector::schema(),
        ),
        ToolSpec::read(
            join_tool("desktop", "element"),
            "flat detail of one a11y node (attributes, states, actions)",
            omnibus_contracts::dto::desktop::ElementRequest::schema(),
        ),
        ToolSpec::write(
            join_tool("desktop", "set_value"),
            "typed write: set a value on an editable element",
            SetValueRequest::schema(),
            cap::DESKTOP_WRITE_TYPED,
            None,
        ),
        ToolSpec::write(
            join_tool("desktop", "invoke"),
            "typed write: invoke a named action (button press)",
            InvokeRequest::schema(),
            cap::DESKTOP_WRITE_TYPED,
            None,
        ),
        ToolSpec::read(
            String::from("clip_read"),
            "current clipboard content",
            serde_json::Value::Null.into_any(),
        ),
        ToolSpec::write(
            String::from("clip_write"),
            "replace the clipboard content (write grant required)",
            ClipboardContent::schema(),
            cap::PORTAL_CLIPBOARD,
            None,
        ),
        ToolSpec::read(
            String::from("screenshot"),
            "one screen capture as base64 image",
            ScreenshotRequest::schema(),
        ),
        ToolSpec::write(
            String::from("notify"),
            "desktop notification (write grant required)",
            NotifyRequest::schema(),
            cap::PORTAL_NOTIFY,
            None,
        ),
        ToolSpec::read(
            String::from("svc_list"),
            "systemd units matching the filter",
            ServiceFilter::schema(),
        ),
        ToolSpec::read(
            String::from("svc_status"),
            "one unit's status",
            omnibus_contracts::dto::services::ServiceNameRequest::schema(),
        ),
        ToolSpec::write(
            String::from("svc_start"),
            "start a unit (write grant required)",
            omnibus_contracts::dto::services::ServiceNameRequest::schema(),
            cap::SERVICE_WRITE,
            None,
        ),
        ToolSpec::write(
            String::from("svc_stop"),
            "stop a unit (write grant required)",
            omnibus_contracts::dto::services::ServiceNameRequest::schema(),
            cap::SERVICE_WRITE,
            None,
        ),
        ToolSpec::read(
            String::from("net_status"),
            "network devices, connectivity, wifi networks (read-only)",
            serde_json::Value::Null.into_any(),
        ),
        ToolSpec::read(
            String::from("journal_query"),
            "filtered journal query; past boots need the prev_boot capability",
            JournalQuery::schema(),
        ),
        ToolSpec::read(
            String::from("pkg_list"),
            "installed packages",
            PackageListRequest::schema(),
        ),
        ToolSpec::read(
            String::from("pkg_info"),
            "one package's metadata",
            PackageNameRequest::schema(),
        ),
        ToolSpec::write(
            String::from("pkg_install"),
            "install a package (write grant required)",
            PackageNameRequest::schema(),
            cap::PKG_WRITE,
            None,
        ),
        ToolSpec::write(
            String::from("pkg_remove"),
            "remove a package (write grant required)",
            PackageNameRequest::schema(),
            cap::PKG_WRITE,
            None,
        ),
        ToolSpec::write(
            String::from("pkg_undo"),
            "undo the last package action (hidden when undo is none)",
            PackageNameRequest::schema(),
            cap::PKG_UNDO,
            None,
        ),
        ToolSpec::read(
            String::from("fs_list"),
            "directory listing inside granted scopes",
            PathQuery::schema(),
        ),
        ToolSpec::read(
            String::from("fs_read"),
            "file content inside granted scopes",
            FileReadRequest::schema(),
        ),
        ToolSpec::write(
            String::from("fs_write"),
            "write a file inside the parameterized write scope",
            FileWriteRequest::schema(),
            cap::FILES_SCOPE_WRITE,
            Some("path"),
        ),
        ToolSpec::read(
            String::from("wiki_search"),
            "knowledge base search (platform docs)",
            KbQuery::schema(),
        ),
        ToolSpec::read(
            String::from("wiki_page"),
            "one knowledge base article",
            KbPageRequest::schema(),
        ),
        ToolSpec::read(
            String::from("capabilities"),
            "machine-readable truth about this machine",
            serde_json::Value::Null.into_any(),
        ),
        ToolSpec::read(
            String::from("session_info"),
            "session identity and effective limits",
            serde_json::Value::Null.into_any(),
        ),
        ToolSpec::read(
            String::from("doctor"),
            "diagnosis + ready commands (never mutates over MCP)",
            serde_json::Value::Null.into_any(),
        ),
    ]
}

trait SchemaFromValue {
    fn into_any(self) -> SchemaNode;
}

impl SchemaFromValue for serde_json::Value {
    fn into_any(self) -> SchemaNode {
        let _ = self;
        SchemaNode::Any
    }
}

/// The subsystem component a tool's floor check runs against. The systemd floor
/// gates only the services tools (platform-matrix.md keeps journal and package
/// reads below the floor); `journal` and `none` have no declared floor row, so
/// `floor_met` passes trivially for them.
fn component_for_tool(tool: &str) -> &'static str {
    if tool.starts_with("desktop") {
        "at-spi2"
    } else if tool.starts_with("svc_") {
        "systemd"
    } else if tool.starts_with("journal") {
        "journal"
    } else {
        // pkg_* and everything else: no declared floor row (trivially met).
        "none"
    }
}

/// Whether `journal_query { boot: previous }` may run this session.
///
/// The decision reads the adapter's `journal.prev_boot` fact status through
/// the ladder (capability-model.md): `present` and `degraded` run; an
/// ungranted `opt-in-required` — the volatile-journal shape the journald
/// adapter emits — refuses; a missing entry counts as an absent capability.
/// The grant that unlocks the opt-in rung is `journal.persistent`, the
/// remedy the adapter names in its fact.
fn prev_boot_usable(caps: &crate::caps::CapabilitiesState, persistent_granted: bool) -> bool {
    matches!(
        caps.ladder(
            cap::JOURNAL_PREV_BOOT,
            component_for_tool("journal_query"),
            persistent_granted,
        ),
        LadderState::Available | LadderState::Degraded
    )
}

/// Parses arguments into a DTO, mapping serde failures to `Unsupported` (the
/// schema is authoritative; a non-conforming call is an honest contract error).
pub(crate) fn parse_args<T: serde::de::DeserializeOwned>(
    arguments: &serde_json::Map<String, serde_json::Value>,
) -> Result<T, ContractError> {
    serde_json::from_value(serde_json::Value::Object(arguments.clone())).map_err(|e| {
        ContractError::Unsupported {
            reason: format!("arguments do not match the tool schema: {e}"),
        }
    })
}

/// Outcome of a dispatch: the JSON value for the agent plus the adapter that
/// served it (for the audit event).
pub struct DispatchOutcome {
    pub value: serde_json::Value,
    pub adapter: String,
}

/// The gating decision for one tool call.
pub enum GateDecision {
    /// Run it.
    Run,
    /// Hidden or opt-in-ungranted: `CapabilityRequired` with a doctor hint.
    Hidden,
    /// Below floor: the adapter is not activated.
    BelowFloor,
    /// Write without a grant.
    Denied,
}

/// Resolves the ladder + grant state for a tool against the capabilities state.
pub fn gate(core: &crate::Core, tool: &str) -> GateDecision {
    let Some(capability) = capability_for_tool(tool) else {
        return GateDecision::Run; // core tools are ungated
    };
    let component = component_for_tool(tool);
    match core
        .caps
        .ladder(capability, component, core.grants.has_grant(capability))
    {
        LadderState::Available | LadderState::Degraded => {
            if let Some(spec) = all_specs().iter().find(|s| s.name == tool)
                && let Some(grant_key) = spec.grant_key
                && !core.grants.has_grant(grant_key)
            {
                return GateDecision::Denied;
            }
            GateDecision::Run
        }
        LadderState::FloorUnsupported => GateDecision::BelowFloor,
        LadderState::Hidden | LadderState::OptInRequired => GateDecision::Hidden,
    }
}

/// Dispatches one tool call: gate → write gate/grants → budget/retry → adapter →
/// audit. Reads retry exactly once on timeout; writes never retry.
///
/// Reads run under the core's wall-clock budget (limits.rs), and the
/// semantics are honest, not cancellation: on elapse the dispatched future is
/// dropped at its next await point and a request already on the wire may
/// still complete natively — `TimeoutError` means "the core stopped waiting;
/// the outcome is unknown", which is the whole truth for a read, since a
/// read observes no native state.
///
/// Writes are deliberately NOT wrapped in this timer: every write adapter
/// bounds its own native work (alpm and fs-xdg 9 s, systemd1 `APPLY_TIMEOUT`
/// 9 s, atspi `NODE_BUDGET` 5 s, the xdg-portals write arm 10 s — all at or
/// below the core read budget), so exactly one timeout layer exists per call
/// — the layer that can observe the native call. The shape of its elapse
/// answer is adapter-defined: alpm, fs-xdg and systemd1 report a
/// payload-bearing indeterminate outcome (alpm `timeout-indeterminate`,
/// fs-xdg code -2, the systemd1 indeterminate-job payload); the atspi typed
/// writes and the portals write arm answer a plain `TimeoutError`, whose
/// honest reading is the same — the core stopped waiting and the native
/// outcome is unknown (topology-and-lifecycle.md, write-gate note). The
/// write gate is released when this function returns, so a later write may
/// run while an earlier native action is still settling; the adapters'
/// error payloads and the audit trail keep both facts correlatable.
pub async fn dispatch(
    core: &crate::Core,
    tool: &str,
    arguments: serde_json::Map<String, serde_json::Value>,
) -> Result<DispatchOutcome, ContractError> {
    // R5 audit is total: a refused call is still a call — gate denials are audited
    // with the denial reason before the typed error surfaces (audit.md, AC 1.3.3).
    let scope_ref = grant_scope_ref(core, tool);
    match gate(core, tool) {
        GateDecision::BelowFloor => {
            return Err(refuse(
                core,
                tool,
                &arguments,
                ContractError::FloorUnsupported,
                scope_ref,
            )
            .await);
        }
        GateDecision::Hidden => {
            return Err(refuse(
                core,
                tool,
                &arguments,
                ContractError::CapabilityRequired,
                scope_ref,
            )
            .await);
        }
        GateDecision::Denied => {
            return Err(refuse(
                core,
                tool,
                &arguments,
                ContractError::ScopeDenied,
                scope_ref,
            )
            .await);
        }
        GateDecision::Run => {}
    }

    // Scope decisions are a core responsibility (files.md): fs_write targets are
    // checked against the parameterized write grant, fs_list/fs_read against the
    // default read scope ($HOME). A call that omits or empties its `path`
    // argument violates the tool schema and answers `Unsupported` honestly —
    // `ScopeDenied` would imply a grant could repair an argument the call never
    // carried (the string schema carries no `minLength`, so the empty case is a
    // core-side policy choice, refused before canonicalization can turn it into
    // a misleading scope answer). These checks run BEFORE the write gate: a
    // schema-invalid or out-of-scope `fs_write` must neither serialize behind
    // another instance's write nor risk its honest code being overwritten by
    // the gate's `TimeoutError` under cross-process contention. The blocking
    // filesystem half (canonicalize, ancestor probing, symlinked scope roots)
    // runs in `spawn_blocking`: a wedged path (stale mount, FIFO) parks a
    // blocking-pool thread the runtime grows around — never an async worker
    // the timers need (the doctrine the fs-xdg adapter states). Refusals are
    // audited like any gate denial — an attempted out-of-scope access leaves a
    // trace (R5).
    if tool == "fs_write" {
        let checked = match fs_path_argument(tool, &arguments) {
            Err(error) => Err(error),
            Ok(raw) => {
                let scopes = core.scopes.clone();
                run_scope_check(move || {
                    scopes.canonical_write_target(&raw).map_or(
                        Err(ContractError::ScopeDenied),
                        |canonical| {
                            scopes.authorize_write(cap::FILES_SCOPE_WRITE, Some(&canonical))
                        },
                    )
                })
                .await
            }
        };
        if let Err(error) = checked {
            return Err(refuse(core, tool, &arguments, error, grant_scope_ref(core, tool)).await);
        }
    }
    if tool == "fs_list" || tool == "fs_read" {
        let checked = match fs_path_argument(tool, &arguments) {
            Err(error) => Err(error),
            Ok(raw) => {
                let scopes = core.scopes.clone();
                run_scope_check(move || {
                    scopes
                        .canonical_read_target(&raw)
                        .map_or(Err(ContractError::ScopeDenied), |canonical| {
                            scopes.authorize_read(&canonical)
                        })
                })
                .await
            }
        };
        if let Err(error) = checked {
            return Err(refuse(core, tool, &arguments, error, grant_scope_ref(core, tool)).await);
        }
    }

    // The prev-boot rung is a capability-ladder decision, not an adapter
    // outcome: on a volatile journal the adapter's prev_boot fact is always
    // emitted as `opt-in-required`, so a past-boot query must be refused
    // before dispatch — the honest answer names the doctor and the grant
    // remedy, never an empty page, and the journald adapter performs no rung
    // check of its own. The decision parses the DTO here and matches on the
    // ENUM: matching the raw wire string instead would silently stop firing
    // if the serde form ever drifted while the DTO parse kept succeeding, and
    // the refusal would vanish without any failing test. Malformed arguments
    // take the same honest `Unsupported` they get in the single dispatch.
    // The refusal is audited through `refuse` under `core`: the adapter was
    // never reached, and the attribution convention is the one every other
    // core-side refusal follows.
    if tool == "journal_query" {
        let parsed: Result<JournalQuery, ContractError> = parse_args(&arguments);
        if let Ok(query) = parsed
            && query.boot == Some(BootSelector::Previous)
            && !prev_boot_usable(&core.caps, core.grants.has_grant(cap::JOURNAL_PERSISTENT))
        {
            return Err(refuse(
                core,
                tool,
                &arguments,
                ContractError::CapabilityRequired,
                scope_ref,
            )
            .await);
        }
    }

    let is_write = all_specs()
        .iter()
        .find(|s| s.name == tool)
        .and_then(|s| s.grant_key)
        .is_some();

    let _permit = if is_write {
        // The gate bounds its own acquisition; a TimeoutError here (another
        // instance held the cross-process flock) is audited like any outcome —
        // under `core`, because the adapter was never reached (the same
        // attribution convention as `refuse`).
        match core.write_gate.acquire().await {
            Ok(permit) => Some(permit),
            Err(error) => {
                core.audit
                    .record(
                        tool,
                        "core",
                        tool,
                        &serde_json::Value::Object(arguments.clone()),
                        crate::error_map::code_name(&error),
                        grant_scope_ref(core, tool),
                    )
                    .await;
                return Err(error);
            }
        }
    } else {
        None
    };

    let budget = if limits::is_tree_tool(tool) {
        limits::TREE_BUDGET
    } else {
        limits::TOOL_CALL_BUDGET
    };
    let retries = if is_write {
        limits::WRITE_RETRIES
    } else {
        limits::READ_RETRIES
    };

    let mut attempt = 0;
    loop {
        // One timeout layer per call (see the dispatch doc): writes answer
        // through their adapter budget and are never wrapped here; reads
        // carry the core timer.
        let result = if is_write {
            Ok(dispatch_once(core, tool, &arguments).await)
        } else {
            tokio::time::timeout(budget, dispatch_once(core, tool, &arguments)).await
        };
        match result {
            Ok(inner) => match inner {
                Ok(outcome) => {
                    core.audit
                        .record(
                            tool,
                            &outcome.adapter,
                            tool,
                            &serde_json::Value::Object(arguments.clone()),
                            "ok",
                            grant_scope_ref(core, tool),
                        )
                        .await;
                    return Ok(outcome);
                }
                Err(err) => {
                    let code = crate::error_map::code_name(&err);
                    let adapter = adapter_name(core, tool);
                    core.audit
                        .record(
                            tool,
                            &adapter,
                            tool,
                            &serde_json::Value::Object(arguments.clone()),
                            code,
                            grant_scope_ref(core, tool),
                        )
                        .await;
                    return Err(err);
                }
            },
            Err(_elapsed) => {
                attempt += 1;
                if attempt > usize::from(retries) {
                    let adapter = adapter_name(core, tool);
                    core.audit
                        .record(
                            tool,
                            &adapter,
                            tool,
                            &serde_json::Value::Object(arguments.clone()),
                            "TimeoutError",
                            grant_scope_ref(core, tool),
                        )
                        .await;
                    return Err(ContractError::TimeoutError);
                }
            }
        }
    }
}

/// Runs the scope decision's blocking half off the async workers (the same
/// doctrine the fs-xdg adapter's `run_fs` states): a blocked `canonicalize`
/// (stale mount, FIFO) parks a blocking-pool thread the runtime can grow
/// around, never a worker that the read timers need.
async fn run_scope_check(
    task: impl FnOnce() -> Result<(), ContractError> + Send + 'static,
) -> Result<(), ContractError> {
    tokio::task::spawn_blocking(task).await.unwrap_or_else(|e| {
        // Unreachable in practice — the checked bodies are panic-free and the
        // task is never aborted — but a join failure still answers through the
        // taxonomy instead of panicking the dispatch.
        Err(ContractError::NativeFailure {
            code: -1,
            payload: format!("scope check task failed: {e}"),
        })
    })
}

/// The fs tools' `path` argument, shape-checked: a missing or non-string value
/// violates the tool schema (the DTO declares `path` as a required string) and
/// answers `Unsupported` — the same honest contract error `parse_args` maps
/// serde failures to; never `ScopeDenied`, which would imply a grant could
/// repair a call that never carried the argument it is being judged by. An
/// empty string is beyond the rendered schema's expressiveness (no
/// `minLength`), so refusing it is a core-side policy choice — a degenerate
/// argument, failed closed before scope canonicalization can mislabel it.
fn fs_path_argument(
    tool: &str,
    arguments: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, ContractError> {
    match arguments.get("path") {
        Some(serde_json::Value::String(raw)) if !raw.is_empty() => Ok(raw.clone()),
        _ => Err(ContractError::Unsupported {
            reason: format!("{tool} requires a non-empty string `path` argument"),
        }),
    }
}

/// The audit `scope_ref` for a call executed under a grant: the grant's
/// provenance plus the capability (`config:files.scope.write` shape) — audit.md
/// "grant-related events always carry the provenance of the grant".
fn grant_scope_ref(core: &crate::Core, tool: &str) -> Option<String> {
    let spec = all_specs().iter().find(|s| s.name == tool)?;
    let key = spec.grant_key?;
    let provenance = core.grants.provenance_of(key)?;
    Some(format!("{}:{key}", provenance.label()))
}

/// Audits one refusal (the adapter was never reached; the audit adapter is
/// `core`) and hands back the typed error to return to the agent.
async fn refuse(
    core: &crate::Core,
    tool: &str,
    arguments: &serde_json::Map<String, serde_json::Value>,
    error: ContractError,
    scope_ref: Option<String>,
) -> ContractError {
    core.audit
        .record(
            tool,
            "core",
            tool,
            &serde_json::Value::Object(arguments.clone()),
            crate::error_map::code_name(&error),
            scope_ref,
        )
        .await;
    error
}

fn adapter_name(core: &crate::Core, tool: &str) -> String {
    let subsystem: Option<String> = if tool.starts_with("desktop") {
        core.registry.a11y.as_ref().map(|a| a.report().adapter)
    } else if tool.starts_with("journal") {
        core.registry.journal.as_ref().map(|a| a.report().adapter)
    } else if tool.starts_with("svc_") {
        core.registry.services.as_ref().map(|a| a.report().adapter)
    } else if tool.starts_with("pkg_") {
        core.registry.packages.as_ref().map(|a| a.report().adapter)
    } else if tool.starts_with("clip") || tool == "screenshot" {
        core.registry.screen.as_ref().map(|a| a.report().adapter)
    } else if tool == "notify" {
        core.registry.notify.as_ref().map(|a| a.report().adapter)
    } else if tool.starts_with("fs_") {
        core.registry.files.as_ref().map(|a| a.report().adapter)
    } else if tool.starts_with("wiki_") {
        core.registry.kb.as_ref().map(|a| a.report().adapter)
    } else if tool == "net_status" {
        core.registry.net.as_ref().map(|a| a.report().adapter)
    } else if tool == "capabilities" {
        core.registry.probe.as_ref().map(|a| a.report().adapter)
    } else {
        None
    };
    subsystem.unwrap_or_else(|| String::from("core"))
}

/// The single-dispatch: route to the trait call on the activated slot.
async fn dispatch_once(
    core: &crate::Core,
    tool: &str,
    arguments: &serde_json::Map<String, serde_json::Value>,
) -> Result<DispatchOutcome, ContractError> {
    macro_rules! slot {
        ($field:ident) => {
            core.registry
                .$field
                .as_ref()
                .ok_or(ContractError::AdapterUnavailable)?
                .clone()
        };
    }

    match tool {
        t if t == join_tool("desktop", "tree") => {
            let selector: TreeSelector = parse_args(arguments)?;
            let a11y = slot!(a11y);
            let mut tree = a11y.tree(selector).await?;
            mint_tree_tokens(&core.session, &mut tree)?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&tree).unwrap_or(serde_json::Value::Null),
                adapter: a11y.report().adapter,
            })
        }
        t if t == join_tool("desktop", "element") => {
            let request: omnibus_contracts::dto::desktop::ElementRequest = parse_args(arguments)?;
            let node_ref = resolve_token(core, &request.element_id)?;
            let a11y = slot!(a11y);
            let mut element = a11y.element(node_ref).await?;
            mint_element_token(&core.session, &mut element.node_ref);
            Ok(DispatchOutcome {
                value: serde_json::to_value(&element).unwrap_or(serde_json::Value::Null),
                adapter: a11y.report().adapter,
            })
        }
        t if t == join_tool("desktop", "set_value") => {
            let mut request: SetValueRequest = parse_args(arguments)?;
            request.node_ref = resolve_token(core, &request.node_ref)?;
            let a11y = slot!(a11y);
            let mut outcome = a11y.set_value(request).await?;
            mint_element_token(&core.session, &mut outcome.node_ref);
            Ok(DispatchOutcome {
                value: serde_json::to_value(&outcome).unwrap_or(serde_json::Value::Null),
                adapter: a11y.report().adapter,
            })
        }
        t if t == join_tool("desktop", "invoke") => {
            let mut request: InvokeRequest = parse_args(arguments)?;
            request.node_ref = resolve_token(core, &request.node_ref)?;
            let a11y = slot!(a11y);
            let mut outcome = a11y.invoke(request).await?;
            mint_element_token(&core.session, &mut outcome.node_ref);
            Ok(DispatchOutcome {
                value: serde_json::to_value(&outcome).unwrap_or(serde_json::Value::Null),
                adapter: a11y.report().adapter,
            })
        }
        "clip_read" => {
            let screen = slot!(screen);
            let content = screen.clip_read().await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&content).unwrap_or(serde_json::Value::Null),
                adapter: screen.report().adapter,
            })
        }
        "clip_write" => {
            let content: ClipboardContent = parse_args(arguments)?;
            let screen = slot!(screen);
            screen.clip_write(content).await?;
            Ok(DispatchOutcome {
                value: serde_json::json!({ "written": true }),
                adapter: screen.report().adapter,
            })
        }
        "screenshot" => {
            let request: ScreenshotRequest = parse_args(arguments)?;
            let screen = slot!(screen);
            let shot = screen.screenshot(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&shot).unwrap_or(serde_json::Value::Null),
                adapter: screen.report().adapter,
            })
        }
        "notify" => {
            let request: NotifyRequest = parse_args(arguments)?;
            let notify = slot!(notify);
            let result = notify.send(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
                adapter: notify.report().adapter,
            })
        }
        "svc_list" => {
            let filter: ServiceFilter = parse_args(arguments)?;
            let services = slot!(services);
            let units = services.list(filter).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&units).unwrap_or(serde_json::Value::Null),
                adapter: services.report().adapter,
            })
        }
        "svc_status" => {
            let request: omnibus_contracts::dto::services::ServiceNameRequest =
                parse_args(arguments)?;
            let unit = request.unit;
            let services = slot!(services);
            let status = services.status(unit).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&status).unwrap_or(serde_json::Value::Null),
                adapter: services.report().adapter,
            })
        }
        "svc_start" | "svc_stop" => {
            let request: omnibus_contracts::dto::services::ServiceNameRequest =
                parse_args(arguments)?;
            let unit = request.unit;
            let action = if tool == "svc_start" {
                omnibus_contracts::dto::services::ServiceAction::Start
            } else {
                omnibus_contracts::dto::services::ServiceAction::Stop
            };
            let services = slot!(services);
            let result = services
                .apply(ServiceActionRequest { unit, action })
                .await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
                adapter: services.report().adapter,
            })
        }
        "net_status" => {
            let net = slot!(net);
            let report = net.status().await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
                adapter: net.report().adapter,
            })
        }
        "journal_query" => {
            let query: JournalQuery = parse_args(arguments)?;
            // The prev-boot rung is enforced in `dispatch` (a ladder decision
            // refused under `core`, before any adapter can be reached).
            let journal = slot!(journal);
            let page = journal.query(query).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&page).unwrap_or(serde_json::Value::Null),
                adapter: journal.report().adapter,
            })
        }
        "pkg_list" => {
            let request: PackageListRequest = parse_args(arguments)?;
            let packages = slot!(packages);
            let list = packages.list(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&list).unwrap_or(serde_json::Value::Null),
                adapter: packages.report().adapter,
            })
        }
        "pkg_info" => {
            let request: PackageNameRequest = parse_args(arguments)?;
            let name = request.name;
            let packages = slot!(packages);
            let info = packages.info(name).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&info).unwrap_or(serde_json::Value::Null),
                adapter: packages.report().adapter,
            })
        }
        "pkg_install" => {
            let request: PackageNameRequest = parse_args(arguments)?;
            let packages = slot!(packages);
            let result = packages.install(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
                adapter: packages.report().adapter,
            })
        }
        "pkg_remove" => {
            let request: PackageNameRequest = parse_args(arguments)?;
            let packages = slot!(packages);
            let result = packages.remove(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
                adapter: packages.report().adapter,
            })
        }
        "pkg_undo" => {
            let request: PackageNameRequest = parse_args(arguments)?;
            let packages = slot!(packages);
            let result = packages.undo(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
                adapter: packages.report().adapter,
            })
        }
        "fs_list" => {
            let query: PathQuery = parse_args(arguments)?;
            let files = slot!(files);
            let listing = files.list(query).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&listing).unwrap_or(serde_json::Value::Null),
                adapter: files.report().adapter,
            })
        }
        "fs_read" => {
            let request: FileReadRequest = parse_args(arguments)?;
            let files = slot!(files);
            let content = files.read(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&content).unwrap_or(serde_json::Value::Null),
                adapter: files.report().adapter,
            })
        }
        "fs_write" => {
            let request: FileWriteRequest = parse_args(arguments)?;
            let files = slot!(files);
            let result = files.write(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
                adapter: files.report().adapter,
            })
        }
        "wiki_search" => {
            let query: KbQuery = parse_args(arguments)?;
            let kb = slot!(kb);
            let refs = kb.search(query).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&refs).unwrap_or(serde_json::Value::Null),
                adapter: kb.report().adapter,
            })
        }
        "wiki_page" => {
            let request: KbPageRequest = parse_args(arguments)?;
            let kb = slot!(kb);
            let article = kb.page(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&article).unwrap_or(serde_json::Value::Null),
                adapter: kb.report().adapter,
            })
        }
        "capabilities" => Ok(DispatchOutcome {
            value: serde_json::to_value(&core.caps.document).unwrap_or(serde_json::Value::Null),
            adapter: String::from("core"),
        }),
        "session_info" => Ok(DispatchOutcome {
            value: serde_json::to_value(core.session.info()).unwrap_or(serde_json::Value::Null),
            adapter: String::from("core"),
        }),
        "doctor" => Ok(DispatchOutcome {
            value: serde_json::to_value(&crate::doctor::diagnose(core).await)
                .unwrap_or(serde_json::Value::Null),
            adapter: String::from("core"),
        }),
        other => Err(ContractError::Unsupported {
            reason: format!("unknown tool {other}"),
        }),
    }
}

/// Swaps one adapter-owned node ref for a core-minted element token: the core
/// always mints (dto/desktop.rs), so no raw adapter ref ever reaches the MCP
/// surface — on trees, element lookups, and typed-write outcomes alike.
fn mint_element_token(session: &crate::session::SessionContext, node_ref: &mut String) {
    let taken = std::mem::take(node_ref);
    *node_ref = session.mint_element(taken).as_str().to_string();
}

/// Swaps adapter node refs for core-minted element tokens in a delivered tree.
/// A delivered tree at or above the per-session token-map bound
/// [`ELEMENT_TOKEN_MAP_CAP`] swaps the generation *before* minting and mints
/// for at most its first [`ELEMENT_TOKEN_MAP_CAP`] nodes (pre-order): one
/// response stays internally consistent — every token minted in it resolves —
/// where the old mid-mint clear invalidated earlier tokens of the same tree,
/// a state re-reading the tree could never converge out of. Nodes beyond the
/// bound carry an empty `node_ref`: no token resolves them, so addressing one
/// answers [`ContractError::ElementStale`] (the contract's modeled staleness),
/// never a raw adapter ref; the core-side cut is signaled by that empty
/// `node_ref` (and the `ElementStale` it produces), not by the adapter's
/// `truncated` flag, which keeps reporting the adapter's own walk result.
///
/// Concurrency note, stated honestly: the guarantee is serial. Two
/// overlapping `desktop.tree` deliveries interleave their
/// count/swap/mint steps on the shared session map, so a concurrent delivery
/// can still invalidate another in-flight response's tokens near the cap —
/// those tokens answer `ElementStale` (the modeled outcome) and a re-read
/// converges. Closing that would need a session-level batch mint holding the
/// map lock across the whole response; deliberately not built — no session
/// has pipelined tree deliveries today, and the race's answer (staleness) is
/// already the contract's modeled outcome.
fn mint_tree_tokens(
    session: &crate::session::SessionContext,
    tree: &mut omnibus_contracts::dto::desktop::A11yTree,
) -> Result<(), ContractError> {
    fn count_nodes(node: &omnibus_contracts::dto::desktop::A11yNode) -> usize {
        1 + node.children.iter().map(count_nodes).sum::<usize>()
    }
    fn walk(
        session: &crate::session::SessionContext,
        node: &mut omnibus_contracts::dto::desktop::A11yNode,
        left: &mut usize,
    ) {
        if *left == 0 {
            // Past the mint bound: no resolvable token — and never a raw ref.
            node.node_ref = String::new();
        } else {
            *left -= 1;
            mint_element_token(session, &mut node.node_ref);
        }
        for child in &mut node.children {
            walk(session, child, left);
        }
    }
    // The delivered node count plus the map's current occupancy — not the
    // DTO's claimed `node_count` — decides the pre-mint swap: a delivery
    // whose `count + len` would cross the cap mid-mint would otherwise have
    // `mint_element`'s clear wipe its own first tokens. Swapping first keeps
    // every response internally consistent.
    if count_nodes(&tree.root) + session.tokens_len() >= ELEMENT_TOKEN_MAP_CAP {
        session.swap_generation();
    }
    let mut left = ELEMENT_TOKEN_MAP_CAP;
    walk(session, &mut tree.root, &mut left);
    Ok(())
}

/// Resolves a core-minted token back to the adapter-owned node ref.
fn resolve_token(core: &crate::Core, token: &str) -> Result<String, ContractError> {
    core.session
        .resolve_element(token)
        .ok_or(ContractError::ElementStale)
}

/// The tools visible to the agent: every spec whose ladder state is runnable.
pub fn visible_tools(core: &crate::Core) -> Vec<ToolSpec> {
    all_specs()
        .iter()
        .filter(|spec| matches!(gate(core, &spec.name), GateDecision::Run))
        .cloned()
        .collect()
}

/// Schema JSON for one tool (generated from the DTO — never hand-written).
pub fn schema_json(spec: &ToolSpec) -> serde_json::Value {
    json_schema(&spec.args_schema)
}

/// Ladder rung 1 marking for write-tool schemas (capability-model.md:
/// "schema marks the limitation"): on a machine whose `pkg.undo` fact carries
/// the typed value `none` (apt: no transactional rollback), `pkg_install` and
/// `pkg_remove` carry the degradation note — sourced from the caps entry's own
/// `note` — while `pkg_undo` itself stays hidden through the ladder. Every
/// other tool, and every other `pkg.undo` value, yields no note; the value is
/// parsed through the contracts enum, so an off-registry string never marks.
pub fn degradation_note(core: &crate::Core, tool: &str) -> Option<String> {
    if !matches!(tool, "pkg_install" | "pkg_remove") {
        return None;
    }
    let entry = core.caps.entry(cap::PKG_UNDO)?;
    let undo_is_none = entry
        .value
        .as_ref()
        .and_then(omnibus_contracts::capabilities::PkgUndoValue::from_json)
        .is_some_and(|value| value == omnibus_contracts::capabilities::PkgUndoValue::None);
    if !undo_is_none {
        return None;
    }
    Some(
        entry
            .note
            .clone()
            .unwrap_or_else(|| String::from("undo unavailable (pkg.undo = none)")),
    )
}

/// The schema JSON the facade serves for one visible tool: the generated
/// schema plus, when ladder rung 1 applies ([`degradation_note`]), the
/// degradation note as the schema's `description` (a JSON Schema annotation —
/// instances stay as strict as serde).
pub fn schema_json_annotated(core: &crate::Core, spec: &ToolSpec) -> serde_json::Value {
    let mut schema = schema_json(spec);
    if let (Some(note), Some(object)) = (degradation_note(core, &spec.name), schema.as_object_mut())
    {
        object.insert(String::from("description"), serde_json::Value::String(note));
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_closed_over_the_normative_list() {
        let specs = all_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names.len(),
            27,
            "24 domain + capabilities + session_info + doctor"
        );
        let mut sorted = names.clone();
        sorted.sort_unstable();
        for pair in sorted.windows(2) {
            assert_ne!(pair[0], pair[1], "duplicate tool name");
        }
        // Dotted name assembled via join_tool (R4-CON: no capability-shaped literal).
        assert!(
            names.contains(&join_tool("desktop", "tree").as_str()),
            "missing desktop.tree"
        );
        for expected in ["svc_start", "pkg_undo", "fs_write", "net_status", "doctor"] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn write_tools_carry_grant_keys() {
        let specs = all_specs();
        let grant_of = |name: &str| {
            specs
                .iter()
                .find(|s| s.name == name)
                .and_then(|s| s.grant_key)
        };
        assert_eq!(grant_of("pkg_install"), Some(cap::PKG_WRITE));
        assert_eq!(grant_of("pkg_undo"), Some(cap::PKG_UNDO));
        assert_eq!(grant_of("fs_write"), Some(cap::FILES_SCOPE_WRITE));
        assert_eq!(grant_of("svc_start"), Some(cap::SERVICE_WRITE));
        assert_eq!(grant_of("clip_write"), Some(cap::PORTAL_CLIPBOARD));
        assert_eq!(grant_of("notify"), Some(cap::PORTAL_NOTIFY));
        assert_eq!(grant_of("journal_query"), None);
    }

    #[test]
    fn every_schema_is_strict_object_json() {
        for spec in all_specs() {
            let schema = schema_json(spec);
            if matches!(spec.args_schema, SchemaNode::Any) {
                continue; // no-argument tools
            }
            assert_eq!(schema["type"], "object", "tool {}", spec.name);
            assert_eq!(schema["additionalProperties"], false);
        }
    }

    #[test]
    fn floor_components_follow_the_platform_matrix() {
        // The systemd floor gates only the services tools; journal and package
        // reads stay available below it (platform-matrix.md).
        assert_eq!(component_for_tool("svc_start"), "systemd");
        assert_eq!(component_for_tool("svc_status"), "systemd");
        assert_eq!(component_for_tool("journal_query"), "journal");
        assert_eq!(component_for_tool("pkg_list"), "none");
        assert_eq!(component_for_tool("pkg_undo"), "none");
        // Dotted names are assembled, never written as literals (R4-CON).
        let tree_tool = join_tool("desktop", "tree");
        assert_eq!(component_for_tool(&tree_tool), "at-spi2");
    }

    /// Every tool's advertised schema accepts what dispatch actually parses:
    /// a schema that follows tools/list must never describe a DTO the core
    /// rejects (the desktop.element result-as-arguments bug class).
    #[test]
    fn every_advertised_schema_accepts_the_dispatch_dto() {
        use omnibus_contracts::dto::desktop::{
            ElementRequest, InvokeRequest, SetValueRequest, TreeSelector,
        };
        for spec in all_specs() {
            if matches!(spec.args_schema, SchemaNode::Any) {
                continue; // no-argument tools
            }
            let sample = omnibus_contracts::schema::sample(&spec.args_schema, 0);
            let Some(map) = sample.as_object().cloned() else {
                panic!("tool {} advertises a non-object schema", spec.name);
            };
            let accepted = match spec.name.as_str() {
                t if t == join_tool("desktop", "tree") => parse_args::<TreeSelector>(&map).is_ok(),
                t if t == join_tool("desktop", "element") => {
                    parse_args::<ElementRequest>(&map).is_ok()
                }
                t if t == join_tool("desktop", "set_value") => {
                    parse_args::<SetValueRequest>(&map).is_ok()
                }
                t if t == join_tool("desktop", "invoke") => {
                    parse_args::<InvokeRequest>(&map).is_ok()
                }
                "clip_write" => parse_args::<ClipboardContent>(&map).is_ok(),
                "screenshot" => parse_args::<ScreenshotRequest>(&map).is_ok(),
                "notify" => parse_args::<NotifyRequest>(&map).is_ok(),
                "svc_list" => parse_args::<ServiceFilter>(&map).is_ok(),
                "svc_status" | "svc_start" | "svc_stop" => {
                    parse_args::<omnibus_contracts::dto::services::ServiceNameRequest>(&map).is_ok()
                }
                "journal_query" => parse_args::<JournalQuery>(&map).is_ok(),
                "pkg_list" => parse_args::<PackageListRequest>(&map).is_ok(),
                "pkg_info" | "pkg_install" | "pkg_remove" | "pkg_undo" => {
                    parse_args::<PackageNameRequest>(&map).is_ok()
                }
                "fs_list" => parse_args::<PathQuery>(&map).is_ok(),
                "fs_read" => parse_args::<FileReadRequest>(&map).is_ok(),
                "fs_write" => parse_args::<FileWriteRequest>(&map).is_ok(),
                "wiki_search" => parse_args::<KbQuery>(&map).is_ok(),
                "wiki_page" => parse_args::<KbPageRequest>(&map).is_ok(),
                other => panic!("unmapped tool {other}: extend the conformance table"),
            };
            assert!(
                accepted,
                "the advertised schema of {} does not accept its dispatch DTO",
                spec.name
            );
        }
    }

    /// One oversized delivered tree stays internally consistent: with the tree
    /// at cap + 5 nodes, the generation swaps before minting, the first
    /// ELEMENT_TOKEN_MAP_CAP nodes all carry resolvable tokens (mint them all,
    /// then resolve each), and the nodes beyond the bound carry no resolvable
    /// token (an empty `node_ref`, never a raw adapter ref).
    #[test]
    fn oversized_tree_is_internally_consistent_for_the_first_cap_nodes() {
        let session = crate::session::SessionContext::try_new(
            [7u8; 32],
            omnibus_contracts::dto::audit::AuditActor {
                uid: 1000,
                pid: 7,
                client_name: Some(String::from("test-client")),
            },
        )
        .expect("non-zero test entropy is a valid mint key");
        let node = |name: String| omnibus_contracts::dto::desktop::A11yNode {
            node_ref: name.clone(),
            role: String::from("entry"),
            name,
            pid: Some(11),
            children: vec![],
        };
        let mut root = node(String::from("root"));
        root.children = (0..ELEMENT_TOKEN_MAP_CAP + 4)
            .map(|i| node(format!("child{i}")))
            .collect();
        let mut tree = omnibus_contracts::dto::desktop::A11yTree {
            app: String::from("demo"),
            pid: Some(11),
            root,
            node_count: u32::try_from(ELEMENT_TOKEN_MAP_CAP + 5).unwrap_or(u32::MAX),
            truncated: false,
        };
        mint_tree_tokens(&session, &mut tree).expect("tree minting is infallible");

        fn collect(node: &omnibus_contracts::dto::desktop::A11yNode, out: &mut Vec<String>) {
            out.push(node.node_ref.clone());
            for child in &node.children {
                collect(child, out);
            }
        }
        let mut refs = Vec::new();
        collect(&tree.root, &mut refs);
        assert_eq!(refs.len(), ELEMENT_TOKEN_MAP_CAP + 5);
        let (minted, beyond) = refs.split_at(ELEMENT_TOKEN_MAP_CAP);
        assert!(
            beyond.iter().all(String::is_empty),
            "nodes past the bound carry no token — and never a raw adapter ref"
        );
        for (i, token) in minted.iter().enumerate() {
            assert!(!token.is_empty(), "node {i} of the tree got a token");
            assert!(
                session.resolve_element(token).is_some(),
                "every token minted in one delivered tree resolves: node {i}"
            );
        }
    }

    /// The prev-boot gate reads the adapter's prev_boot FACT status through
    /// the ladder: a volatile journal always emits the fact (as
    /// `opt-in-required`), so only a status check can gate it — the entry
    /// check the dispatch arm used to test could never fire.
    #[test]
    fn prev_boot_gate_reads_the_fact_status() {
        use omnibus_contracts::dto::caps::{CapabilitiesDocument, CapabilityEntry};
        use omnibus_contracts::dto::probe::{DistroFamily, DistroInfo};
        fn state(entries: Vec<CapabilityEntry>) -> crate::caps::CapabilitiesState {
            crate::caps::CapabilitiesState {
                document: CapabilitiesDocument {
                    contract_version: String::from(omnibus_contracts::version::CONTRACT_VERSION),
                    distro: DistroInfo {
                        id: String::from("manjaro"),
                        name: String::from("Manjaro Linux"),
                        release: String::from("rolling"),
                        family: DistroFamily::Manjaro,
                    },
                    capabilities: entries,
                    floors: Vec::new(),
                },
                floors: Vec::new(),
            }
        }
        let entry = |name: &'static str, status: &str| CapabilityEntry {
            name: String::from(name),
            ver: String::from(omnibus_contracts::version::CONTRACT_VERSION),
            status: String::from(status),
            value: None,
            adapter: Some(String::from("journald")),
            note: None,
            remedy: None,
        };
        // Persistent journal with past boots: the query runs.
        let present = state(vec![
            entry(cap::JOURNAL_QUERY, "present"),
            entry(cap::JOURNAL_PERSISTENT, "present"),
            entry(cap::JOURNAL_PREV_BOOT, "present"),
        ]);
        assert!(prev_boot_usable(&present, false));
        // Persistent journal without past boots: degraded — the tool runs.
        let degraded = state(vec![
            entry(cap::JOURNAL_QUERY, "present"),
            entry(cap::JOURNAL_PERSISTENT, "present"),
            entry(cap::JOURNAL_PREV_BOOT, "degraded"),
        ]);
        assert!(prev_boot_usable(&degraded, false));
        // Volatile journal: the fact is ALWAYS emitted, as opt-in-required —
        // refused until journal.persistent is granted, runnable after.
        let volatile = state(vec![
            entry(cap::JOURNAL_QUERY, "present"),
            entry(cap::JOURNAL_PERSISTENT, "opt-in-required"),
            entry(cap::JOURNAL_PREV_BOOT, "opt-in-required"),
        ]);
        assert!(!prev_boot_usable(&volatile, false));
        assert!(prev_boot_usable(&volatile, true));
        // An adapter that reports no prev_boot fact at all: absent capability.
        let absent = state(vec![entry(cap::JOURNAL_QUERY, "present")]);
        assert!(!prev_boot_usable(&absent, false));
    }
}

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

/// The full normative registry in tool-registry.md order.
pub fn all_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::read(
            join_tool("desktop", "tree"),
            "a11y tree of one application, role+name addressing",
            TreeSelector::schema(),
        ),
        ToolSpec::read(
            join_tool("desktop", "element"),
            "flat detail of one a11y node (attributes, states, actions)",
            omnibus_contracts::dto::desktop::A11yElement::schema(),
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

/// The subsystem component a tool's floor check runs against.
fn component_for_tool(tool: &str) -> &'static str {
    if tool.starts_with("desktop") {
        "at-spi2"
    } else if tool.starts_with("svc_") || tool.starts_with("journal") || tool.starts_with("pkg_") {
        "systemd"
    } else {
        "none"
    }
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
pub async fn dispatch(
    core: &crate::Core,
    tool: &str,
    arguments: serde_json::Map<String, serde_json::Value>,
) -> Result<DispatchOutcome, ContractError> {
    // R5 audit is total: a refused call is still a call — gate denials are audited
    // with the denial reason before the typed error surfaces (audit.md, AC 1.3.3).
    match gate(core, tool) {
        GateDecision::BelowFloor => {
            return Err(refuse(core, tool, &arguments, ContractError::FloorUnsupported).await);
        }
        GateDecision::Hidden => {
            return Err(refuse(core, tool, &arguments, ContractError::CapabilityRequired).await);
        }
        GateDecision::Denied => {
            return Err(refuse(core, tool, &arguments, ContractError::ScopeDenied).await);
        }
        GateDecision::Run => {}
    }

    let is_write = all_specs()
        .iter()
        .find(|s| s.name == tool)
        .and_then(|s| s.grant_key)
        .is_some();

    let _permit = if is_write {
        Some(core.write_gate.acquire().await)
    } else {
        None
    };

    // Parameterized scope check for fs_write: canonical path must be inside the
    // granted paths (the adapter re-checks defensively).
    if tool == "fs_write" {
        let raw = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let canonical = std::fs::canonicalize(&raw)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(raw);
        core.scopes
            .authorize_write(cap::FILES_SCOPE_WRITE, Some(&canonical))?;
    }

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
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(budget, dispatch_once(core, tool, &arguments)).await;
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
                            None,
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
                            None,
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
                            None,
                        )
                        .await;
                    return Err(ContractError::TimeoutError);
                }
                let _ = started;
            }
        }
    }
}

/// Audits one gate refusal (the adapter was never reached; the audit adapter is
/// `core`) and hands back the typed error to return to the agent.
async fn refuse(
    core: &crate::Core,
    tool: &str,
    arguments: &serde_json::Map<String, serde_json::Value>,
    error: ContractError,
) -> ContractError {
    core.audit
        .record(
            tool,
            "core",
            tool,
            &serde_json::Value::Object(arguments.clone()),
            crate::error_map::code_name(&error),
            None,
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
            mint_tree_tokens(core, &mut tree)?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&tree).unwrap_or(serde_json::Value::Null),
                adapter: a11y.report().adapter,
            })
        }
        t if t == join_tool("desktop", "element") => {
            let request: omnibus_contracts::dto::desktop::ElementRequest = parse_args(arguments)?;
            let node_ref = resolve_token(core, &request.element_id)?;
            let a11y = slot!(a11y);
            let element = a11y.element(node_ref).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&element).unwrap_or(serde_json::Value::Null),
                adapter: a11y.report().adapter,
            })
        }
        t if t == join_tool("desktop", "set_value") => {
            let mut request: SetValueRequest = parse_args(arguments)?;
            request.node_ref = resolve_token(core, &request.node_ref)?;
            let a11y = slot!(a11y);
            let outcome = a11y.set_value(request).await?;
            Ok(DispatchOutcome {
                value: serde_json::to_value(&outcome).unwrap_or(serde_json::Value::Null),
                adapter: a11y.report().adapter,
            })
        }
        t if t == join_tool("desktop", "invoke") => {
            let mut request: InvokeRequest = parse_args(arguments)?;
            request.node_ref = resolve_token(core, &request.node_ref)?;
            let a11y = slot!(a11y);
            let outcome = a11y.invoke(request).await?;
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
            if query.boot == Some(BootSelector::Previous)
                && core.caps.entry(cap::JOURNAL_PREV_BOOT).is_none()
            {
                // prev_boot is an independent capability: absent means denied, and
                // the honest answer names the doctor, not an empty page.
                return Err(ContractError::CapabilityRequired);
            }
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

/// Swaps adapter node refs for core-minted element tokens in a delivered tree.
fn mint_tree_tokens(
    core: &crate::Core,
    tree: &mut omnibus_contracts::dto::desktop::A11yTree,
) -> Result<(), ContractError> {
    fn walk(core: &crate::Core, node: &mut omnibus_contracts::dto::desktop::A11yNode) {
        let token = core
            .session
            .mint_element(std::mem::take(&mut node.node_ref));
        node.node_ref = token.as_str().to_string();
        for child in &mut node.children {
            walk(core, child);
        }
    }
    walk(core, &mut tree.root);
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
        .into_iter()
        .filter(|spec| matches!(gate(core, &spec.name), GateDecision::Run))
        .collect()
}

/// Schema JSON for one tool (generated from the DTO — never hand-written).
pub fn schema_json(spec: &ToolSpec) -> serde_json::Value {
    json_schema(&spec.args_schema)
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
            let schema = schema_json(&spec);
            if matches!(spec.args_schema, SchemaNode::Any) {
                continue; // no-argument tools
            }
            assert_eq!(schema["type"], "object", "tool {}", spec.name);
            assert_eq!(schema["additionalProperties"], false);
        }
    }
}

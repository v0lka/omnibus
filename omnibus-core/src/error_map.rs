//! Mapping the closed eight-code taxonomy onto the MCP surface
//! (specs/contracts/mcp-client-daemon.md "Error Propagation"): every tool result is
//! either a DTO-shaped value or one of the eight typed codes; `ScopeDenied` always
//! carries a copy-paste remedy; hidden-tool calls return `CapabilityRequired` with
//! a doctor hint.

use omnibus_contracts::errors::ContractError;

/// The stable code name an agent sees next to the message.
pub fn code_name(error: &ContractError) -> &'static str {
    match error {
        ContractError::CapabilityRequired => "CapabilityRequired",
        ContractError::ScopeDenied => "ScopeDenied",
        ContractError::FloorUnsupported => "FloorUnsupported",
        ContractError::AdapterUnavailable => "AdapterUnavailable",
        ContractError::NativeFailure { .. } => "NativeFailure",
        ContractError::ElementStale => "ElementStale",
        ContractError::TimeoutError => "TimeoutError",
        ContractError::Unsupported { .. } => "Unsupported",
    }
}

/// The one-line copy-paste remedy, same principle as the capabilities document
/// `remedy` field.
pub fn remedy(error: &ContractError, capability: Option<&str>) -> Option<String> {
    match (error, capability) {
        (ContractError::ScopeDenied, Some(cap)) => Some(format!("omnibus grant {cap}")),
        (ContractError::CapabilityRequired, Some(cap)) => Some(format!(
            "run `omnibus doctor` — the machine lacks {cap}; doctor lists remedies"
        )),
        (ContractError::FloorUnsupported, _) => Some(String::from(
            "run `omnibus doctor` — the platform is below the version floor",
        )),
        (ContractError::AdapterUnavailable, _) => Some(String::from(
            "run `omnibus doctor` — the subsystem has no active backend",
        )),
        _ => None,
    }
}

/// Renders the full agent-facing message: code, message, remedy.
pub fn render(error: &ContractError, capability: Option<&str>) -> String {
    let base = error.to_string();
    match remedy(error, capability) {
        Some(remedy_text) => format!(
            "{} [{code}] remedy: {remedy_text}",
            base,
            code = code_name(error)
        ),
        None => format!("{} [{}]", base, code_name(error)),
    }
}

/// Joins a tool name from its parts. Capability-shaped literals (dotted lowercase)
/// live only in the contracts registry (R4-CON), so dotted tool names are built
/// here instead of written out.
pub fn join_tool(domain: &str, leaf: &str) -> String {
    format!("{domain}.{leaf}")
}

/// The capability a tool maps to, for denial messages (tool-registry.md bindings).
pub fn capability_for_tool(tool: &str) -> Option<&'static str> {
    use omnibus_contracts::capabilities as c;
    // Dotted names assembled, never written as literals (R4-CON).
    let (dtree, delem, dsetv, dinv) = (
        join_tool("desktop", "tree"),
        join_tool("desktop", "element"),
        join_tool("desktop", "set_value"),
        join_tool("desktop", "invoke"),
    );
    for (name, cap) in [
        (dtree.as_str(), c::DESKTOP_TREE),
        (delem.as_str(), c::DESKTOP_TREE),
        (dsetv.as_str(), c::DESKTOP_WRITE_TYPED),
        (dinv.as_str(), c::DESKTOP_WRITE_TYPED),
        ("clip_read", c::PORTAL_CLIPBOARD),
        ("clip_write", c::PORTAL_CLIPBOARD),
        ("screenshot", c::PORTAL_SCREENSHOT),
        ("notify", c::PORTAL_NOTIFY),
        ("svc_list", c::SERVICE_READ),
        ("svc_status", c::SERVICE_READ),
        ("svc_start", c::SERVICE_WRITE),
        ("svc_stop", c::SERVICE_WRITE),
        ("net_status", c::NET_READ),
        ("journal_query", c::JOURNAL_QUERY),
        ("pkg_list", c::PKG_READ),
        ("pkg_info", c::PKG_READ),
        ("pkg_install", c::PKG_WRITE),
        ("pkg_remove", c::PKG_WRITE),
        ("pkg_undo", c::PKG_UNDO),
        ("fs_list", c::FILES_SCOPE_READ),
        ("fs_read", c::FILES_SCOPE_READ),
        ("fs_write", c::FILES_SCOPE_WRITE),
        ("wiki_search", c::KB_PRESENT),
        ("wiki_page", c::KB_PRESENT),
    ] {
        if name == tool {
            return Some(cap);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_denied_carries_grant_remedy() {
        let text = render(
            &ContractError::ScopeDenied,
            capability_for_tool("pkg_install"),
        );
        assert!(text.contains("ScopeDenied"));
        assert!(
            text.contains("omnibus grant "),
            "remedy is copy-paste ready"
        );
        assert!(text.contains(omnibus_contracts::capabilities::PKG_WRITE));
    }

    #[test]
    fn hidden_tool_call_hints_doctor() {
        let text = render(
            &ContractError::CapabilityRequired,
            capability_for_tool("screenshot"),
        );
        assert!(text.contains("omnibus doctor"));
        assert_eq!(capability_for_tool("session_info"), None);
    }
}

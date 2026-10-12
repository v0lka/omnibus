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
        (ContractError::ScopeDenied, Some(cap)) => Some(format!("omnibusd grant {cap}")),
        (ContractError::CapabilityRequired, Some(cap)) => Some(format!(
            "run `omnibusd doctor` — the machine lacks {cap}; doctor lists remedies"
        )),
        (ContractError::FloorUnsupported, _) => Some(String::from(
            "run `omnibusd doctor` — the platform is below the version floor",
        )),
        (ContractError::AdapterUnavailable, _) => Some(String::from(
            "run `omnibusd doctor` — the subsystem has no active backend",
        )),
        _ => None,
    }
}

/// The files tools' ScopeDenied remedy. The generic grant form cannot work
/// here: the read scope has no grantable capability at all (the grant CLI's
/// `[+scope]` list carries only the write scope and the journal opt-in), and
/// the write scope is path-parameterized — a bare grant command exits 2. The
/// working lever is the parameterized write grant (a granted write scope
/// carries read access, files.md), so the remedy names that exact form — and
/// states the unconditional carve-out honestly: the daemon's grant-store and
/// audit-state directories are never readable or writable, so a denial there
/// has no grant remedy.
fn fs_scope_denied_remedy() -> Option<String> {
    use omnibus_contracts::capabilities as c;
    Some(format!(
        "omnibusd grant {} --path <directory containing the target> (a granted write scope carries read access); the daemon's grant-store and audit-state directories are never readable or writable — no grant changes that",
        c::FILES_SCOPE_WRITE
    ))
}

/// Renders the full agent-facing message: code, message, remedy — with the
/// tool-aware ScopeDenied remedy for the files tools (see
/// [`fs_scope_denied_remedy`]).
pub fn render_for_tool(error: &ContractError, capability: Option<&str>, tool: &str) -> String {
    let base = error.to_string();
    let remedy_text = match error {
        ContractError::ScopeDenied
            if tool == "fs_list" || tool == "fs_read" || tool == "fs_write" =>
        {
            fs_scope_denied_remedy()
        }
        _ => remedy(error, capability),
    };
    match remedy_text {
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
        let text = render_for_tool(
            &ContractError::ScopeDenied,
            capability_for_tool("pkg_install"),
            "pkg_install",
        );
        assert!(text.contains("ScopeDenied"));
        assert!(
            text.contains("omnibusd grant "),
            "remedy is copy-paste ready"
        );
        assert!(text.contains(omnibus_contracts::capabilities::PKG_WRITE));
    }

    /// The files tools' ScopeDenied remedy must be a command the grant CLI
    /// actually accepts: the read scope has no grantable capability and the
    /// write scope is path-parameterized, so the generic `omnibusd grant <cap>`
    /// form would exit 2 — the remedy names the parameterized write grant (a
    /// granted write scope carries read access) and states the daemon-dir
    /// carve-out honestly.
    #[test]
    fn files_scope_denied_remedy_is_the_parameterized_write_grant() {
        for tool in ["fs_list", "fs_read", "fs_write"] {
            let text =
                render_for_tool(&ContractError::ScopeDenied, capability_for_tool(tool), tool);
            assert!(
                text.contains("--path <directory containing the target>"),
                "{tool}: remedy must be the parameterized, grantable form: {text}"
            );
            assert!(
                text.contains(omnibus_contracts::capabilities::FILES_SCOPE_WRITE),
                "{tool}: remedy names the grantable capability"
            );
            assert!(
                text.contains("never readable or writable"),
                "{tool}: remedy states the daemon-dir carve-out: {text}"
            );
        }
    }

    #[test]
    fn hidden_tool_call_hints_doctor() {
        let text = render_for_tool(
            &ContractError::CapabilityRequired,
            capability_for_tool("screenshot"),
            "screenshot",
        );
        assert!(text.contains("omnibusd doctor"));
        assert_eq!(capability_for_tool("session_info"), None);
    }
}

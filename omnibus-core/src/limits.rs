//! Limits and retries (specs/architecture/topology-and-lifecycle.md "Limits and
//! Retries"). Defaults are hardcoded — there is no config surface until a real
//! request exists; the effective values are visible via the `session_info` tool
//! (`SessionInfo.limits`).

use std::time::Duration;

/// Tool-call budget for reads — the only timeout layer a read has.
///
/// Writes never ride this timer: each write adapter bounds its own native
/// work, and the shape of its elapse answer is adapter-defined — a
/// payload-bearing indeterminate outcome where the adapter can name it
/// (alpm, fs-xdg, systemd1), a plain `TimeoutError` elsewhere (see
/// `tools::dispatch`, "exactly one timeout layer per call").
///
/// Timeout semantics are honest, not cancellation (see `tools::dispatch`): on
/// elapse the dispatched future is dropped at its next await point and a
/// request already on the wire may still complete natively — `TimeoutError`
/// means "the core stopped waiting; the outcome is unknown".
pub const TOOL_CALL_BUDGET: Duration = Duration::from_secs(10);

/// Wall-clock bound on cross-process write-gate acquisition. The gate is taken
/// *before* the tool budget starts, so it bounds itself; a second instance
/// holding the flock past this window answers `TimeoutError` (audited).
pub const WRITE_GATE_BUDGET: Duration = Duration::from_secs(5);

/// `desktop.tree` gets a wider budget.
pub const TREE_BUDGET: Duration = Duration::from_secs(15);

/// Per-D-Bus-operation budget used by the adapters' transports.
pub const DBUS_BUDGET: Duration = Duration::from_secs(2);

/// Idempotent reads are retried exactly once; writes are never retried.
pub const READ_RETRIES: u8 = 1;
pub const WRITE_RETRIES: u8 = 0;

/// The limits document embedded into `session_info` (dto/caps.rs normative fields).
pub fn limits_document() -> omnibus_contracts::dto::caps::LimitsDocument {
    omnibus_contracts::dto::caps::LimitsDocument {
        tool_call_ms: i64::try_from(TOOL_CALL_BUDGET.as_millis()).unwrap_or(10_000),
        tree_ms: i64::try_from(TREE_BUDGET.as_millis()).unwrap_or(15_000),
        dbus_ms: i64::try_from(DBUS_BUDGET.as_millis()).unwrap_or(2_000),
        read_retries: READ_RETRIES,
    }
}

/// True when the tool is the wide-budget tree tool.
pub fn is_tree_tool(tool: &str) -> bool {
    tool == omnibus_contracts::capabilities::DESKTOP_TREE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_match_the_spec_defaults() {
        let doc = limits_document();
        assert_eq!(doc.tool_call_ms, 10_000);
        assert_eq!(doc.tree_ms, 15_000);
        assert_eq!(doc.dbus_ms, 2_000);
        assert_eq!(doc.read_retries, 1);
        assert!(is_tree_tool(omnibus_contracts::capabilities::DESKTOP_TREE));
        assert!(!is_tree_tool("journal_query"));
    }
}

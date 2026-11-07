# Contract: MCP Client <-> Daemon

## Boundary Rule

The dependency points from the client side to the daemon only through the MCP protocol over stdio;
the daemon never calls the client, and no other transport or control channel exists in v1.

## Interfaces

| Interface | Crate | Consumed By | Purpose |
| --------- | ----- | ----------- | ------- |
| `capabilities()` | `omnibus-core` (tool) | agent | machine-readable truth about this machine (P4) |
| `session_info` | `omnibus-core` (tool) | agent | session identity for audit correlation |
| `doctor` | `omnibus-core` (tool) | agent | diagnosis + ready commands only (never mutates) |
| 34 domain tools | `omnibus-core` facade, backed by SPI traits | agent | reads, typed writes, knowledge, network, app launch, process/system/power state |
| `tools/list` (hidden-tool rule) | `omnibus-core` facade | agent | tools without capabilities are absent |
| notifications: none in v1 | - | - | no push, no subscription surface (un-plan) |

The complete 37-tool registry with capability bindings lives in
[tool-registry.md](../architecture/tool-registry.md); this contract fixes the boundary semantics.

## Initialization

- The MCP client starts `omnibusd` as a child process; the stdio connection is the session
  (see [topology-and-lifecycle.md](../architecture/topology-and-lifecycle.md)).
- `omnibusd` runs a fresh probe, assembles adapters through the SPI registry, and produces the
  capability document; the tool list already reflects it before the first call.
- `actor` is recorded from `{uid, client_name (from MCP initialize), pid}`; there is no separate
  channel authentication - the trust anchor is the local user (standard desktop threat model).

## Data Flow Across Boundary

- Requests: tool name + JSON arguments (schemas generated from contract DTOs; hand-written schemas
  fail the build).
- Results: JSON values mapped from DTOs; errors use the closed eight-code taxonomy (see
  [error-taxonomy.md](../architecture/error-taxonomy.md)) - a hidden tool called by name returns
  `CapabilityRequired` with a doctor hint, never "method not found".
- Machine truth flows one way: the capability document is produced by the daemon; the client never
  supplies environment facts.
- Consent never crosses this boundary: no MCP method accepts a consent token or field, and an
  in-band "the user confirmed" is always ignored (see
  [security-model.md](../architecture/security-model.md)).

## Error Propagation

- Every tool result is either a DTO-shaped value or one of the eight typed codes.
- `ScopeDenied` always carries a one-line copy-paste remedy (`omnibusd grant <name>`).
- Native errors never surface: adapters map them to `NativeFailure{code}` with the original message
  preserved in the payload; the facade forwards them unchanged.

## Breaking Change Checklist

If you change the tool surface, you MUST also:

- Update the registry table in [tool-registry.md](../architecture/tool-registry.md) (the registry is
  closed; additions are additive minor contract changes - R1).
- Add capability constants in `omnibus-contracts` first (contract before code).
- Update the per-domain spec under `specs/domains/` and the capability registry in
  [capability-model.md](../architecture/capability-model.md).
- Regenerate JSON schemas from DTOs and re-run the contract suite (R2).

## Source Docs

- `docs/development/omnibus-spec.md` §4.
- `docs/development/omnibus-architecture.md` §3.1, §3.6, §5, §7.3.

## Related Specs

- [Tool Registry](../architecture/tool-registry.md) - the full binding table.
- [Topology and Lifecycle](../architecture/topology-and-lifecycle.md) - stdio-only rules.
- [Security Model](../architecture/security-model.md) - what may never cross this boundary.

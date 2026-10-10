# Audit

## Purpose

Total auditing of agent actions (P5): every tool call - reads and writes alike - produces exactly one
structured audit event. The event schema is owned by the core (an `omnibus-contracts` DTO); delivery
is pluggable through the `AuditSink` trait. Audit is what makes Omnibus verifiable: the consent
existed outside the agent, and the journal shows what happened.

## Key Files

- `omnibus-contracts/src/spi/` (planned) - the `AuditSink` trait and the `AuditEvent` DTO (task 1.2.1).
- `omnibus-core/src/audit/` (planned) - the facade-level pipeline (task 1.3.4).
- `omnibus-core/src/audit/file_sink.rs` (planned) - file-fallback sink (task 1.3.4).
- `omnibus-adapter-journald/src/` (planned) - the primary Linux sink (task 1.3.4).
- `omnibus-adapter-unifiedlog/src/` (planned) - the macOS sink, subsystem/category `omnibus` (task 3.3.1).

## Capability and Tools

Audit is not a tool: it is a core pipeline. There is no capability and no MCP surface; agents
observe audit only indirectly (via `capabilities()`/doctor notes about sink degradation).

## Behavior

```
agent --> any tool call ------------------> core facade
                                             | exactly one AuditEvent per call:
                                             | {ts, session, actor, tool, adapter,
                                             |  op, params_hash, result, scope_ref}
                                             v
                                     AuditSink::write(AuditEvent)
                                        |-- journald (Linux, primary)
                                        |-- unifiedlog (macOS, MS3)
                                        `-- file-fallback (tests / sink failure)
```

- The schema is fixed: `{ts, session, actor, tool, adapter, op, params_hash, result, scope_ref}`,
  with `actor = {uid, client_name (from MCP initialize), pid}`. Tool parameters are recorded as
  `params_hash`, never as raw parameters - sensitive data does not reach the log.
- Audit happens at the facade level: adapters never know about it and never write audit themselves.
- `session` is the session id (a session is the MCP connection) - it correlates events across one
  agent session.
- Grant-related events always carry the provenance of the grant: `config`, `cli`, or `doctor`.
- Denial events (`ScopeDenied`, `CapabilityRequired`) are audited with the denial reason.
- On macOS the audit event is readable back from the unified log by the same adapter
  (write -> query round trip).

## Degradation

A failing sink never fails the tool call: the event goes to the file-fallback sink, and the
degradation is visible in `capabilities()` and doctor. On macOS `journal.persistent` is always
`true` (the unified log is persistent within retention), so the audit trail has no persistence
opt-in there.

## Invariants

- Every tool call always produces exactly one structured audit event, including reads.
- The pipeline always runs at the facade level; adapters always stay audit-unaware.
- `params_hash` never carries raw parameters and never reversibly restores them.
- Grant-related events always carry provenance `config|cli|doctor`.
- A sink failure always degrades to file-fallback and never breaks the tool call.
- No write path is ever declared ready without a structured event in `AuditSink` (roadmap rule R5).

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - the `AuditSink` trait row.
- [Security Model](../architecture/security-model.md) - provenance and the actor model.
- [Capability Model](../architecture/capability-model.md) - sink degradation visibility.
- [Contract: MCP Client <-> Daemon](../contracts/mcp-client-daemon.md) - actor and session source.
- [Topology and Lifecycle](../architecture/topology-and-lifecycle.md) - session = connection.
- [Doctor](doctor.md) - sink degradation surfaced by doctor.
- [Config and Grants](../contracts/config-and-grants.md) - grant manifests behind provenance.

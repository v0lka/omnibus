# Audit

## Purpose

Total auditing of agent actions (P5): every tool call - reads and writes alike - produces exactly one
structured audit event. The event schema is owned by the core (an `omnibus-contracts` DTO); delivery
is pluggable through the `AuditSink` trait. Audit is what makes Omnibus verifiable: the consent
existed outside the agent, and the journal shows what happened. One bounded exception: a primary
write that lands just after its sink budget elapses can double-deliver alongside the file-fallback
copy - the pipeline treats the budget elapse as a sink failure and routes the event to the
fallback, while the late native write may still complete.

## Key Files

- `omnibus-contracts/src/spi/journal.rs` - the `AuditSink` trait (re-exported by `omnibus-contracts/src/spi/audit_sink.rs` next to the `Journal` it shares subsystem 2 with), the `AuditEvent` DTO in `omnibus-contracts/src/dto/audit.rs` (task 1.2.1).
- `omnibus-core/src/audit.rs` - the facade-level pipeline, file-fallback sink included (task 1.3.4).
- `omnibus-adapter-journald/src/sink.rs` - the primary Linux sink (task 1.3.4).
- `omnibus-adapter-unifiedlog/src/` (planned) - the macOS sink, subsystem/category `omnibus` (task 3.3.1).

## Capability and Tools

Audit is not a tool: it is a core pipeline. There is no capability and no MCP surface; agents
observe audit only indirectly (via doctor notes about sink degradation).

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
degradation is visible in doctor (sink-health findings); the startup `capabilities()` document
does not carry sink health. On macOS `journal.persistent` is always
`true` (the unified log is persistent within retention), so the audit trail has no persistence
opt-in there.

## Invariants

- Every tool call always produces exactly one structured audit event, including reads — one
  event *produced* per call; under the bounded sink-budget exception (Purpose) the same event may
  be *delivered* twice (late primary write plus fallback copy).
- The pipeline always runs at the facade level; adapters always stay audit-unaware.
- `params_hash` never carries raw parameters and never reversibly restores them: the
  digest is keyed with a per-install secret (persisted in the state directory), so
  low-entropy parameters cannot be recovered by enumeration from the journal. The
  state directory is therefore never readable through the file tools either (files.md
  read-scope invariant) - an agent that could read the key out of the state directory
  could enumerate the digests. Bounded degradation exception (same shape as the
  double-delivery exception above): when neither the persisted key file nor the
  entropy source is available, the key is derived from the state path, the process id
  and the start time - never persisted and not secret (the pid is journal-visible in
  every event's actor), so for that start the anti-enumeration property is degraded
  and a stderr warning says so. An existing key file that cannot be read likewise
  mints a fresh per-start key (with the same warning), which churns every per-install
  hash for that start.
- Grant-related events always carry provenance `config|cli|doctor`.
- A sink failure always degrades to file-fallback and never breaks the tool call.
- No write path is ever declared ready without a structured event in `AuditSink` (roadmap rule R5).

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - the `AuditSink` trait row.
- [Security Model](../architecture/security-model.md) - provenance and the actor model.
- [Contract: MCP Client <-> Daemon](../contracts/mcp-client-daemon.md) - actor and session source.
- [Topology and Lifecycle](../architecture/topology-and-lifecycle.md) - session = connection.
- [Doctor](doctor.md) - sink degradation surfaced by doctor.
- [Config and Grants](../contracts/config-and-grants.md) - grant manifests behind provenance.

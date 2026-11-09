# Topology and Lifecycle

## Context

Omnibus deliberately has no persistent daemon. `omnibusd` is a child of the MCP client over stdio;
the connection is the session; everything else (admin channels, TTLs, change watchers) was removed as
machinery without a consumer. This spec fixes the resulting lifecycle, concurrency, limits, and retry
rules.

## Process Topology

- Transport: MCP over stdio only. HTTP transport, a persistent daemon, and an admin control channel
  (socket/D-Bus) are out of v1.
- stdout carries only the MCP protocol; every diagnostic (session banner, degraded-audit-key
  warnings, doctor CLI output) goes to stderr - the operator's best-effort channel, never a
  machine-readable one. The structured truth for agents and admins is the audit trail (journald
  sink): a lost stderr line loses a warning, never a fact.
- One instance per MCP connection: N agents = N instances of the same binary.
- The daemon always lives and dies with the client process; the client owns start and shutdown.
- Persistent state = config only (scope manifests); everything else is ephemeral per session.

## Session = Connection

- A session lives and dies with its instance process; there is no TTL and no background timers.
- The session identifier exists only for audit correlation.
- The core mints every opaque token (`session_id`, `element_id`); an `element_id` issued by one
  instance never resolves in another instance (typed error; tested across two sequential instances).

## Fresh Probe per Session

- Every new instance runs a fresh probe and gets the current tool list.
- Environment changes (doctor CLI, config edits, package install) are picked up by the next session;
  there is no inotify, no file flag, and no push notifications to a running instance - the monitoring
  machinery never pays for itself against one reconnect.

## Concurrency Model

- Writes are globally serialized: in-process mutex + cross-process `flock` at
  `XDG_RUNTIME_DIR/omnibus/write.lock` between instances. The application order equals the lock
  acquisition order; the audit records the factual order. Two bounded exceptions: when the lock
  file cannot be created, the gate degrades to in-process serialization only (visible in doctor,
  never silent); and a write that outlives its adapter's write budget releases the gate while the
  native operation may still complete; the elapse answer is adapter-defined - a payload-bearing
  indeterminate `NativeFailure` where the adapter can name the possibly-still-running write, a
  plain `TimeoutError` elsewhere - and the audited event keeps the late outcome correlatable.
- Reads always run in parallel; no v1 scenario requires parallel writes.

## Limits and Retries

| Limit | Value | Note |
|---|---|---|
| Tool-call budget | 10 s | bounds reads; `desktop.tree` gets 15 s |
| Adapter write budgets | 9 s and below | the only timeout layer a write has (see Concurrency Model) |
| D-Bus operation | 2 s | per call |
| Retry policy | exactly one auto-retry for idempotent reads; writes never retried | exhaustion produces typed `TimeoutError` |

- Defaults are hardcoded (no config surface until a real request exists); the effective values are
  always visible to the agent via the `session_info` tool (`SessionInfo.limits`).

## Adapter Lifecycle (FSM)

`probe -> register -> serve -> shutdown` is a finite state machine owned by the core; an adapter
physically cannot be called before `register`. The order of tool calls inside a session is free.

## Invariants

- The daemon never outlives its MCP client process.
- Two write operations always apply in lock acquisition order, with the audit preserving that
  order - except under the two bounded exceptions above: a degraded process-local gate serializes
  only within one instance, and a timed-out write releases the gate and may still settle after a
  later write has applied.
- A write always executes at most once per call - the core never retries it.
- Timeout/retry behavior is always observable by the agent via the audit trail or doctor.

## Anti-Patterns

- Adding a supervisor/daemon "for convenience" - it re-creates the admin-channel problem the stdio
  topology removed.
- Retrying a write to "fix" a flaky adapter - writes are side-effectful and never retried.

## Source Docs

- `docs/development/omnibus-architecture.md` §3.1, §3.2, §3.9, §3.10, §7.2.
- `docs/development/omnibus-roadmap.md` tasks 1.3.1, 1.3.2, un-plan §5.4.

## Related Specs

- [Security Model](security-model.md) - grants and the actor model.
- [Contract: MCP client <-> daemon](../contracts/mcp-client-daemon.md).

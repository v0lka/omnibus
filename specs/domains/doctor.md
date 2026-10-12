# Doctor

## Purpose

The only core tool that orchestrates several subsystem facts: it diagnoses the machine against Omnibus
prerequisites, leads
the human through opt-ins, and returns ready-to-paste remedies. Doctor is orchestration - it composes
the probe report, capability entries, adapter-registry presence, and audit-sink and write-gate state,
and never mutates the system when reached through MCP; mutations are executed only by a
human.

## Key Files

- `omnibus-core/src/doctor.rs` - core orchestration of the subsystem facts (roadmap task 1.7.1; P6).
- `omnibus-probe/` - re-reads platform state at instance start, so post-doctor changes surface in the next session (task 2.5.1).

## Capability and Tools

| MCP tool | Capabilities | Composition |
| --- | --- | --- |
| `doctor` (core) | none required | composes subsystem facts: the probe report, capability entries, adapter-registry presence, audit-sink and write-gate state |

Doctor is a core tool: it always lives in `omnibus-core` and composes subsystem facts (P6: an
adapter never calls another adapter). It never requires a capability and is never hidden.

## Behavior

```
agent --> doctor (over MCP) ---> core: probe report + capability entries + adapter-registry presence + audit-sink and write-gate state
      <-- diagnosis + copy-paste remedy ONLY (never a mutation)
human --> omnibusd doctor (CLI) -or- the printed command
      <-- executes the opt-in mutations (human hands only)
next session --> fresh probe --> updated capabilities() and tool list
```

- The MCP channel produces diagnosis plus a copy-paste remedy - the same principle as the
  `ScopeDenied` remedy. Doctor over MCP can never mutate the system regardless of the agent's
  answers; a CI test with a "consenting" agent verifies the denial.
- Mutations happen only in the CLI mode `omnibusd doctor` (or by the human running the printed
  command): consent is always collected outside the agent loop.
- Opt-ins led by doctor: persistent journald (the flagship "laptop did not wake up" diagnostics on
  any machine the probe reports with a volatile journald config - roadmap 2.5.1; the ready command
  is `omnibusd grant journal.persistent`, and the past-boot view additionally requires the human to
  create persistent storage by hand - `journal.persistent=true` appears only after that storage
  mutation, `journal.prev_boot` after the following reboot), and the macOS TCC wizard; the per-app
  Electron a11y-flag opt-in is planned (roadmap F1.7.1 remainder).
- The TCC wizard walks the human through Accessibility, Screen Recording, Automation, and Full Disk
  Access: doctor opens the right panel, checks the state, and re-probes. It does not "issue"
  grants - without GUI interaction (or MDM/PPPC) the grant never appears. Monthly Screen Recording
  re-grant reminders are driven by grant state (macOS 15+).
- Degradation-ladder role: every `CapabilityRequired` error carries a doctor hint.
- Onboarding: doctor finds the missing prerequisites and hands the human a ready command; TTFM
  under 5 minutes is the onboarding autotest's target (roadmap 2.6.1 AC) - the clean-VM
  measurement is external work (ms1-ms2 acceptance, REQ-EXT). There is no telemetry in v1
  (the autotest replaces it).
- Every doctor-driven system change is one granular, separately audited opt-in - never a "fix
  everything"; a declined opt-in leaves the read-only status quo without repeated pushy offers.
- Changes (doctor CLI, config edits) are picked up by the next session's fresh probe: a session is
  a connection, and a new instance sees the updated tool list.

## Degradation

Doctor is the guide across all four ladder states rather than a subject of one: it explains
degraded schemas, names hidden tools behind `CapabilityRequired`, reports below-floor reasons, and
walks the human through `opt-in-required` grants (TCC or `[optin]` manifests). After the human
executes the remedy, the next session's probe reflects the change when the change is a machine fact
(example: the macOS TCC grant); a grant alone does not move a probe fact - `journal.persistent=true`
appears once the human has also created persistent storage, `prev_boot` after the following reboot).

## Invariants

- Doctor over MCP never mutates the system, whatever the agent answers.
- Every opt-in mutation is always executed by a human, one granular change at a time.
- Every doctor-driven system change always produces its own audit event.
- Doctor never proposes anything from the never-list.
- Doctor never attempts programmatic TCC issuance; without GUI interaction the grant never appears.
- Post-doctor state is always picked up by the next session's fresh probe, never by a running one.

## Related Specs

- [Capability Model](../architecture/capability-model.md) - the ladder states doctor explains.
- [Security Model](../architecture/security-model.md) - the three-tier change policy.
- [Topology and Lifecycle](../architecture/topology-and-lifecycle.md) - fresh probe per session.
- [SPI Registry](../architecture/spi-registry.md) - P6: orchestration lives in the core.
- [Contract: MCP Client <-> Daemon](../contracts/mcp-client-daemon.md) - doctor boundary semantics.
- [Audit](audit.md) - audit events for doctor-driven changes.
- [Desktop Accessibility](desktop-a11y.md) - opt-ins led by doctor.
- [Screen Capture](screen-capture.md) - Screen Recording re-grant reminders.

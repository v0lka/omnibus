# Doctor

## Purpose

The only core tool that orchestrates several traits: it diagnoses the machine against Omnibus
prerequisites, leads
the human through opt-ins, and returns ready-to-paste remedies. Doctor is orchestration - it composes
four traits and never mutates the system when reached through MCP; mutations are executed only by a
human.

## Key Files

- `omnibus-core/src/tools/doctor.rs` (planned) - core orchestration of four traits (roadmap task 1.7.1; P6).
- `omnibus-probe/` (planned) - re-reads platform state at instance start, so post-doctor changes surface in the next session (task 2.5.1).

## Capability and Tools

| MCP tool | Capabilities | Trait composition |
| --- | --- | --- |
| `doctor` (core) | none required | `PlatformProbe` + `Journal` + `ServiceManager` + `KnowledgeBase` |

Doctor is a core tool: it always lives in `omnibus-core` and orchestrates traits (P6: an adapter
never calls another adapter). It never requires a capability and is never hidden.

## Behavior

```
agent --> doctor (over MCP) ---> core: probe + journal + services + KB
      <-- diagnosis + copy-paste remedy ONLY (never a mutation)
human --> omnibus doctor (CLI) -or- the printed command
      <-- executes the opt-in mutations (human hands only)
next session --> fresh probe --> updated capabilities() and tool list
```

- The MCP channel produces diagnosis plus a copy-paste remedy - the same principle as the
  `ScopeDenied` remedy. Doctor over MCP can never mutate the system regardless of the agent's
  answers; a CI test with a "consenting" agent verifies the denial.
- Mutations happen only in the CLI mode `omnibus doctor` (or by the human running the printed
  command): consent is always collected outside the agent loop.
- Opt-ins led by doctor: persistent journald (the flagship "laptop did not wake up" diagnostics on
  Ubuntu/Mint, plus Debian/Fedora wherever the M0 check shows a volatile default - roadmap 2.5.1;
  `journal.prev_boot` appears after the next reboot), per-app Electron a11y flags, and the macOS
  TCC wizard.
- The TCC wizard walks the human through Accessibility, Screen Recording, Automation, and Full Disk
  Access: doctor opens the right panel, checks the state, and re-probes. It does not "issue"
  grants - without GUI interaction (or MDM/PPPC) the grant never appears. Monthly Screen Recording
  re-grant reminders are driven by grant state (macOS 15+).
- Degradation-ladder role: every `CapabilityRequired` error carries a doctor hint.
- Onboarding: doctor finds the missing prerequisites and hands the human a ready command; TTFM
  under 5 minutes is verified by an onboarding CI test on a fresh VM. There is no telemetry in v1
  (the autotest replaces it).
- Every doctor-driven system change is one granular, separately audited opt-in - never a "fix
  everything"; a declined opt-in leaves the read-only status quo without repeated pushy offers.
- Changes (doctor CLI, config edits) are picked up by the next session's fresh probe: a session is
  a connection, and a new instance sees the updated tool list.

## Degradation

Doctor is the guide across all four ladder states rather than a subject of one: it explains
degraded schemas, names hidden tools behind `CapabilityRequired`, reports below-floor reasons, and
walks the human through `opt-in-required` grants (TCC or `[optin]` manifests). After the human
executes the remedy, the next session's probe reflects the change (example: `journal.persistent=true`
visible to the next instance, `prev_boot` after the following reboot).

## Invariants

- Doctor over MCP never mutates the system, whatever the agent answers.
- Every opt-in mutation is always executed by a human, one granular change at a time.
- Every doctor-driven system change always produces its own audit event.
- Doctor never proposes anything from the never-list (verified by a list-driven CI test).
- Doctor never attempts programmatic TCC issuance; without GUI interaction the grant never appears.
- Post-doctor state is always picked up by the next session's fresh probe, never by a running one.

## Related Specs

- [Capability Model](../architecture/capability-model.md) - the ladder states doctor explains.
- [Security Model](../architecture/security-model.md) - the three-tier change policy.
- [Topology and Lifecycle](../architecture/topology-and-lifecycle.md) - fresh probe per session.
- [SPI Registry](../architecture/spi-registry.md) - P6: orchestration lives in the core.
- [Contract: MCP Client <-> Daemon](../contracts/mcp-client-daemon.md) - doctor boundary semantics.
- [Audit](audit.md) - audit events for doctor-driven changes.
- [Desktop Accessibility](desktop-a11y.md) - the Electron a11y-flag opt-in.
- [Screen Capture](screen-capture.md) - Screen Recording re-grant reminders.

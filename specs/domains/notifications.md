# Notifications

## Purpose

Sending desktop notifications as a typed, scope-gated write. A notification is a user-visible side
effect outside the agent's own transport, so the domain applies the write machinery (grant, audit)
even though the action looks harmless.

## Key Files

- `omnibus-contracts/src/spi/portals.rs` - the `Notify` trait; the `portal.notify` constant in `omnibus-contracts/src/capabilities.rs` (roadmap task 1.2.1).
- `omnibus-adapter-xdg-portals/src/lib.rs` - Linux provider: notifications over D-Bus (task 1.5.5).
- `omnibus-adapter-screencapture/src/` (planned) - macOS provider: UNUserNotificationCenter, MS3 (task 3.5.1).

## Capability and Tools

| MCP tool | Capabilities | Grant | Trait op |
| --- | --- | --- | --- |
| `notify` [+scope] | `portal.notify` | write grant `portal.notify` | `Notify::send` |

## Behavior

```
agent --> notify(title, body) [+scope]
      <-- result ----------------------------> grant check --> Notify::send --> provider
                                                                            |            |
                                                  audit event (facade) <----+            |
                                                                            Linux: D-Bus notifications
                                                                            macOS: UNUserNotificationCenter
```

- Sending a notification is a typed write: it produces a user-visible side effect, so it requires
  the `portal.notify` write grant issued by a human outside the agent loop.
- Every call is audited: one structured event per notification attempt, success or denial.
- On macOS `notify` does not require Screen Recording (only `screenshot` does) and works once the
  backend is present.
- A missing backend is visible per machine state: a missing session bus hides the whole portals
  adapter; an absent notification service on an existing bus reports the capability `degraded`
  (the tool stays visible and calls answer `AdapterUnavailable`).

## Degradation

| Ladder state | This domain |
| --- | --- |
| Degraded | With the portal service present (the adapter exists): no notification service on the session bus — the fact reports `degraded` with the reason, the tool stays visible with the limitation marked, and calls answer `AdapterUnavailable`. |
| Missing capability | The portals adapter is absent this session — no session bus, or the probe found no portal service: capabilities absent, tools hidden from `tools/list`; a direct call returns `CapabilityRequired` with a doctor hint. |
| Below version floor | Portal interfaces below the floor deactivate the adapter; the reason is visible in `capabilities()`. |
| Opt-in required | Not applicable: the grant is a TOML write grant (`portal.notify`), not a TCC-style opt-in. |

A `notify` call with capability present but grant absent returns typed `ScopeDenied` with a one-line
copy-paste remedy.

## Invariants

- `notify` always requires the `portal.notify` write grant issued by a human outside the agent loop.
- Every notification attempt always produces exactly one structured audit event.
- A notification is always treated as a user-visible system write, never as a free side effect.
- An absent portal adapter hides the tool; an absent notification service keeps it visible,
  answering `AdapterUnavailable`.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - the portals subsystem and the `Notify` trait.
- [Tool Registry](../architecture/tool-registry.md) - binding of `notify`.
- [Capability Model](../architecture/capability-model.md) - `portal.notify` and ladder states.
- [Security Model](../architecture/security-model.md) - write grants and `ScopeDenied` remedies.
- [Platform Matrix](../architecture/platform-matrix.md) - providers per platform.
- [Audit](audit.md) - the event every notification produces.

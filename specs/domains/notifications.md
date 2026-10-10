# Notifications

## Purpose

Sending desktop notifications as a typed, scope-gated write. A notification is a user-visible side
effect outside the agent's own transport, so the domain applies the write machinery (grant, audit)
even though the action looks harmless.

## Key Files

- `omnibus-contracts/src/spi/` (planned) - `Notify` trait, the `portal.notify` constant (roadmap task 1.2.1).
- `omnibus-adapter-xdg-portals/src/` (planned) - Linux provider: notifications over D-Bus (task 1.5.5).
- `omnibus-adapter-screencapture/src/` (planned) - macOS provider: UNUserNotificationCenter, MS3 (task 3.5.1).

## Capability and Tools

| MCP tool | Capabilities | Grant | Trait op |
| --- | --- | --- | --- |
| `notify` [+scope `notify.send`] | `portal.notify` | write grant `notify.send` | `Notify::notify` |

## Behavior

```
agent --> notify(title, body) [+scope notify.send]
      <-- result ----------------------------> grant check --> Notify::notify --> provider
                                                                            |            |
                                                  audit event (facade) <----+            |
                                                                            Linux: D-Bus notifications
                                                                            macOS: UNUserNotificationCenter
```

- Sending a notification is a typed write: it produces a user-visible side effect, so it requires
  the `notify.send` grant issued by a human outside the agent loop.
- Every call is audited: one structured event per notification attempt, success or denial.
- On macOS `notify` does not require Screen Recording (only `screenshot` does) and works once the
  backend is present.
- A missing backend is detected by the probe; the capability is declared per detected backend.

## Degradation

| Ladder state | This domain |
| --- | --- |
| Degraded | Not observed in the sources: notify is either present or hidden. |
| Missing capability | No notification backend detected: `notify` hidden from `tools/list`; a direct call returns `CapabilityRequired` with a doctor hint. |
| Below version floor | Portal interfaces below the floor deactivate the adapter; the reason is visible in `capabilities()`. |
| Opt-in required | Not applicable: the grant is a TOML write grant (`notify.send`), not a TCC-style opt-in. |

A `notify` call with capability present but grant absent returns typed `ScopeDenied` with a one-line
copy-paste remedy.

## Invariants

- `notify` always requires the `notify.send` grant issued by a human outside the agent loop.
- Every notification attempt always produces exactly one structured audit event.
- A notification is always treated as a user-visible system write, never as a free side effect.
- A missing backend always hides the tool instead of failing at call time.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - the portals subsystem and the `Notify` trait.
- [Tool Registry](../architecture/tool-registry.md) - binding of `notify`.
- [Capability Model](../architecture/capability-model.md) - `portal.notify` and ladder states.
- [Security Model](../architecture/security-model.md) - write grants and `ScopeDenied` remedies.
- [Platform Matrix](../architecture/platform-matrix.md) - providers per platform.
- [Audit](audit.md) - the event every notification produces.

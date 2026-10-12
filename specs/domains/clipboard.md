# Clipboard

## Purpose

Clipboard access in both directions: reading the clipboard as a plain read, and substituting its
content as a typed, scope-gated write. The write side exists because clipboard substitution is a
system write and an injection vector, so it is treated with the full write machinery, not as a
portal convenience.

## Key Files

- `omnibus-contracts/src/spi/portals.rs` - the `ScreenCapture` trait ops `clip_read`/`clip_write`; the `portal.clipboard` constant in `omnibus-contracts/src/capabilities.rs` (roadmap task 1.2.1).
- `omnibus-adapter-xdg-portals/src/lib.rs` - Linux provider: the `xdg-desktop-portal` Clipboard interface (the `wl-clipboard`/`xclip` backend alternative is planned, not shipped) (task 1.5.5).
- `omnibus-adapter-screencapture/src/` (planned) - macOS provider on NSPasteboard, MS3 (task 3.5.1).

## Capability and Tools

| MCP tool | Capabilities | Grant | Trait op |
| --- | --- | --- | --- |
| `clip_read` | `portal.clipboard` | none | `ScreenCapture::clip_read` |
| `clip_write` [+scope] | `portal.clipboard` | write grant `portal.clipboard` | `ScreenCapture::clip_write` |

Backend detection (`wl-clipboard`/`xclip` auto-detection) is a planned extension, not part of the
shipped probe report: 0.1 declares the capability through the portal Clipboard interface alone.

## Behavior

```
agent --> clip_read -----------------> core --> ScreenCapture::clip_read --> provider
      <-- clipboard DTO                                   |
                                audit event <-------------+
agent --> clip_write [+scope]
      <-- result -----------------> grant check --> typed write --> provider
                                                                            Linux: xdg-desktop-portal
                                                                                   (wl-clipboard / xclip: planned)
                                                                            macOS: NSPasteboard (MS3)
```

- `clip_write` is a typed write tool: substitution of clipboard content changes system state and is
  a known injection vector, so it is gated by the `portal.clipboard` write grant issued by a human.
- Reads and writes alike are audited: every call produces one structured audit event at the facade
  level.
- On macOS clipboard tools do not require Screen Recording - only `screenshot` does.
- A portal clipboard round-trip is future work (it needs the RemoteDesktop session omnibus does
  not create): the suite covers the honest `AdapterUnavailable` refusal for `clip_read`, with `clip_write`'s absent-backend refusal covered by adapter-level tests
  today.

## Degradation

| Ladder state | This domain |
| --- | --- |
| Degraded | The shipped state with the portal service present (the adapter exists): the portal Clipboard interface is session-gated (usable clipboard requires a RemoteDesktop session omnibus does not create) or absent — the fact reports `degraded` either way, the tools stay visible, and calls answer `AdapterUnavailable` honestly. |
| Missing capability | The portals adapter is absent this session — no session bus, or the probe found no portal service: capabilities absent, tools hidden from `tools/list` (`CapabilityRequired` with a doctor hint). |
| Below version floor | Portal interfaces below the floor deactivate the adapter; the reason is visible in `capabilities()`. |
| Opt-in required | Not applicable on Linux (read is default-allowed; the write needs only a TOML grant). On macOS the tools work without Screen Recording. |

A `clip_write` call with capability present but grant absent returns typed `ScopeDenied` with a
one-line copy-paste remedy.

## Invariants

- `clip_write` always requires the `portal.clipboard` write grant issued by a human outside the agent loop.
- Every clipboard read and write always produces exactly one structured audit event.
- Clipboard substitution is always treated as a typed system write; it is never performed through
  input emulation.
- The absence of the whole portals adapter hides both tools; an absent or session-gated Clipboard
  interface keeps them visible, answering `AdapterUnavailable`; a missing grant always denies only
  the write.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - the portals subsystem and trait ops.
- [Tool Registry](../architecture/tool-registry.md) - binding of `clip_read`/`clip_write`.
- [Capability Model](../architecture/capability-model.md) - `portal.clipboard` and ladder states.
- [Security Model](../architecture/security-model.md) - write grants and `ScopeDenied` remedies.
- [Error Taxonomy](../architecture/error-taxonomy.md) - `ScopeDenied` semantics.
- [Audit](audit.md) - the event every clipboard call produces.

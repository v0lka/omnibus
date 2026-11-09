# Clipboard

## Purpose

Clipboard access in both directions: reading the clipboard as a plain read, and substituting its
content as a typed, scope-gated write. The write side exists because clipboard substitution is a
system write and an injection vector, so it is treated with the full write machinery, not as a
portal convenience.

## Key Files

- `omnibus-contracts/src/spi/portals.rs` - the `ScreenCapture` trait ops `clip_read`/`clip_write`; the `portal.clipboard` constant in `omnibus-contracts/src/capabilities.rs` (roadmap task 1.2.1).
- `omnibus-adapter-xdg-portals/src/lib.rs` - Linux provider: the `xdg-desktop-portal` service for screenshot/notify; `clip_read`/`clip_write` ride the detected `wl-clipboard`/`xclip` backend (task 1.5.5).
- `omnibus-adapter-screencapture/src/` (planned) - macOS provider on NSPasteboard, MS3 (task 3.5.1).

## Capability and Tools

| MCP tool | Capabilities | Grant | Trait op |
| --- | --- | --- | --- |
| `clip_read` | `portal.clipboard` | none | `ScreenCapture::clip_read` |
| `clip_write` [+scope] | `portal.clipboard` | write grant `portal.clipboard` | `ScreenCapture::clip_write` |

Backend detection (`wl-clipboard`/`xclip` auto-detection) is shipped: the portal Clipboard channel
is session-gated behind RemoteDesktop, so a real round trip rides the platform's own tooling -
`wl-clipboard` (both binaries) first, `xclip` as the X11 fallback, detected in `/usr/bin` like every
probe fact. The `portal.clipboard` fact reports `present` with the backend named in the note;
without any detected backend the fact is absent and the ladder hides both tools.

## Behavior

```
agent --> clip_read -----------------> core --> ScreenCapture::clip_read --> provider
      <-- clipboard DTO                                   |
                                audit event <-------------+
agent --> clip_write [+scope]
      <-- result -----------------> grant check --> typed write --> provider
                                                                            Linux: xdg-desktop-portal
                                                                                   (wl-clipboard / xclip backend)
                                                                            macOS: NSPasteboard (MS3)
```

- `clip_write` is a typed write tool: substitution of clipboard content changes system state and is
  a known injection vector, so it is gated by the `portal.clipboard` write grant issued by a human.
- Reads and writes alike are audited: every call produces one structured audit event at the facade
  level.
- On macOS clipboard tools do not require Screen Recording - only `screenshot` does.
- A portal clipboard round-trip stays out (it needs the RemoteDesktop session omnibus does
  not create): the suite covers the backend-backed `clip_read`/`clip_write` round trip (content
  preserved), and the honest refusals - without a detected backend the capability fact is absent
  and a direct call answers `AdapterUnavailable`.

## Degradation

| Ladder state | This domain |
| --- | --- |
| Degraded | No degraded state is defined for the clipboard pair: a detected backend serves both tools fully (the fact note names the backend and the unused session-gated portal channel); without a backend the fact is absent - the Missing capability row, with a direct call answering `AdapterUnavailable`. |
| Missing capability | The portals adapter is absent this session (no session bus, or the probe found no portal service), or no clipboard backend is detected: the `portal.clipboard` fact is absent, the tools hide from `tools/list` (`CapabilityRequired` with a doctor hint). |
| Below version floor | Portal interfaces below the floor deactivate the adapter; the reason is visible in `capabilities()`. |
| Opt-in required | Not applicable on Linux (read is default-allowed; the write needs only a TOML grant). On macOS the tools work without Screen Recording. |

A `clip_write` call with capability present but grant absent returns typed `ScopeDenied` with a
one-line copy-paste remedy.

## Invariants

- `clip_write` always requires the `portal.clipboard` write grant issued by a human outside the agent loop.
- Every clipboard read and write always produces exactly one structured audit event.
- Clipboard substitution is always treated as a typed system write; it is never performed through
  input emulation.
- The absence of the whole portals adapter hides both tools; an absent clipboard backend hides
  them the same way (the session-gated portal Clipboard interface alone never serves them); a
  missing grant always denies only the write.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - the portals subsystem and trait ops.
- [Tool Registry](../architecture/tool-registry.md) - binding of `clip_read`/`clip_write`.
- [Capability Model](../architecture/capability-model.md) - `portal.clipboard` and ladder states.
- [Security Model](../architecture/security-model.md) - write grants and `ScopeDenied` remedies.
- [Error Taxonomy](../architecture/error-taxonomy.md) - `ScopeDenied` semantics.
- [Audit](audit.md) - the event every clipboard call produces.

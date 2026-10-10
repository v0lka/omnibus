# Clipboard

## Purpose

Clipboard access in both directions: reading the clipboard as a plain read, and substituting its
content as a typed, scope-gated write. The write side exists because clipboard substitution is a
system write and an injection vector, so it is treated with the full write machinery, not as a
portal convenience.

## Key Files

- `omnibus-contracts/src/spi/` (planned) - `ScreenCapture` trait ops `clip_read`/`clip_write`, the `portal.clipboard` constant (roadmap task 1.2.1).
- `omnibus-adapter-xdg-portals/src/` (planned) - Linux provider: `xdg-desktop-portal` + `wl-clipboard`/`xclip` (task 1.5.5).
- `omnibus-adapter-screencapture/src/` (planned) - macOS provider on NSPasteboard, MS3 (task 3.5.1).

## Capability and Tools

| MCP tool | Capabilities | Grant | Trait op |
| --- | --- | --- | --- |
| `clip_read` | `portal.clipboard` | none | `ScreenCapture::clip_read` |
| `clip_write` [+scope `clip.write`] | `portal.clipboard` | write grant `clip.write` | `ScreenCapture::clip_write` |

Backend detection (`wl-clipboard`/`xclip` auto-detection) is part of the probe report; the
capability is declared per detected backend.

## Behavior

```
agent --> clip_read -----------------> core --> ScreenCapture::clip_read --> provider
      <-- clipboard DTO                                   |
                                audit event <-------------+
agent --> clip_write [+scope clip.write]
      <-- result -----------------> grant check --> typed write --> provider
                                                                            Linux: xdg-desktop-portal
                                                                                   + wl-clipboard / xclip
                                                                            macOS: NSPasteboard (MS3)
```

- `clip_write` is a typed write tool: substitution of clipboard content changes system state and is
  a known injection vector, so it is gated by the `clip.write` grant issued by a human.
- Reads and writes alike are audited: every call produces one structured audit event at the facade
  level.
- On macOS clipboard tools do not require Screen Recording - only `screenshot` does.
- Round-trip (read after write) is covered by integration tests on GNOME and KDE, X11 and Wayland,
  in a virtual session.

## Degradation

| Ladder state | This domain |
| --- | --- |
| Degraded | Not observed in the sources: clipboard is either present or hidden. |
| Missing capability | No backend (e.g. a stand without `wl-clipboard`): both tools hidden; `capabilities()` explains. |
| Below version floor | Portal interfaces below the floor deactivate the adapter; the reason is visible in `capabilities()`. |
| Opt-in required | Not applicable on Linux (read is default-allowed; the write needs only a TOML grant). On macOS the tools work without Screen Recording. |

A `clip_write` call with capability present but grant absent returns typed `ScopeDenied` with a
one-line copy-paste remedy.

## Invariants

- `clip_write` always requires the `clip.write` grant issued by a human outside the agent loop.
- Every clipboard read and write always produces exactly one structured audit event.
- Clipboard substitution is always treated as a typed system write; it is never performed through
  input emulation.
- A missing backend always hides both tools; a missing grant always denies only the write.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - the portals subsystem and trait ops.
- [Tool Registry](../architecture/tool-registry.md) - binding of `clip_read`/`clip_write`.
- [Capability Model](../architecture/capability-model.md) - `portal.clipboard` and ladder states.
- [Security Model](../architecture/security-model.md) - write grants and `ScopeDenied` remedies.
- [Error Taxonomy](../architecture/error-taxonomy.md) - `ScopeDenied` semantics.
- [Audit](audit.md) - the event every clipboard call produces.

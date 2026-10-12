# Screen Capture

## Purpose

Screenshot capture as a read-only capability of the portals subsystem. The domain is small on the
surface (one tool) but carries a documented platform asymmetry: the Linux path has no Omnibus-side
gate, while macOS is gated by TCC Screen Recording.

## Key Files

- `omnibus-contracts/src/spi/portals.rs` - the `ScreenCapture` trait; the `portal.screenshot` constant in `omnibus-contracts/src/capabilities.rs` (roadmap task 1.2.1).
- `omnibus-adapter-xdg-portals/src/lib.rs` - Linux provider: `screenshot` (portal) (task 1.5.5).
- `omnibus-adapter-screencapture/src/` (planned) - macOS provider on ScreenCaptureKit, MS3 (task 3.5.1).

## Capability and Tools

| MCP tool | Capabilities | Grant | Trait op |
| --- | --- | --- | --- |
| `screenshot` | `portal.screenshot` | none (read-only) | `ScreenCapture::screenshot` |

`screenshot` never requires a scope grant: it is a read. The `portal.screenshot` capability is
declared by the probe per detected backend; with a session bus but an unpublished Screenshot
interface the capability reports `degraded` and the tool stays visible; without a session bus
(no portals adapter at all) the tool is hidden.

## Behavior

```
agent --> screenshot ----------> core --> ScreenCapture::screenshot --> provider
      <-- image DTO                                        |                |
                            audit event (facade level) <---+                |
                                                    Linux: xdg-desktop-portal (Wayland)
                                                           plain X11 path
                                                    macOS: ScreenCaptureKit (MS3)
```

- Wayland: capture goes through `xdg-desktop-portal`. The DE portal is not an Omnibus contract -
  its consent dialog belongs to the desktop environment.
- X11: the plain path has no system gate at all. This is an accepted read risk of v1, documented in
  the security model; the revisit condition is an opt-in grant on proven demand.
- macOS: ScreenCaptureKit behind TCC Screen Recording. On macOS 15+ the grant is re-confirmed
  monthly; doctor issues the reminders driven by grant state (see [Doctor](doctor.md)).
- Every call is audited like every read, regardless of whether a system gate exists.

## Degradation

| Ladder state | This domain |
| --- | --- |
| Degraded | With the portal service present (the adapter exists): the portal Screenshot interface is not published on this machine — the fact reports `degraded` with the reason, the tool stays visible with the limitation marked, and calls answer `AdapterUnavailable`. |
| Missing capability | The portals adapter is absent this session — no session bus, or the probe found no portal service: capabilities absent, tools hidden from `tools/list` (`CapabilityRequired` with a doctor hint). On macOS an absent ScreenCaptureKit backend is the opt-in state below. |
| Below version floor | Portal interfaces below the `xdg-desktop-portal` floor deactivate the adapter; `capabilities()` explains the reason. |
| Opt-in required | macOS: Screen Recording not granted (or expired/revoked) - status `opt-in-required`; the tool stays hidden until the human issues the grant via TCC; doctor explains the monthly re-grant. |

On Linux the tool is exposed by default (reads are default-allowed); the accepted X11 risk is a
documented posture, not a ladder state.

## Invariants

- `screenshot` is always audited like every read, even where no system gate exists.
- `screenshot` always requires only `portal.screenshot` and never a scope grant.
- The Wayland DE portal is never treated as part of the Omnibus contract.
- macOS Screen Recording is always issued by a human through GUI or MDM/PPPC; no adapter or doctor
  flow ever attempts programmatic TCC.
- An absent portal adapter hides the tool; an unpublished Screenshot interface keeps it visible,
  answering `AdapterUnavailable`; macOS without the TCC grant is the opt-in state, never a
  call-time surprise.

## Related Specs

- [Security Model](../architecture/security-model.md) - the accepted read risk and audit rule.
- [Tool Registry](../architecture/tool-registry.md) - binding of `screenshot`.
- [Capability Model](../architecture/capability-model.md) - `portal.screenshot` detection and ladder.
- [Platform Matrix](../architecture/platform-matrix.md) - providers per platform and floors.
- [Non-Goals](../architecture/non-goals.md) - the accepted-risk posture; un-plan items carry revisit conditions.
- [Doctor](doctor.md) - monthly Screen Recording re-grant reminders.

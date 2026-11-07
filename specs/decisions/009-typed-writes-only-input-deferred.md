# ADR-009: Typed Writes Only; Input Emulation Deferred

## Status

Accepted

## Context

Input emulation (ydotool on Linux, CGEvent on macOS) was planned as the input fallback of last
resort. Its cost: a separate adapter, udev rights on `/dev/uinput` with known fragility, a
`ydotoold` service, doctor onboarding, an opt-in grant, per-session probe checks, and X11/Wayland
matrix rows - while no scenario, demo, or principle terminates in emulated input, and the a11y
ladder already handles apps without an a11y tree honestly.

## Decision

v1 performs writes only through typed methods (`desktop.set_value`, `desktop.invoke`, and the
other scope-gated tools). Keyboard/mouse emulation is outside v1 for both platforms and returns
only on a proven, demonstrated case as an opt-in extension. P2 states write = typed method,
always.

## Consequences

- MS1 loses a whole adapter, a service dependency, a doctor flow, and a fragile udev grant from
  its critical path.
- The write surface stays fully auditable per operation - emulation would blur "what exactly was
  done".

## Alternatives Considered

- Keep ydotool as the MS1 fallback - rejected: heavy last-resort machinery with no consumer;
  honest deferral beats speculative coverage.

## Source Docs

- `docs/development/omnibus-spec.md` sections 2, 7.
- `docs/development/omnibus-roadmap.md` un-plan section 5.4.

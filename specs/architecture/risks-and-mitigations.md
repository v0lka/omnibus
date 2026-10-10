# Risks and Mitigations

## Context

Every risk below is stated in the source docs (`omnibus-spec.md` §6, `omnibus-roadmap.md` §4.2) with
its mitigation already built into the plan. This spec is the normative restatement: what can hurt the
project, which task or design element closes it, and which risks are accepted rather than mitigated.
The five ⚠M0 open questions (`omnibus-architecture.md` §10) are handled here as verification gates,
not as unresolved design.

## Risk Register

| Risk | Class | Mitigation | Closed by |
|---|---|---|---|
| Electron apps without the a11y flag (per-app enable needed) | platform | doctor leads the per-app enable flow | MS1, roadmap F1.7.1 |
| Distro default fragmentation (volatile journald on Ubuntu/Mint; NM absent behind netplan/networkd) | platform | version floors + opt-in persistence via doctor + honest capability schemas | MS2, roadmap F2.1.2, F2.2.3, F2.5.1 |
| QA matrix 6 distros x DE x X11/Wayland | process | tier model: T1 full CI, T2 smoke on reference versions, T3 community | MS2, roadmap F2.6.2 |
| macOS TCC friction (human grants via GUI only; Screen Recording re-grant monthly on 15+) | platform | doctor TCC wizard walks the human; reminders by grant state; headless/SSH behavior investigated before work starts | MS3, roadmap F3.1, F3.7.1 |
| objc2 AX bindings weaker than expected | feasibility | M0 gate with explicit escalation: if AC-4 of F3.1.1 fails, work escalates to F3.2 planning | MS3, roadmap F3.1.1 |
| Vendor race (RHEL Lightspeed; Apple building a Siri channel) | market | not closed by tasks; mitigated by MS1->MS2 speed and the open contract (MCP, ACP) | accepted - no task owns it |
| `screenshot` on Linux gated only by the DE portal (Wayland) or nothing (X11) | security (read) | accepted risk of v1 for a read operation, always audited; macOS gates via TCC; revisit as an opt-in grant on proven demand | accepted - see [security-model.md](security-model.md) |

## ⚠M0 Open Questions as Verification Gates

The five open questions in `omnibus-architecture.md` §10 change doctor hints, floors, estimates, or
CI strategy - never contracts. Each is closed by a roadmap task with acceptance criteria:

| Open question | Affects | Closed by |
|---|---|---|
| journald persistence defaults on Debian (trixie/bookworm/bullseye), Fedora, Manjaro | doctor hints | F1.1.1 container verification; surfaced via `journal.persistent` |
| Actual AT-SPI2 floor across DE matrix (GNOME/KDE/XFCE/Cinnamon/MATE) | version floor constants | F1.1.1; constants land in `omnibus-contracts` |
| Quality of Rust AX bindings (objc2) | macOS effort estimate, not the contract | F3.1.1 with escalation to F3.2 |
| TCC behavior in headless/SSH sessions | macOS CI strategy | F3.1 investigation |
| systemd version in current Ubuntu LTS (confirm 255) | floor table | F1.1.1 / F2.1 confirmation |

The specs carry these as conditional facts (marked M0); the running system always reports the actual
machine state through `capabilities()` (see [capability-model.md](capability-model.md)).

## Invariants

- Every risk always has either a closing task or an explicit accepted-risk entry.
- An accepted risk never bypasses the audit: every read, including `screenshot`, always emits an
  audit event.
- A ⚠M0 question never changes a contract - only hints, floors, estimates, or CI strategy.

## Anti-Patterns

- Promoting an accepted risk into silent scope creep (for example, building a screenshot gate that
  no scenario demanded) - acceptance is a decision, revisited via ADR, not by accident.
- Treating vendor competition as a feature to chase - the roadmap responds with speed and the open
  MCP contract, not with proprietary channels.

## Source Docs

- `docs/development/omnibus-spec.md` §6 (risks).
- `docs/development/omnibus-roadmap.md` §4.2 (per-milestone risk table).
- `docs/development/omnibus-architecture.md` §10 (open questions).

## Related Specs

- [Platform Matrix](platform-matrix.md) - tier model that resolves the QA-matrix risk.
- [Quality Gates](quality-gates.md) - the mechanical checks behind the mitigations.
- [Security Model](security-model.md) - the accepted screenshot risk in context.

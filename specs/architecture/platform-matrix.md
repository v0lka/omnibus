# Platform Matrix

## Context

Nine desktop targets in v1, grouped into three promise tiers. The tier is what Omnibus guarantees and
tests; the machine truth is always `capabilities()`. Source: `omnibus-architecture.md` §6.

## Matrix

| Platform | a11y | Journal | Services | Packages | Portals | Files | Knowledge | Tier |
|---|---|---|---|---|---|---|---|---|
| Arch | atspi | journald, persistent yes (confirmed) | systemd1 | alpm (cache-downgrade undo) | xdg-portal | fs-xdg | ArchWiki | **T1** |
| Manjaro | atspi | journald, persistent yes (M0 check) | systemd1 | alpm (delayed repos, mhwd, pamac) | xdg-portal | fs-xdg | ArchWiki + Manjaro overlay | **T1** (own test matrix) |
| Debian trixie | atspi | journald, default M0 check; doctor enables | systemd1 257 | apt (no undo) | xdg-portal; NM read-only | fs-xdg | Debian wiki | **T2** |
| Debian bookworm | atspi (at-spi2 2.46 below the 2.50 floor: `desktop.*` hidden) | journald, M0 check | systemd1 252 (on the floor) | apt (no undo) | xdg-portal; NM read-only | fs-xdg | Debian wiki | **T2** (on the floor) |
| Debian bullseye | atspi | journald, M0 check | systemd1 247 (below floor) | apt | xdg-portal partial | fs-xdg | Debian wiki | **T3** |
| Ubuntu LTS | atspi | journald, persistence is a per-machine probe fact (m0-findings Table 1: fresh installs default persistent); doctor enables where volatile | systemd1 255 (24.04, M0 check) | apt (no undo); NM behind netplan | xdg-portal; NM read-only | fs-xdg | Ubuntu docs | **T2** |
| Mint | atspi | journald, persistence is a per-machine probe fact (m0-findings Table 1: Mint 22.x inherits Ubuntu's persistent default); doctor enables where volatile | systemd1 = Ubuntu LTS | apt (no undo) | xdg-portal; X11 default | fs-xdg | Mint docs/forums | **T2** |
| Fedora desktop | atspi | journald, default M0 check | systemd1 (fresh) | dnf (transactional undo) | xdg-portal (fresh interfaces) | fs-xdg | Fedora docs | **T2** |
| macOS desktop | ax | unifiedlog | launchd | brew + mas | ScreenCaptureKit (+ AppIntents optional) | fs-tcc | kb-guides | **T2** (second target after Arch) |

## Tier Definitions

- **T1 - "guaranteed contract"**: full tool set, read+write, past-boot diagnostics out of the box,
  `pkg.undo` per the package manager's ability, CI on every core and adapter release, zero documented
  degradations.
- **T2 - "full read + safe write"**: all reading guaranteed; writes where the capability is declared;
  documented degradations: apt has no transactional undo (the tool honestly does not promise one),
  `journal.prev_boot` only with the fact of past boots (machines probed volatile: opt-in persistence via doctor,
  then a reboot), network is read-only; on the floor boundary (bookworm) at-spi2 below the floor
  hides `desktop.*` (a documented, honest row-level degradation). CI: smoke on reference versions.
- **T3 - "community"**: the build exists, `capabilities()` reports honestly; some interfaces below
  the version floor are absent; no guarantees, community support.

Platform specifics are notes to the matrix, never new categories: Manjaro is a T1 contract with its
own test matrix and package-freshness caveats; Debian bookworm is T2 on the lower floor boundary (below the atspi floor, on the systemd floor — `desktop.*` hidden);
Fedora adds transactional undo and the freshest portal interfaces.

## Version Floors

- systemd >= 252 for T2 (bookworm = 252 exactly on the floor; bullseye = 247 below it, hence T3).
- AT-SPI2: floor fixed numerically by the M0 measurements (roadmap task 1.1.1).
- macOS >= 14 (recommended 15+; the monthly Screen Recording re-grant is accounted for in doctor).
- Wayland: portal interfaces per `xdg-desktop-portal` version; X11 paths have no floor.
- varlink is outside T2 guarantees (only with fresh systemd); the main path is D-Bus.

## CI Session Axes

- Container CI runs the session-kind axis only (X11 and Wayland on Arch
  containers). The desktop-environment axis (GNOME/KDE) is deliberately not a
  container-CI axis: no CI step branches on the session type, so the extra
  legs would be byte-identical duplicates (rule R7). The axis enters container
  CI together with the first step that actually consumes the session type —
  the recorded criterion, mirrored in the header of
  `.github/workflows/ci.yml`. Real desktop-environment coverage stays with
  the VM legs (the Manjaro own matrix and the T2 VM legs).

## Invariants

- A platform below a floor always deactivates the affected adapter and always explains the reason in
  `capabilities()`.
- The floor constants always live in `omnibus-contracts` (never magic numbers in the probe).
- Tier claims in release notes always match the matrix in this spec.

## Anti-Patterns

- Promising per-release "T2.5" style categories - specifics are notes and `capabilities()` values,
  not tiers.

## Source Docs

- `docs/development/omnibus-spec.md` §2 (platform list, floors).
- `docs/development/omnibus-architecture.md` §6 (matrix, definitions, floors).
- `docs/development/omnibus-roadmap.md` tasks 1.1.1, 2.1.2, 2.6.2.

## Related Specs

- [Capability Model](capability-model.md) - tier vs capabilities arbitration.
- [Risks and Mitigations](risks-and-mitigations.md) - the QA-matrix risk.

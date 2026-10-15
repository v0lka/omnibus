# Release checklist — v1.0 (MS2)

Operational checklist for cutting the v1.0 release (roadmap tasks 2.6.1–2.6.3).
Every item is either runnable inside this repository (command and expected
result given) or marked `REQ-EXT` — it needs infrastructure the repo does not
have (real VMs, signing secrets, publication channels) and is executed by a
human at release time. The evidence for what is already verified locally lives
in [ms2-acceptance.md](verification/ms2-acceptance.md); this file is the
procedure the release owner executes to close the rest.

## 1. Gates — all green before anything else

Run from the repo root, in this order. A red gate has no override.

| # | Gate | Command | Expected |
|---|---|---|---|
| G1 | Spec system | `make specs` | exit 0 ("OK: structure green; ...") |
| G2 | Formatting | `cargo fmt --all -- --check` | no output |
| G3 | Lints | `cargo clippy --workspace --all-targets -- -D warnings` | zero warnings |
| G4 | Build | `cargo build --workspace` | success |
| G5 | Tests (all contract suites included) | `cargo test --workspace` | every suite `0 failed` |
| G6 | R4 boundary lints | `cargo run -p omnibus-lints --bin omnibus-lints` | `OK (R4-COP, R4-NATIVE, R4-CON, R4-EDGES green)` |
| G7 | R4 red/green fixtures | `cargo test -p omnibus-lints --test fixtures` | 10 passed, 0 failed |
| G8 | Coverage floor | `cargo llvm-cov --workspace --summary-only --fail-under-lines 75` | exit 0 |
| G9 | Packaging + matrix validator | `make packaging` | `all green: 150 checks passed` |
| G10 | Smoke demos | `ci/smoke/run-demos.sh` | `all demos passed (skips are honest and allowed)` |

Notes:

- `make gate` runs G1–G5 (specs, fmt, clippy, build, test) in one target;
  G6–G10 are the release-specific additions.
- Hygiene interplay (hit in practice): running the demos creates
  `ci/smoke/__pycache__/*.pyc`, and the G9 secret/hygiene scan rejects the
  workspace-local absolute paths embedded in bytecode. Run G9 before G10, or
  delete `ci/smoke/__pycache__/` before re-running G9.
- A flaky red in an adapter suite is a bug, not a rerun candidate: the apt
  fixture race fixed in MS2 (`ETXTBSY` retry in `spawn_bounded`, mirrored
  from the dnf adapter) is the template — fix the race, then require several
  consecutive green runs.

## 2. Matrix legs — 8 Linux platforms (roadmap 2.6.2 AC1)

The authoritative leg list is `ci/matrix/t2.yml` (schema-validated by G9
against `specs/architecture/platform-matrix.md`). Tier definitions live in
`specs/architecture/platform-matrix.md`: T1 = full contract, read + write,
past-boot diagnostics out of the box; T2 = full read + safe write, smoke on
the reference distro version; T3 = the build exists, `capabilities()` is
honest, no guarantees.

| Leg | Tier | Green means | Runs where |
|---|---|---|---|
| arch | T1 | full contract suite + demos against a live desktop session | reference machine + `.github/workflows/ci.yml` |
| manjaro | T1 | full T1 set + the own-matrix legs (`.github/workflows/manjaro.yml`; true-VM legs dispatch-gated) | reference machine + gated workflow |
| debian-trixie | T2 | build + `cargo test --workspace` + demos inside the reference container | `.github/workflows/t2.yml` (container) |
| debian-bookworm | T2 | same, on the floor: systemd 252 active, at-spi2 2.46 below the 2.50 floor so `desktop.*` hidden — and that state visible in `capabilities()` | t2.yml (container) |
| ubuntu-lts | T2 | same as trixie | t2.yml (container) |
| mint | T2 | VM leg (`kind: vm`), documented ubuntu:24.04 stand-in | t2.yml (skipped until `T2_VM_ENABLED`) |
| fedora | T2 | same as trixie + the dnf transactional-undo legs | t2.yml (container) |
| debian-bullseye | T3 | build only (systemd 247 < 252: the pkg family must stay hidden) | t2.yml (build-only) |

Rules for filling the release report's matrix table:

- `package-install` legs are `req-ext` by design (real VM installs,
  roadmap 2.6.1 AC1); a container leg never counts as an install leg.
- A leg that did not run is recorded as "did not run", never as green.
- Manjaro legs may cite `docs/verification/ms1-ms2-manjaro-acceptance.md`
  where the check is identical and re-confirmed by the current session's
  evidence catalog.

## 3. Signing — PPA, Debian repo, COPR (roadmap 2.6.1 AC2)

Posture already in the tree (static properties, re-verified by G9 on every
run — no action needed unless a check fails):

- PPA/Debian upload posture: `packaging/ppa/dput.cf` keeps
  `allow_unsigned_uploads = 0` (enforced by G9's signing-posture check);
  the signing secret lives outside the repository.
- The signed source-package cut is `packaging/ppa/upload.sh`
  (`dpkg-buildpackage -S` + dput) — a human-executed, out-of-band action;
  CI never uploads.
- Debian repo (trixie/bookworm) and COPR (Fedora) carry their repository
  signatures (Release file / COPR's automatic per-repo key).

Human, release-time actions:

- [ ] REQ-EXT — verify every channel build host BEFORE cutting binaries: the
      builder's rustc must meet the edition-2024 floor (>= 1.85; this
      packaging set ships no toolchain of its own), and the source package /
      SRPM must carry the vendored dependency tree (`cargo vendor`, see
      packaging/README.md "Build hosts") because the builders are
      network-isolated. A series whose rustc sits below the floor gets
      source-only or prebuilt-binary uploads instead of a binary build.
- [ ] REQ-EXT — publish the signed channels (PPA upload via
      `packaging/ppa/upload.sh`, Debian repo, COPR build), then the
      fresh-machine no-trust check on each channel: on a clean VM without
      the imported key, the package manager must honestly warn about the
      untrusted repository; with the key imported, the install proceeds.
- [ ] REQ-EXT — install via the native package manager on every T2 platform
      with the daemon working and TTFM < 5 min (roadmap 2.6.1 AC1), and the
      MVP → v1.0 upgrade with no manual steps (roadmap 2.6.1 AC3;
      cross-family migration is not promised).

## 4. Release report contents (roadmap 2.6.2 AC2, 2.6.3 AC1)

The v1.0 release notes must contain, in this order:

1. **The 8-leg matrix status table** — per leg: tier, result (green per its
   tier / did not run / REQ-EXT), and the workflow that owns it (§2 above).
2. **The tier ↔ capabilities divergence table — the gate.** For every leg
   that ran, compare the platform's published `capabilities()` document
   against its tier promise and give every divergence a row:

   `platform | tier | tier promise | capabilities() reality | explanation`

   Known candidates entering the release: debian-bookworm `desktop.*` hidden
   (at-spi2 2.46 < 2.50, T2 on the floor); the whole apt family
   `pkg.undo = none`; `journal.prev_boot` behind the journald-persistence
   opt-in on volatile machines; NM-backed `net_status` absent behind
   netplan/networkd (Ubuntu Server). A leg with no divergences is listed as
   clean; the table appears in the report whenever any leg ran, and every
   existing divergence is a row — none may be silently dropped (roadmap:
   the `tier ≠ capabilities` gate; divergences are fixed in the release
   report, §7.5 of the architecture).
3. **The reference machine's capabilities document** (verbatim JSON), as in
   the MVP notes (`docs/release-notes-0.1.0-draft.md` is the format template).
4. **Known limitations + the never-list** (from
   `specs/architecture/non-goals.md`), including the honest boundaries
   roadmap 2.6.3 names: apt has no undo, `prev_boot` appears only after the
   persistence opt-in, the network domain is read-only.
5. **Metrics**: TTFM per platform (the automatable onboarding-test half plus
   the REQ-EXT VM half) and the north-star trend vs MVP (REQ-EXT).

## 5. Tag, notes, announcements (REQ-EXT by nature)

- [ ] `git tag v1.0.0`; at tag time, run the additive-only audit of
      `omnibus-core` against the MVP tag in the diff log (roadmap 2.6.3 DoD:
      only additive extensions — `net_status` registered over the Ф2.2.3
      trait and new capability constants). The MS2 session proxy for this
      audit is recorded in `docs/verification/ms2-acceptance.md`.
- [ ] GitHub Release carrying the §4 report.
- [ ] Positioning updates (Ubuntu/Mint/Fedora/Debian segments from the
      pitch), Show HN follow-up, Phoronix/Hackaday article.

## References

- Roadmap: `docs/development/omnibus-roadmap.md` (tasks 2.6.1–2.6.3)
- Tiers, floors, reference versions: `specs/architecture/platform-matrix.md`
- The seven gates: `specs/architecture/quality-gates.md`
- Acceptance evidence for MS2: `docs/verification/ms2-acceptance.md`
- MS1 (Manjaro subset) evidence: `docs/verification/ms1-ms2-manjaro-acceptance.md`

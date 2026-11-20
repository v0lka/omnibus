# Contributing to Omnibus

Thank you for your interest in improving Omnibus. This document covers what you
need to build the project yourself, work on it, debug it, and navigate the
design docs and specs. If you only want to *use* the daemon, see the
[README](README.md) instead.

> If you drive development through AI coding agents, read
> [`AGENTS.md`](AGENTS.md) — it is the operating guide the repository's
> tooling assumes. The rules below apply to human and agent contributors alike.

## What the project is

Omnibus is an agent contract for the operating system: the daemon `omnibusd`
(Rust workspace, one static binary) gives AI agents a uniform, auditable,
least-privilege MCP API over a desktop OS. All inference lives in the MCP
client; `omnibusd` performs zero model calls.

Normative documents, in order of authority:

1. `docs/development/omnibus-spec.md` — product layer
2. `docs/development/omnibus-architecture.md` — SPI architecture
3. `docs/development/omnibus-roadmap.md` — work plan: milestones MS1–MS3,
   atomic tasks in What/Where/How/AC format
4. `specs/` — formal English specs derived from (1)–(3); specs never introduce
   new product or architecture decisions

Quirks to know up front:

- The source design docs (1)–(3) are written in Russian; everything you
  produce (code, comments, specs, commit messages) is English.
- When a spec conflicts with a source doc, the source doc wins and the spec is
  a bug — fix the spec. Among specs, `specs/META.md` wins.
- Specs describe the *target* system; code catches up task by task from the
  roadmap. A discrepancy between spec and code is a bug in one of them —
  determine which by context, never paper over it.

## Repository layout

| Crate / directory | Role |
|---|---|
| `omnibus-contracts` | the seam and the ONLY shared dependency: SPI traits, DTOs, capability constants, the 8-code error taxonomy. Zero OS dependencies. |
| `omnibus-core` | OS-agnostic core: MCP facade (`rmcp`), sessions, grant checks, capability registry, audit pipeline, doctor. |
| `omnibus-probe` | platform detection, adapter selection, version floors. |
| `omnibus-adapter-*` | one crate per subsystem × OS family (`atspi`, `journald`, `systemd1`, `alpm`, `apt`, `dnf`, `flatpak`, `xdg-portals`, `fs-xdg`, `kb-archwiki`, `kb-guides`, `nm`, `launch`, `proc`). |
| `omnibus-lints` | source-level lints encoding the boundary rules (the four R4 checks); depends on nothing, instant to build. |
| `omnibusd` | the composition-root binary: wiring (`src/wiring.rs`), grant CLI (`src/grant.rs`), CLI doctor (`src/doctor_cli.rs`). |
| `specs/` | formal English specs — the auditable source of truth about intended behavior. |
| `docs/development/` | source design docs (Russian) + the roadmap. |
| `docs/verification/` | acceptance evidence: M0 findings, MS1–MS2 gap analyses, v1.5 gap analysis. |
| `packaging/` | PKGBUILD (Arch/Manjaro), `debian/`, `fedora/` (COPR), `ppa/` (Ubuntu/Mint), scope-manifest templates. |
| `ci/` | packaging validator, CI matrix definitions (`ci/matrix/`), smoke demos (`ci/smoke/`). |
| `.github/workflows/` | `ci.yml` (gates + container sessions), `manjaro.yml` (T1 VM track), `t2.yml` (T2 matrix). |

The dependency direction is lint-enforced: `omnibus-contracts` is depended on
by everything; adapters never talk to each other or to `omnibus-core`
internals; `omnibus-core` never links adapter crates — adapters arrive through
the probe-assembled registry in the composition root, and only the `omnibusd`
binary depends on the core.

## Prerequisites

- Rust toolchain with **rustc >= 1.85** (the workspace uses edition 2024).
- A C compiler (needed by `ring` via `rustls` in the knowledge-base adapter).
- `make` and `python3` (the spec validator and the packaging validator).
- A Linux desktop (Wayland or X11) for the live-backend test legs and smoke
  demos. Container CI runs X11/Wayland sessions on Arch containers; the full
  desktop-environment coverage lives in the VM legs.

## Quality gates

`make gate` is the default Makefile target and the definition of green. Run it
before you claim any change is done; **a red gate has no override**.

| Target | Effect |
|---|---|
| `make gate` | specs + lint + build + test, in order (default) |
| `make specs` | `python3 specs/tools/validate_specs.py` — spec-system validator; must exit 0 |
| `make lint` | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; the four R4 lints (`cargo run -p omnibus-lints --bin omnibus-lints`); their red/green fixtures (`cargo test -p omnibus-lints --test fixtures`) |
| `make build` | `cargo build --workspace` |
| `make test` | `cargo test --workspace` |
| `make packaging` | `ci/validate_packaging.py` — parses and cross-checks the packaging inputs and CI matrix definitions |
| `make run-debug` | run the `omnibusd` daemon in the debug profile |
| `make clean` | `cargo clean` |

Editing anything under `specs/` or `docs/development/` requires `make specs`
to pass; editing Rust requires the full `make gate`.

The workspace-wide lint policy (root `Cargo.toml`, inherited by every crate):
`unsafe_code = "forbid"` and `clippy::unwrap_used` / `expect_used = "warn"` —
a panic-free seam. Production code carries zero `.unwrap()` / `.expect()`;
test code is allowed them via scoped attributes at crate roots / test-file
tops.

### The four R4 boundary rules

Each is implemented in `omnibus-lints` and proven red/green by fixtures under
`omnibus-lints/tests/fixtures/`:

- **R4-COP** — no tuple types in trait signatures (checked in contracts).
- **R4-NATIVE** — zero OS dependencies in `omnibus-contracts`; native types
  never leave an adapter.
- **R4-CON** — capability name literals only in the registry file
  `omnibus-contracts/src/capabilities.rs`.
- **R4-EDGES** — the dependency direction above.

## The spec system

`specs/` is the law. Start with:

- [`specs/INDEX.md`](specs/INDEX.md) — the task-to-spec navigation table.
- [`specs/WORKFLOW.md`](specs/WORKFLOW.md) — recipes for typical tasks
  (implement a roadmap task, add an MCP tool, add an adapter, add or change a
  grant, close an M0 question, change the spec system itself).
- [`specs/META.md`](specs/META.md) — templates, naming, update rules.

Change protocol: if behavior changes, edit the source design doc first, then
the affected specs; register new files in `specs/INDEX.md`; add a row to
`specs/tools/coverage.toml` if the spec covers a source-doc heading; then run
`python3 specs/tools/validate_specs.py` — it must exit 0. The validator checks
required structure, INDEX completeness, link resolution, coverage of every
source-doc heading, English-only content (any Cyrillic in `specs/**/*.md`
fails), and that every backticked repo-root path exists on disk or is marked
planned.

ADRs live in `specs/decisions/`, named `NNN-` + kebab-case slug (template:
`specs/decisions/_template.md`). Numbers are never reused. `Accepted` ADRs are
immutable — supersede with a new ADR and flip the old status. `Proposed`
marks a decision gated on a verification fact; the closing roadmap task
converts it to `Accepted`.

## Working rules (R1–R7)

The seven cross-cutting rules (`specs/architecture/quality-gates.md`, source:
roadmap section 0.2) apply to every task:

- **R1** contract before code: a tool appears only after its capability
  constants and trait are declared in `omnibus-contracts`.
- **R2** the contract test suite is the merge gate for any trait
  implementation.
- **R3** degradation follows the ladder: every write tool and platform-dependent
  feature defines behavior in all four states — degraded / hidden
  (`CapabilityRequired`) / below-floor (`FloorUnsupported`) / opt-in-required.
- **R4** negative checks in CI: the boundary prohibitions are automatic.
- **R5** audit is total: no write path is done without a structured audit
  event; reads are audited too.
- **R6** the never-list (below) is absolute.
- **R7** product honesty: acceptance criteria never promise more than the
  platform tier (`specs/architecture/platform-matrix.md`).

## The never-list

Single source: section 7.4 of `docs/development/omnibus-architecture.md`
(normative copy: `specs/architecture/non-goals.md`). Planning any of these is
rejected in review, by design:

- Write actions outside human-issued out-of-band grants. There is no runtime
  consent API; an agent-provided "user confirmed" token is always denied.
- Model calls from `omnibusd` (zero inference; ADR-007).
- A consent broker with UI, or any in-band confirmation flow (ADR-006,
  ADR-010).
- Switching the network backend; the network domain stays read-only.
- systemd backports; programmatic TCC issuance or any change to system
  security policies.
- A playbook engine or core-side triage (ADR-012); an event channel / push
  events in v1 (ADR-008); input emulation (ydotool/uinput) (ADR-009).

## Finding work

- The work plan is `docs/development/omnibus-roadmap.md`: milestones MS1
  (MVP, Arch) → MS2 (v1.0, all target distros) → MS2.5 (v1.5) → MS3 (v2.0:
  macOS, AppIntents). Tasks are atomic with What/Where/How/AC; the AC checkboxes are
  the acceptance bar. Unticked ACs mean live-environment legs are still
  pending — the evidence base (what is repo-verified vs. what needs real
  desktops, VM matrices, CI runs, publication) is in `docs/verification/`.
- Pick a task, read its AC, read the domain spec + contracts it touches, then
  follow the matching recipe in `specs/WORKFLOW.md` §6.

## Building and running

```bash
make build            # cargo build --workspace; produces the omnibusd binary
make run-debug        # run omnibusd in the debug profile (serves stdio)
./target/debug/omnibusd --version
```

The binary target is `omnibusd` (ADR-013 fixes the shipped name). Modes:

```bash
omnibusd                                  # serve one stdio MCP session
omnibusd grant <capability> [--path <dir>]...   # record a grant in the user store
omnibusd doctor [--apply <name>]          # CLI doctor; mutations need interactive human confirmation
omnibusd --version
```

## Testing and debugging

- `make test` runs the whole workspace suite: the R2 contract suites (one per
  trait implementation), the R4 lint fixtures, the R5/R6 security seeds, and
  the R3 degradation-ladder suite.
- Some suites exercise **live backends** (xdg portals, AT-SPI, the launch
  closed loop). They need a working desktop session; on a machine with a
  broken desktop stack they fail honestly — check the environment notes in
  `docs/verification/` before suspecting code. CI runs these legs against
  virtual sessions and fixture backends.
- `ci/smoke/` contains the end-to-end demos (`demo1`–`demo5`); `demo5-closed-loop.sh`
  exercises launch → observe (`ps.list` / `journal_query`) → terminate
  (`ps.signal`).
- Debugging aids that already exist:
  - the startup banner prints the probed distro and release (on Manjaro also
    the delayed-repos / mhwd / pamac facts);
  - `capabilities` (the assembled capability document) is the ground truth of
    what the current machine exposes and why anything is hidden;
  - `session_info` reports session identity and effective limits;
  - every call — reads included — emits exactly one structured audit event to
    the local journald sink; correlate via `session_info`;
  - `omnibusd doctor` diagnoses probe, adapters, audit sink and the write
    gate, and prints copy-paste remedies.

## Packaging and release

- `packaging/` holds one convention across families: the repo working tree is
  the source (no upstream tarballs), and every package installs
  `/usr/bin/omnibusd` plus the read-only scope templates to
  `/etc/omnibus/scopes.d/`.
- `make packaging` (`ci/validate_packaging.py`) locally validates the deb
  control/rules/changelog, the RPM spec, the PPA upload script posture, the
  T2 matrix schema and version consistency with the workspace version.
- Channel builds for deb/rpm are offline and floor-gated: `cargo vendor` at
  source-package cut time, cross-family builds exclude the alpm adapter
  (`--no-default-features --exclude omnibus-adapter-alpm`), and builder
  rustc must meet the 1.85 edition-2024 floor. Details in
  `packaging/README.md`.
- The release procedure is owned by `docs/release-checklist.md`. Its
  in-repo half is automated: `.github/workflows/release.yml` builds the
  per-family binary tarballs (`arch` full with alpm; `debian` / `ubuntu` /
  `fedora` cross-family without it) plus `SHA256SUMS` and attaches them to
  the GitHub Release at the pushed `v*` tag. The signed distro channels
  below stay human-issued.

## CI

The same commands as the Makefile are the CI jobs:

- `ci.yml` — the quality gates, container sessions on X11 and Wayland.
- `manjaro.yml` — the T1 VM track (own Manjaro matrix).
- `t2.yml` — the T2 matrix legs (dispatch-gated, variable-gated where real
  desktops are required).
- `release.yml` — the release builds and GitHub Releases: a `v*` tag push
  builds the four family artifacts (`omnibusd-<version>-<os>-x86_64.tar.xz`
  + `SHA256SUMS`) in the reference containers and publishes the release
  whose body is `docs/release-notes-<version>.md`. Historical tags that
  predate the workflow are released by dispatch:
  `gh workflow run release.yml -f tag=<tag> -f notes_ref=main`.

CI never claims a leg it cannot run: REQ-EXT steps (real-desktop round-trips,
VM matrices, channel publication) are documented, never faked.

## Conventions for code changes

- English everywhere you produce — code, comments, specs, commit messages.
  Commit messages follow the conventional-commits style already in the log
  (`feat(scope):`, `fix(scope):`, `docs(specs):`, `chore(scope):`).
- DTOs are flat and named (no tuples); primitives get newtype wrappers;
  `element_id` / `session_id` are opaque tokens minted only by the core.
- Every SPI trait method returns a contract error (`omnibus-contracts/src/errors.rs`);
  `anyhow` and `Box<dyn Error>` never appear at the boundary. OS failures map
  to `NativeFailure { code }` inside the adapter, with the original message
  preserved in the payload.
- SPI traits are declared just-in-time, together with their first
  implementation — no speculative trait stubs (ADR-011).
- Contracts carry their own semver (`CONTRACT_VERSION`), independent of the
  workspace package version.
- JSON schemas are generated from DTOs; hand-written schemas fail the build.
- Adding a tool: capability constants and the trait first (R1), then the
  registry row, then the domain spec — see the "Add a new MCP tool" recipe in
  `specs/WORKFLOW.md`.

## Before you open a change

1. `make gate` is green.
2. Specs are updated in the same change when behavior changed (protocol above);
   `make specs` exits 0.
3. The roadmap task's AC (if you are implementing one) is satisfied and its
   checkboxes honestly reflect what is repo-verified.
4. Nothing on the never-list is implied by the change — when in doubt,
   re-read ADR-006 before proposing anything consent-adjacent.

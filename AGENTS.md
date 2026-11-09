# AGENTS.md

Operating guide for AI coding agents in this repository. This file is the entry
point; the `specs/` tree is the law. Read `specs/INDEX.md` before your first
change.

## What this project is

Omnibus is an agent contract for the operating system: the daemon `omnibusd`
(Rust workspace, edition 2024, one static binary) gives AI agents a uniform,
auditable, least-privilege MCP API over a desktop OS - services, packages,
journal, a11y UI, files, clipboard, screenshots, notifications, knowledge
bases, network status; v1.5 adds app launch and live process/system/power
state. All inference lives in the MCP client; `omnibusd`
performs zero model calls.

Normative documents, in order of authority (rules in `specs/META.md`):

1. `docs/development/omnibus-spec.md` - product layer
2. `docs/development/omnibus-architecture.md` - SPI architecture
3. `docs/development/omnibus-roadmap.md` - work plan: milestones MS1-MS3,
   atomic tasks in What/Where/How/AC format
4. `specs/` - formal English specs derived from (1)-(3); specs never introduce
   new product or architecture decisions

Quirks to know up front:

- The source design docs (1)-(3) are written in Russian; `specs/` is
  English-only. Everything you produce (code, comments, specs, commit
  messages) is English.
- When a spec conflicts with a source doc, the source doc wins and the spec is
  a bug - fix the spec. Among specs, `specs/META.md` wins.
- Specs describe the target system; code catches up task by task from the
  roadmap. A discrepancy between spec and code is a bug in one of them -
  determine which by context, never paper over it.

## Quality gates

`make gate` is the default Makefile target and the definition of green. Run it
before you claim any change is done; a red gate has no override.

| Target | Effect |
|---|---|
| `make gate` | specs + lint + build + test, in order (default) |
| `make specs` | `python3 specs/tools/validate_specs.py` - spec-system validator; must exit 0 |
| `make lint` | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; the four R4 lints (`cargo run -p omnibus-lints --bin omnibus-lints`); their red/green fixtures (`cargo test -p omnibus-lints --test fixtures`) |
| `make build` | `cargo build --workspace` |
| `make test` | `cargo test --workspace` |
| `make run-debug` | run the `omnibusd` daemon in the debug profile |
| `make clean` | `cargo clean` |

Editing anything under `specs/` or `docs/development/` requires `make specs`
to pass; editing Rust requires the full `make gate`.

## Architecture: crates and the dependency direction

Crate map (normative: `specs/architecture/layers-and-crates.md`):

- `omnibus-contracts` - the seam and the ONLY shared dependency: SPI traits,
  DTOs, capability constants, error taxonomy. Zero OS dependencies;
  external deps are restricted to serde/async types.
- `omnibus-core` - OS-agnostic core; builds the `omnibusd` binary: MCP
  facade, sessions, grant checks, capability registry, audit pipeline,
  doctor.
- `omnibus-probe` - platform detection, adapter selection, version floors.
- `omnibus-adapter-*` - one crate per subsystem x OS family.
- `omnibus-lints` - source-level lints encoding the boundary rules
  (see below). Depends on nothing; stays instant to build.

Dependency direction (lint-enforced; `make lint` fails on violation):

- Allowed: `omnibus-contracts <- adapters`, `omnibus-contracts <- core`,
  `omnibus-contracts <- probe`. Everything depends on contracts; nothing else
  is shared.
- Forbidden: adapter -> adapter (P6: adapters never conspire); adapter ->
  `omnibus-core` internals; `omnibus-contracts` -> any OS crate.
- `omnibus-core` never links adapter crates directly: adapters arrive through
  the probe-assembled registry in the composition root (the `omnibusd` binary
  target). Nothing depends on the core.

The four R4 boundary rules (implemented in `omnibus-lints`, each proven
red/green by fixtures under `omnibus-lints/tests/fixtures/`):

- R4-COP: no tuple types in trait signatures (checked in contracts).
- R4-NATIVE: zero OS dependencies in `omnibus-contracts`; native types never
  leave an adapter.
- R4-CON: capability name literals only in the registry file
  `omnibus-contracts/src/capabilities.rs`.
- R4-EDGES: the dependency direction above.

## Panic-free, unsafe-free workspace

The root `Cargo.toml` declares the workspace lint policy; every crate
inherits it via `[lints] workspace = true` (do the same for new crates):

- `unsafe_code = "forbid"` - no unsafe anywhere in the workspace; the seam
  must stay auditable.
- `clippy::unwrap_used = "warn"` - panic-free seam: contracts, core, and
  adapters return contract errors instead of unwrapping or panicking.

Conventions that follow (`specs/architecture/tech-stack.md` and the contracts
source):

- Every SPI trait method returns a contract error; `anyhow` and
  `Box<dyn Error>` never appear at the boundary. The taxonomy is a closed set
  of eight codes (`omnibus-contracts/src/errors.rs`): `CapabilityRequired`,
  `ScopeDenied`, `FloorUnsupported`, `AdapterUnavailable`,
  `NativeFailure { code, payload }`, `ElementStale`, `TimeoutError`,
  `Unsupported { reason }`.
- OS failures map to `NativeFailure { code }` inside the adapter, with the
  original message preserved in the payload.
- DTOs are flat and named (no tuples); primitives get newtype wrappers;
  `element_id` / `session_id` are opaque tokens minted only by the core.
- SPI traits are declared just-in-time, together with their first
  implementation - no speculative trait stubs (ADR-011).
- Contracts carry their own semver, independent of code versions.

## Spec system workflow

`specs/` is the auditable source of truth about intended behavior; code
follows specs, never the reverse.

- `specs/META.md` - templates, naming, update rules. `specs/INDEX.md` -
  task-to-spec navigation; every file under `specs/` must be listed there.
  `specs/WORKFLOW.md` - recipes for typical tasks (implement a roadmap task,
  add an MCP tool, add an adapter, add or change a grant, close an M0
  question, change the spec system itself).
- Change protocol: if behavior changes, edit the source design doc first,
  then the affected specs; register new files in `specs/INDEX.md`; add a row
  to `specs/tools/coverage.toml` if the spec covers a source-doc heading;
  then run `python3 specs/tools/validate_specs.py` - it must exit 0.
- The validator checks: required structure; INDEX completeness; relative
  markdown links resolve; every heading of the three source docs is covered
  by the coverage matrix; English-only content (any Cyrillic in
  `specs/**/*.md` fails); every backticked repo-root path exists on disk or
  sits on a line explicitly marked planned.
- ADRs live in `specs/decisions/`, named `NNN-` + kebab-case slug (template:
  `specs/decisions/_template.md`). Numbers are never reused. `Accepted` ADRs
  are immutable; supersede with a new ADR and flip the old status.

## Working rules (R1-R7)

The seven cross-cutting rules (`specs/architecture/quality-gates.md`,
source: `docs/development/omnibus-roadmap.md` section 0.2) apply to every
task:

- R1 contract before code: a tool appears only after its capability constants
  and trait are declared in `omnibus-contracts`.
- R2 the contract test suite is the merge gate for any trait implementation.
- R3 degradation follows the ladder: every write tool and platform-dependent
  feature defines behavior in all four states - degraded / hidden
  (`CapabilityRequired`) / below-floor (`FloorUnsupported`) /
  opt-in-required.
- R4 negative checks in CI: the boundary prohibitions are automatic (see the
  four R4 lints above).
- R5 audit is total: no write path is done without a structured audit event
  `{ts, session, actor, tool, adapter, op, params_hash, result, scope_ref}`;
  reads are audited too.
- R6 the never-list (below) is absolute.
- R7 product honesty: acceptance criteria never promise more than the
  platform tier (`specs/architecture/platform-matrix.md`).

## Never implement

From the never-list (`specs/architecture/non-goals.md`; single source:
section 7.4 of `docs/development/omnibus-architecture.md`). Planning any of
these is rejected in review, by design:

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

## Where to dig deeper

- System entry point: `specs/architecture/overview.md`; principles P1-P7:
  `specs/architecture/principles.md`.
- Why the seam is shaped this way: `specs/architecture/coupling-model.md`;
  the seven gates: `specs/architecture/quality-gates.md`.
- Normative inventory: 14 SPI traits across 12 subsystems (13 declared;
  `AppLauncher` and `ProcessTelemetry` arrived with v1.5, `AppIntentsBridge`
  stays JIT with MS3) (`specs/architecture/spi-registry.md`), 37 MCP tools
  (27 through v1.0; eight more with v1.5, Linux only - all eight shipped;
  the two `intent_*` with MS3) (`specs/architecture/tool-registry.md`),
  26 capability constants (24 shipped; the two `intents.*` with MS3)
  (`specs/architecture/capability-model.md`), the 8 error codes
  (`specs/architecture/error-taxonomy.md`).
- Boundaries: `specs/contracts/core-adapters.md`,
  `specs/contracts/mcp-client-daemon.md`,
  `specs/contracts/config-and-grants.md`.
- Security and grants: `specs/architecture/security-model.md`; sessions and
  limits: `specs/architecture/topology-and-lifecycle.md`; platform tiers and
  floors: `specs/architecture/platform-matrix.md`; risk register and open M0
  questions: `specs/architecture/risks-and-mitigations.md`; versioning:
  `specs/architecture/evolution-and-compat.md`; stack choices:
  `specs/architecture/tech-stack.md`; what is deliberately not built:
  `specs/architecture/non-goals.md`.
- Find your task in `docs/development/omnibus-roadmap.md` - its AC checkboxes
  are the acceptance bar. Product context and demo scenarios:
  `docs/development/omnibus-pitch.md`.

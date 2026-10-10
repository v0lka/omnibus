# Contract: Core <-> Adapters (SPI)

## Boundary Rule

Everything depends on `omnibus-contracts`; adapters never depend on the core or on each other; the
core calls traits through a probe-assembled registry and never links adapter crates directly.

## Interfaces

| Interface | Crate | Consumed By | Purpose |
| --------- | ----- | ----------- | ------- |
| 12 SPI traits (10 subsystems) | `omnibus-contracts` | `omnibus-core` tools | the full subsystem registry - see [spi-registry.md](../architecture/spi-registry.md) |
| DTOs (flat named structs, newtype units) | `omnibus-contracts` | core + adapters | the only data shapes crossing the seam |
| capability constants | `omnibus-contracts` | core + adapters | name literals are forbidden outside this crate (CI lint) |
| error taxonomy (8 codes) | `omnibus-contracts` | core + adapters | every trait method returns a contract error |
| `PlatformProbe` | `omnibus-probe` | core (startup) | detection, adapter selection, floors |
| contract test suites | `omnibus-contracts` (suite crates) | CI (R2 gate) | behavioral acceptance of every adapter implementation |

## Initialization

- Startup order is the adapter FSM owned by the core: `probe -> register -> serve -> shutdown`; an
  adapter physically cannot be called before `register`.
- The probe selects exactly one active adapter per subsystem; extra implementations may exist only
  as fallbacks.
- Linking happens in the composition root (the binary target); `omnibus-core` compiles against
  fakes, never against real adapters.
- doctor is a core tool that orchestrates several traits (`PlatformProbe` + `Journal` +
  `ServiceManager` + `KnowledgeBase`) - composition is always core work (P6); adapters never call
  adapters.

## Data Flow Across Boundary

- Core -> adapter: trait calls with DTO arguments only; native types (`zbus::Value`, ALPM handles,
  `AXUIElementRef`) never leave an adapter.
- Adapter -> core: DTO results or contract errors; native error codes become `NativeFailure{code}`
  inside the adapter with the original message preserved in the payload.
- Adapter -> audit: never direct - the audit pipeline lives in the facade (P5); adapters stay
  audit-agnostic.

## Error Propagation

- `anyhow`/`Box<dyn Error>` never appear at the boundary; the eight-code taxonomy is closed.
- OS authorization failures (polkit, launchd domains) surface as `NativeFailure` with an honest
  human-readable hint - Omnibus never intercepts system authorization.

## Breaking Change Checklist

If you change an SPI trait, you MUST also:

- Bump the trait's contract version (semver independent of code version); breaking shape changes are
  major-only (see [evolution-and-compat.md](../architecture/evolution-and-compat.md)).
- Update the contract suite and run it against every implementation (R2) - a red suite blocks the
  merge.
- Update [spi-registry.md](../architecture/spi-registry.md) and the affected domain spec.
- Check the connascence rules: no tuple signatures, no native types, single-sourced constants
  (clippy lints fail the build).

## Source Docs

- `docs/development/omnibus-architecture.md` §2, §4, §8 (coupling audit), §8.8 (consolidation).
- `docs/development/omnibus-roadmap.md` F1.2 (omnibus-contracts: full SPI).

## Related Specs

- [Layers and Crates](../architecture/layers-and-crates.md) - dependency direction, crate map.
- [Coupling Model](../architecture/coupling-model.md) - why this seam stays weak.
- [SPI Registry](../architecture/spi-registry.md) - the trait/subsystem table.

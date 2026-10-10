# Coupling Model (core <-> adapters)

## Context

A design-time audit of the core/adapter seam using the Balanced Coupling Model (integration strength
x distance x volatility), a connascence audit, and a demonstration of the key property: adding a new
OS adapter requires zero core changes. Source: `omnibus-architecture.md` §8.

## Component Map

- Core (`omnibus-core`): facade, sessions, scopes, audit, capability registry, doctor orchestration.
- Contracts (`omnibus-contracts`): 12 SPI traits (10 subsystems), DTOs, capability registry, error
  taxonomy, addressing types.
- Adapters: ~15 crates (subsystem x family) + probe.
- Edges: core -> adapters via `dyn Trait` (probe-assembled registry); adapters -> contracts;
  adapter <-> adapter forbidden (P6).

## Strength, Distance, Volatility

| Edge | Strength | Distance | Volatility | Balance |
|---|---|---|---|---|
| core -> adapter (via SPI) | Contract (weak) | high | high (implementations) | OK: weak x far |
| adapter -> adapter | none | high | high | OK: no coupling |
| inside core | Functional/Model (strong) | minimal | low-medium | OK: strong x near |
| inside adapter | Intrusive (to native APIs) | minimal | high | OK: strong x near - that is its job |
| core <-> contracts | Contract (weak) | minimal (one workspace) | surface more stable than implementations | OK: conscious compromise |

Modularity formula `MODULARITY = (STRENGTH XOR DISTANCE) OR NOT VOLATILITY` holds on every edge; no
"strong coupling x large distance" (distributed monolith) pair exists - the main checkable claim.
Target shape per the Stable Abstractions Principle: `omnibus-contracts` maximally stable and abstract
(all depend on it, it depends on nobody); adapters concrete and unstable.

## Connascence Audit

- Through the boundary only weak static forms remain: names (CoN, single-sourced constants) and types
  (CoT, DTOs).
- Converted into weaker forms: CoM -> CoT (newtypes), CoP excluded (named-struct signatures), CoE ->
  the core-owned adapter FSM, CoI -> core-minted opaque tokens, CoA -> single definition in contracts.
- Algorithms (tree traversal, address resolution, journal delta reads) stay adapter property; the
  contract fixes inputs/outputs only.
- Dynamic connascence is locked by the FSM and by core-owned timeouts; the "contract version <>
  capability version" pair converges in one registry.

## Adding a New Adapter Without Core Changes

Example: NixOS support. New crate `omnibus-adapter-nix` implements `PackageManager` (nix generations
as `history`/`undo`), reuses atspi/journald/systemd1; the probe gains a detection branch; capabilities
change only additively (provider swap, or a new constant = minor contract version). The core: zero
changes. Regression guards: the contract test suite runs against every implementation, and the
dependency-rules CI job fails any adapter that imports `omnibus_core`. The reverse case (dnf -> dnf5)
is contained entirely inside one adapter crate.

The only case that touches the core is a new subsystem type (new trait) - an additive extension; in
v1 exactly two, both just-in-time: `NetworkStatus` (task 2.2.3) and `AppIntentsBridge` (task 3.6.2).

## Red Flags the Architecture Avoids

- Intrusive links: adapters never reach into core internals (lint + review).
- Duplicated logic: remediation knowledge in KB, access policy in core; adapters do native mapping only.
- Forced core/adapter release trains: contracts version independently; breaking changes are major-only.
- Dependency cycles: impossible by direction (contracts <- everything; adapters never -> core).

## Consolidation Priorities (§8.8)

1. Contract test suite as the gate for every adapter (the mechanical guarantee).
2. Connascence rules as clippy lints (no tuple signatures; no native types in contracts; no capability
   name literals).
3. Extract `omnibus-contracts` to its own repository on the first external adapter contributor.
4. Never introduce adapter-to-adapter calls "for convenience" - orchestration belongs to the core.

## Invariants

- Every cross-boundary dependency is always expressible as trait signatures plus DTOs.
- The core always compiles against fakes, never against real adapters.

## Anti-Patterns

- Sharing an algorithm through a "helper crate" that both core and adapters import - it re-couples the
  seam that contracts decoupled.

## Source Docs

- `docs/development/omnibus-architecture.md` §8.1-§8.8.

## Related Specs

- [Layers and Crates](layers-and-crates.md) - dependency direction.
- [Contract: core <-> adapters](../contracts/core-adapters.md).

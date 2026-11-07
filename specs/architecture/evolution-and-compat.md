# Evolution and Compatibility

## Context

How contracts, capabilities, and the adapter set evolve without breaking agents or adapters. Source:
`omnibus-architecture.md` §9 plus the just-in-time trait rule and the un-plan from the roadmap.

## Versioning Rules

- Contracts use semver. Minor versions add capabilities; breaking changes ship only as a major
  version, with migration.
- Capability deletion happens only in a major version. A deprecation mechanic (`deprecated` ->
  hidden from the default `capabilities()`) is introduced by the first release that actually needs
  one - not speculatively. There is no N/N-1 compatibility window until the first external consumer
  of the contracts exists.
- Core and contracts release together (one "tectonic plate": zero distance inside the plate); the
  contract surface is more stable than implementations.

## Adapter Set Evolution

- Exactly one active adapter per subsystem, selected by the probe; extra implementations may exist
  only as fallbacks. This keeps behavior and audit explainable. Sole documented exception (v1.5):
  in the packages subsystem the read-only flatpak/snap inventory adapter is active alongside the
  native manager - both providers visible in `capabilities()`, write semantics on the native manager
  only.
- New traits are declared just-in-time together with their first implementation; the trait registry
  is closed over the roadmap task list. In v1 exactly two late traits: `NetworkStatus` (task 2.2.3)
  and `AppIntentsBridge` (task 3.6.2); v1.5 adds `AppLauncher` (task 2.7.1) and `ProcessTelemetry`
  (task 2.8.1).
- Adding an adapter never changes the core (see [coupling-model.md](coupling-model.md)).
- `omnibus-contracts` moves to its own repository when the first external adapter contributor
  appears - a trigger-based decision, not a calendar one.

## What Does Not Evolve in v1

The un-plan items (see [non-goals.md](non-goals.md)) each carry their own revisit condition
(first proven scenario, first external contributor, server/headless profiles). None of them is
"never forever" except the never-list proper.

## Invariants

- Every additive contract change always ships as a minor version.
- The registry of capabilities always grows additively; a removal implies a major version.
- An adapter always enters the repository only with a green contract suite against it.

## Anti-Patterns

- Pre-building deprecation windows, plugin formats, or compat shims before any external consumer
  exists.
- Silently changing a capability's meaning under the same name - that is a breaking change and needs
  a major version.

## Source Docs

- `docs/development/omnibus-architecture.md` §9, §8.8.3.
- `docs/development/omnibus-roadmap.md` §1 (just-in-time traits), §5.4 (un-plan).

## Related Specs

- [Coupling Model](coupling-model.md) - why additive growth is cheap here.
- [Quality Gates](quality-gates.md) - gates that keep evolution honest.

# ADR-001: dyn Trait over generics and dynamic plugins

## Status

Accepted

## Context

Adapter selection is a runtime fact: the probe inspects the concrete machine
(distro family, daemon liveness, version floors) and only then decides which
SPI implementations become active. The composition root - the `omnibusd`
binary target in `omnibus-core` - links the adapter crates and assembles the
registry once, from the probe report. The boundary rules constrain the shape
of that assembly: R4-EDGES forbids everything except
`omnibus-contracts <- {adapters, core, probe}`, and `omnibus-core` must never
link adapter crates directly. The question was how the core holds adapter
implementations it cannot name at compile time.

## Decision

SPI traits are used as trait objects (`dyn Trait`) stored in the
probe-assembled registry. The MCP facade and the core operate on the
contract traits only; no generic parameter of any core type names an
adapter, and no adapter is loaded from outside the workspace. Adapter
crates are compile-time workspace members, instantiated exactly once, in
the composition root.

## Consequences

Positive:

- One static binary; the whole seam stays auditable under
  `unsafe_code = "forbid"`.
- Runtime adapter selection follows the probe report with no platform
  build matrix - the same binary serves Arch, Manjaro, and future
  families.
- Adapter cross-talk (P6) is structurally impossible: adapters meet only
  through the contracts, and R4-EDGES rejects any direct edge.

Negative:

- Dynamic dispatch on every adapter call; accepted as negligible for
  I/O-bound operations (bus calls, process spawns, portal round-trips).
- Extending the system with a new adapter is a compile-time change
  (workspace member + probe branch); no third-party runtime plugins.

## Alternatives Considered

- Static dispatch via generics: monomorphization would force every core
  structure to carry adapter type parameters and reintroduce a
  per-platform build matrix, against the one-static-binary packaging
  (task 1.8.1); it also pushes adapter names into core, against
  R4-EDGES.
- Dynamic `.so` plugin loading: requires a stable ABI and unsafe
  boundaries, contradicts the panic-free, unsafe-free workspace policy,
  and recreates a custom plugin format - while MCP already is the
  plugin standard (never-list).

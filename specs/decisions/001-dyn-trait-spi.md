# ADR-001: Dynamic Trait SPI for the Adapter Boundary

## Status

Accepted

## Context

Omnibus needs OS-specific adapters (subsystem x OS family) behind one stable boundary without a
plugin format of its own. The boundary must be replaceable per platform probe, typed end to end,
and free of OS dependencies in the shared crate.

## Decision

The SPI is a set of Rust traits (`omnibus-contracts`) dispatched dynamically (`dyn Trait`) by the
core: traits + DTOs + capability constants + the error taxonomy, with semver independent of the
code version. Adapters implement traits in their own crates; exactly one adapter per subsystem is
active, selected by the probe.

## Consequences

- New adapters add crates, never core edits (demonstrated by the section 8.6 walkthrough).
- Dynamic dispatch cost is negligible next to D-Bus/process calls on the boundary.
- The contract crate stays dependency-light (serde and async types only).

## Alternatives Considered

- Static generics - rejected: the adapter set would be fixed at compile time per binary.
- A dynamic plugin format - rejected: MCP is the standard we expose, not a plugin system we
  invent; trait objects give replaceability without a loader.

## Source Docs

- `docs/development/omnibus-roadmap.md` task 1.1.2 (records this decision as ADR-001).
- `docs/development/omnibus-architecture.md` sections 2, 4.

# ADR-002: apt Adapter Backend (libapt vs CLI Mapping)

## Status

Proposed

`Proposed` is gated on implementation facts; roadmap task 2.2.1 closes this ADR to `Accepted`
when the apt read path (`list`, `info`, `search`, `history`) lands.

## Context

The apt adapter needs a backend choice: linking `libapt` (rich API, ABI churn, C++ binding cost)
versus mapping the apt CLI (stable surface, parse cost, history needs `apt` logs). The Debian T2
targets (trixie, bookworm) need full read coverage with documented degradation and no undo.

## Decision

Decide during task 2.2.1 against the contract suite: whichever backend passes
`PackageManager` read operations on Debian trixie/bookworm, Ubuntu LTS, and Mint with honest
`NativeFailure{code}` mapping. The contract (traits, DTOs, capability `pkg.read`) is fixed; the
backend is an adapter-internal detail.

## Consequences

- The SPI boundary keeps the choice local to `omnibus-adapter-apt` (planned).
- `pkg.undo=none` on apt is a contract fact regardless of backend.

## Alternatives Considered

- Fixing one backend now in the spec - rejected: the source docs deliberately leave this to the
  implementation task; the spec would guess an implementation detail.

## Source Docs

- `docs/development/omnibus-roadmap.md` task 2.2.1 (names this ADR for the backend decision).

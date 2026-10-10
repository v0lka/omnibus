# ADR-004: unified log Query Backend

## Status

Proposed

`Proposed` is gated on implementation facts; roadmap task 3.3.1 closes this ADR to `Accepted`
when the unified log adapter lands.

## Context

On macOS, `Journal` semantics map to the unified log. The candidate backends: shelling out to
`log show --predicate` (documented, slow, subject to TCC-ish console constraints) versus a private
API alternative (faster, unsupported surface). `Filter{unit}` has no unified log equivalent and is
specified to return typed `Unsupported{reason}` with a severity/window/process hint.

## Decision

Decide during task 3.3.1 against the contract suite: whichever backend serves
`query(Filter{...})` and `tail` within the limits budget with honest typed errors. AuditSink
delivery uses the unified log with a dedicated subsystem/category `omnibus` either way.

## Consequences

- The `Journal` contract (Filter, typed errors, boot semantics) is stable regardless of backend.
- Performance and supportability trade-offs stay inside `omnibus-adapter-unifiedlog` (planned).

## Alternatives Considered

- Committing to the private API now - rejected: unsupported surfaces change; the spec does not
  guess implementation details.

## Source Docs

- `docs/development/omnibus-roadmap.md` task 3.3.1 (names this ADR for the backend choice).

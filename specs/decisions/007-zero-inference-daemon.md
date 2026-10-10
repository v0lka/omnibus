# ADR-007: Zero Model Calls in the Daemon

## Status

Accepted

## Context

An agent contract component that calls a model inside the daemon would embed inference policy,
latency, cost, and a second trust boundary into the process the user audits. The product premise
is a deterministic OS contract; model choice belongs to the user's agent (local or cloud).

## Decision

`omnibusd` performs zero model calls - ever, in any milestone. All inference lives in the
agent's context. A future local-ML integration is at most an optional embeddings adapter for
search, never core. The daemon keeps zero conversational surface: no model calls, no in-band
consent, no in-band mutations.

## Consequences

- The runtime is a deterministic policy function: grant in config -> execute; otherwise typed
  denial with a remedy.
- Behavior is fully testable without networks or models; the daemon has no dependency on model
  vendors.

## Alternatives Considered

- Summarization/classification in the daemon (log triage, tree pruning) - rejected: convenience
  that re-opens inference policy; the agent layer does this with the same data.

## Source Docs

- `docs/development/omnibus-spec.md` sections 2, 7.
- `docs/development/omnibus-architecture.md` section 1 (P1).

# ADR-011: Closed Registry with Just-in-Time Traits

## Status

Accepted

## Context

Early drafts declared all future traits up front (AppIntentsBridge two milestones before any
implementation, NetworkStatus one before) plus contract suites against fakes - all to pass a
"core unchanged" diff gate. But adding a trait is additive anyway; paying for stubs bought
nothing, and suites for unimplemented traits get rewritten when real requirements arrive.

## Decision

The trait registry is closed over the roadmap task list: a trait is declared in
`omnibus-contracts` together with its first implementation (`NetworkStatus` with task 2.2.3,
`AppIntentsBridge` with task 3.6.2). No speculative stubs; contract suites grow just-in-time.
"Core unchanged across a milestone" is a release observation, not a CI gate. New traits enter
only additively (minor contract version).

## Consequences

- MS1 declares exactly the traits it implements; later milestones grow the registry without core
  redesign, which is the actual claim under test.
- The registry count is always checkable against the roadmap task list.

## Alternatives Considered

- Up-front declaration + diff gate - rejected: pre-paid generality with zero savings and suites
  written against guesses.

## Source Docs

- `docs/development/omnibus-architecture.md` section 4, 8.6.
- `docs/development/omnibus-roadmap.md` tasks 1.2.1, 2.2.3, 3.6.2, 2.6.3, 3.7.3.

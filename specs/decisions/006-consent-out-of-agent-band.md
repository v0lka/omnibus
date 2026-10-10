# ADR-006: Consent Only out of the Agent's Band

## Status

Accepted

## Context

In-band confirmation protocols ask the agent to relay both sides of a consent dialogue. The agent
is the transport for question and answer alike, so an in-band "the user confirmed" is
indistinguishable from a forgery - it defends only against honest-buggy agents, zero against
compromised or injected ones. The threat model names prompt injection explicitly.

## Decision

The runtime has no consent API at all: no MCP method accepts a confirmation token or field.
Grants exist only where the agent has no vote - TOML manifests and `omnibus grant` - and doctor
over MCP returns diagnosis plus a ready command while only the human-executed CLI mutates.
Denial returns `ScopeDenied` with a one-line copy-paste remedy. Omnibus neither relies on nor
duplicates any consent UX an MCP host may provide; the runtime gate stays deterministic.

## Consequences

- Security properties stop depending on agent honesty - the core product claim becomes provable.
- Confirmation policy is testable: negative CI tests feed a "confirming" agent and expect denial
  with grant-less classification; grant audit events always carry provenance `config|cli|doctor`.
- The consent-broker-with-UI idea is retired to the never-list, like the router before it.

## Alternatives Considered

- In-band confirmation with signed tokens - rejected: still relayed by the agent; signature
  schemes move the forgery, not remove it, and add key management with no trust anchor.

## Source Docs

- `docs/development/omnibus-spec.md` section 7.
- `docs/development/omnibus-architecture.md` sections 3.3, 7.4.

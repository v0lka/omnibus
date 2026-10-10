# ADR-005: stdio-only Topology (Daemon per Connection)

## Status

Accepted

## Context

Three deployment shapes were entertained across drafts: an HTTP service, a resident session
daemon with a separate admin channel (for `omnibus grant` delivery and re-probe), and a stdio
child of the MCP client. Only stdio matches the MCP standard, the desktop trust model (one uid),
and the onboarding path (one line in the client config).

## Decision

`omnibusd` is a stdio child of the MCP client: one instance per client, one session per
connection, the session lives and dies with the process. No resident daemon, no admin/control
channel, no HTTP transport in v1. Every session runs a fresh probe; configuration changes made by
`omnibus grant` or doctor CLI are picked up by the next session. Revisit conditions: server or
headless profiles.

## Consequences

- No cross-instance state: an `element_id` from one instance never resolves in another (typed
  error), and write serialization uses a mutex plus a cross-process `flock` (the only cross-
  instance concern).
- Grant delivery needs no channel: the human edits TOML or runs `omnibus grant`; the next session
  sees it.
- Lifetimes are trivial - no TTL, no orphan cleanup.

## Alternatives Considered

- HTTP transport - rejected: audiences undefined, adds authentication surface without a consumer.
- Resident daemon + admin socket for live re-probe - rejected: monitoring machinery does not pay
  for saving one reconnect; no LaunchAgent is registered in v1 - on macOS the daemon also starts
  as the MCP client's child (roadmap task 3.7.2).

## Source Docs

- `docs/development/omnibus-spec.md` sections 2, 7.
- `docs/development/omnibus-architecture.md` sections 3.1-3.3, 7.2.

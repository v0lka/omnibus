# ADR-010: Exactly One Grant Mechanism

## Status

Accepted

## Context

Drafts accumulated three grant channels: write scopes (TOML + `omnibus grant`), opt-in grants
(a separate section plus a separate `omnibus optin` command plus a launch flag), and session
grants (env/flag in the client config). Under the stdio topology a client-config "session" grant
lives exactly as long as the static config itself - the persistent/session distinction is
illusory, and three formats mean three audit stories.

## Decision

One mechanism: entries in the same TOML manifests (`/etc/omnibus/scopes.d/*.toml`, sections
`[write]` and `[optin]`), one command (`omnibus grant <name>`), one audit provenance set
(`config|cli|doctor`). "Temporary" means issued-then-revoked. The default is read-only; the grant
list is a whitelist; anything not listed is denied.

## Consequences

- One format, one command, one test surface; doctor prints a ready-made grant command.
- No flag surface in client configs; no grant semantics outside the manifest.

## Alternatives Considered

- Separate `optin` command and session-grant flags - rejected: duplicated semantics, an illusion
  of ephemerality under stdio, and a second provenance to audit.

## Source Docs

- `docs/development/omnibus-architecture.md` section 3.3.
- `docs/development/omnibus-roadmap.md` task 1.3.3.

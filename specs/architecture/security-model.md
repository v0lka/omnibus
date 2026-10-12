# Security Model

## Context

Omnibus sells verifiability. The security property is honest precisely because it does not depend on
agent honesty: consent exists only out of the agent's reach, the runtime has no consent API at all,
and every action is audited. This spec is the normative restatement of `omnibus-architecture.md`
§3.3, §3.4, §7.4.

## Grants: Exactly One Mechanism

- Stores: TOML manifests — system templates `/etc/omnibus/scopes.d/*.toml` plus the user store `omnibusd grant` writes (`$XDG_CONFIG_HOME/omnibus/scopes.d/90-grants.toml`, fallback `$HOME/.config/omnibus/scopes.d`; sections `[write]` and `[optin]`; field names
  fixed in [config contract](../contracts/config-and-grants.md)).
- Issuer: a human, out of the agent's reach - by editing the manifest or by running
  `omnibusd grant <name>` (the command edits the manifest; doctor returns the ready-made command).
- Default: read-only. The grant list is a whitelist; anything not listed is denied.
- Revocation: remove the entry. "Temporary" means issued-then-revoked; there is no separate
  session-grant channel (with stdio the config is static - persistent/session distinction is
  illusory).
- Opt-in grants (capability status `opt-in-required`, e.g. `journal.persistent`) use the same
  manifests (`[optin]` section), the same command, and the same audit provenance.

## No In-Band Consent (hard rule)

- The runtime has no API for receiving confirmations: no MCP method accepts a consent token or field.
- An in-band "the user confirmed" from an agent is indistinguishable from a forgery - the agent is the
  transport for both sides of the dialogue. Therefore such input is meaningless and is always ignored
  (negative CI test: a "confirming" agent gets the action denied as grant-less).
- Denial returns `ScopeDenied` with a one-line copy-paste remedy.
- Every grant-related audit event always carries provenance `config|cli|doctor`.

## Policy of System Changes (three tiers)

| Tier | Rule | Examples |
|---|---|---|
| Never through the agent | Writes only under human-issued grants; no runtime consent API | all [+scope] tools |
| Opt-in via doctor, explicit consent | Changes to the system that Omnibus itself needs; the mutation is executed by a human (CLI `omnibusd doctor` or the command doctor printed) | persistent journald, macOS TCC wizard (the per-app Electron a11y-flag opt-in is planned) |
| Never | No implementation may do these at all | switching the network backend, systemd backports, changing system security policies (TCC is granted only by a human via GUI or MDM/PPPC) |

- doctor over MCP can never mutate the system regardless of the agent's answers: it returns diagnosis
  plus a ready command (CI test with a "consenting" agent).
- Accepted read risk: `screenshot` on Linux has no Omnibus-side gate (Wayland is protected by the DE
  portal, which is not an Omnibus contract; the X11 path has no system gate). Revisit = an opt-in
  grant on proven demand. On macOS the gate is TCC (Screen Recording).
- Consent UX, where an MCP host provides one, belongs to that host; Omnibus neither relies on nor
  duplicates it. The Omnibus gate stays deterministic: a grant present in config executes, anything
  else is a typed denial with a remedy.

## Actor and Trust

- Audit actor: `{uid, client_name (from MCP initialize), pid}`.
- Trust model: stdio transport within one uid; the local user is the trust anchor (standard desktop
  threat model); there is no separate channel authentication.

## Invariants

- No code path ever grants a capability based on anything an agent says.
- A fresh default install always exposes exactly the read-only tool set.
- `screenshot` is always audited like every read, even where no system gate exists.
- TCC grants are always issued by a human (GUI or MDM/PPPC); no adapter or doctor flow ever attempts
  programmatic TCC.

## Anti-Patterns

- Adding a "confirm" parameter to any tool "for UX" - it re-opens the forgery hole.
- Treating TCC presence as sufficient on macOS - the core scope model always applies on top (TCC
  granted + no TOML grant = `ScopeDenied`).

## Source Docs

- `docs/development/omnibus-spec.md` §4, §7 (never-list).
- `docs/development/omnibus-architecture.md` §3.3, §3.4, §7.4.
- `docs/development/omnibus-roadmap.md` tasks 1.3.3, 1.7.1, un-plan §4.4.

## Related Specs

- [Non-goals](non-goals.md) - the never-list in full.
- [Domain: capabilities and grants config](../contracts/config-and-grants.md) - manifest format.
- [Audit](../domains/audit.md) - event schema and provenance.

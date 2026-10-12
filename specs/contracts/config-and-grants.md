# Contract: Config and Grants (human <-> daemon)

## Boundary Rule

The grant store is edited only by the human - by hand or through `omnibusd grant` - and is read by the
core; nothing in the daemon or the agent can write it, and no runtime consent API exists.

## Interfaces

| Interface | Crate | Consumed By | Purpose |
| --------- | ----- | ----------- | ------- |
| TOML scope manifests — system templates `/etc/omnibus/scopes.d/*.toml` plus the user store `$XDG_CONFIG_HOME/omnibus/scopes.d` (fallback `$HOME/.config/omnibus/scopes.d`), where `omnibusd grant` writes `90-grants.toml` | `omnibus-core/src/scopes.rs` (reader) | core at probe time; human via editor or `omnibusd grant` | the grant stores (`[write]`, `[optin]` sections) |
| `omnibusd grant <name>` | `omnibusd` (human-executed CLI; edits the store itself, `omnibusd/src/grant.rs`; core only reads) | human | edits the manifest; doctor returns the ready-made command |
| `omnibusd doctor` (CLI mode) | `omnibusd` (CLI entry; diagnosis logic in `omnibus-core`) | human | executes opt-in mutations; doctor over MCP can only diagnose |
| `capabilities()` document | `omnibus-core` | agent | the assembled capability report with `remedy` fields |

## Manifest Format (field names are normative)

```toml
# grant manifests: system templates in /etc/omnibus/scopes.d; the user store that `omnibusd grant <name>` writes is $XDG_CONFIG_HOME/omnibus/scopes.d/90-grants.toml
[write]                                     # anything not listed is deny
"service.write"     = {}                    # -> svc_start, svc_stop
"pkg.undo"          = {}                    # -> pkg_undo (hidden without this entry)
"files.scope.write" = { paths = ["/home/user/Projects"] }   # parameterized scope -> fs_write

[optin]                                     # capability in status opt-in-required
"journal.persistent" = {}                   # consent to a mutation; executed by a human (doctor CLI)
```

- Section `[write]` gates every `[+scope]` tool; a parameterized grant carries its arguments (for
  example `paths`) in the entry value.
- Section `[optin]` gates capabilities whose status is `opt-in-required`; the same manifests, the
  same command, and the same audit provenance apply.
- Default is read-only; "temporary" means issued-then-revoked - there is no separate session channel
  (with stdio the config is static; the persistent/session distinction is illusory).

## Assembled `capabilities()` Document (field names are normative)

```json
{ "capabilities": [
  { "name": "pkg.undo",          "ver": "1.0.0", "status": "present",
    "value": "cache-downgrade",  "adapter": "alpm" },
  { "name": "journal.prev_boot", "ver": "1.0.0", "status": "opt-in-required",
    "value": false,              "adapter": "journald",
    "note": "the journal is volatile; past boots appear after persistent storage is granted",
    "remedy": "omnibusd grant journal.persistent" },
  { "name": "journal.persistent","ver": "1.0.0", "status": "opt-in-required",
    "adapter": "journald",       "remedy": "omnibusd grant journal.persistent" } ] }
```

- `ver` - semver of the capability contract; `value` - typed value for polymorphic capabilities;
  `note` - degradation limitation; `remedy` - copy-paste command (same principle as the
  `ScopeDenied` remedy); absence of a capability = absence of the entry.
- Grant entries never appear in the document; the document describes the machine, the manifests
  describe permissions - the core joins the two at tool-list build time.

## Initialization

- Every fresh instance reads the manifests at probe time and builds the grant table in the core;
  access decisions happen exclusively in the core.
- Packaging installs the manifest templates (`packaging/scopes.d/*.toml` into
  `/etc/omnibus/scopes.d/`, roadmap 1.8.1); a fresh install exposes exactly the read-only tool set.

## Data Flow Across Boundary

- Human -> store: TOML edits or `omnibusd grant` (which edits the manifest); doctor over MCP returns
  the command text but cannot execute a mutation.
- Store -> core: grant entries read at probe time; a changed store is picked up by the next session
  (no config watching - see [topology-and-lifecycle.md](../architecture/topology-and-lifecycle.md)).
- Core -> audit: every grant-related event carries provenance `config|cli|doctor` (see
  [audit](../domains/audit.md)).

## Error Propagation

- A write without a grant, or outside a parameterized scope, yields `ScopeDenied` with a one-line
  copy-paste remedy naming the exact grant to issue.
- A malformed manifest is a probe-time configuration error surfaced in doctor diagnostics; the
  daemon never guesses the intent of an unparsable entry.

## Breaking Change Checklist

If you change the manifest or document format, you MUST also:

- Update this contract and [capability-model.md](../architecture/capability-model.md) in the same
  change (the two formats are one decision surface).
- Ship a migration for existing manifests; format breaks are major-only.
- Update the doctor remedy generator and the `ScopeDenied` remedy text (they embed field names).
- Re-run the security CI tests (no-consent-API, provenance, doctor no-mutate - see
  [quality-gates.md](../architecture/quality-gates.md)).

## Source Docs

- `docs/development/omnibus-architecture.md` §3.3 (format anchors - the examples above are the
  normative field-name source), §3.4 (provenance).
- `docs/development/omnibus-roadmap.md` task 1.3.3 (single mechanism), 1.8.1 (packaging).

## Related Specs

- [Security Model](../architecture/security-model.md) - why exactly one mechanism exists.
- [Config and Grants domain: audit provenance](../domains/audit.md).
- [Tool Registry](../architecture/tool-registry.md) - which tools are `[+scope]`.

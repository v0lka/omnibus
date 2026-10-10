# Capability Model

## Context

`capabilities()` is the single machine-readable truth about a concrete machine (P4). The agent never
assumes: a missing capability is expressed as a missing tool, a limited one as a marked schema. This
spec fixes the format, the registry, detection levels, and the degradation ladder.

## Capability Format

`{domain}.{feature}[.{qualifier}]` + version (semver of the capability contract) + status
(`present` / `degraded` / `opt-in-required`) +, for polymorphic capabilities, a typed enum value from
contracts. Registry constants live in `omnibus-contracts`; name literals elsewhere in code are
forbidden (CI lint).

## Registry (20 constants)

| Domain | Constants |
|---|---|
| desktop | `desktop.tree`, `desktop.write.typed` |
| journal | `journal.query`, `journal.persistent`, `journal.prev_boot` |
| service | `service.read`, `service.write` |
| packages | `pkg.read`, `pkg.write`, `pkg.undo` (value enum: `none` / `cache-downgrade` / `transactional`) |
| portals | `portal.clipboard`, `portal.screenshot`, `portal.notify` |
| files | `files.scope.read`, `files.scope.write`, `files.scope.tcc` |
| kb | `kb.present` |
| net | `net.read` |
| intents (MS3) | `intents.list`, `intents.run` |

## Detection Levels

1. Compile-time - feature flags of adapter crates (per-OS-family build).
2. Install-time - package selection (PKGBUILD installs the linux set, Homebrew formula the macos set).
3. Runtime - the probe checks versions and daemon liveness (systemd1 on the bus, AT-SPI2 in the
   session, TCC state) and produces the final report. The runtime report overrides the first two.

## Assembled Document (field names are normative)

```json
{ "capabilities": [
  { "name": "pkg.undo",          "ver": "1.0.0", "status": "present",
    "value": "cache-downgrade",  "adapter": "alpm" },
  { "name": "journal.prev_boot", "ver": "1.0.0", "status": "present",
    "value": false,              "adapter": "journald", "note": "volatile: current boot only" },
  { "name": "journal.persistent","ver": "1.0.0", "status": "opt-in-required",
    "adapter": "journald",       "remedy": "omnibus grant journal.persistent" } ] }
```

- `ver` - semver of the capability contract; `value` - typed value for polymorphic capabilities;
  `note` - degradation limitation; `remedy` - copy-paste command (same principle as the `ScopeDenied`
  remedy); absence of a capability = absence of the entry.

## Degradation Ladder (by increasing severity)

1. Degraded capability - tool present, schema marks the limitation (example: `pkg_install` without
   `pkg.undo` on apt).
2. Missing capability - tool hidden from `tools/list`; a direct call returns `CapabilityRequired`
   with a doctor hint.
3. Below version floor - the adapter is not activated at all; `capabilities()` explains the reason
   (example: "systemd 247 < 252").
4. Grant not issued (TCC-style) - capability status `opt-in-required`; doctor walks the human through
   issuance; until then behavior equals state 2.

## Independent Capabilities (no derivation)

`journal.persistent` (storage setting survives reboots) and `journal.prev_boot` (the fact that past
boots exist in the journal) are independent: a freshly persistent machine without reboots has
`persistent=true` and no `prev_boot` yet. Neither is ever derived from the other.

## Tier vs Capabilities

A tier is a promise-and-testing category; `capabilities()` is machine truth. They may diverge
(example: volatile-by-default Ubuntu LTS reports `journal.persistent` as `opt-in-required` and hides
`journal.prev_boot` until the doctor opt-in, although its tier promises full reading);
`capabilities()` always wins a dispute.

## Invariants

- Every capability name exists exactly once, as a constant in `omnibus-contracts`.
- The tool list always reflects the capability document of the current session's fresh probe.
- The document always names the provider adapter, the version, and the limitations for every entry.

## Anti-Patterns

- Inferring one capability from another (the persistent/prev_boot rule is the counterexample).
- Runtime errors as the discovery mechanism for missing features.

## Source Docs

- `docs/development/omnibus-architecture.md` §7.1-§7.3, §7.5.
- `docs/development/omnibus-roadmap.md` tasks 1.2.3, 1.3.5, 1.4.2.

## Related Specs

- [Tool Registry](tool-registry.md) - binding and hiding.
- [Platform Matrix](platform-matrix.md) - floors and tiers.

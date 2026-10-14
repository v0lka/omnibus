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

## Registry (20 constants; 18 shipped, +2 with MS3)

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
  { "name": "journal.prev_boot", "ver": "1.0.0", "status": "opt-in-required",
    "value": false,              "adapter": "journald",
    "note": "the journal is volatile; past boots appear after persistent storage is granted",
    "remedy": "omnibusd grant journal.persistent" },
  { "name": "journal.persistent","ver": "1.0.0", "status": "opt-in-required",
    "adapter": "journald",       "remedy": "omnibusd grant journal.persistent" } ],
  "floors": [ { "component": "systemd", "reason": "systemd 247.0 < 252.0" } ] }
```

- `ver` - semver of the capability contract; `value` - typed value for polymorphic capabilities;
  `note` - degradation limitation; `remedy` - copy-paste command (same principle as the `ScopeDenied`
  remedy); absence of a capability = absence of the entry; `floors` - the additive document-level
  array explaining every unmet version floor, one `{component, reason}` entry per failing
  component (empty when all floors are met) - ladder rung 3 reaches the agent through it.

## Degradation Ladder (by increasing severity)

1. Degraded capability - tool present, schema marks the limitation (example: `pkg_install` without
   `pkg.undo` on apt).
2. Missing capability - tool hidden from `tools/list`; a direct call returns `CapabilityRequired`
   with a doctor hint.
3. Below version floor - the adapter is not activated at all; `capabilities()` explains the reason
   (example: "systemd 247.3 < 252.0").
4. Grant not issued (TCC-style) - capability status `opt-in-required`; doctor walks the human through
   issuance; until then behavior equals state 2.

## Independent Capabilities (no derivation)

`journal.persistent` (storage setting survives reboots) and `journal.prev_boot` (the fact that past
boots exist in the journal) are independent: a freshly persistent machine without reboots has
`persistent=true` and no `prev_boot` yet. Neither is ever derived from the other.

## Tier vs Capabilities

A tier is a promise-and-testing category; `capabilities()` is machine truth. They may diverge
(example: a machine with a volatile-configured journald reports `journal.persistent` as
`opt-in-required` and keeps `journal.prev_boot` under the ladder until the doctor-led opt-in
completes — the grant plus the manual persistent-storage creation and a reboot; see journal.md —
although its tier promises full reading);
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

# Packages

## Purpose

The packages domain covers listing, inspecting, installing, removing, and - where the provider genuinely can - undoing package operations through one `PackageManager` contract. Rollback is promised exactly where the package manager has one.

## Key Files

- `omnibus-contracts/` (planned) - `PackageManager` trait, package DTOs, capability constants
- `omnibus-adapter-alpm/src/` (planned) - Arch/Manjaro provider (crate `alpm`; ALPM handles never leave the adapter)
- `omnibus-adapter-apt/src/` (planned) - Debian/Ubuntu/Mint provider (backend choice: libapt vs CLI, ADR-002)
- `omnibus-adapter-dnf/src/` (planned) - Fedora provider
- `omnibus-adapter-brew/src/` (planned) - macOS provider (MS3): formula + cask; `mas` as the optional App Store backend

## Capability and Tools

| MCP tool | Capability | Scope |
| --- | --- | --- |
| `pkg_list` / `pkg_info` | `pkg.read` | none (reads are audited) |
| `pkg_install` / `pkg_remove` | `pkg.write` | [+scope] write grant required |
| `pkg_undo` | `pkg.undo` | [+scope]; hidden from `tools/list` when `pkg.undo=none` |

`pkg.undo` carries a typed value from the enum `none` / `cache-downgrade` / `transactional`.

## Behavior

Trait `PackageManager` operations: `list`, `info`, `search`, `install`, `remove`, `history`, `undo`.

| Provider | Platforms | `pkg.undo` value | Undo semantics |
| --- | --- | --- | --- |
| alpm | Arch (T1), Manjaro (T1) | `cache-downgrade` | downgrade to the previous version from the pacman cache, located via `history` |
| apt | Debian trixie/bookworm/bullseye, Ubuntu LTS, Mint | `none` | the tool honestly does not exist; apt has no transactional rollback |
| dnf | Fedora (T2) | `transactional` | dnf history undo/rollback to the previous transaction point |
| brew + mas | macOS (MS3, T2) | expected `none` (per the F3.1 gate) | honest schema degradation instead of a rollback promise |

- `history` always reads the provider's own transaction log: the pacman log for alpm, `/var/log/apt/history.log` for apt.
- An undo attempt where `pkg.undo=none` returns `CapabilityRequired` with a hint - never a failure in the middle of a transaction.
- On alpm, a version missing from the cache yields `NativeFailure` with an explanation; the pointer to ALA/AUR Archive lives in knowledge-base articles, not in adapter code (P7).
- apt transactions always run without interactive input (non-tty CLI flags are pinned by tests).
- On macOS, formula and cask share one contract (the package type is a DTO field); without `mas`, App Store packages drop out of `search` while everything else keeps working.
- A provider swap (dnf to dnf5) changes neither trait signatures nor capability constants.

## Degradation

- Degraded capability: on apt, `pkg_install`/`pkg_remove` stay visible with the limitation marked in the JSON schema ("undo unavailable - apt has no transactional rollback").
- Missing capability: `pkg_undo` is absent from `tools/list` when `pkg.undo=none`; a direct call returns `CapabilityRequired`.
- Below version floor: no version floor is defined for this subsystem.
- Grant not issued: `pkg_install`/`pkg_remove`/`pkg_undo` without a write grant return `ScopeDenied` with a copy-paste remedy command.

## Invariants

- Rollback is always promised exactly where the provider has one; `capabilities()` never promises an undo the backend lacks.
- `pkg_undo` is always hidden while `pkg.undo=none`.
- The install/remove schema always marks the missing rollback on machines with `pkg.undo=none`.
- ALPM handles always stay inside the alpm adapter; only DTOs cross the SPI boundary.
- Transaction history always comes from the provider's own log.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - subsystem #4: `PackageManager`.
- [Capability Model](../architecture/capability-model.md) - the `pkg.undo` polymorphic value.
- [Tool Registry](../architecture/tool-registry.md) - the hiding rule for `pkg_undo`.
- [Platform Matrix](../architecture/platform-matrix.md) - per-provider tiers and documented degradations.
- [Domain: Knowledge](knowledge.md) - undo hints (ALA/AUR Archive) live in KB articles, not adapter code.

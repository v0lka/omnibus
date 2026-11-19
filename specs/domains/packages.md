# Packages

## Purpose

The packages domain covers listing, inspecting, installing, removing, and - where the provider genuinely can - undoing package operations through one `PackageManager` contract. Rollback is promised exactly where the package manager has one.

## Key Files

- `omnibus-contracts/src/spi/packages.rs` - the `PackageManager` trait; package DTOs in `omnibus-contracts/src/dto/packages.rs`, capability constants in `omnibus-contracts/src/capabilities.rs`
- `omnibus-adapter-alpm/src/` - Arch/Manjaro provider (crate `alpm`; ALPM handles never leave the adapter)
- `omnibus-adapter-apt/src/` - Debian/Ubuntu/Mint provider (CLI mapping per ADR-002: `dpkg-query`, `apt-cache`, `apt-get`, history from `/var/log/apt/history.log`)
- `omnibus-adapter-dnf/src/` - Fedora provider (the dnf/dnf5 backend swap is resolved inside the adapter)
- `omnibus-adapter-brew/src/` (planned) - macOS provider (MS3): formula + cask; `mas` as the optional App Store backend
- `omnibus-adapter-flatpak/src/` - v1.5 flatpak/snap inventory provider: one crate for both stores, called through their CLI binaries; handles never leave the adapter. Read-part suite binder: the aggregate fixtures live in `omnibus-core/tests/package_aggregation.rs` (alpm+flatpak and apt+flatpak shapes)

## Capability and Tools

| MCP tool | Capability | Scope |
| --- | --- | --- |
| `pkg_list` / `pkg_info` | `pkg.read` | none (reads are audited) |
| `pkg_install` / `pkg_remove` | `pkg.write` | [+scope] write grant required |
| `pkg_undo` | `pkg.undo` | [+scope]; hidden from `tools/list` when `pkg.undo=none` |

`pkg.undo` carries a typed value from the enum `none` / `cache-downgrade` / `transactional`.

(v1.5) `pkg_list` / `pkg_info` aggregate the native manager and the flatpak/snap inventory; the DTO
carries an explicit source field for every package: `native` / `flatpak` / `snap`.

## Behavior

Trait `PackageManager` operations: `list`, `info`, `search`, `install`, `remove`, `history`, `undo`.

| Provider | Platforms | `pkg.undo` value | Undo semantics |
| --- | --- | --- | --- |
| alpm | Arch (T1), Manjaro (T1) | `cache-downgrade` | downgrade to the previous version from the pacman cache, located via `history` |
| apt | Debian trixie/bookworm/bullseye, Ubuntu LTS, Mint | `none` | the tool honestly does not exist; apt has no transactional rollback |
| dnf | Fedora (T2) | `transactional` | dnf history undo/rollback to the previous transaction point |
| brew + mas | macOS (MS3, T2) | expected `none` (per the F3.1 gate) | honest schema degradation instead of a rollback promise |
| flatpak/snap (v1.5) | Linux, wherever flatpak/snap are installed (snap ships by default only on Ubuntu among the targets) | n/a - read-only inventory | implements only `list`/`info`; no write operations exist for these sources |

- `history` always reads the provider's own transaction log: the pacman log for alpm, `/var/log/apt/history.log` for apt, the dnf history database for dnf.
- A machine with NO history recorded yet (fresh install, disposable container: the log file or the dnf history database is absent) answers `history` with an empty list, never a `NativeFailure` — a missing auxiliary artifact degrades to the empty read (the contract suite pins `history yields Ok` on every live backend). A PRESENT log or database that cannot be read stays a `NativeFailure`: `undo` must never mistake an unreadable history for "nothing ever happened".
- An undo attempt where `pkg.undo=none` returns `CapabilityRequired` with a hint - never a failure in the middle of a transaction.
- On alpm, a version missing from the cache yields `NativeFailure` (`undo-no-cache-version`); the payload names the ALA/AUR archive, and the actionable treatment — which article, which URL — lives in knowledge-base articles (P7).
- apt transactions always run without interactive input (non-tty CLI flags are pinned by tests).
- On macOS, formula and cask share one contract (the package type is a DTO field); without `mas`, App Store packages drop out of `search` while everything else keeps working.
- A provider swap (dnf to dnf5) changes neither trait signatures nor capability constants.
- (v1.5) The flatpak/snap inventory provider implements only the read part of `PackageManager`
  (`list`, `info`); `install`/`remove`/`undo` are not declared for these sources - the honest
  limitation is marked in the schema and in `capabilities()` (roadmap task 2.9.1).
- (v1.5) The probe activates the native manager and the inventory adapter together - the sole
  documented exception to the one-active-adapter rule (architecture §9); both providers are visible
  in `capabilities()`, and write semantics belong to the native manager only.
- (v1.5) With flatpak/snap absent from the machine, the aggregated `pkg_list`/`pkg_info` output is
  unchanged and reports no degradation (R7).
- (v1.5) The aggregated `pkg_list` reads the active providers concurrently; native rows always
  precede inventory rows and the request `limit` truncates the merged output. An error from either
  active provider fails the whole call - a silently skipped half would claim completeness the
  merged output does not have.
- (v1.5) The aggregated `pkg_info` asks the native manager first and falls through to the inventory
  only on its miss; when nothing answers, the native manager's error is the honest answer. The
  audit event names its contributors: the `adapter` field carries the two adapters joined with `+`
  for a merged `pkg_list`, and the answering adapter for `pkg_info`.

## Degradation

- Degraded capability: on apt, `pkg_install`/`pkg_remove` stay visible with the limitation marked in the JSON schema ("undo unavailable - apt has no transactional rollback").
- Missing capability: `pkg_undo` is absent from `tools/list` when `pkg.undo=none`; a direct call returns `CapabilityRequired`.
- Below version floor: no version floor is defined for this subsystem.
- Grant not issued: `pkg_install`/`pkg_remove`/`pkg_undo` without a write grant return `ScopeDenied` with a copy-paste remedy command.
- (v1.5) An absent flatpak or snap source is plain absence from the aggregated output - an honest
  omission, never a degradation state.

## Invariants

- Rollback is always promised exactly where the provider has one; `capabilities()` never promises an undo the backend lacks.
- `pkg_undo` is always hidden while `pkg.undo=none`.
- The install/remove schema always marks the missing rollback on machines with `pkg.undo=none`.
- ALPM handles always stay inside the alpm adapter; only DTOs cross the SPI boundary.
- Transaction history always comes from the provider's own log.
- An absent history artifact (no log, no dnf history database) always reads as an empty history, never as an error; an unreadable present artifact always reads as `NativeFailure`.
- (v1.5) The inventory provider always declares only read operations; the source of every package in
  `pkg_list` is always explicit in the DTO.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - subsystem #4: `PackageManager`.
- [Capability Model](../architecture/capability-model.md) - the `pkg.undo` polymorphic value.
- [Tool Registry](../architecture/tool-registry.md) - the hiding rule for `pkg_undo`.
- [Platform Matrix](../architecture/platform-matrix.md) - per-provider tiers and documented degradations.
- [Domain: Knowledge](knowledge.md) - undo hints (ALA/AUR Archive) live in KB articles, not adapter code.

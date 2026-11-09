# App Launch

## Purpose

The app-launch domain closes the agent loop on Linux (decision D1, v1.5): the agent inventories
installed applications, launches them as transient user units, and opens URIs/files with the
associated application. Launched processes land in the user's cgroup and journald, observable through
`ps.list` and `journal_query` - launch and observation share one addressable world.

## Key Files

- `omnibus-contracts/src/spi/app-launcher.rs` - the `AppLauncher` trait, declared
  just-in-time (task 2.7.1) together with its first implementation (task 2.7.2)
- `omnibus-contracts/src/suite/` - the `app_launcher_suite` contract-suite family, wired
  from the adapter's `tests/contract_suite.rs` (task 2.7.2)
- `omnibus-adapter-launch/src/` - Linux provider: `.desktop` inventory, `systemd-run
  --user`, `xdg-open` (task 2.7.2); `omnibus-adapter-launch/tests/contract_suite.rs`
  binds the suite against a synthetic `.desktop` fixture stand plus the live
  systemd user manager

## Capability and Tools

| MCP tool | Capability | Scope |
| --- | --- | --- |
| `app.list` | `app.read` | none (reads are audited) |
| `app.launch` / `app.open` | `app.launch` | [+scope `app.launch`] write grant required |

## Behavior

- `app.list` inventories `.desktop` files from `/usr/share/applications` plus
  `$XDG_DATA_HOME`/`~/.local/share/applications`; `Exec`, `Name`, `Icon`, `NoDisplay` feed the DTO.
- `app.launch` starts the application as a transient user unit via `systemd-run --user`; the process
  lands in the user's cgroup and journald, so `ps.list` and `journal_query` observe it (the
  launch -> observe side of the closed loop).
- `app.open` opens a URI or a file with the associated application via `xdg-open`.
- `AppSpec{app_id, args}` is a flat named struct; application arguments always travel as a typed
  list inside the DTO and reach the target process verbatim - shell metacharacters (`;`, `|`,
  backtick) are data, never syntax.
- Every trait method returns a contract error; `NativeFailure{code}` preserves the original message
  payload.

## Degradation

- Degraded capability: no partial provider states are defined - the provider serves the full surface
  or is absent.
- Missing capability: `app.open` is hidden on a stand without `xdg-open`, with a clean log; the
  reason lives in `capabilities()` (R3). A direct call by name returns `CapabilityRequired`.
- Below version floor: `systemd-run` ships with systemd itself - the systemd >= 252 floor covers
  every target Linux platform; a platform below the floor leaves the launch part inactive (see
  [Platform Matrix](../architecture/platform-matrix.md)).
- Grant not issued: `app.launch`/`app.open` without the `app.launch` write grant return `ScopeDenied`
  plus an audit event.

## Invariants

- Application arguments always cross the SPI as a typed list in the DTO; the adapter always executes
  the target directly, and metacharacters in arguments always arrive verbatim.
- `app.launch`/`app.open` always produce a full audit event
  `{ts, session, actor, tool, adapter, op, params_hash, result, scope_ref}` (R5).
- Every launched application always lands in a transient user unit observable by `ps.list` and
  `journal_query`.
- The `AppLauncher` trait enters the registry only together with its first implementation (ADR-011);
  the registry stays closed over the roadmap task list.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - subsystem #11: `AppLauncher`.
- [Tool Registry](../architecture/tool-registry.md) - tool bindings and hiding rule.
- [Capability Model](../architecture/capability-model.md) - `app.read` / `app.launch`.
- [Platform Matrix](../architecture/platform-matrix.md) - v1.5 floors (`systemd-run`, `xdg-open`).
- [Domain: Process Telemetry](process-telemetry.md) - the observe side of the closed loop.

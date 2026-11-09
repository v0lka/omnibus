# Process Telemetry

## Purpose

The process-telemetry domain gives the agent live system state on Linux (decision D2, v1.5): the
process list, per-PID snapshots, system load statistics, and power status. Its grant-gated
`ps.signal` write is the "act" side of the closed agent loop (launch -> observe -> signal).

## Key Files

- `omnibus-contracts/src/spi/process-telemetry.rs` - the `ProcessTelemetry` trait, declared
  just-in-time with its first implementation (task 2.8.1)
- `omnibus-contracts/src/suite/` - the `process_telemetry_suite` contract-suite family, including
  negative cases (a nonexistent PID yields a contract error, never a panic)
- `omnibus-adapter-proc/tests/contract_suite.rs` - the suite binder: the live `/proc` stand plus
  a synthetic 300+-process fixture stand (task 2.8.2)
- `omnibus-adapter-proc/src/` - Linux read provider: `/proc`, sysfs, UPower over D-Bus
  (task 2.8.2)
- `omnibus-adapter-proc/src/write.rs` - the `ps.signal` write path (task 2.8.3);
  write-scope gating lives in the core

## Capability and Tools

| MCP tool | Capability | Scope |
| --- | --- | --- |
| `ps.list` / `ps.info` | `ps.read` | none (reads are audited) |
| `sys.stats` | `sys.read` | none (reads are audited) |
| `power.status` | `power.read` | none; hidden without UPower/battery |
| `ps.signal` | `ps.signal` | [+scope `ps.signal`] write grant required |

## Behavior

- `ps.list` reads `/proc`: pid, name, state, CPU, RSS, cgroup; limits and filters travel in the DTO
  (the same discipline as `journal_query`).
- `ps.info` returns a consistent per-PID snapshot; a nonexistent PID produces a contract error.
- `sys.stats` reports CPU/RAM load, load average, memory and swap from `/proc` and `/sys`.
- `power.status` reports battery and power supply through UPower D-Bus "when present"
  (probe auto-detect).
- `ps.signal` sends one signal from the closed contract enum (`TERM`, `INT`, `HUP`, `USR1`, `USR2`,
  `KILL`); addressing is always by `pid` taken from `ps.list` as fact, never by a guessed name.
- PID 1 and systemd-unit processes stay outside listing, info, and signaling: services are addressed
  only through `svc_*`; a signal aimed at them returns `Unsupported` with a hint to the service
  tools.
- Reading `/proc` of other users stays within the daemon's uid view - the subsystem reports exactly
  what the daemon's uid can see.
- Units use newtype wrappers: `Percent`, `Bytes`, `Milliseconds`, `Seconds`.

## Degradation

- Degraded capability: no partial provider states are defined - each tool is either fully served or
  absent.
- Missing capability: `power.status` is hidden on a machine without UPower/battery, with a clean
  log; the reason lives in `capabilities()` (R7: the machine is never promised what it lacks).
- Below version floor: `/proc` and sysfs carry no version floor; the read part activates on every
  target Linux platform.
- Grant not issued: `ps.signal` without the `ps.signal` write grant returns `ScopeDenied` plus an
  audit event; with the grant, the signal is delivered (integration-tested against a test process).

## Invariants

- PID 1 and systemd-unit processes always stay outside `ps.list`, `ps.info`, and `ps.signal`;
  service addressing always goes through the services subsystem.
- `ps.signal` always accepts only the closed signal enum from contracts; a raw signal number always
  fails.
- Every `ps.signal` call always produces a full audit event
  `{ts, session, actor, tool, adapter, op, params_hash, result, scope_ref}` (R5), under the single
  grant mechanism (`omnibusd grant ps.signal`; TOML manifests, provenance `config|cli|doctor`).
- Reads always reflect the daemon's uid view of `/proc` - the subsystem always stays inside the
  daemon's own privileges.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - subsystem #12: `ProcessTelemetry`.
- [Tool Registry](../architecture/tool-registry.md) - tool bindings and hiding rule.
- [Capability Model](../architecture/capability-model.md) - `ps.read`, `ps.signal`, `sys.read`,
  `power.read`.
- [Platform Matrix](../architecture/platform-matrix.md) - the UPower/battery presence rule.
- [Domain: Services](services.md) - the `svc_*` addressing that replaces PID addressing for units.
- [Domain: App Launch](app-launch.md) - the launch side of the closed loop.

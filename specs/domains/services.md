# Services

## Purpose

The services domain lets the agent inspect and, with a write grant, control system services through one typed contract on both OS families. Reads are always available and audited; writes require `service.write` plus a scope entry.

## Key Files

- `omnibus-contracts/` (planned) - `ServiceManager` trait, unit DTOs, capability constants
- `omnibus-adapter-systemd1/src/` (planned) - Linux provider: `list`/`status` via org.freedesktop.systemd1 (zbus)
- `omnibus-adapter-systemd1/src/write.rs` (planned) - `start/stop/restart`, `enable/disable`
- `omnibus-adapter-launchd/src/` (planned) - macOS provider (MS3): launchctl, ServiceManagement 13+

## Capability and Tools

| MCP tool | Capability | Scope |
| --- | --- | --- |
| `svc_list` / `svc_status` | `service.read` | none (reads are audited) |
| `svc_start` / `svc_stop` | `service.write` | [+scope] write grant required |

## Behavior

Trait `ServiceManager` operations: `list`, `status`, `start/stop/restart`, `enable/disable`.

```
svc_* tool --> ServiceManager trait --> provider
  systemd1 (Linux): org.freedesktop.systemd1 over D-Bus (zbus)
  launchd (macOS, MS3): LaunchAgent context - only agents are visible
    in the GUI session; Scope::User / Scope::System is a DTO field
```

- The systemd1 read path maps D-Bus error codes into `NativeFailure{code}` with the payload preserved; a nonexistent unit always yields a contract error, never a panic.
- The write path never intercepts OS authorization: polkit rules apply as they are; launchd system-domain actions without root produce an honest contract error.
- On macOS the core policy forbids raising system daemons from the session; the adapter serves the agent-visible LaunchAgent domain.
- A restarted unit is verified to reach the active state under the core timeout policy.

## Degradation

- Degraded capability: not applicable - the subsystem has no partial-value capabilities.
- Missing capability: with no active provider the tools are absent from `tools/list`; a direct call returns `CapabilityRequired` with a doctor hint.
- Below version floor: systemd below 252 deactivates the adapter (Debian bullseye = 247, tier T3); `capabilities()` explains the reason, e.g. "systemd 247 < 252".
- Grant not issued: `svc_start`/`svc_stop` without a write grant return `ScopeDenied` with a one-line copy-paste grant command; the denial itself is written to the audit sink.

## Invariants

- The write path is always audited, including polkit denials.
- `svc_start`/`svc_stop` always require `service.write` plus a write grant from `/etc/omnibus/scopes.d/*.toml`.
- OS authorization (polkit, launchd domains) always stays with the operating system; Omnibus never intercepts it.
- A nonexistent unit always produces a contract error with a preserved payload, never a crash.
- System daemons are never raised from the user session on macOS.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - subsystem #3: `ServiceManager`.
- [Tool Registry](../architecture/tool-registry.md) - the `svc_*` binding table.
- [Platform Matrix](../architecture/platform-matrix.md) - the systemd >= 252 floor.
- [Security Model](../architecture/security-model.md) - write grants and the `ScopeDenied` remedy.
- [Error Taxonomy](../architecture/error-taxonomy.md) - `NativeFailure` for authorization failures.

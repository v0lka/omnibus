# Network

## Purpose

The network domain exposes read-only network status - devices, connectivity, and wifi networks - through one tool. It is the smallest subsystem in v1: one trait, one operation, one provider, and no macOS implementation.

## Key Files

- `omnibus-contracts/src/spi/net.rs` - the `NetworkStatus` trait, declared together with its first implementation (just-in-time)
- `omnibus-adapter-nm/` - the only provider: NetworkManager over D-Bus (zbus), its own crate and subsystem per ADR-003

## Capability and Tools

| MCP tool | Capability | Notes |
| --- | --- | --- |
| `net_status` | `net.read` | visible only when NetworkManager is live on the bus |

## Behavior

Trait `NetworkStatus` operation: `status()` returning devices, connectivity, and wifi networks.

```
net_status --> NetworkStatus trait --> nm adapter (read-only)
  NetworkManager on D-Bus, "when present"
  behind netplan/networkd (e.g. Ubuntu Server):
      capability absent -> tool hidden
  macOS: no provider in v1 (out of v1)
```

- The trait is declared just-in-time with its first implementation in the same roadmap task (2.2.3) - no speculative stubs; `NetworkStatus` is the registry's named example of this rule.
- The adapter contains no NM write methods: the contract declares none and the code review verifies it.
- The absence of NetworkManager affects no other subsystem - every other domain's tools keep working.
- Network write actions are out of scope for v1 (un-plan item; revisit: a separate decision with a
  real use case). The never-list covers switching the network backend, not status reads.
- On Debian-family desktops NetworkManager is the common case; a machine behind netplan/networkd simply reports no `net.read`.

## Degradation

- Degraded capability: not applicable - the subsystem reports one state, with no partial values.
- Missing capability: behind netplan/networkd, `net.read` is absent and `net_status` is hidden from `tools/list` without errors in the log; a direct call returns `CapabilityRequired` (never "method not found").
- Below version floor: not applicable - the gate is NM liveness on the bus, not a version number.
- Grant not issued: not applicable - the tool is read-only and is audited like every read.

## Invariants

- `net_status` is always read-only; no NM write method ever exists in the adapter or the contract.
- The `NetworkStatus` trait is always declared together with its first implementation, in its own crate.
- Absence of NetworkManager always leaves the other subsystems unaffected.
- The v1 adapter and the `NetworkStatus` contract never contain a write method; network write
  actions stay an un-plan item, never a silent feature.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - subsystem #10 and the just-in-time trait rule.
- [Non-goals](../architecture/non-goals.md) - network write actions on the un-plan; the backend switch on the never-list.
- [Tool Registry](../architecture/tool-registry.md) - the `net_status` binding.
- [Capability Model](../architecture/capability-model.md) - hiding a missing capability.
- [Error Taxonomy](../architecture/error-taxonomy.md) - `CapabilityRequired` for a hidden-tool direct call.

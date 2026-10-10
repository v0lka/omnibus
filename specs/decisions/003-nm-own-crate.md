# ADR-003: NetworkManager Status in Its Own Crate

## Status

Accepted

## Context

The platform matrix promises NM read-only status "when present". NM is a separate D-Bus service,
not part of the systemd1 service surface; stuffing it into an existing adapter would violate the
one-subsystem-per-trait-set mapping and the adapter independence rule.

## Decision

A dedicated crate `omnibus-adapter-nm` (planned) implements the `NetworkStatus` trait, which is
declared in `omnibus-contracts` together with this first implementation (just-in-time, task 2.2.3).
Read-only: devices, connectivity, wifi networks. Behind netplan/networkd the capability is absent
and `net_status` is hidden. Network writes are out of scope for v1 (un-plan; the never-list covers
switching the network backend); no macOS provider in v1.

## Consequences

- Absence of NM never affects the other subsystems (contract suite stays green).
- The closed registry grows by exactly one trait at its first implementation - the closed-registry
  rule demonstrated, not amended.

## Alternatives Considered

- Reusing `omnibus-adapter-systemd1` for NM - rejected: different D-Bus service, different
  subsystem; an adapter-to-adapter or multi-service adapter blurs the SPI map.
- Omitting network status from v1 - rejected: a cheap read-only diagnostic closing a top support
  question; the contradiction-fix round added the subsystem deliberately.

## Source Docs

- `docs/development/omnibus-roadmap.md` task 2.2.3 ("ADR-003 closed": NM as its own crate).
- `docs/development/omnibus-architecture.md` section 4 (network status subsystem).

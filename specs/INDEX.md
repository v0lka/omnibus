# INDEX — Omnibus Specification System

Navigation entry point. For formats and rules see [META.md](META.md); for day-to-day usage see
[WORKFLOW.md](WORKFLOW.md). Source of truth hierarchy and language rules are defined in
[META.md](META.md).

## Task -> Spec

| Task | Read |
|---|---|
| Understand what Omnibus is (product, topology, principles) | [overview](architecture/overview.md), [principles](architecture/principles.md) |
| Find the complete MCP tool list and bindings | [tool-registry](architecture/tool-registry.md) |
| Implement or modify an MCP tool | [tool-registry](architecture/tool-registry.md), [MCP client <-> daemon](contracts/mcp-client-daemon.md), the tool's domain spec under [domains/](#directory-listing) |
| Check what a concrete machine supports | [capability-model](architecture/capability-model.md) |
| Add or change a capability | [capability-model](architecture/capability-model.md), [config and grants](contracts/config-and-grants.md) |
| Implement a subsystem adapter | [spi-registry](architecture/spi-registry.md), [core <-> adapters](contracts/core-adapters.md), the domain spec |
| Launch apps, observe processes/system/power (v1.5) | [app-launch](domains/app-launch.md), [process-telemetry](domains/process-telemetry.md) |
| Understand grants, scopes, consent rules | [security-model](architecture/security-model.md), [config and grants](contracts/config-and-grants.md), [ADR-006](decisions/006-consent-out-of-agent-band.md), [ADR-010](decisions/010-single-grant-mechanism.md) |
| Understand the audit event format | [audit](domains/audit.md) |
| Interpret a typed error | [error-taxonomy](architecture/error-taxonomy.md) and the tool's domain spec |
| Reason about sessions, limits, concurrency | [topology-and-lifecycle](architecture/topology-and-lifecycle.md), [ADR-005](decisions/005-stdio-only-topology.md) |
| Port to a new distro or platform | [platform-matrix](architecture/platform-matrix.md), [quality-gates](architecture/quality-gates.md) |
| Onboarding, doctor flows, TTFM | [doctor](domains/doctor.md) |
| Check what is deliberately not built | [non-goals](architecture/non-goals.md), [ADR-007](decisions/007-zero-inference-daemon.md), [ADR-008](decisions/008-polling-over-push-events.md), [ADR-009](decisions/009-typed-writes-only-input-deferred.md), [ADR-012](decisions/012-no-playbook-engine.md) |
| Understand why X is decided that way | [decisions/](decisions/_template.md) (ADRs), the domain's Invariants |
| Change specs (format, update rules) | [META.md](META.md), [WORKFLOW.md](WORKFLOW.md) |
| Validate specs after changes | `python3 specs/tools/validate_specs.py` (must exit 0) |

## Dependency Graph

```
MCP client (user's agent; all inference lives here)
    ||  stdio, one session per connection      [contracts/mcp-client-daemon.md]
omnibusd core (omnibus-core)  ------------------ [architecture/topology-and-lifecycle.md]
    ||  SPI traits + DTOs + capability consts   [contracts/core-adapters.md]
omnibus-adapter-* (probe-selected, one per subsystem)
    ||  TOML scopes.d manifests (human-edited)  [contracts/config-and-grants.md]
human (grants, opt-ins, doctor CLI mutations) -- [architecture/security-model.md]
```

Dependency direction: adapters depend on `omnibus-contracts` only; the core crate depends on contracts
alone; the composition root (the `omnibusd` binary target) links adapters and registers them through
the probe-assembled registry; only the composition-root binary depends on the core — no library
crate does. See
[layers-and-crates](architecture/layers-and-crates.md).

## Directory Listing

Every file under `specs/`:

- [META.md](META.md) - spec system rules, templates, validation contract
- [INDEX.md](INDEX.md) - this file
- [WORKFLOW.md](WORKFLOW.md) - spec usage and update workflow

`architecture/`:

- [architecture/capability-model.md](architecture/capability-model.md) - capabilities(), registry, degradation ladder
- [architecture/coupling-model.md](architecture/coupling-model.md) - core<->adapter coupling analysis
- [architecture/error-taxonomy.md](architecture/error-taxonomy.md) - the 8 typed contract errors
- [architecture/evolution-and-compat.md](architecture/evolution-and-compat.md) - versioning, adapter evolution
- [architecture/layers-and-crates.md](architecture/layers-and-crates.md) - workspace layout, dependency rules
- [architecture/non-goals.md](architecture/non-goals.md) - never-list and un-plan
- [architecture/overview.md](architecture/overview.md) - system overview
- [architecture/platform-matrix.md](architecture/platform-matrix.md) - tiers T1/T2/T3, floors, providers
- [architecture/principles.md](architecture/principles.md) - P1-P7
- [architecture/quality-gates.md](architecture/quality-gates.md) - the seven rules, CI security tests
- [architecture/risks-and-mitigations.md](architecture/risks-and-mitigations.md) - risk register, M0 gates
- [architecture/security-model.md](architecture/security-model.md) - grants, consent, trust
- [architecture/spi-registry.md](architecture/spi-registry.md) - 14 traits / 12 subsystems
- [architecture/tech-stack.md](architecture/tech-stack.md) - Rust stack, packaging
- [architecture/tool-registry.md](architecture/tool-registry.md) - the 37 MCP tools
- [architecture/topology-and-lifecycle.md](architecture/topology-and-lifecycle.md) - stdio topology, sessions, limits

`domains/`:

- [domains/app-launch.md](domains/app-launch.md) - app.list / app.launch / app.open (v1.5)
- [domains/audit.md](domains/audit.md) - audit event schema, AuditSink
- [domains/clipboard.md](domains/clipboard.md) - clip_read / clip_write
- [domains/desktop-a11y.md](domains/desktop-a11y.md) - desktop tree, element, typed writes
- [domains/doctor.md](domains/doctor.md) - diagnosis, remedies, opt-ins, TCC wizard
- [domains/files.md](domains/files.md) - scoped file access
- [domains/journal.md](domains/journal.md) - journal_query, persistence, past boots
- [domains/knowledge.md](domains/knowledge.md) - wiki adapters, caching
- [domains/network.md](domains/network.md) - net_status (read-only)
- [domains/notifications.md](domains/notifications.md) - notify
- [domains/packages.md](domains/packages.md) - package tools, undo semantics, flatpak/snap inventory (v1.5)
- [domains/process-telemetry.md](domains/process-telemetry.md) - ps.list / ps.info / sys.stats / power.status / ps.signal (v1.5)
- [domains/screen-capture.md](domains/screen-capture.md) - screenshot, accepted read risk
- [domains/services.md](domains/services.md) - service tools

`contracts/`:

- [contracts/config-and-grants.md](contracts/config-and-grants.md) - TOML manifest format, grant flow
- [contracts/core-adapters.md](contracts/core-adapters.md) - SPI boundary rules
- [contracts/mcp-client-daemon.md](contracts/mcp-client-daemon.md) - transport, tool/error surface

`decisions/`:

- [decisions/_template.md](decisions/_template.md) - ADR template
- [decisions/001-dyn-trait-spi.md](decisions/001-dyn-trait-spi.md) - dyn Trait SPI
- [decisions/002-apt-backend.md](decisions/002-apt-backend.md) - apt backend (Accepted, closed by task 2.2.1)
- [decisions/003-nm-own-crate.md](decisions/003-nm-own-crate.md) - NM in its own crate
- [decisions/004-unifiedlog-backend.md](decisions/004-unifiedlog-backend.md) - unified log backend (Proposed, task 3.3.1)
- [decisions/005-stdio-only-topology.md](decisions/005-stdio-only-topology.md) - stdio-only topology
- [decisions/006-consent-out-of-agent-band.md](decisions/006-consent-out-of-agent-band.md) - consent out of the agent's band
- [decisions/007-zero-inference-daemon.md](decisions/007-zero-inference-daemon.md) - zero model calls
- [decisions/008-polling-over-push-events.md](decisions/008-polling-over-push-events.md) - no event channel in v1
- [decisions/009-typed-writes-only-input-deferred.md](decisions/009-typed-writes-only-input-deferred.md) - typed writes, emulation deferred
- [decisions/010-single-grant-mechanism.md](decisions/010-single-grant-mechanism.md) - one grant mechanism
- [decisions/011-closed-registry-just-in-time-traits.md](decisions/011-closed-registry-just-in-time-traits.md) - closed registry, JIT traits
- [decisions/012-no-playbook-engine.md](decisions/012-no-playbook-engine.md) - no playbook engine
- [decisions/013-binary-name-omnibusd.md](decisions/013-binary-name-omnibusd.md) - shipped binary name is `omnibusd` (spelling erratum for earlier ADRs)

`tools/`:

- [tools/coverage.toml](tools/coverage.toml) - heading -> spec coverage matrix
- [tools/validate_specs.py](tools/validate_specs.py) - validator (exit 0 = green)

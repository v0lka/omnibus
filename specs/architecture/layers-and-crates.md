# Layers and Crates

## Context

Three layers with one explicit seam. The seam (`omnibus-contracts`) is the only shared dependency
and the only place where traits, DTOs, capability constants, and the error taxonomy live.

## Crate Map

```
+------------------------------------------------------------------------+
| omnibus-core (binary omnibusd) - OS-agnostic                           |
|   MCP facade (rmcp) - sessions - grant checks - audit pipeline         |
|   capability registry - capabilities() - doctor                        |
+------------------------------------+-----------------------------------+
                                     | SPI: traits + DTO + capability
                                     | registry + error taxonomy
+------------------------------------v-----------------------------------+
| omnibus-contracts - the ONLY shared dependency, zero OS dependencies   |
+------------------------------------+-----------------------------------+
                                     |
+------------------------------------v-----------------------------------+
| omnibus-adapter-* (subsystem x family) + omnibus-probe                 |
+------------------------------------------------------------------------+
```

Planned layout (created by roadmap task 1.1.2; no code exists yet):

```
omnibus-contracts/    # spi/*.rs, dto/*.rs, tokens.rs, capabilities.rs, errors.rs
omnibus-core/         # facade/, sessions/, scopes/, audit/, capabilities/, tools/doctor.rs
omnibus-probe/        # detect.rs, floors.rs
omnibus-adapter-*/    # one crate per active subsystem implementation
```

## Dependency Rules (CI-enforced)

```
contracts <- adapters          contracts <- core          core <- (dynamically) adapters
adapters not-> adapters (forbidden)        adapters not-> core internals (forbidden, CI lint)
```

- The direction is acyclic and fixed: everything depends on contracts; nothing depends downwards
  on implementations.
- Core receives adapters through a registry assembled by the probe; core never links adapter crates
  directly (linking happens in the composition root / binary target).
- A PR in which an adapter depends on `omnibus_core` fails the CI dependency-rules job.

## Why `dyn Trait`, Not Generics or Plugins

Generics would freeze the adapter set at compile time and break runtime platform detection (P4).
Dynamic `.so` loading was rejected: Rust ABI is unstable, and the contracts crate already provides a
clean seam. Third-party adapters ship as fork+build for v1; extracting contracts into a separate
repository happens on the first external adapter contributor (ADR-001, architecture §8.8.3).

## Decision Map

| Decision | Where recorded |
|---|---|
| Traits + `dyn Trait` over generics/plugins | ADR-001 |
| One active adapter per subsystem, probe selects | [spi-registry.md](spi-registry.md), ADR in evolution rules |
| apt backend: libapt vs CLI | ADR-002 (Proposed, closed by task 2.2.1) |
| NM as its own crate and subsystem | ADR-003 |
| unified log backend choice | ADR-004 (Proposed, closed by task 3.3.1) |

## Invariants

- `omnibus-contracts` always has zero OS dependencies and zero dependencies on core.
- Adapter-to-adapter calls never happen (P6); composition is always core work.
- CI always fails on: adapter importing `omnibus_core`, contracts importing an OS crate, capability
  name literals outside the registry.

## Anti-Patterns

- "Convenience" adapter-to-adapter calls (for example journald adapter calling the systemd1 adapter).
- Feature-detecting the OS inside the core instead of reading the probe report.

## Source Docs

- `docs/development/omnibus-architecture.md` §2, §8.8.

## Related Specs

- [Coupling Model](coupling-model.md) - why this boundary stays weak.
- [Quality Gates](quality-gates.md) - the CI jobs that enforce the rules.

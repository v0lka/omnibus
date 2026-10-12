# Desktop Accessibility

## Purpose

Semantic access to desktop applications through accessibility trees: read the UI structure, inspect
elements, and perform typed writes. The domain implements the read/write asymmetry principle -
reading is always semantic (role+name addressing), writing is always typed (`set_value`/`invoke`),
never keyboard or mouse emulation.

## Key Files

- `omnibus-contracts/src/spi/a11y.rs` - the `DesktopA11y` trait; tree/element DTOs in `omnibus-contracts/src/dto/desktop.rs`, capability constants in `omnibus-contracts/src/capabilities.rs` (roadmap task 1.2.1).
- `omnibus-adapter-atspi/src/lib.rs` - Linux provider: `tree(app)`, `element(id)`, `read_attr` over AT-SPI2, plus the typed write path `set_value`, `invoke` (tasks 1.5.1, 1.6.1).
- `omnibus-adapter-ax/src/` (planned) - macOS provider on NSAccessibility/AXUIElement, MS3 (task 3.2.1).

## Capability and Tools

| MCP tool | Capabilities | Grant | `DesktopA11y` ops |
| --- | --- | --- | --- |
| `desktop.tree` | `desktop.tree` | none | `tree(app)` |
| `desktop.element` | `desktop.tree` | none | `element(id)`, `read_attr` |
| `desktop.set_value` [+scope] | `desktop.tree` + `desktop.write.typed` | write grant | `set_value` |
| `desktop.invoke` [+scope] | `desktop.tree` + `desktop.write.typed` | write grant | `invoke` |

A missing `desktop.tree` capability hides all four tools from `tools/list`; a missing
`desktop.write.typed` (or grant) hides only the two write tools.

## Behavior

```
agent --> desktop.tree(app) ------> core --> DesktopA11y::tree --> provider
      <-- tree DTO: role+name nodes, opaque element_id (minted by core)
agent --> desktop.element(id) ----> core --> read_attr ----------> attribute DTO
agent --> desktop.set_value/invoke [+scope]
      <---------------------------- core grant check --> typed write --> audit event
UI changed -> next tree read -> old element_id -> typed ElementStale
```

- Addressing is role+name, never pixels. The `app` selector is the executable basename (exact,
  case-insensitive) plus an optional `pid` for disambiguation.
- Multiple app matches produce a typed error listing the candidates (name+pid); the agent
  picks one and retries.
- A stale `element_id` (the UI changed) produces typed `ElementStale` with no auto-re-resolution:
  the agent re-reads the tree.
- Writes go through typed AT-SPI2/AX interfaces only; they are layout-independent by construction
  (the `:` -> `^` ru-layout lesson is part of the test matrix) and never reference coordinates
  (HiDPI-safe). Keyboard/mouse emulation is outside v1 (un-plan).
- Rationale: a tree read answers in ~300 bytes against ~1 MB for a screenshot (prior-art lesson).
- There is no event channel in v1 (neither push nor buffered polling - un-plan): UI observation is
  polling `desktop.tree`; watch scenarios use `journal_query`/`svc_status`.
- Electron applications surface in the tree only with a per-app accessibility flag; the doctor-led
  opt-in for it is planned (remaining roadmap F1.7.1 work; see [Doctor](doctor.md)).
- Providers: `atspi` (AT-SPI2 over D-Bus; the bus must be present in the session) on all Linux
  targets; `ax` (NSAccessibility/AXUIElement) on macOS in MS3, gated by TCC Accessibility.
- `desktop.tree` runs with a 15 s budget (other tools: 10 s).

## Degradation

| Ladder state | This domain |
| --- | --- |
| Degraded | The tool stays usable while an Electron app without the a11y flag is absent from the tree; the flag opt-in via doctor is planned (roadmap F1.7.1 remainder) and will remove the gap. |
| Missing capability | No AT-SPI2 in the session (or no AX backend): `desktop.*` hidden; a direct call returns `CapabilityRequired` with a doctor hint. |
| Below version floor | AT-SPI2 below the floor fixed by the M0 measurements: the adapter is not activated; `capabilities()` explains the reason. |
| Opt-in required | macOS: Accessibility grant not issued or revoked - status `opt-in-required`, behavior equals the hidden state until the TCC wizard leads the human through issuance. |

## Invariants

- A11y addressing always goes through role+name; coordinates never appear in operations or DTOs.
- Multiple app matches always produce a typed error listing candidates with name+pid.
- A stale `element_id` always produces `ElementStale`; the core always leaves re-resolution to the
  agent's next `desktop.tree` call.
- Write tools always require both `desktop.write.typed` and a write grant; input emulation is
  always outside v1.
- The core always mints `element_id`; a token issued by one instance never resolves in another.
- `desktop.tree` always runs with the extended 15 s budget.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - `DesktopA11y` operations and addressing semantics.
- [Tool Registry](../architecture/tool-registry.md) - tool-to-capability binding.
- [Capability Model](../architecture/capability-model.md) - the `desktop.*` constants and ladder.
- [Error Taxonomy](../architecture/error-taxonomy.md) - `ElementStale` and the eight codes.
- [Security Model](../architecture/security-model.md) - typed writes under human-issued grants.
- [Topology and Lifecycle](../architecture/topology-and-lifecycle.md) - opaque tokens and budgets.
- [Doctor](doctor.md) - opt-ins led by doctor.

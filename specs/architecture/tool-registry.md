# Tool Registry

## Context

The complete normative list of 29 MCP tools. Names and capability bindings are fixed here and mirror
`omnibus-architecture.md` §5 and `omnibus-spec.md` §4. `[+scope]` means a write grant from the TOML
manifest is required in addition to the capability. Core tools are marked (core).

## Registry

| MCP tool | Required capabilities | Trait(s) |
|---|---|---|
| `desktop.tree` | `desktop.tree` | `DesktopA11y` |
| `desktop.element` | `desktop.tree` | `DesktopA11y` |
| `desktop.set_value` [+scope] | `desktop.tree` + `desktop.write.typed` (+ write grant) | `DesktopA11y` |
| `desktop.invoke` [+scope] | `desktop.tree` + `desktop.write.typed` (+ write grant) | `DesktopA11y` |
| `clip_read` | `portal.clipboard` | `ScreenCapture` |
| `clip_write` [+scope] | `portal.clipboard` | `ScreenCapture` |
| `screenshot` | `portal.screenshot` | `ScreenCapture` |
| `notify` [+scope] | `portal.notify` | `Notify` |
| `svc_list` / `svc_status` | `service.read` | `ServiceManager` |
| `svc_start` / `svc_stop` [+scope] | `service.write` | `ServiceManager` |
| `net_status` | `net.read` | `NetworkStatus` |
| `journal_query` | `journal.query` (+ `journal.prev_boot` for past boots) | `Journal` |
| `pkg_list` / `pkg_info` | `pkg.read` | `PackageManager` |
| `pkg_install` / `pkg_remove` [+scope] | `pkg.write` | `PackageManager` |
| `pkg_undo` [+scope] | `pkg.undo`; hidden when `pkg.undo=none` | `PackageManager` |
| `fs_list` / `fs_read` | `files.scope.read` | `FileScope` |
| `fs_write` [+scope] | `files.scope.write` | `FileScope` |
| `wiki_search` / `wiki_page` | `kb.present` | `KnowledgeBase` |
| `intent_list` / `intent_run` [+scope `intents.run`] (MS3, macOS) | `intents.list` / `intents.run`; hidden without actions in probe report | `AppIntentsBridge` |
| `capabilities()` (core) | none | `PlatformProbe` aggregate |
| `session_info` (core) | none | core |
| `doctor` (core; composes subsystem facts) | none | probe/caps/registry state + audit-sink and write-gate health |

## Hiding Rule

A tool with missing capabilities is absent from `tools/list`. A direct call by name to a hidden tool
returns `CapabilityRequired` with a doctor hint. Degraded capabilities keep the tool visible with the
limitation marked in the schema (example: `pkg_install` without `pkg.undo` on apt).

## Registry Invariants

- Every tool always lives in the core even when it has a single adapter backend; the core calls the
  trait, never the implementation; no tool imports native crates.
- JSON schemas are always generated from DTOs; hand-written schemas fail the build.
- The registry is closed over this table; adding a tool requires capability constants and a trait
  declared first (roadmap rule R1).

## Anti-Patterns

- Exposing a tool whose capability is absent (the agent would learn by runtime error instead of by
  the hidden tool - breaks P4).
- Per-platform tool names (platform differences surface only through capabilities and providers).

## Source Docs

- `docs/development/omnibus-spec.md` §4 (tool surface).
- `docs/development/omnibus-architecture.md` §5 (binding table), §7.3 (hiding rule).

## Related Specs

- [Capability Model](capability-model.md) - statuses, hiding, degradation ladder.
- [Domain specs](../INDEX.md) - per-domain behavior details.

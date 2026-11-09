# Tool Registry

## Context

The complete normative list of 37 MCP tools (27 through v1.0; eight more with v1.5, Linux only —
all eight shipped; the two `intent_*` tools stay MS3). Names and capability bindings are fixed
here and mirror
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
| `app.list` (v1.5, Linux) | `app.read` | `AppLauncher` |
| `app.launch` / `app.open` [+scope `app.launch`] (v1.5, Linux) | `app.launch` (+ write grant) | `AppLauncher` |
| `ps.list` / `ps.info` (v1.5, Linux) | `ps.read` | `ProcessTelemetry` |
| `sys.stats` (v1.5, Linux) | `sys.read` | `ProcessTelemetry` |
| `power.status` (v1.5, Linux) | `power.read`; hidden without UPower/battery | `ProcessTelemetry` |
| `ps.signal` [+scope `ps.signal`] (v1.5, Linux) | `ps.signal` (+ write grant); PID 1 and systemd-unit processes excluded (`Unsupported` with a `svc_*` hint) | `ProcessTelemetry` |
| `intent_list` / `intent_run` [+scope `intents.run`] (MS3, macOS) | `intents.list` / `intents.run`; hidden without actions in probe report | `AppIntentsBridge` |
| `capabilities()` (core) | none | `PlatformProbe` aggregate |
| `session_info` (core) | none | core |
| `doctor` (core; composes subsystem facts) | none | probe/caps/registry state + audit-sink and write-gate health |

(v1.5) `pkg_list` / `pkg_info` aggregate the native manager and the read-only flatpak/snap
inventory; the source of every package is explicit in the DTO (`native` / `flatpak` / `snap`). Write
tools for flatpak/snap sources are not declared.

(v1.0) The `ServiceManager` ops `restart` / `enable` / `disable` stay trait-level
(contract-suite coverage): the tool surface binds only `svc_start` / `svc_stop`, and an agent
composes a restart as stop + start. Binding the remaining ops to tools is a registry change,
never a hidden alias.

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
- Two naming conventions exist, both normative: the v1.0 underscore names (`clip_read`,
  `svc_start`, `fs_write`) stay exactly as shipped - renaming a shipped tool is a breaking change
  for MCP clients and is not planned; tools added from v1.5 on use dotted lowercase
  `{domain}.{action}` names assembled in code from registry-domain segments (never written as
  capability-shaped literals - R4-CON). A dotted tool name may equal the capability constant behind
  it (`desktop.tree`, `app.launch`, `ps.signal`) or deliberately differ (`app.list` behind
  `app.read`, `sys.stats` behind `sys.read`); both pairings are pinned by the core's registry
  binding tests.

## Anti-Patterns

- Exposing a tool whose capability is absent (the agent would learn by runtime error instead of by
  the hidden tool - breaks P4).
- Per-platform tool names (platform differences surface only through capabilities and providers).

## Source Docs

- `docs/development/omnibus-spec.md` §4 (tool surface).
- `docs/development/omnibus-architecture.md` §5 (binding table), §7.3 (hiding rule).
- `docs/development/omnibus-roadmap.md` tasks 2.7.1-2.9.2 (v1.5 tool additions).

## Related Specs

- [Capability Model](capability-model.md) - statuses, hiding, degradation ladder.
- [Domain specs](../INDEX.md) - per-domain behavior details.

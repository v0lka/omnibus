# SPI Registry: Subsystems and Traits

## Context

The explicit interface boundary is the set of Rust traits in `omnibus-contracts`. Each subsystem =
trait(s) + capability descriptor + error mapping rules + contract version (semver independent of code
version). The registry below is the complete MS1+MS2+MS2.5+MS3 set: 14 traits across 12
subsystems. Traits are declared just-in-time together with their first implementation
(`NetworkStatus` with task 2.2.3, `AppIntentsBridge` with task 3.6.2, and the two v1.5 traits
`AppLauncher` with task 2.7.1 and `ProcessTelemetry` with task 2.8.1); no speculative stubs are built.

## Subsystem Registry

| # | Subsystem | Trait(s) | Key contract operations | Linux provider | macOS provider |
|---|-----------|----------|-------------------------|----------------|----------------|
| 1 | a11y / desktop | `DesktopA11y` | `tree(app)`, `element(id)`, `read_attr`, `set_value`, `invoke` | `atspi` (AT-SPI2 over D-Bus) | `ax` (NSAccessibility / AXUIElement) |
| 2 | journaling + audit sink | `Journal`, `AuditSink` | `query(JournalQuery{unit, boot, severity_at_least, since/until epoch window, limit})`, `tail`, `write(AuditEvent)` | `journald` (persistence is a per-machine probe fact; distro defaults are persistent — M0 Table 1) | `unifiedlog` (`log show --predicate`) |
| 3 | service management | `ServiceManager` | `list`, `status`, `apply` (action `start/stop/restart/enable/disable`) | `systemd1` | `launchd` (LaunchAgents - only agents see GUI) |
| 4 | package management | `PackageManager` | `list`, `info`, `search`, `install`, `remove`, `history`, `undo` | `alpm` / `apt` (no undo) / `dnf` (transactional undo); `flatpak`/`snap` (v1.5) - read-only inventory implementing only `list`/`info` | `brew` (+ `mas` optional) |
| 5 | portals | `ScreenCapture`, `Notify` | `screenshot`, `clip_read`/`clip_write`, `send` | xdg-desktop-portal (clipboard rides the detected `wl-clipboard`/`xclip` backend) | ScreenCaptureKit (monthly re-grant on 15+), UNUserNotificationCenter (`notify`), NSPasteboard (`clip_read`/`clip_write`) |
| 6 | file scopes | `FileScope` | `list/read/write` in granted scopes, `resolve` | `fs-xdg` (XDG dirs) | `fs-tcc` (granted folders; beyond - Full Disk Access) |
| 7 | knowledge base | `KnowledgeBase` | `search`, `page` | `kb-archwiki` (ArchWiki + Manjaro overlay), `kb-guides` (distro docs) | `kb-guides` (Apple Support, SS64, community) |
| 8 | platform probe | `PlatformProbe` | `detect() -> PlatformReport{distro/release, versions, daemon liveness}` | `/etc/os-release`, `systemctl --version`, bus state | `sw_vers`, system profiles, TCC state |
| 9 | app-intents backend (macOS, MS3) | `AppIntentsBridge` | `list_actions`, `run` | none | Shortcuts / App Intents (cooperating apps; backend, not an a11y replacement) |
| 10 | network status | `NetworkStatus` | `status()` - devices, connectivity, wifi networks; read-only | `nm` ("when present"; behind netplan/networkd - absent) | none (out of v1) |
| 11 | app launch (v1.5, Linux) | `AppLauncher` | `list`, `launch(AppSpec{app_id, args})`, `open(uri\|path)` | `launch` (`.desktop` inventory + `systemd-run --user` + `xdg-open`) | none (out of v1.5) |
| 12 | processes and telemetry (v1.5, Linux) | `ProcessTelemetry` | `list`, `info(pid)`, `sys_stats`, `power_status`, `signal(pid, sig)` | `proc` (`/proc` + sysfs + UPower; PID 1 and systemd-unit processes are excluded - services are addressed only via `svc_*`) | none (out of v1.5) |

## DTO Discipline

- Flat named structs only; no tuple parameters anywhere on the SPI.
- Newtype wrappers for units: `Milliseconds`, `Bytes`, `Severity`, and (v1.5 telemetry DTOs)
  `Percent`, `Seconds`.
- Native types never cross the boundary; mapping to DTO happens inside the adapter.

## Addressing Semantics

- `tree(app)`: selector `app` = executable basename (exact, case-insensitive) plus optional `pid`
  for disambiguation. Multiple matches produce a typed error listing candidates (name+pid).
- A stale `element_id` (UI changed) produces typed `ElementStale`; there is no auto-re resolution -
  the agent re-reads `tree`.

## Trait Lifecycle

`probe -> register -> serve -> shutdown` is a finite state machine owned by the core. An adapter
physically cannot be called before `register`. Free order of tool calls within a session.

## Invariants

- Exactly one active adapter per subsystem at any moment; probe selects it (extra implementations
  may exist only as fallbacks). Sole documented exception (v1.5): in the packages subsystem the
  read-only flatpak/snap inventory adapter is active alongside the native manager; both providers
  appear in `capabilities()`, and only the native manager carries write semantics.
- Every trait method always returns a contract error; `NativeFailure{code}` always preserves the
  original message payload.
- The trait registry is always closed over the roadmap task list; new traits enter only additively
  (minor contract version).

## Anti-Patterns

- Declaring a trait before its first implementation exists (speculative generality).
- An adapter calling another adapter (P6); orchestration such as doctor always lives in the core.

## Source Docs

- `docs/development/omnibus-architecture.md` §4, §8.6 (new adapter demo).
- `docs/development/omnibus-roadmap.md` tasks 1.2.1, 2.7.1, 2.8.1 (just-in-time traits), §3 (MS2.5 decisions D1-D3).

## Related Specs

- [Tool Registry](tool-registry.md) - how tools bind to traits and capabilities.
- [Core <-> Adapters contract](../contracts/core-adapters.md) - the boundary rules.
- [Domain: App Launch](../domains/app-launch.md) - subsystem #11 behavior.
- [Domain: Process Telemetry](../domains/process-telemetry.md) - subsystem #12 behavior.

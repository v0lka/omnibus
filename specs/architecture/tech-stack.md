# Technology Stack

## Context

The stack is fixed by `docs/development/omnibus-spec.md` §3 and `omnibus-architecture.md` §2.
Choices below are decisions, not suggestions; changing one requires a new ADR.

## Language and Workspace

- Language: Rust. One static binary (`omnibusd`).
- Workspace crates: `omnibus-contracts`, `omnibus-core`, `omnibus-probe`, `omnibus-adapter-*`
  (one crate per subsystem x OS family; ~21 adapter crates + probe at full matrix - v1.5 adds the
  launch, proc, and flatpak inventory crates).

## Core Stack

| Concern | Choice | Notes |
|---|---|---|
| MCP SDK | `rmcp` (official Rust MCP SDK) + `rmcp-macros` | lives in the core only, wrapped by the facade |
| Async runtime | `tokio` | core and adapters |
| Serialization | `serde` | the only shared dependency type in contracts |

## Adapter Stack (by subsystem)

| Subsystem | Linux | macOS |
|---|---|---|
| a11y | `atspi` crate (AT-SPI2 over D-Bus) | `objc2` bindings to NSAccessibility / AXUIElement |
| journal | journald JSON interface | unified log via `log show --predicate` (or private API - ADR-004) |
| services | `zbus` (org.freedesktop.systemd1) | launchctl + ServiceManagement (13+) |
| packages | `alpm` crate (Arch/Manjaro); apt via libapt or CLI mapping (ADR-002); dnf/dnf5; flatpak/snap read-only inventory via their CLIs (v1.5) | `brew` (+ `mas` optional) |
| portals | xdg-desktop-portal + wl-clipboard/xclip | ScreenCaptureKit; UNUserNotificationCenter; NSPasteboard |
| files | XDG directories | TCC-granted folders (fs-tcc) |
| network | `zbus` (org.freedesktop.NetworkManager), read-only | none (out of v1) |
| app launch (v1.5) | `.desktop` inventory + `systemd-run --user` + `xdg-open` | none (out of v1.5) |
| process telemetry (v1.5) | `/proc` + sysfs + UPower over D-Bus | none (out of v1.5) |
| intents | none | Shortcuts / App Intents bridge (optional backend, MS3) |
| probe | `/etc/os-release`, `systemctl --version`, bus liveness | `sw_vers`, system profiler, TCC state |

## Packaging (install-time detection level)

- Linux: PKGBUILD (linux adapter set) to AUR; later PPA, Debian repo, COPR (MS2).
- macOS: Homebrew formula with Developer ID signing and notarization (MS3).
- Both install the scope manifests into `/etc/omnibus/scopes.d/` (Linux path is the canonical
  example; together with the user store `omnibusd grant` writes they form the grant stores).

## Invariants

- `omnibus-contracts` always depends only on `serde`/async types; zero OS dependencies.
- Native types (`zbus::Value`, ALPM handles, `AXUIElementRef`) never leave an adapter.
- Native error codes are always mapped to `NativeFailure{code}` inside the adapter, keeping the
  original message in the payload.
- `omnibusd` ships as one static binary; no plugin loading, no `.so` ABI surface.

## Anti-Patterns

- Importing `zbus`, `alpm`, `objc2`, or any OS crate from `omnibus-core` or `omnibus-contracts`.
- Adding a second MCP SDK or hand-written JSON schemas for tools (schemas are generated from DTOs).

## Source Docs

- `docs/development/omnibus-spec.md` §3 (Rust stack).
- `docs/development/omnibus-architecture.md` §2 (crates), §4 (adapter providers).

## Related Specs

- [Layers and Crates](layers-and-crates.md) - dependency direction rules.
- Decisions: [ADR-001](../decisions/001-dyn-trait-spi.md) (traits over generics/plugins).

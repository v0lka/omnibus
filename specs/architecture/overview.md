# System Overview

## Context

Omnibus is an agent contract for the operating system: one semantic API over swappable OS adapters,
exported as MCP. The daemon `omnibusd` gives AI agents a uniform, auditable, least-privilege way to read
and drive a desktop OS (services, packages, logs, UI, files, clipboard, screenshots, notifications,
knowledge bases, network status). This page is the entry point; details live in the specs linked from
[INDEX.md](../INDEX.md).

Scope: desktop implementations of target OSes only. Server/headless profiles are out of scope for v1.

## System Shape

```
MCP hosts: Claude Code / goose / Cursor / any MCP client
     |   the "big brain" (LLM) lives entirely in the agent, chosen by the user
     | MCP over stdio; one omnibusd instance per connection (child of the client)
+------------------------------------------------------------------------+
| CORE  omnibus-core (library crate) - OS-agnostic                       |
|   MCP facade (rmcp) - sessions (= connections) - grant checks          |
|   capability registry - capabilities() document - doctor               |
|   audit pipeline (schema owned here) - global write serialization      |
+------------------------------------------------------------------------+
     | SPI boundary: crate omnibus-contracts (traits + DTOs +
     | capability constants + error taxonomy). ONLY shared dependency.
+------------------------------------------------------------------------+
| ADAPTERS  omnibus-adapter-* (subsystem x OS family), one active each   |
|   a11y:      atspi (Linux)           / ax (macOS)                      |
|   journal:   journald                / unifiedlog                      |
|   services:  systemd1                / launchd                         |
|   packages:  alpm / apt / dnf        / brew (+mas)                     |
|   portals:   xdg-portals + wl-clipboard / screencapture + notify       |
|   files:     fs-xdg                  / fs-tcc (TCC grants)             |
|   knowledge: kb-archwiki, kb-guides / kb-guides (Apple, SS64)          |
|   network:   nm (read-only)          / none (out of v1)                |
|   intents:   none                    / appintents (optional backend)   |
|   probe:     platform detection, adapter selection, floors           |
+------------------------------------------------------------------------+
```

## The Numbers (normative inventory)

- 12 SPI traits across 10 subsystems (11 declared; `AppIntentsBridge` is JIT with MS3 — see [spi-registry.md](spi-registry.md)).
- 29 MCP tools (see [tool-registry.md](tool-registry.md)).
- 20 capability constants (18 shipped; the two `intents.*` arrive with MS3 — see [capability-model.md](capability-model.md)).
- 8 error codes (see [error-taxonomy.md](error-taxonomy.md)).
- 3 support tiers, 9 target desktop platforms (see [platform-matrix.md](platform-matrix.md)).
- 0 model calls, 0 in-band confirmations, 0 consent UI in `omnibusd` (see [security-model.md](security-model.md)).

## How an Agent Uses Omnibus

1. The MCP client starts `omnibusd` as a child process (stdio). The connection is the session.
2. The agent calls `capabilities()` - the single machine-readable truth about this machine.
3. The tool list already reflects reality: tools without their capabilities are hidden.
4. Reads execute immediately. Writes require a grant the human issued out of band
   (TOML manifest / `omnibusd grant` / doctor CLI). Denied writes return `ScopeDenied`
   with a one-line copy-paste remedy.
5. Every call - reads included - produces exactly one structured audit event.

## Invariants

- The daemon always runs as a child of the MCP client; it never runs as a system daemon.
- Inference always stays in the agent's trust boundary; `omnibusd` performs zero model calls.
- `capabilities()` is always the single source of truth about a concrete machine; the agent never
  has to guess the distro.
- Every action always produces exactly one audit event with the full schema.

## Anti-Patterns

- Adding a second way to learn machine state besides `capabilities()`.
- Putting OS-specific branches into the core instead of an adapter.
- Treating an agent-provided "confirmation" as consent (see [security-model.md](security-model.md)).

## Related Specs

- [Principles](principles.md) - P1-P7 in detail.
- [Layers and Crates](layers-and-crates.md) - workspace layout and dependency rules.

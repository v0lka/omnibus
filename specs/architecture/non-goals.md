# Non-Goals and the Never-List

## Context

Two distinct categories live here. The never-list is forbidden by the security/architecture posture -
no task may plan it, and a review that sees one rejects it (roadmap rule R6; the single source is
`omnibus-architecture.md` §7.4). The un-plan items are consciously deferred with explicit revisit
conditions.

## The Never-List (never, in any milestone)

- Write actions outside human-issued grants; any in-band confirmation flow; a consent broker with UI.
  Reason: an in-band "the user confirmed" from an agent is indistinguishable from a forgery.
- Model calls from `omnibusd` (local or cloud). Reason: inference belongs to the user's agent trust
  boundary.
- Switching the network backend; systemd backports; changing system security policies; programmatic
  TCC issuance (a human grants via GUI or MDM/PPPC).
- OS kernels (Linux and macOS); a compositor fork.
- A custom plugin format (MCP is the standard - not reinvented).
- Server/headless OS profiles (desktop-only scope).

## The Un-Plan (deferred, with revisit conditions)

| Item | Revisit condition |
|---|---|
| Network write actions | never in v1; separate decision with a real use case |
| Event channel for a11y (push or buffered polling) | first scenario that polling `desktop.tree`/`journal_query`/`svc_status` cannot serve; push also depends on heterogeneous MCP client subscription support |
| Input emulation (ydotool/uinput, CGEvent) | typed writes fail to cover a proven target scenario; would return as an opt-in extension |
| Persistent daemon, admin channel, HTTP transport | server/headless profiles become a goal |
| `omnibus-contracts` in a separate repository | first external adapter contributor |
| Local ML in the daemon (embeddings for KB search) | a measured need for semantic search beyond lexical matching |
| KB playbook engine (TOML playbooks, applicability matrix) | the first real content pipeline that the "agent judges applicability" model (P7) cannot serve |
| `[limits]` config overrides | the first real request; defaults are code-fixed until then |
| TTFM telemetry | none - replaced by the onboarding autotest; north-star metrics run without telemetry (survey + issue tracking) |
| Tool-schema deprecation mechanics/window | the first release that needs it |
| MPRIS / audio-media control (D4) | out of v1.5 - typed `invoke` on a player's UI (a11y addressing) covers the recurring scenarios; the first proven scenario needing more MPRIS semantics than a11y addressing |
| Window management: focus, layout, workspaces, tiling (D5) | out of v1.5 - Linux has no unified protocol (X11/EWMH vs heterogeneous Wayland mechanisms; macOS private APIs); a window-management protocol standardized in xdg-desktop-portal, or a first scenario a11y addressing cannot serve |
| Notification reading / history (D6) | out of v1.5 - the portals' notify surface is write-only with no read API; a read API for notifications appearing in xdg-desktop-portal (notification history is never promised before that) |
| Power/session write actions: suspend/reboot/logout, session switch (D7) | out of v1.5 - terminal system states are never delegated from the general write scope (reading exists: `power.status`); only as a separate grant type with its own audit under sustained demand - never in the general write list |
| Dedicated OS-settings subsystem (dconf/gsettings bridge) (D8) | the settings-via-a11y position holds - settings change through typed writes in the settings UI (`desktop.set_value`) or config edits inside file scopes; a proven recurring scenario needing structured settings semantics - then a separate subsystem behind its own trait (ADR-011), never a special mode of existing tools |

## Invariants

- The never-list is always identical in source docs and specs; this page restates, it does not amend.
- Every roadmap task always respects R6 (no task plans a never-list item).
- Every un-plan item always names its revisit condition.

## Anti-Patterns

- "Let's add it opt-in for now" for a never-list item - the list has no opt-in tier.
- Treating un-plan items as roadmap backlog - they have triggers, not dates.

## Source Docs

- `docs/development/omnibus-spec.md` §7.
- `docs/development/omnibus-architecture.md` §7.4.
- `docs/development/omnibus-roadmap.md` §5.4, rule R6 (§0.2).

## Related Specs

- [Security Model](security-model.md) - why in-band consent is on the never-list.
- [Principles](principles.md) - zero inference.

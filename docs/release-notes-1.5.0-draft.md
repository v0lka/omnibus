# omnibus 1.5.0 — Release Notes (v1.5 draft)

Status: DRAFT for the v1.5 minor release (roadmap task 2.9.2). v1.5 is an
additive extension of the v1.0 contract: eight new tools over the two traits
`AppLauncher` and `ProcessTelemetry`, plus the read-only flatpak/snap
inventory in the packages subsystem — no core mechanism changed. Nothing in
this document promises more than the platform tier: every claim below is
what `capabilities()` reports per machine, and `capabilities()` always wins
a dispute with this document (see `specs/architecture/capability-model.md`).

## The claims ladder (decision D9)

Omnibus releases claim by version, never by slogan:

- **v1.0** — "the full contract on the target distros": reads + typed
  writes, `pkg.undo` where the package manager provides it, past-boot
  diagnostics where the journal is persistent.
- **v1.5** — "the closed agent loop + live state + full inventory" on
  Linux: the agent launches applications (`app.list` / `app.launch` /
  `app.open` — launch behind a grant), sees live state (`ps.list` /
  `ps.info` / `sys.stats` / `power.status`), and knows the full package
  inventory (native managers + read-only flatpak/snap), closing the loop
  "launched → checked the process → read the journal → terminated it"
  (`ps.signal` — behind a grant) with no human operator between the steps.
- The strong "AI-native OS" form is never claimed, in any release: Omnibus
  is an OS contract for agents, not "an OS with AI". All inference stays
  inside the user's agent; `omnibusd` performs zero model calls.

## What's new in v1.5

### 1. The closed loop (app-launch subsystem, roadmap 2.7)

`app.list` inventories the machine's `.desktop` applications (`/usr/share`
plus the user's `$XDG_DATA_HOME/applications`); `app.launch` starts one as a
**transient user unit** through `systemd-run --user`; `app.open` hands a URI
or file to the associated application via `xdg-open`. The launched process
lands in the user's cgroup and journald, where the existing observation
tools already look:

```
app.launch (grant)  →  ps.list sees the process (the unit name inside the
                       cgroup path is the join key reported in the launch
                       outcome)
journal_query       →  reads the launched output (window filters; the
                       output lands in journald asynchronously)
ps.signal (grant)   →  terminates the process by the pid taken from ps.list
```

Application arguments always travel as a typed list inside the DTO and reach
the target process verbatim — no shell anywhere (`;`, `|`, backtick are
data, never syntax; proven end-to-end by the contract suite's live binder).
The closed loop is exercised in CI by the smoke demo
`ci/smoke/demo5-closed-loop.sh`: the no-grant refusal legs run everywhere,
and the granted full loop runs wherever a systemd user manager is live
(reference machines and the Manjaro VM track; CI containers skip it
honestly).

### 2. Live state (process-telemetry subsystem, roadmap 2.8)

- `ps.list` / `ps.info` — the `/proc` inventory and one consistent per-PID
  snapshot: pid, name, state, CPU, RSS, cgroup — exactly what the daemon's
  own uid can see, no escalations. **PID 1 and systemd-unit processes stay
  outside**: services are addressed only through the services subsystem
  (`svc_list` / `svc_status` / `svc_start` / `svc_stop`) — one addressable
  world per subsystem, no double addressing. Processes in the *user*
  hierarchy (including the transient units `app.launch` mints) stay inside:
  that is the launch → observe half of the closed loop.
- `sys.stats` — CPU busy percent, load averages, memory and swap, read from
  `/proc` and `/sys` (no version floor; activates on every target Linux
  platform).
- `power.status` — battery and supply facts through UPower D-Bus,
  **"when present"**: on a machine without UPower or a battery the tool is
  hidden and `capabilities()` names the reason (R3/R7 — the machine is never
  promised what it lacks).

### 3. The full package inventory (packages subsystem, roadmap 2.9.1)

`pkg_list` / `pkg_info` now aggregate the native manager **and** the
read-only flatpak/snap inventory: every row carries its `source`
(`native` / `flatpak` / `snap`), the package identity round-trips into
`pkg_info`, and a store-less machine sees byte-identical output to v1.0 —
no degradation, no new promises.

## New capabilities

| Capability | Tools | Grant | Notes |
|---|---|---|---|
| `app.read` | `app.list` | none (reads are audited) | the `.desktop` inventory |
| `app.launch` | `app.launch`, `app.open` | write grant `app.launch` required | `systemd-run --user`; hidden entirely when `systemd-run` is absent |
| `ps.read` | `ps.list`, `ps.info` | none (reads are audited) | PID 1 and system-unit processes excluded |
| `ps.signal` | `ps.signal` | write grant `ps.signal` required | closed signal enum (`term`, `int`, `hup`, `usr1`, `usr2`, `kill`) — a raw signal number cannot cross the seam |
| `sys.read` | `sys.stats` | none (reads are audited) | no version floor |
| `power.read` | `power.status` | none (reads are audited) | hidden without UPower/battery |

Every write (`app.launch`, `app.open`, `ps.signal`) produces a full audit
event; every read is audited too (R5). All six capabilities and the eight
tools are Linux-only in v1.5; macOS (MS3) serves them from its own adapters
as they arrive.

## Honest boundaries

- **flatpak/snap are read-only.** The inventory lists and describes store
  applications; install/remove/undo for flatpak/snap are not declared — the
  adapter refuses with a typed honest reason before any subprocess runs, and
  `capabilities()` carries the read-only limitation (R7).
- **`power.status` appears only when present.** No UPower or no battery →
  the tool is hidden, the reason lives in `capabilities()`, the log stays
  clean.
- **Launch and signal sit behind human-issued grants.** Without the write
  grant, `app.launch` / `app.open` / `ps.signal` answer `ScopeDenied` (and
  the attempt is audited); the tools do not even appear in `tools/list`
  until the grant exists. There is no runtime consent API — grants are
  human-issued, out-of-band (`omnibusd grant app.launch`,
  `omnibusd grant ps.signal`).
- **Signals cannot touch services.** A signal aimed at PID 1 or any
  systemd-unit process answers `Unsupported` with a hint to the service
  tools — no "restarting the system under the guise of a process".
- **The journal unit filter is exact.** `journal_query {unit}` matches the
  record's system unit tag exactly (messages merely *about* a unit are not
  returned); launched user-unit output is read through the window filters,
  as the closed loop above shows.
- **A launched unit may be indeterminate under a stalled manager.** When
  the launch budget elapses, the answer names the candidate unit and tells
  the caller to re-check with `ps.list` / `journal_query` — a dispatched
  write is never silently recalled.

## Platforms (unchanged from v1.0; additivity is the point)

The v1.5 matrix is the v1.0 matrix — the same nine Linux platform legs, the
same packaging channels, a minor version bump in each. The eight new tools
activate wherever their backends exist (`systemd-run`, `xdg-open`, `/proc`,
UPower) and hide honestly where they do not. Reference platforms for the
v1.5 smoke (minimum, roadmap 2.9.2 AC1): Arch, Manjaro, Ubuntu LTS, Fedora —
see `docs/release-checklist.md` §6 for the per-leg table.

## The never-list (unchanged)

Copied verbatim from `specs/architecture/non-goals.md`; v1.5 does not amend
it. Never, in any milestone:

> - Write actions outside human-issued grants; any in-band confirmation flow; a consent broker with UI.
>   Reason: an in-band "the user confirmed" from an agent is indistinguishable from a forgery.
> - Model calls from `omnibusd` (local or cloud). Reason: inference belongs to the user's agent trust
>   boundary.
> - Switching the network backend; systemd backports; changing system security policies; programmatic
>   TCC issuance (a human grants via GUI or MDM/PPPC).
> - OS kernels (Linux and macOS); a compositor fork.
> - A custom plugin format (MCP is the standard - not reinvented).
> - Server/headless OS profiles (desktop-only scope).

## No telemetry (unchanged)

There is no telemetry at all. `omnibusd` makes zero model calls and performs
no network communication of its own; the only network traffic a session can
produce is `wiki_search` / `wiki_page` fetches the agent explicitly requests.
The new write tools are audited locally (R5) — events go to the local
journald sink, never anywhere else.

## Install (unchanged channels; minor bump)

The same channels as v1.0 (AUR, PPA, Debian repo, COPR) carry v1.5 as a
minor update; `omnibus-contracts` moves 1.0.0 → 1.4.0 (additive only: the
two v1.5 trait modules `AppLauncher` / `ProcessTelemetry` with their DTOs,
six new capability constants, and the `source` field on `PackageBrief` —
no existing signature changes). After the upgrade, run `doctor` once — it
reports which of the new backends this machine has and which grants the
human can issue to unlock the loop:

```
omnibusd grant app.launch
omnibusd grant ps.signal
```

Neither command is ever executed by an agent or by the daemon — grants are
the human's levers, and their existence is the whole security model.

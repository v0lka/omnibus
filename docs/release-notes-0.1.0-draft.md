# omnibus 0.1.0 — Release Notes (MS1 draft)

Status: DRAFT for the tagged MVP release (roadmap task 1.8.3). Nothing in
this document promises more than the platform tier: the reference machine
below is a Manjaro T1 installation, and the guarantees named here are the
ones its `capabilities()` document shows — illustrated below in the exact
shape the shipped `CapabilitiesDocument` serializes.

## Platforms (honest matrix)

| Platform | Tier | Status in 0.1.0 |
|---|---|---|
| Arch | T1 | Guaranteed: full CI matrix, contract suite, packaged (PKGBUILD, task 1.8.1). |
| Manjaro | T1 | Guaranteed: same alpm/journald/systemd1/AT-SPI2 backend family as Arch, own test matrix (task 2.4.1); delayed repos, mhwd drivers and pamac are noted by the probe, and a Manjaro kb overlay ships with the package. |
| Debian, Ubuntu, Mint, Fedora | T2 | Not yet. Planned for MS2 (roadmap section F2). No guarantee is made in 0.1.0. |
| Debian bullseye and other below-floor systems | T3 | Not yet; when supported, build-only: `capabilities()` will honestly report what is missing. |

`capabilities()` is machine truth and always wins a dispute with this
matrix (see `specs/architecture/capability-model.md`).

## Reference machine: capabilities document shape (Manjaro T1)

`capabilities()` is machine truth and always wins a dispute with this matrix
(see `specs/architecture/capability-model.md`). The block below is an
**illustrative document in the shipped shape**: the flat `CapabilitiesDocument`
envelope (`contract_version`, `distro`, `capabilities`), with entries the way a
healthy T1 Manjaro machine reports them. It is not a literal capture of one
release-time run — run `capabilities()` against your own daemon for machine
truth.

The version floors (systemd 261 ≥ 252, at-spi2 2.60.6 ≥ 2.50) are probe/doctor
material: they ride in the platform probe report and the doctor output, not in
the serialized capabilities document. The journal facts are different:
`journal.persistent` and `journal.prev_boot` are capability entries in the
document itself (below), carrying the volatile-machine `opt-in-required`
status and its grant remedy there.

```json
{
  "contract_version": "1.0.0",
  "distro": { "id": "manjaro", "name": "Manjaro Linux", "release": "rolling", "family": "manjaro" },
  "capabilities": [
    { "name": "desktop.tree",        "ver": "1.0.0", "status": "present",  "value": null, "adapter": "atspi",       "note": "role+name addressing only; walk capped at depth 12 / 2000 nodes", "remedy": null },
    { "name": "desktop.write.typed", "ver": "1.0.0", "status": "present",  "value": null, "adapter": "atspi",       "note": "Value property set and named-action invocation; no keyboard emulation", "remedy": null },
    { "name": "journal.query",       "ver": "1.0.0", "status": "present",  "value": null, "adapter": "journald",    "note": null, "remedy": null },
    { "name": "journal.persistent",  "ver": "1.0.0", "status": "present",  "value": true, "adapter": "journald",    "note": null, "remedy": null },
    { "name": "journal.prev_boot",   "ver": "1.0.0", "status": "present",  "value": true, "adapter": "journald",    "note": null, "remedy": null },
    { "name": "service.read",        "ver": "1.0.0", "status": "present",  "value": null, "adapter": "systemd1",    "note": null, "remedy": null },
    { "name": "service.write",       "ver": "1.0.0", "status": "present",  "value": null, "adapter": "systemd1",    "note": null, "remedy": null },
    { "name": "pkg.read",            "ver": "1.0.0", "status": "present",  "value": null, "adapter": "alpm",        "note": null, "remedy": null },
    { "name": "pkg.write",           "ver": "1.0.0", "status": "present",  "value": null, "adapter": "alpm",        "note": null, "remedy": null },
    { "name": "pkg.undo",            "ver": "1.0.0", "status": "present",  "value": "cache-downgrade", "adapter": "alpm", "note": "downgrade to the previous version from the pacman cache, located via history", "remedy": null },
    { "name": "portal.clipboard",    "ver": "1.0.0", "status": "degraded", "value": null, "adapter": "xdg-portals", "note": "the portal Clipboard interface is published but session-gated: a usable clipboard channel requires a RemoteDesktop session, which omnibus does not create (no input-emulation surface)", "remedy": null },
    { "name": "portal.screenshot",   "ver": "1.0.0", "status": "present",  "value": null, "adapter": "xdg-portals", "note": "the portal Screenshot interface answers on the session bus", "remedy": null },
    { "name": "portal.notify",       "ver": "1.0.0", "status": "present",  "value": null, "adapter": "xdg-portals", "note": "the notification service answers on the session bus", "remedy": null },
    { "name": "files.scope.read",    "ver": "1.0.0", "status": "present",  "value": null, "adapter": "fs-xdg",      "note": null, "remedy": null },
    { "name": "files.scope.write",   "ver": "1.0.0", "status": "present",  "value": null, "adapter": "fs-xdg",      "note": "active only inside granted write scopes", "remedy": null },
    { "name": "kb.present",          "ver": "1.0.0", "status": "present",  "value": null, "adapter": "kb-archwiki", "note": "ArchWiki plus the manjaro-overlay directory", "remedy": null },
    { "name": "net.read",            "ver": "1.0.0", "status": "present",  "value": null, "adapter": "nm",          "note": null, "remedy": null }
  ]
}
```

### Tools exposed on this machine

Default install: 17 read-only tools.

```
desktop.tree, desktop.element, clip_read, screenshot, svc_list, svc_status,
net_status, journal_query, pkg_list, pkg_info, fs_list, fs_read, wiki_search,
wiki_page, capabilities, session_info, doctor
```

The 10 write tools appear in `tools/list` only after the human provisions
grants; a direct call without a grant is refused with `ScopeDenied` (and the
refusal is audited). `CapabilityRequired` is a different refusal: it answers
when a tool's capability is absent from this machine (or an opt-in rung is
ungranted) — such tools are hidden from `tools/list` in the first place.

```
desktop.set_value, desktop.invoke, clip_write, notify, svc_start, svc_stop,
pkg_install, pkg_remove, pkg_undo, fs_write
```

## Known limitations

- Every write tool requires a human-issued, out-of-band grant. The default
  scope manifest ships empty (`/etc/omnibus/scopes.d/00-read-only.toml`);
  there is no runtime consent API.
- `portal.clipboard` is degraded, and clipboard reads/writes are not
  functional in 0.1.0: the portals adapter reports itself unavailable
  (`AdapterUnavailable`). A usable clipboard channel would require a
  RemoteDesktop session, and omnibus does not create sessions — a
  session-based implementation is future work.
- `journal.prev_boot` requires a persistent journal. It is an independent
  capability, never derived from `journal.persistent`: a freshly persistent
  machine reports `persistent=true` but has no `prev_boot` until the next
  reboot.
- The network domain is read-only (`net.read`); network writes are on the
  un-plan list and do not exist in v1.

## The never-list

Copied verbatim from `specs/architecture/non-goals.md`; this page restates,
it does not amend. Never, in any milestone:

> - Write actions outside human-issued grants; any in-band confirmation flow; a consent broker with UI.
>   Reason: an in-band "the user confirmed" from an agent is indistinguishable from a forgery.
> - Model calls from `omnibusd` (local or cloud). Reason: inference belongs to the user's agent trust
>   boundary.
> - Switching the network backend; systemd backports; changing system security policies; programmatic
>   TCC issuance (a human grants via GUI or MDM/PPPC).
> - OS kernels (Linux and macOS); a compositor fork.
> - A custom plugin format (MCP is the standard - not reinvented).
> - Server/headless OS profiles (desktop-only scope).

## No telemetry

There is no mandatory telemetry, and no telemetry at all. `omnibusd` makes
zero model calls and performs no network communication of its own. The only
network traffic a session can produce is `wiki_search` / `wiki_page` fetches
that the agent itself explicitly requests (ArchWiki and the Manjaro
overlay); nothing else is sent anywhere, and reads are audited locally, not
reported.

## Install

From this repository's packaging (in-repo part of task 1.8.1; AUR
publication is out of scope for this draft):

```
cd packaging && makepkg -si
```

Then run `doctor` once. The MCP `doctor` tool is read-only by construction: it
diagnoses missing prerequisites (AT-SPI2, persistent journald) and reports
ready commands as strings for a human to run. The doctor channel's only
mutation surface is the human-executed `omnibusd doctor --apply <name>`: it
runs exactly one named command after an interactive `YES` confirmation (one
audit event per change). Write tools exist too, but only under explicit
human-issued grants — they are the daemon's other mutation surface, never the
doctor's.

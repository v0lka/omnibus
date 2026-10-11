# omnibus 0.1.0 — Release Notes (MS1 draft)

Status: DRAFT for the tagged MVP release (roadmap task 1.8.3). Nothing in
this document promises more than the platform tier: the reference machine
below is a Manjaro T1 installation, and the guarantees named here are the
ones its `capabilities()` document actually shows.

## Platforms (honest matrix)

| Platform | Tier | Status in 0.1.0 |
|---|---|---|
| Arch | T1 | Guaranteed: full CI matrix, contract suite, packaged (PKGBUILD, task 1.8.1). |
| Manjaro | T1 | Guaranteed: same alpm/journald/systemd1/AT-SPI2 backend family as Arch, own test matrix (task 2.4.1); delayed repos, mhwd drivers and pamac are noted by the probe, and a Manjaro kb overlay ships with the package. |
| Debian, Ubuntu, Mint, Fedora | T2 | Not yet. Planned for MS2 (roadmap section Ф2). No guarantee is made in 0.1.0. |
| Debian bullseye and other below-floor systems | T3 | Not yet; when supported, build-only: `capabilities()` will honestly report what is missing. |

`capabilities()` is machine truth and always wins a dispute with this
matrix (see `specs/architecture/capability-model.md`).

## Reference machine: capabilities document (verified live, Manjaro)

Reproduced verbatim from the reference machine's `capabilities()` output
at release time:

```json
{
  "platform": {
    "distro": { "family": "manjaro", "id": "manjaro", "name": "Manjaro Linux", "release": "rolling" },
    "systemd": { "version": 261, "floor": 252, "floor_met": true },
    "at-spi2": { "version": "2.60.6-1", "floor": "2.50", "floor_met": true },
    "journald": { "persistent": true, "prev_boot": true }
  },
  "capabilities": [
    { "name": "desktop.tree",        "ver": "1.0.0", "status": "present", "adapter": "atspi" },
    { "name": "desktop.write.typed", "ver": "1.0.0", "status": "present", "adapter": "atspi" },
    { "name": "journal.query",       "ver": "1.0.0", "status": "present", "adapter": "journald" },
    { "name": "journal.persistent",  "ver": "1.0.0", "status": "present", "value": true,  "adapter": "journald" },
    { "name": "journal.prev_boot",   "ver": "1.0.0", "status": "present", "value": true,  "adapter": "journald" },
    { "name": "service.read",        "ver": "1.0.0", "status": "present", "adapter": "systemd1" },
    { "name": "service.write",       "ver": "1.0.0", "status": "present", "adapter": "systemd1" },
    { "name": "pkg.read",            "ver": "1.0.0", "status": "present", "adapter": "alpm" },
    { "name": "pkg.write",           "ver": "1.0.0", "status": "present", "adapter": "alpm" },
    { "name": "pkg.undo",            "ver": "1.0.0", "status": "present", "value": "cache-downgrade", "adapter": "alpm" },
    { "name": "portal.clipboard",    "ver": "1.0.0", "status": "degraded", "adapter": "xdg-desktop-portal", "note": "writes gated behind an active RemoteDesktop session" },
    { "name": "portal.screenshot",   "ver": "1.0.0", "status": "present", "adapter": "xdg-desktop-portal" },
    { "name": "portal.notify",       "ver": "1.0.0", "status": "present", "adapter": "xdg-desktop-portal" },
    { "name": "files.scope.read",    "ver": "1.0.0", "status": "present", "adapter": "fs-xdg" },
    { "name": "files.scope.write",   "ver": "1.0.0", "status": "present", "adapter": "fs-xdg" },
    { "name": "kb.present",          "ver": "1.0.0", "status": "present", "adapter": "kb-archwiki", "note": "ArchWiki + Manjaro overlay" },
    { "name": "net.read",            "ver": "1.0.0", "status": "present", "adapter": "nm" }
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
grants; a direct call without a grant returns `CapabilityRequired`.

```
desktop.set_value, desktop.invoke, clip_write, notify, svc_start, svc_stop,
pkg_install, pkg_remove, pkg_undo, fs_write
```

## Known limitations

- Every write tool requires a human-issued, out-of-band grant. The default
  scope manifest ships empty (`/etc/omnibus/scopes.d/00-read-only.toml`);
  there is no runtime consent API.
- `portal.clipboard` is degraded: clipboard writes are gated behind an
  active RemoteDesktop session, so `clip_write` works only while that
  session is up.
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

Then run `doctor` once - it diagnoses missing prerequisites (AT-SPI2,
persistent journald) and prints copy-paste remediation commands for a human
to run; `omnibusd` itself never mutates the system through it.

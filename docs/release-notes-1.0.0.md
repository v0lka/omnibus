# omnibus 1.0.0 — Release Notes (v1.0)

Released at tag `v1.0.0` (commit `e6358c0`, 2026-11-07 — the commit that
closes the v1.0 scope; the next commit starts the v1.5 scope). The release
was cut on 2026-11-20 through `.github/workflows/release.yml` after the
project's publication on GitHub: the tag predates the release workflow, so
the release build ran as a workflow dispatch against the tag.

**Version-string caveat, stated up front:** at this tag the workspace
`Cargo.toml` still says `0.1.0` (the version was bumped to `1.5.0` only in
the v1.5 cycle), so a binary built from this tag prints `omnibusd 0.1.0`
for `omnibusd --version`. The release version is carried by the tag and
the artifact names; `omnibus-contracts` carries its own semver
(`1.0.0` at this release, per `specs/architecture/evolution-and-compat.md`).

v1.0 is claimed per the D9 ladder as **"the full contract on the target
distros"**: reads + typed writes, `pkg.undo` where the package manager
provides it, past-boot diagnostics where the journal is persistent. Nothing
below promises more than the platform tier; `capabilities()` is machine
truth and always wins a dispute with this document
(`specs/architecture/capability-model.md`).

## 1. The nine-leg platform matrix status

Tiers per `specs/architecture/platform-matrix.md`. Evidence shorthand:
[ms2-acceptance.md](verification/ms2-acceptance.md) gates E1–E10 (run
2026-10-14, E6/E7 re-run green 2026-10-15), the family adapter suites
(E14 apt, E15 dnf), and the probe/floor/capabilities unit proxies
(E11–E13).

| Leg | Tier | Result at this tag | Runs where |
|---|---|---|---|
| Arch | T1 | PASS on the reference machine: full workspace suite 462/0 (E6), smoke demos 1–3 (E8), live T1 daemon (E9/E10). The `ci.yml` X11/Wayland container legs were finalized green on GitHub-hosted runners on 2026-11-19 — after this tag; at tag time they ran in the pre-publication CI. | reference machine + `.github/workflows/ci.yml` |
| Manjaro | T1 | PASS on the reference machine (same alpm/journald/systemd1/atspi family; delayed repos / mhwd / pamac live in the probe report, E9/E10). True-VM KDE/X11 × X11/Wayland legs remain dispatch-gated. | reference machine + `.github/workflows/manjaro.yml` |
| Debian trixie | T2 | Contract suite green on the fixture backend (E14: 24 passed; family and floor selection unit-pinned, E11–E13). The real runner container leg is REQ-EXT at this tag. | `.github/workflows/t2.yml` (container) |
| Debian bookworm | T2 | Same local proxies; the on-the-floor selection (systemd 252 met, at-spi2 2.46 below the 2.50 floor → `desktop.*` hidden) is pinned by tests. Real runner container leg REQ-EXT at this tag. | `.github/workflows/t2.yml` (container) |
| Ubuntu LTS | T2 | Same local proxies, including the volatile-journal opt-in halves (E13). Real runner container leg REQ-EXT at this tag. | `.github/workflows/t2.yml` (container) |
| Ubuntu 26.04 LTS | T2 | Row added by gap-analysis decision Р-6 (systemd 259, above both floors) with the same suite; real leg REQ-EXT at this tag. | `.github/workflows/t2.yml` (container) |
| Mint | T2 | Did not run: the VM leg is manual-dispatch only (`T2_VM_ENABLED`); its CI stand-in is the Ubuntu 24.04 base (documented, smoke signal — never the Mint guarantee). | `.github/workflows/t2.yml` (VM, REQ-EXT) |
| Fedora | T2 | dnf contract suite green on the fixture backend (E15: 29 passed, including the `history undo` legs). Fedora container/VM legs REQ-EXT at this tag. | `.github/workflows/t2.yml` (container) |
| Debian bullseye | T3 | Did not run. The build-only row existed in `ci/matrix/t2.yml` at this tag and was dropped on 2026-11-19: bullseye went EOL in August 2026 and its packages moved to the archive, so an honest in-container apt install is no longer possible. The T3 tier promise is unchanged. | dropped from CI (was `.github/workflows/t2.yml`) |

A leg that did not run is recorded as "did not run", never as green.

## 2. Tier ↔ capabilities divergence table (the gate)

Every known divergence is a row; none is silently dropped
(`docs/release-checklist.md` §4; `specs/architecture/platform-matrix.md`
is the tier source).

| Platform | Tier | Tier promise | capabilities() reality | Explanation |
|---|---|---|---|---|
| Debian bookworm | T2 | full read, incl. desktop | `desktop.*` hidden | at-spi2 2.46 is below the 2.50 floor — the documented row-level degradation; tests pin the exact floor arithmetic (E11) |
| apt family (Debian, Ubuntu, Mint) | T2 | writes where declared | `pkg.undo` absent | apt has no transactional rollback: `pkg_install`/`pkg_remove` schemas carry the undo-unavailable note, `pkg_undo` is hidden (E13/E14) |
| volatile-journal machines | T2 | past-boot diagnostics out of the box | `journal.prev_boot` = opt-in-required | persistence is a per-machine fact; doctor offers the grant + storage command, `prev_boot` answers only after the grant (E13; M0 findings Table 1: fresh installs on the target distros default persistent) |
| networkd / no-NM stands | T2 | reads incl. network status | `net_status` hidden (`CapabilityRequired`) | the nm adapter activates only on a live NM bus name; no journal errors on absence (unit-verified; a real networkd stand is REQ-EXT) |
| machines without wl-clipboard/xclip | T1/T2 | clipboard read/write where declared | `clip_read`/`clip_write` hidden | decision Р-4 shipped the wl-clipboard/xclip backend: `portal.clipboard` is present only where a backend binary exists; the portal Clipboard RemoteDesktop path is deliberately never created (no input-emulation surface) |
| Manjaro | T1 | full contract | delayed repos, mhwd, pamac | probe facts in the report, never quiet divergences (E9/E10) |

## 3. Reference machine: capabilities document shape (Manjaro T1)

The block below is an **illustrative document in the shipped shape** —
the flat `CapabilitiesDocument` envelope (`contract_version`, `distro`,
`capabilities`) the way a healthy T1 Manjaro machine reports it, as
recorded live in E10 (17 read-only tools, all floors met, `floors: []`,
`journal.persistent` and `journal.prev_boot` present with `value: true`,
`net.read` via nm, `pkg.undo` cache-downgrade via alpm). It is not a
literal capture of one release-time run: run `capabilities()` against
your own daemon for machine truth.

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
    { "name": "portal.clipboard",    "ver": "1.0.0", "status": "present",  "value": null, "adapter": "xdg-portals", "note": "wl-clipboard/xclip backend (decision Р-4): present only where a backend binary exists; the portal RemoteDesktop path is never created", "remedy": null },
    { "name": "portal.screenshot",   "ver": "1.0.0", "status": "present",  "value": null, "adapter": "xdg-portals", "note": "the portal Screenshot interface answers on the session bus", "remedy": null },
    { "name": "portal.notify",       "ver": "1.0.0", "status": "present",  "value": null, "adapter": "xdg-portals", "note": "the notification service answers on the session bus", "remedy": null },
    { "name": "files.scope.read",    "ver": "1.0.0", "status": "present",  "value": null, "adapter": "fs-xdg",      "note": null, "remedy": null },
    { "name": "files.scope.write",   "ver": "1.0.0", "status": "present",  "value": null, "adapter": "fs-xdg",      "note": "active only inside granted write scopes", "remedy": null },
    { "name": "kb.present",          "ver": "1.0.0", "status": "present",  "value": null, "adapter": "kb-archwiki", "note": "ArchWiki plus the manjaro-overlay directory", "remedy": null },
    { "name": "net.read",            "ver": "1.0.0", "status": "present",  "value": null, "adapter": "nm",          "note": "NetworkManager, read-only: the adapter declares no write methods", "remedy": null }
  ]
}
```

The version floors (systemd 261 ≥ 252, at-spi2 2.60.6 ≥ 2.50 on this
machine) are probe/doctor material: they ride in the platform probe report
and the doctor output, not in the serialized capabilities document.

### Tools exposed on this machine

Default install: 17 read-only tools.

```
desktop.tree, desktop.element, clip_read, screenshot, svc_list, svc_status,
net_status, journal_query, pkg_list, pkg_info, fs_list, fs_read, wiki_search,
wiki_page, capabilities, session_info, doctor
```

The 10 write tools appear in `tools/list` only after the human provisions
grants; a direct call without a grant is refused with `ScopeDenied` (and
the refusal is audited).

## 4. Known limitations

- Every write tool requires a human-issued, out-of-band grant. The default
  scope manifest ships empty (`/etc/omnibus/scopes.d/00-read-only.toml`);
  there is no runtime consent API, and an agent-provided "user confirmed"
  token is always denied.
- The apt family has no transactional undo: `pkg_undo` does not exist
  there, and the write schemas say so explicitly.
- `journal.prev_boot` requires a persistent journal and appears only after
  the persistence opt-in (grant + storage command, then the next reboot).
- The network domain is read-only (`net.read`); network writes are on the
  un-plan list and do not exist in v1.
- Clipboard reads/writes need the wl-clipboard (wl-copy + wl-paste) or
  xclip binaries on the machine; without them the tools hide honestly.
- macOS is an MS3 (v2.0) target: no macOS adapters or artifacts ship in
  v1.0.

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

There is no telemetry at all. `omnibusd` makes zero model calls and
performs no network communication of its own. The only network traffic a
session can produce is `wiki_search` / `wiki_page` fetches that the agent
itself explicitly requests; nothing else is sent anywhere, and reads are
audited locally, not reported.

## Release artifacts

Built by `.github/workflows/release.yml` inside each family's reference
container (Linux x86_64 only; aarch64 and macOS are not targets of any
spec or CI matrix):

| Artifact | Built on | Covers |
|---|---|---|
| `omnibusd-1.0.0-arch-x86_64.tar.xz` | archlinux:base-devel (full build) | Arch, Manjaro (T1) |
| `omnibusd-1.0.0-debian-x86_64.tar.xz` | debian:bookworm (cross-family build) | Debian bookworm, trixie (T2) |
| `omnibusd-1.0.0-ubuntu-x86_64.tar.xz` | ubuntu:24.04 (cross-family build) | Ubuntu LTS, Mint 22.x (T2) |
| `omnibusd-1.0.0-fedora-x86_64.tar.xz` | fedora:latest (cross-family build) | Fedora (T2) |

`SHA256SUMS` covers every artifact. Install:

```bash
tar -xJf omnibusd-1.0.0-<os>-x86_64.tar.xz
install -m 0755 omnibusd ~/.local/bin/     # or /usr/local/bin
omnibusd --version                          # prints 0.1.0 at this tag — see the caveat above
```

The cross-family builds exclude the alpm adapter by design (it links the
system libalpm and cannot build off the Arch family); on those platforms
the probe hides the absent alpm row and the family backend (apt/dnf) is
the packages subsystem — the honest product shape. Distribution packages
(AUR, Debian repo, Fedora COPR, Ubuntu PPA) remain the human-issued,
signed channels of `docs/release-checklist.md` §3 — REQ-EXT, not part of
this release.

## Metrics

- TTFM per platform: REQ-EXT (the release-time measurement of
  `docs/release-checklist.md` §4.5; the automatable onboarding-test half
  plus the VM half).
- North-star trend (successful agent tasks per week): REQ-EXT — manual
  tracking per the pitch; there is no telemetry to measure it with, by
  design.

## Provenance

- Verification record: [verification/ms2-acceptance.md](verification/ms2-acceptance.md)
  (33/33 acceptance criteria; the REQ-EXT register lists what stays
  external — VM matrices, signed publication, the announcements).
- Manjaro T1 acceptance: [verification/ms1-ms2-manjaro-acceptance.md](verification/ms1-ms2-manjaro-acceptance.md).
- Floor/dist facts: [verification/m0-findings.md](verification/m0-findings.md).
- Scope-closing decisions (typed `pkg.undo` enum, wl-clipboard/xclip
  clipboard backend, demo4, the Ubuntu 26.04 row):
  [verification/ms1-ms2-gap-analysis.md](verification/ms1-ms2-gap-analysis.md), decisions Р-1/Р-4/Р-6.
- Release procedure: [release-checklist.md](release-checklist.md).

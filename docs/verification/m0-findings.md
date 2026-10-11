# M0 Findings — Verification of the Open M0 Assumptions (Linux Part)

- **Purpose:** closes roadmap task 1.1.1 ("verification of the open M0 assumptions, Linux part"): a fact table
  for the Linux-scope items of the five open questions in §10 of
  `docs/development/omnibus-architecture.md`, with a source per fact — facts, not assumptions.
- **Method:** (a) local live check on the reference Manjaro machine; (b) packaging and documentation
  research (package indexes, release notes, Debian/Fedora package artifacts downloaded and inspected
  directly); (c) items that can only be settled on real installed systems are listed in
  [External runs required](#external-runs-required) and are **not** claimed as verified here.
- **Date:** research retrieval date 2026-10-10; local live check 2026-10-10 (Manjaro, kernel
  7.1.13-2-MANJARO).
- **Companion code:** the version-floor constants live in `omnibus-contracts/src/floors.rs`
  (`SYSTEMD_FLOOR = 252.0.0`, `ATSPI_FLOOR = 2.50.0`); this document is the measurement record they
  reference.

## Closure status of the five §10 questions

| # | §10 question | Status in this document |
|---|--------------|-------------------------|
| 1 | Default journald persistence on Debian (trixie/bookworm/bullseye), Fedora, Manjaro | **Closed** (Table 1), fresh-install confirmation kept in the external-runs checklist |
| 2 | Actual AT-SPI2 floor per DE matrix (GNOME/KDE/XFCE/Cinnamon/MATE) | **Partially closed** (Table 3): floor constant 2.50 stands; the per-DE contract-suite sweep requires an external VM run |
| 3 | Quality of Rust AX bindings (objc2) — macOS | **Out of scope here** (macOS item; stays open for the MS3 macOS track) |
| 4 | TCC behavior in headless/SSH sessions — macOS | **Out of scope here** (macOS item; stays open for the MS3 macOS track) |
| 5 | systemd version in the current Ubuntu LTS (confirm 255) | **Closed** (Table 2): 24.04 LTS ships 255 as assumed; the newer 26.04 LTS ships 259 |

## Table 1 — journald persistence defaults

Mechanism common to all rows: with the default `Storage=auto`, journald is persistent if and only if
`/var/log/journal` exists (source: [journald.conf(5),
man7.org](https://www.man7.org/linux/man-pages/man5/journald.conf.5.html), retrieved 2026-10-10).
So "persistent by default" is a packaging fact: does a fresh install create the directory?

| Distro | Default persistence (fact) | Source | Consequence for roadmap task 2.5.1 (who gets the doctor opt-in suggestion) |
|---|---|---|---|
| Manjaro | **Persistent** — `/var/log/journal` exists; `journalctl --list-boots` shows 7 boots (`journal.prev_boot=true`) | local live check, 2026-10-10, kernel 7.1.13-2-MANJARO (`ls /var/log/journal`, `journalctl --list-boots`) | No suggestion on the reference machine; probe reports the fact, doctor has nothing to offer |
| Debian trixie (13) | **Persistent** — the `systemd` package postinst contains `# Enable persistent journal, in auto-mode, by default on new installs` + `mkdir -p "$DPKG_ROOT/var/log/journal"` (postinst lines 71–73) | artifact `systemd_257.13-1~deb13u1_amd64.deb` from deb.debian.org, control.tar/postinst inspected 2026-10-10; package page [packages.debian.org/stable/systemd](https://packages.debian.org/stable/systemd) | No suggestion on fresh installs |
| Debian bookworm (12) | **Persistent** — same packaging (the postinst change shipped in the bullseye cycle and is inherited) | [LWN 812238, "Enabling the persistent journal in Debian"](https://lwn.net/Articles/812238/) (retrieved 2026-10-10); fresh-install spot check kept in the external-runs checklist | No suggestion on fresh installs |
| Debian bullseye (11) | **Persistent** — the enabling change (Debian wishlist bug [717388](https://bugs.debian.org/cgi-bin/bugreport.cgi?bug=717388), fixed by the systemd maintainer in February 2020) landed in the bullseye cycle: new installs and upgrades create the directory | [LWN 812238](https://lwn.net/Articles/812238/); note this corrects the earlier working assumption "persistent since buster": buster (10, 2019) still defaulted to volatile `/run/log/journal` + rsyslog | No suggestion on fresh installs |
| Fedora | **Persistent** — `systemd.spec` `%post` runs on initial install only (`[ $1 -eq 1 ] || exit 0`) and executes `mkdir -p %{_localstatedir}/log/journal` | artifact `systemd-258.7-1.fc43.src.rpm` from kojipkgs.fedoraproject.org, `systemd.spec` %post block inspected 2026-10-10; package page [packages.fedoraproject.org …/fedora-43.html](https://packages.fedoraproject.org/pkgs/systemd/systemd/fedora-43.html) | No suggestion on fresh installs |
| Ubuntu 24.04 LTS | **Persistent** — `debian/systemd.postinst` creates `/var/log/journal` on new installs **and upgrades** since 18.04 (`mkdir -p /var/log/journal` + `systemd-tmpfiles --create --prefix /var/log/journal`, LP #1618188) | [Launchpad commit f94f18d9](https://git.launchpad.net/~ubuntu-core-dev/ubuntu/+source/systemd/commit/?id=f94f18d9dbc085b6a9ff33c141a6e542142f85b5) (2017-12-11); [LWN 812238](https://lwn.net/Articles/812238/) quotes the Ubuntu maintainer: systems installed with / upgraded to 18.04 LTS "have persistent journal enabled by default across the board" | **Corrects the roadmap/spec assumption "Ubuntu volatile by default"** — see Plan impact below; fresh-install confirmation kept in the external-runs checklist |
| Mint 22.x | **Persistent by inheritance** — "Linux Mint 22.x is based on Ubuntu 24.04"; Mint consumes Ubuntu's `systemd` package, hence the same postinst behavior | [Mint 22 release notes](https://www.linuxmint.com/rel_wilma.php) (section "Known issues → Other issues"), retrieved 2026-10-10; same fresh-install caveat as Ubuntu | **Corrects the assumption "Mint volatile (inherited from Ubuntu)"** — see Plan impact |

Net result: **on all six distro families in the matrix, a default install is persistent.** Volatile
journals remain possible through local admin configuration (explicit `Storage=volatile/auto` without
the directory), not through distro defaults; the probe flag `journal.persistent` is the per-machine
truth either way.

## Table 2 — systemd versions vs the 252 floor

Floor: `SYSTEMD_FLOOR = 252.0.0` (`omnibus-contracts/src/floors.rs`; bookworm sits exactly on the
floor and stays active, anything lower does not activate the systemd1 subsystem).

| Distro | systemd version (fact) | On/above floor 252? | Source |
|---|---|---|---|
| Manjaro (reference machine) | **261** (`systemctl --version`: systemd 261) | yes | local live check, 2026-10-10, kernel 7.1.13-2-MANJARO |
| Ubuntu 24.04 LTS (noble) | **255.4-1ubuntu8.x** | yes | [packages.ubuntu.com/noble/systemd](https://packages.ubuntu.com/noble/systemd), retrieved 2026-10-10 |
| Ubuntu 26.04 LTS (resolute, "Resolute Raccoon", released 2026-04-23) | **259.5-0ubuntu3.x** | yes | [packages.ubuntu.com/resolute/systemd](https://packages.ubuntu.com/resolute/systemd); release notes [documentation.ubuntu.com/release-notes/26.04](https://documentation.ubuntu.com/release-notes/26.04/), retrieved 2026-10-10 |
| Debian trixie (13, current stable; 13.0 on 2025-08-09) | **257.13-1~deb13u1** | yes | [packages.debian.org/stable/systemd](https://packages.debian.org/stable/systemd), retrieved 2026-10-10 |
| Debian bookworm (12) | **252** | on the floor (active) | [bookworm release notes, ch-whats-new table: "Systemd 247 → 252"](https://www.debian.org/releases/bookworm/amd64/release-notes/ch-whats-new.en.html), retrieved 2026-10-10 |
| Debian bullseye (11) | **247** | **below the floor** (systemd1 adapter must not activate) | same bookworm release-notes table ("Version in 11 (bullseye): Systemd 247"), retrieved 2026-10-10 |
| Fedora 43 (updates) | **258.7-1.fc43** (258 series) | yes | [packages.fedoraproject.org …/fedora-43.html](https://packages.fedoraproject.org/pkgs/systemd/systemd/fedora-43.html); update announce [lists.fedoraproject.org](https://lists.fedoraproject.org/archives/list/package-announce@lists.fedoraproject.org/thread/TIQ5PHVFYFGJQRLV5FW4TSEBDBAMZYH4/), retrieved 2026-10-10 |
| Fedora 44 (updates) / older 42 | 259.9-1.fc44 / 257.6-1.fc42 | yes | [Fedora packages overview](https://packages.fedoraproject.org/pkgs/systemd/systemd/fedora-42.html); F42 announce [lists.fedoraproject.org](https://lists.fedoraproject.org/archives/list/package-announce@lists.fedoraproject.org/thread/TIQ5PHVFYFGJQRLV5FW4TSEBDBAMZYH4/), retrieved 2026-10-10 |

Roadmap question 5 resolution: the "systemd 255 in the current Ubuntu LTS" figure in the §6 matrix
is **confirmed for 24.04 LTS** (255.4 series) and already superseded on 26.04 LTS (259.5 series).
No supported distro in the matrix sits below the 252 floor except Debian bullseye (247), exactly as
the floor constant's doc comment anticipates.

## Table 3 — AT-SPI2 versions per DE and the floor decision

Key structural fact: **AT-SPI2 is a distro-wide stack, not a per-DE component.** `at-spi2-core`
defines the D-Bus protocol and runs the accessibility bus; toolkits (GTK 3/4, Qt 5/6, WebKit) talk
to it directly, so the desktop environment on top does not ship or pin its own at-spi2 version
(sources: [GNOME at-spi2-core architecture
doc](https://gnome.pages.gitlab.gnome.org/at-spi2-core/devel-docs/architecture.html),
[freedesktop.org AT-SPI2](https://www.freedesktop.org/wiki/Accessibility/AT-SPI2/), retrieved
2026-10-10). The per-DE question therefore reduces to: which distro package does that DE's install
get, and does the runtime bus actually serve the contract operations.

**Floor decision:** `ATSPI_FLOOR = 2.50.0` stays fixed in `omnibus-contracts/src/floors.rs`
(proxy value from the live checks — Manjaro 2.60.6 and Ubuntu 24.04's 2.52 are at/above it). The
measure that would *justify* the number — the minimum at-spi2 version at which the contract
operations (tree / element / set_value / invoke) pass the contract test suite on all five DEs —
**requires an external VM run** and is tracked in the checklist below.

| DE (version context) | Distro package it consumes | at-spi2 version available to the DE | Full contract suite on this DE | Source |
|---|---|---|---|---|
| GNOME (local machine: GNOME Wayland session) | Manjaro distro-wide package | **2.60.6-1** (`pacman -Q at-spi2-core`) | REQUIRES EXTERNAL VM RUN | local live check, 2026-10-10 (`pacman -Q at-spi2-core`, GNOME Wayland session) |
| GNOME 48 (Debian trixie) / GNOME 48 (Ubuntu 24.04) | distro-wide package | trixie **2.56.2-1+deb13u1**; noble **2.52.0-1build1** | REQUIRES EXTERNAL VM RUN | [packages.debian.org/trixie/at-spi2-core](https://packages.debian.org/trixie/at-spi2-core); [packages.ubuntu.com/noble/at-spi2-core](https://packages.ubuntu.com/noble/at-spi2-core); trixie ships GNOME 48 per the [Debian 13 release announcement](https://www.debian.org/News/2025/20250809), retrieved 2026-10-10 |
| KDE Plasma | distro-wide package (Qt talks to at-spi2 directly; the DE pins nothing) | = the distro's `at-spi2-core` (e.g. 2.52.0 on Ubuntu 24.04 KDE) | REQUIRES EXTERNAL VM RUN | [GNOME at-spi2-core architecture doc](https://gnome.pages.gitlab.gnome.org/at-spi2-core/devel-docs/architecture.html) ("Modern toolkits: GTK4, Qt5, and WebKit talk DBus directly"), retrieved 2026-10-10 |
| XFCE | distro-wide package (GTK-based) | = the distro's `at-spi2-core` | REQUIRES EXTERNAL VM RUN | same architecture doc; GTK toolkit path, retrieved 2026-10-10 |
| Cinnamon | distro-wide package (GTK-based) | = the distro's `at-spi2-core` (Mint 22.x → 2.52.0, Ubuntu 24.04 base) | REQUIRES EXTERNAL VM RUN | same architecture doc; [Mint 22 release notes](https://www.linuxmint.com/rel_wilma.php), retrieved 2026-10-10 |
| MATE | distro-wide package (GTK-based) | = the distro's `at-spi2-core` | REQUIRES EXTERNAL VM RUN | same architecture doc, retrieved 2026-10-10 |

Distro-level at-spi2-core versions collected for the floor analysis (retrieved 2026-10-10):

| Distro | at-spi2-core | vs floor 2.50 | Source |
|---|---|---|---|
| Manjaro/Arch (local) | 2.60.6-1 | above | local live check, 2026-10-10 |
| Ubuntu 24.04 LTS | 2.52.0-1build1 | above | [packages.ubuntu.com/noble/at-spi2-core](https://packages.ubuntu.com/noble/at-spi2-core) |
| Ubuntu 26.04 LTS | 2.60.0-1 | above | [packages.ubuntu.com/resolute/at-spi2-core](https://packages.ubuntu.com/resolute/at-spi2-core) |
| Debian trixie | 2.56.2-1+deb13u1 | above | [packages.debian.org/trixie/at-spi2-core](https://packages.debian.org/trixie/at-spi2-core); mirrored at [debian.pkgs.org](https://debian.pkgs.org/13/debian-main-amd64/at-spi2-core_2.56.2-1+deb13u2_amd64.deb.html) |
| Debian bookworm | 2.46.0-5 | **below the floor** | [packages.debian.org/bookworm/at-spi2-core](https://packages.debian.org/bookworm/at-spi2-core); version per [debian.pkgs.org](https://debian.pkgs.org/12/debian-main-arm64/at-spi2-core_2.46.0-5_arm64.deb.html) |
| Debian bullseye | 2.38.0-4+deb11u1 | **below the floor** | deb.debian.org pool listing [pool/main/a/at-spi2-core/](http://deb.debian.org/debian/pool/main/a/at-spi2-core/) (artifact filename inspected 2026-10-10) |
| Fedora 43 | 2.58.9-1.fc43 (updates) | above | [packages.fedoraproject.org …/fedora-43.html](https://packages.fedoraproject.org/pkgs/at-spi2-core/at-spi2-core/fedora-43.html) (overview also lists F44 2.60.7) |
| Fedora 42 | 2.56 series (at-spi2-atk 2.56.8-1.fc42 in updates; exact at-spi2-core micro **UNVERIFIED**) | above | [fedora.pkgs.org at-spi2-atk page](https://fedora.pkgs.org/42/fedora-updates-aarch64/at-spi2-atk-devel-2.56.8-1.fc42.aarch64.rpm.html) |

## Plan impact

1. **Roadmap task 2.5.1 opt-in list derived from Table 1 — the plan's premise is outdated.** The plan and the
   spec currently say: Ubuntu/Mint volatile by default → doctor suggests persistence; Manjaro/
   Debian/Fedora persistent → no suggestion (roadmap task 2.5.1; `docs/development/omnibus-spec.md`
   §5 rows "Ubuntu LTS: volatile by default", "Mint: volatile (inherited from Ubuntu)";
   `docs/development/omnibus-architecture.md` §6 rows for Ubuntu/Mint). The evidence above shows the
   opposite for Ubuntu/Mint: persistent since Ubuntu 18.04 (postinst, LP #1618188), inherited by
   Mint 22.x. **Volatility is no longer a distro default anywhere in the supported matrix.**
2. **Current code matches the old premise, not the measured facts.** `omnibus-core/src/doctor.rs`
   (journal-persistence finding, family check at the `DistroFamily::Ubuntu | DistroFamily::Mint`
   match, command push beneath it) emits the "enable journal persistence" command only for the
   Ubuntu/Mint families — i.e. the command is wired to fire exactly where fresh installs are
   persistent, so on default installs it will effectively never fire. On volatile machines created
   by local admin choice (any family) it will not fire at all. Suggested follow-up (roadmap AC
   "confirmed or corrected"): drive the suggestion off the probe fact
   (`journal.persistent == false`), not off the distro family, and update the spec §5 / arch §6 rows
   at the next minor revision. **No code or spec files were changed by this document** (scope of
   task 1.1.1 is the findings table).
3. **`journal.prev_boot` becomes a T1-grade capability almost everywhere.** With all six defaults
   persistent, past-boot diagnostics (`journalctl --list-boots`, 7 boots on the reference machine)
   work out of the box on Manjaro, Debian 11–13, Fedora, Ubuntu 24.04/26.04 and Mint 22.x; the
   "laptop did not wake up" flagship scenario does not need an opt-in step on default installs.
4. **bookworm tension to resolve in the same §6 revision.** Debian 12 is T2 and exactly on the
   systemd floor (252), but its at-spi2-core is 2.46 — below the fixed 2.50 floor. As the constants
   stand, an omnibusd on bookworm would run systemd1 but deactivate the atspi adapter with
   `FloorUnsupported`. Either the 5-DE VM sweep shows the contract suite passing on 2.46 (then the
   floor drops to ≤ 2.46) or bookworm's atspi tools degrade by the §7.3 ladder (documented, honest).
5. **Bullseye stays the negative case** (systemd 247 < 252, at-spi2 2.38 < 2.50): both floors bite,
   matching the floor constants' doc comments and the T3 tier.

## External runs required

The items below cannot be settled by packaging research and remain **explicitly unmeasured**; per
the approved scope decision this task ran the headless proxy checks above, and the true VM sweep is
external work:

- [ ] Fresh-install `ls /var/log/journal` on Manjaro, Debian trixie/bookworm/bullseye, Fedora 43,
      Ubuntu 24.04/26.04, Mint 22.x — confirm the packaging-derived Table 1 facts on real installs.
- [ ] Same installs after **first reboot**: `journalctl --list-boots` shows ≥ 2 boots (persistence
      end-to-end, not just the directory).
- [ ] A deliberately volatile machine (no `/var/log/journal`, any distro): confirm the probe reports
      `journal.persistent=false`, `journal.prev_boot` absent, and the corrected doctor logic suggests
      the opt-in (see Plan impact item 2).
- [ ] 5-DE contract-suite sweep (tree / element / set_value / invoke) on GNOME 48+, KDE Plasma 6,
      XFCE 4.20, Cinnamon 6.x, MATE 1.28 — record the minimum at-spi2 version that passes on each
      DE and confirm or correct `ATSPI_FLOOR = 2.50.0` (roadmap 1.1.1 "How").
- [ ] `gdbus introspect org.a11y.Bus` per DE per Wayland and X11 — confirm the bus is up on every
      supported DE without per-app flags (Electron per-app enable stays a separate task).
- [ ] bookworm at-spi2 2.46 vs floor: either lower the floor with sweep evidence or document the
      `FloorUnsupported` degradation on bookworm (Plan impact item 4).
- [ ] Debian bullseye negative test on a real VM: systemd1 and atspi adapters both refuse to
      activate with exact `capabilities()` reasons (floor mechanism, roadmap task 2.1.2).

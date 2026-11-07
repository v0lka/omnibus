# Runbook — external steps closing the Linux part (MS1–MS2)

Companion to `docs/verification/ms1-ms2-gap-analysis.md`. Every repo-side item
of the gap analysis is closed; what remains is exactly the external step list
of its section 5 (blocks A–D) plus the two organizational decisions Р-7 and
Р-8. This runbook is the manual-execution procedure for those: each step names
its preconditions, the action, the expected result, and the roadmap task it
closes. Execute top to bottom inside a block; blocks are ordered so earlier
blocks feed later ones. A human executes every step; no step is delegated to
an agent, and no step mutates a machine without an explicit command printed
below.

Evidence convention: record the command output (or a pointer to the CI run)
next to the step in the release/verification notes — the gap analysis is
closed only when every step here carries that evidence.

## Block A — real CI infrastructure (roadmap 1.1.3, 1.8.2, 2.1.1, 2.6.2)

A1. **First real CI runs.** Precondition: the repository is hosted with
    Actions enabled (see Р-8). Action: push to `main` / open a PR and let
    `.github/workflows/ci.yml` run (gate, dependency-rules, matrix-t1 on
    X11/Wayland, coverage, package-build), then `.github/workflows/t2.yml`
    (container legs: trixie, bookworm, ubuntu 24.04, ubuntu 26.04, fedora;
    bullseye build-only). Expected: every job green on a runner; the
    dependency-rules negative injection turns red inside the job (that is its
    green condition). Closes the "CI runs it" halves of 1.1.3, 1.1.4, 1.8.2.

A2. **Gate timing.** Precondition: A1 ran at least once (caches warm).
    Action: re-run the `gate` job and read its wall-clock duration; compare
    against the 10-minute AC (1.1.3). Expected: < 10 min on the cached
    runner; record the number. If over, profile before touching the timeout.

A3. **Branch protection.** Precondition: Р-8 decided (hosting). Action:
    enable branch protection on `main` (required status checks: every ci.yml
    job; no force push; no bypass). Expected: a red required job blocks
    merge — "красный main невозможен" (AC 1.8.2). Record a screenshot/audit
    entry as evidence.

A4. **t2.yml schedule.** Action: trigger `.github/workflows/t2.yml`
    (workflow_dispatch or its schedule) and let the container legs run.
    Expected: trixie/bookworm/ubuntu-24.04/ubuntu-26.04/fedora green
    (smoke+contract), bullseye green (build-only); vm-legs fail loudly until
    A5 lands — that is the designed honesty, not a failure of this step.

A5. **VM runners (Р-7, part 1).** Action: provision self-hosted VM runners
    (see Р-7 below for the decision record), then set the repo variables
    `MANJARO_VM_ENABLED=true` / `T2_VM_ENABLED=true` and dispatch the VM
    matrix jobs. Expected: manjaro-vm-matrix and the t2 vm-legs execute for
    real; Mint runs its VM leg (no stand-in classification anymore).

## Block B — VM confirmations (roadmap 1.1.1, 1.4.1, 1.5.1, 1.5.2, 1.5.5, 1.6.1, 1.7.1, 2.1.1, 2.1.2, 2.2.3, 2.3.1, 2.4.1, 2.5.1, 2.5.2)

B1. **Six-distro probe matrix.** Action: on fresh VMs of Arch, Manjaro,
    Debian trixie, bookworm, Ubuntu LTS, Mint, Fedora: run the daemon, call
    `capabilities`. Expected: family-correct adapter selection; bullseye
    (T3 spot-check) hides pkg with the exact floor reason; Ubuntu/Mint show
    the journald persistence fact. Closes 2.1.1 AC1, 2.1.2 legs.

B2. **Volatile-journald walkthrough.** Action: an Ubuntu VM with
    intentionally volatile journald: probe shows `persistent=false`,
    `prev_boot` absent; run `omnibusd doctor` → apply the opt-in (grant +
    storage command, executed by the human), reboot, re-probe. Expected:
    `persistent=true`, `prev_boot=true`, past-boot query returns records.
    Closes 2.5.1 and the two-stand half of 1.5.2.

B3. **5-DE AT-SPI2 sweep (Р-5 input).** Action: on GNOME, KDE, XFCE,
    Cinnamon, MATE (Wayland and X11 each): run the contract suite against a
    live app; record the minimal at-spi2 version with a green suite;
    `gdbus introspect` the a11y bus per DE per session kind. Expected: the
    2.50 floor confirmed or a decision record to lower it (if lowered:
    bookworm regains `desktop.*` — update the matrix row and t2.yml comment
    in the same change). Closes the per-DE half of 1.1.1 and settles Р-5.

B4. **Desktop read/write matrix.** Action: GNOME/KDE × X11/Wayland VM legs:
    `desktop_tree` on a GTK and an Electron app, `set_value`/`invoke` with a
    grant, ru-layout leg, HiDPI stand, screenshot + notify, and the
    clipboard round-trip via the wl-clipboard/xclip backend (installed or
    honestly absent — the capability fact must follow). Expected: all legs
    green or honestly skipped with the reason recorded. Closes 1.4.1 AC1,
    1.5.1 AC1, 1.5.5 AC1, 1.6.1 AC1 (the CI leg of 1.8.2 closed by A1).

B5. **Manjaro true-VM legs.** Action: KDE/X11 combinations plus the non-free
    mhwd driver smoke. Expected: the full T1 set green; the mhwd-specific
    note in the capabilities document. Closes 2.4.1 VM legs.

B6. **networkd / no-NM stands.** Action: a VM with systemd-networkd and a VM
    without NetworkManager: check `net_status` hidden, no errors in the
    daemon log, direct call → `CapabilityRequired`. Closes 2.2.3 stands.

B7. **Fedora undo on a disposable VM.** Action: install a package, then
    `pkg_undo` through the daemon; plus demo3 on the Fedora leg and a portal
    smoke on fresh Fedora interfaces. Expected: `dnf history undo` returns
    the system to the prior transaction point; audit carries the write
    events. Closes 2.3.1 external legs.

B8. **apt-scale and apt-undo-schemas measurement.** Action: on a real
    apt machine with ~2000 packages: measure `pkg_list`/`pkg_info`; verify
    the `pkg_install`/`pkg_remove` schemas carry the no-undo marking and
    `pkg_undo` stays hidden. Closes 2.2.1/2.2.2 live halves.

B9. **Live systemd write.** Action: restart a disposable test unit through
    `svc_start` with a grant; watch it reach `active` (the sentinel suite
    already covers semantics); plus a real cache-downgrade `pkg_undo` on a
    disposable Arch VM. Closes 1.6.2 AC1 letter, 1.6.3 live leg.

B10. **kb-guides against real mirrors.** Action: on an Ubuntu stand with the
     packaged guides overlay absent: `wiki_search` routes to the distro docs
     source; on an Arch/Ubuntu pair run the one-diagnosis-two-articles
     comparison. Closes 2.5.2 external legs.

## Block C — MVP release (roadmap 1.7.1, 1.8.1, 1.8.3)

C1. **AUR publication.** Action: publish `packaging/PKGBUILD` to the AUR;
    then on a clean Arch VM: `pacman -S omnibus` → daemon works, TTFM
    < 5 min; then the version-bump update test (no manual steps). Closes
    1.8.1 and the TTFM half of 1.7.1.

C2. **MVP tag + release.** Action: `git tag v0.1.0` (name per
    `docs/release-notes-0.1.0-draft.md`), GitHub Release with those notes;
    then the announcements: Show HN, r/archlinux, the GNOME/KDE a11y-track
    talk application; start the north-star tracking log. Closes 1.8.3. Note:
    the v1.0 additive-only audit (block D) diffs against THIS tag.

## Block D — channels and the v1.0 release (roadmap 2.6.1–2.6.3)

D1. **Signed channels.** Action: publish PPA (`packaging/ppa/upload.sh`,
    `allow_unsigned_uploads = 0` posture verified by `make packaging`), the
    Debian repo (trixie/bookworm), COPR (Fedora). Build hosts: rustc ≥ 1.85
    and the vendored tree (`packaging/README.md` "Build hosts"). Closes
    2.6.1 publication leg.

D2. **No-trust fresh machine check.** Action: on a clean VM per channel,
    install WITHOUT importing the key: the package manager must honestly
    warn; with the key imported, install proceeds. Closes 2.6.1 AC2.

D3. **TTFM installs + upgrade.** Action: install via the native manager on
    every T2 platform; daemon works, TTFM < 5 min; then the MVP → v1.0
    upgrade with no manual steps. Closes 2.6.1 AC1/AC3.

D4. **Demo №4 on reference platforms.** Action: run `ci/smoke/run-demos.sh`
    on Arch and on Ubuntu (demo4 "From scratch" green with honest skips;
    the granted install/reboot arc runs per B2 on the VM). Closes the demo4
    legs of 2.6.2 (external step №22).

D5. **v1.0 tag + additive audit.** Action: `git tag v1.0.0`; run the
    additive-only audit of the `omnibus-core` diff-log against the MVP tag
    from C2 per `docs/release-checklist.md` §5 (only additive extensions:
    `net_status`, new capability constants). Closes the 2.6.3 DoD audit.

D6. **v1.0 release report.** Action: GitHub Release carrying the
    `docs/release-checklist.md` §4 report: the 9-leg matrix status table,
    the tier↔capabilities divergence table (bookworm `desktop.*`, apt
    `pkg.undo = none`, `prev_boot` opt-in, NM behind netplan), the reference
    machine's capabilities document, limitations + never-list, TTFM metrics
    and the north-star trend; then positioning updates (Ubuntu/Mint/Fedora/
    Debian), the Show HN follow-up, the Phoronix/Hackaday article. Closes
    2.6.2 report AC and 2.6.3.

## Р-7 — VM runner infrastructure (decision + execution)

Decision to record (owner: the project lead): which hypervisor/host runs the
self-hosted runners, who maintains them, and the security posture (ephemeral
disposable VMs; no secrets inside; runners pinned to these two workflows via
labels). Minimum set derived from the matrices: one Arch/Manjaro-capable host
for the manjaro-vm-matrix (GNOME/KDE × X11/Wayland × mhwd non-free), one
Ubuntu VM for the volatile-journald walkthrough and Mint stand-in replacement,
one Fedora VM for B7. Execution: register the runners, label them
(`vm`, `manjaro-vm`, `t2-vm`), set `MANJARO_VM_ENABLED`/`T2_VM_ENABLED=true`,
then run A5 and block B. Until this decision is executed, the VM guarantees
(Mint, package-install, Manjaro matrix) stay REQ-EXT by design.

## Р-8 — Hosting and branch protection (decision)

Decision to record (owner: the project lead): the public hosting (GitHub
organization vs personal), the Actions plan (public runners suffice for the
container legs; VM legs need A5's self-hosted set), and branch protection on
`main` (A3). Nothing in the repository can express this decision; the gap
analysis records it as the last purely organizational dependency of 1.8.2 and
2.6.2.

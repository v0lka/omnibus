# RPM spec for the Fedora T2 channel (roadmap task 2.6.1, COPR).
#
# Build convention mirrors packaging/PKGBUILD and packaging/debian: the
# source is the repository tree itself, so there is no Source0 tarball - the
# COPR channel builds from a source RPM that the repository release
# infrastructure assembles from a checkout (an srpm of this tree; the actual
# COPR project setup is channel infrastructure, outside the repo). %prep and
# %build run cargo against the workspace root, and %install picks the
# artifacts from target/release/ exactly like the PKGBUILD does.
#
# The SRPM assembly step has TWO hard contracts (both checked at build time,
# neither assumed):
#   * Vendored dependencies: the SRPM tree must carry vendor/ plus the
#     .cargo/config.toml source replacement (cut exactly like
#     packaging/ppa/upload.sh does: `cargo vendor vendor >
#     .cargo/config.toml`). COPR builders are network-isolated; %build runs
#     cargo --offline, so a missing vendor/ fails loudly instead of reaching
#     for the network.
#   * Toolchain floor: edition 2024 = rustc >= 1.85. The chroot's rustc must
#     meet the floor — cargo aborts on the first manifest otherwise. Current
#     Fedora carries a current-enough rustc; the release checklist
#     (docs/release-checklist.md) has the release owner re-verify the chroot
#     before every channel cut instead of trusting this comment.
#
# Fedora deltas, packaged here (roadmap task 2.6.1; tiers per
# specs/architecture/platform-matrix.md - Fedora desktop is T2):
#   * dnf is transactional: pkg_undo works on this family (the one T2
#     platform where undo is real, unlike apt).
#   * Freshest xdg-desktop-portal interfaces: screenshot/clipboard ride the
#     newest portal releases; smoke on the current Fedora release is the
#     2.6.2 matrix leg (.github/workflows/t2.yml).
#   * Weak deps (Recommends) instead of hard Requires for the components the
#     probe reports at run time: the daemon degrades honestly along the R3
#     ladder when they are absent, capabilities() stays truthful.
#
# License: no LICENSE file exists in the repository yet; the tag stays
# Undecided for the same reason the PKGBUILD omits `license` (see
# packaging/PKGBUILD). This is the one rpmlint warning accepted by design
# until upstream designates a license.

Name:           omnibus
Version:        1.5.0
Release:        1%{?dist}
Summary:        omnibusd - uniform, auditable, least-privilege MCP API over a desktop OS

License:        Undecided

# Edition 2024 needs a current toolchain; COPR chroots carry rustc
# Packages/ current enough, and the release infra pins the toolchain.
BuildRequires:  rust
BuildRequires:  cargo
BuildRequires:  gcc

# Hard requires: the components the T2 contract reads through unconditionally
# (journald and systemd1 via systemd, atspi via at-spi2-core). apt/dnf are a
# given on the distro and are not listed (same reasoning as Debian's
# Essential: apt).
Requires:       systemd
Requires:       at-spi2-core
Recommends:     NetworkManager
Recommends:     xdg-desktop-portal

%description
omnibusd gives AI agents a uniform, auditable, least-privilege MCP API over
a desktop OS: services, packages, journal, accessibility UI, files,
clipboard, screenshots, notifications, knowledge bases, network status.
All inference lives in the MCP client; the daemon performs zero model
calls.

The package installs one static binary plus read-only scope-manifest
templates under /etc/omnibus/scopes.d/. A fresh install exposes exactly the
read-only tool set; a human grants writes out-of-band by dropping
additional manifests there - there is no runtime consent API. Writes go
through dnf's transactional undo (pkg_undo) on this family.

%prep
# The source is the checked-out repository tree (see the header): no
# upstream tarball to unpack. An empty recipe keeps %prep explicit.
:

%build
# Offline by contract: the SRPM tree carries the vendored dependencies (see
# the header — the SRPM assembly step cuts them like upload.sh does). A
# builder missing vendor/ fails here, loudly, instead of reaching for the
# network.
#
# Cross-family by contract, byte-identical to debian/rules and the T2 CI legs
# (.github/workflows/t2.yml): --no-default-features drops omnibusd's
# family-alpm wiring and --exclude keeps the omnibus-adapter-alpm member out
# of the build — the alpm adapter links the SYSTEM libalpm via pkg-config
# (alpm-sys has no vendored fallback), absent in this chroot by design (this
# family's backend is dnf; the probe + R3 ladder hide the absent alpm row).
# gcc (BuildRequires above) covers ring's C compiler (ureq -> rustls in
# kb-archwiki); with alpm excluded no pkg-config is needed.
cargo build --workspace --no-default-features --exclude omnibus-adapter-alpm --release --offline

%install
install -Dm0755 target/release/omnibusd %{buildroot}%{_bindir}/omnibusd
install -d %{buildroot}%{_sysconfdir}/omnibus/scopes.d
install -m0644 packaging/scopes.d/*.toml %{buildroot}%{_sysconfdir}/omnibus/scopes.d/

%check
# The contract suite and smoke demos are matrix work (ci/matrix/,
# .github/workflows/t2.yml), not package-build work: a package build stays
# hermetic. Nothing to check here by design.
:

%files
%{_bindir}/omnibusd
%dir %{_sysconfdir}/omnibus/scopes.d
%config(noreplace) %{_sysconfdir}/omnibus/scopes.d/*.toml

%changelog
* Wed Nov 18 2026 Omnibus maintainers <maintainers@omnibus.invalid> - 1.5.0-1
- v1.5 feature set (MS2.5, roadmap phases Ф2.7-Ф2.9): app launch,
  process telemetry, read-only flatpak+snap inventory.

* Wed Oct 14 2026 Omnibus maintainers <maintainers@omnibus.invalid> - 0.1.0-1
- Initial RPM packaging input for the Fedora T2 channel (roadmap task
  2.6.1): one static binary, read-only scope templates, dnf undo intact.

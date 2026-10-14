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
Version:        0.1.0
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
cargo build --release

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
* Wed Oct 14 2026 Omnibus maintainers <maintainers@omnibus.invalid> - 0.1.0-1
- Initial RPM packaging input for the Fedora T2 channel (roadmap task
  2.6.1): one static binary, read-only scope templates, dnf undo intact.

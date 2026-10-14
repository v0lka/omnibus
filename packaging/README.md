# Packaging

In-repo packaging for omnibusd. One convention across all families: the
source is the repository working tree itself — no upstream tarballs — and
every package installs the same artifact set:

- `/usr/bin/omnibusd` — the one static binary (built from the workspace root,
  `cargo build --release`);
- `/etc/omnibus/scopes.d/*.toml` — the read-only scope-manifest templates
  (`scopes.d/`); admins extend write grants by dropping their own manifests
  there, out-of-band. There is no runtime consent API anywhere.

## Layout

| Path | Family | Channel | Roadmap |
|---|---|---|---|
| `PKGBUILD` | Arch + Manjaro (T1) | in-repo makepkg; AUR stays community | 1.8.1 / 2.4.1 |
| `debian/` | Debian trixie + bookworm (T2) | Debian repo infrastructure | 2.6.1 |
| `fedora/omnibus.spec` | Fedora (T2) | COPR infrastructure | 2.6.1 |
| `ppa/` | Ubuntu LTS (T2) + Mint 22.x by base inheritance | Launchpad PPA | 2.6.1 |
| `scopes.d/` | — the shipped read-only default, referenced by every family | | 1.8.1 |

Per-distro notes (Manjaro deltas, Debian floors, dnf undo) live in the
headers of the packaging files themselves; the promise tiers they map to are
normative in `specs/architecture/platform-matrix.md`.

## Validation — `make packaging`

Every artifact here is a declarative definition validated locally by
`ci/validate_packaging.py` (the Make target `packaging` runs it; the CI
`package-build` job and the T2 matrix workflow run the same script):

```
make packaging
```

It parses and cross-checks everything a local run can honestly decide:
debian control/rules/install/changelog structure and version consistency
with the workspace version, the RPM spec's tags/sections/files, the dput
config's signing posture (`allow_unsigned_uploads = 0`), `bash -n` on the
upload script, the T2 matrix schema (all eight Linux legs, tiers per the
platform matrix), and a secret/hygiene scan. Exit 0 = artifacts are
consistent; exit 1 prints every failed check.

## What validation deliberately does not do

The REQ-EXT boundary: installing the native package on a machine with a
real desktop — the roadmap 2.6.1 AC (daemon works, TTFM < 5 min) — is a
VM leg of the T2 matrix (`ci/matrix/t2.yml`,
`.github/workflows/t2.yml`, variable-gated, manual dispatch). CI never
claims it, and no artifact here contains credentials: signing keys and
Launchpad credentials live outside the repository (see `ppa/README.md`).

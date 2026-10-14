# PPA channel — Ubuntu LTS + Mint (roadmap task 2.6.1)

The Personal Package Archive is the T2 distribution channel for the Ubuntu
LTS family. One channel covers two distros:

```
Ubuntu 24.04 LTS (PPA source series) ──► Ubuntu 24.04 LTS
                                      └─► Linux Mint 22.x (inherits the base)
```

## Why Mint needs no separate packaging

Mint 22.x is built on the Ubuntu 24.04 LTS base: same archive, same libc,
same systemd/at-spi2 versions, same `network-manager` package names. The
PPA binaries built for `~ubuntu24.04` install unchanged on Mint. What is
Mint's own (X11 default session, Mint docs/forums as the knowledge base) is
a probe/runtime fact the daemon reports - see
`specs/architecture/platform-matrix.md`, not a packaging concern.

## Artifacts

| File | Role |
|---|---|
| `packaging/debian/` | the shared `debian/` packaging input (control, rules, install, changelog, source/format) |
| `dput.cf` | upload host config for the PPA; uploads are signed (`allow_unsigned_uploads = 0`) |
| `upload.sh` | cuts the source package from the repo tree, applies the series suffix, signs and dputs it |

## Upload procedure (maintainer, out-of-band)

1. Import the PPA signing key into your local GnuPG keyring (outside this
   repository) and configure your own `~/.dput.cf` / `~/.dput.d/` with the
   real Launchpad credentials - the committed `dput.cf` carries placeholder
   names and the security posture, not credentials.
2. Run `packaging/ppa/upload.sh '~ubuntu24.04'`.
3. Launchpad builds the binary debs for the configured series; the roadmap
   acceptance "install via the native package manager on each T2 platform →
   daemon works, TTFM < 5 min" is the VM install leg of the T2 matrix
   (`ci/matrix/t2.yml`, `.github/workflows/t2.yml`) - a REQ-EXT run, never
   faked green in CI.

## Signing

Repository signature enforcement starts at the upload: `allow_unsigned_uploads = 0`
means a source package that was not signed by the maintainer key never
leaves the machine. A fresh machine without the key gets an honest
warning/refusal from apt (the roadmap 2.6.1 AC), because Launchpad-side
signing propagates to the archive metadata.

## Secrets — nothing here

No keys, tokens or credentials exist in this directory; the validation
script (`ci/validate_packaging.py`, Make target `make packaging`) asserts
that, alongside structure checks of every artifact. CI has no credentials
and never uploads; uploads are human-issued actions (the never-list).

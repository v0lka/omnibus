# CI matrix — T2 legs (roadmap task 2.6.2)

`t2.yml` is the declarative definition of the QA matrix: which platforms,
at which tier, running what. `.github/workflows/t2.yml` turns every
`kind: container` leg into a CI job by piping the file through
`to_json.py` into a `fromJSON` strategy — the workflow stays generic, this
file stays the single source of truth.

## Run it locally

```
make packaging            # full validation: matrix, workflows, packaging artifacts
python3 ci/matrix/to_json.py < ci/matrix/t2.yml   # the exact JSON CI consumes
```

## Leg map (all nine Linux platforms, tiers per specs/architecture/platform-matrix.md)

| Leg | Tier | Level | Executes as |
|---|---|---|---|
| arch | T1 | full | `.github/workflows/ci.yml` (owned there) |
| manjaro | T1 | full | `.github/workflows/manjaro.yml` (owned there) |
| debian-trixie | T2 | smoke + contract suite | `debian:trixie` container |
| debian-bookworm | T2 | smoke + contract suite | `debian:bookworm` container (on-the-floor row) |
| ubuntu-lts | T2 | smoke + contract suite | `ubuntu:24.04` container |
| ubuntu-26.04 | T2 | smoke + contract suite | `ubuntu:26.04` container (current LTS, systemd 259) |
| mint | T2 | smoke + contract suite | stand-in: `ubuntu:24.04` container; the real Mint VM is manual-dispatch (REQ-EXT) |
| fedora | T2 | smoke + contract suite | `fedora:latest` container (freshest portals) |
| debian-bullseye | T3 | build only | `debian:bullseye` container |

## What each T2 container leg runs

`cargo build --workspace` → `cargo test --workspace` (the contract suite,
R2) → `ci/smoke/run-demos.sh` (honest SKIPs when a backend is absent).

## The two honesty rules

- **`package-install` is always `req-ext`.** Installing the native package
  on a machine with a real desktop (roadmap 2.6.1 AC, TTFM < 5 min) cannot
  be claimed by a container; it stays a release-runbook item. CI never
  fakes that leg.
- **Stand-ins are classified, never silently substituted.** The Mint leg
  runs its Ubuntu-base stand-in in CI and the workflow's
  leg-classification job prints which legs were real containers, which
  stand-ins, and which were skipped — mirroring the Manjaro workflow's
  rule.

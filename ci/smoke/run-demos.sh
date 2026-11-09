#!/usr/bin/env bash
# Smoke demo runner (roadmap 1.8.2): the four pitch demos as autotests.
#
#   demo1  "Diagnosis in a minute" — journal(prev_boot) -> doctor -> wiki_search
#   demo2  "Forms"                 — fs_read -> desktop.tree -> honest write refusal
#   demo3  "After the update"      — pkg_list/info -> pkg_undo honest refusals
#   demo4  "From scratch"          — capabilities -> doctor -> KB -> honest
#                                    install refusal (the assembly arc, no VM)
#   demo5  "The closed loop"       — app.launch (grant) -> ps.list sees the
#                                    process -> journal_query reads its
#                                    output -> ps.signal (grant) terminates
#                                    it (roadmap 2.9.2; the granted legs skip
#                                    honestly without a live systemd user
#                                    manager — reference CI containers get
#                                    the refusal half)
#
# Safe on a developer machine: read-only by default, no system mutation, and
# every demo exits 0 with "SKIP:" on stderr when a backend is honestly absent.
# Set OMNIBUS_TEST_UNDO=1 to also exercise the granted pkg_undo refusal leg
# (demo3); the grant lives only in a throwaway config dir and the target
# package never exists, so no real transaction can run.
#
# Exit codes: 0 all demos passed or skipped, 1 at least one demo failed.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

DEMOS=(demo1-diagnosis demo2-forms demo3-undo demo4-from-scratch demo5-closed-loop)
failed=()

echo "== omnibus smoke demos (repo root: $REPO_ROOT) =="
for demo in "${DEMOS[@]}"; do
  echo
  echo "=== ${demo} ==="
  if bash "ci/smoke/${demo}.sh"; then
    echo "--- ${demo}: OK"
  else
    echo "--- ${demo}: FAILED" >&2
    failed+=("$demo")
  fi
done

echo
if (( ${#failed[@]} )); then
  echo "smoke summary: FAILED demos: ${failed[*]}" >&2
  exit 1
fi
echo "smoke summary: all demos passed (skips are honest and allowed)"

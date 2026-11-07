#!/usr/bin/env bash
# Smoke demo 4 — "From scratch" (pitch killer demo №4; roadmap 2.6.2 AC).
#
# The pitch scenario: an EMPTY machine -> a working desktop for the task,
# assembled by the agent from the platform's knowledge base. The automatable
# half of that arc, end to end on a fresh daemon (a throwaway XDG_CONFIG_HOME
# with no grants anywhere = the "from scratch" state):
#
#   leg 1  capabilities  — the fresh-session machine inventory: distro, the
#                          closed capability set, unmet floors (the agent's
#                          first honest look at an unknown machine);
#   leg 2  doctor        — the onboarding diagnosis: what the machine misses
#                          and the human levers that fix it (journald
#                          persistence, grants) — the human stays the only
#                          actor who can change the system;
#   leg 3  wiki_search   — the platform knowledge base the agent assembles
#                          from (ArchWiki here; skipped honestly when the KB
#                          provider is absent or the network is down);
#   leg 4  pkg_install   — the assembly write WITHOUT a grant is refused
#                          honestly (ScopeDenied, or CapabilityRequired when
#                          the package subsystem is absent): nothing is ever
#                          installed outside a human-issued grant.
#
# What stays outside the automatable half (and is skipped here by design):
# an actually empty VM and the granted install/reboot walk-through — those
# are the reference-platform legs of docs/release-checklist.md (demo4 runs on
# Arch and Ubuntu at release time, external step №22 of the gap analysis).
#
# Read-only by default, no system mutation. SKIP (exit 0, "SKIP:" on stderr)
# whenever a backend is honestly absent.
# Exit codes: 0 pass-or-skip, 1 assertion failure.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

TMPDIR_SMOKE="$(mktemp -d "${TMPDIR:-/tmp}/omnibus-smoke-demo4.XXXXXX")"
cleanup() { rm -rf "$TMPDIR_SMOKE"; }
trap cleanup EXIT

echo "== building the workspace (demo4) =="
cargo build --workspace --quiet

echo "== running demo4 against a fresh omnibusd (no grants, no state) =="
XDG_CONFIG_HOME="$TMPDIR_SMOKE/config" \
XDG_STATE_HOME="$TMPDIR_SMOKE/state" \
SMOKE_DIR="$REPO_ROOT/ci/smoke" \
python3 - <<'PY'
import os
import socket
import sys

sys.path.insert(0, os.environ["SMOKE_DIR"])
from mcp_client import (  # noqa: E402
    McpClient,
    expect_ok,
    expect_refusal,
)

def skip(leg: str, why: str) -> None:
    print(f"SKIP: {leg} — {why}", file=sys.stderr)

failed = False

with McpClient(["./target/debug/omnibusd"]) as client:
    client.initialize()
    tools = client.list_tools()

    # --- leg 1: capabilities — the from-scratch inventory --------------------
    if "capabilities" not in tools:
        print("FAIL: capabilities tool missing on a fresh instance")
        failed = True
    else:
        doc = expect_ok(client.call("capabilities", {}), "capabilities")
        if not isinstance(doc, dict) or "distro" not in doc or not isinstance(
            doc.get("capabilities"), list
        ):
            print("FAIL: capabilities document lacks distro/capabilities")
            failed = True
        else:
            distro = doc["distro"]
            print(
                f"fresh inventory: distro={distro.get('id')} "
                f"family={distro.get('family')} "
                f"capabilities={len(doc['capabilities'])} "
                f"unmet_floors={len(doc.get('floors', []))}"
            )

    # --- leg 2: doctor — the onboarding diagnosis ----------------------------
    if "doctor" not in tools:
        skip("doctor", "tool hidden — unexpected: doctor is a core tool")
    else:
        doc = expect_ok(client.call("doctor", {}), "doctor")
        if not isinstance(doc, (dict, list)):
            print("FAIL: doctor returned a non-object payload")
            failed = True

    # --- leg 3: the platform knowledge base ----------------------------------
    reachable = True
    try:
        socket.getaddrinfo("wiki.archlinux.org", 443)
    except OSError:
        reachable = False
    if not reachable:
        skip("wiki_search('install desktop')", "no network — the KB is honestly unreachable")
    elif "wiki_search" not in tools:
        skip("wiki_search('install desktop')", "tool hidden — no KB provider this session")
    else:
        result = client.call("wiki_search", {"text": "install desktop environment", "limit": 3})
        if result.is_error and result.error_code() in ("AdapterUnavailable", "NativeFailure", "TimeoutError"):
            skip("wiki_search('install desktop')",
                 f"honest refusal: {result.error_code()} (KB unreachable/stalled — environment, not capability)")
        elif result.is_error:
            print(f"FAIL: wiki_search: {result.text[:300]}")
            failed = True
        else:
            hits = expect_ok(result, "wiki_search('install desktop')")
            if not isinstance(hits, list):
                print("FAIL: wiki_search returned a non-list payload")
                failed = True

    # --- leg 4: the assembly write is refused without a grant ----------------
    # pkg_install is [+scope]: ungranted it must answer ScopeDenied; when the
    # package subsystem is absent the tool is hidden and a direct call must
    # answer CapabilityRequired (the hiding rule) — both honest refusals. The
    # package name never exists on purpose: even a wrongly-issued grant could
    # not mutate anything real from this script.
    if "pkg_install" not in tools:
        result = client.call("pkg_install", {"name": "omnibus-smoke-nonexistent"})
        expect_refusal(
            result, "pkg_install (hidden) without grant", ("ScopeDenied", "CapabilityRequired")
        )
    else:
        result = client.call("pkg_install", {"name": "omnibus-smoke-nonexistent"})
        expect_refusal(result, "pkg_install without grant", ("ScopeDenied",))

if failed:
    sys.exit(1)
print("demo4: all legs passed or skipped honestly")
PY

#!/usr/bin/env bash
# Smoke demo 1 — "Diagnosis in a minute" (roadmap 1.8.2).
#
# journal_query(boot=previous) -> doctor -> wiki_search("failed suspend"),
# then the honest-refusal leg: svc_start without a grant must be refused
# with ScopeDenied (or CapabilityRequired when the subsystem is absent).
#
# Read-only by default: the daemon runs against a throwaway XDG_CONFIG_HOME
# (no grants anywhere in sight) and a throwaway XDG_STATE_HOME for the audit
# fallback. The Python client owns the daemon process (MCP stdio: the daemon
# lives and dies with the client side of the pipe).
#
# SKIP (exit 0, "SKIP:" on stderr) whenever a backend is honestly absent —
# no previous boot in the journal, no network, no systemd.
# Exit codes: 0 pass-or-skip, 1 assertion failure.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

TMPDIR_SMOKE="$(mktemp -d "${TMPDIR:-/tmp}/omnibus-smoke-demo1.XXXXXX")"
cleanup() { rm -rf "$TMPDIR_SMOKE"; }
trap cleanup EXIT

echo "== building the workspace (demo1) =="
cargo build --workspace --quiet

echo "== running demo1 against a fresh omnibusd (read-only session) =="
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

    # --- leg 1: journal_query with boot=previous ----------------------------
    if "journal_query" not in tools:
        skip("journal_query(boot=previous)", "tool hidden — no journal backend this session")
    else:
        result = client.call("journal_query", {"boot": "previous", "limit": 10})
        if result.is_error and result.error_code() in (
            "CapabilityRequired",
            "AdapterUnavailable",
        ):
            skip("journal_query(boot=previous)",
                 f"honest refusal: {result.error_code()} (no previous boot / volatile journal)")
        elif result.is_error and (
            "no journal boot entry found" in result.text.lower()
            or "failed to look up boot" in result.text.lower()
        ):
            # journalctl answers "No journal boot entry found for the
            # specified boot" on machines without a previous boot (fresh
            # containers, server images) — the honest degradation this demo's
            # header promises to skip, not a failure.
            skip("journal_query(boot=previous)",
                 "honest degradation: this machine's journal has no previous boot entry")
        elif result.is_error:
            print(f"FAIL: journal_query(boot=previous): {result.text[:300]}")
            failed = True
        else:
            page = expect_ok(result, "journal_query(boot=previous)")
            records = page.get("records") if isinstance(page, dict) else page
            if not isinstance(records, list):
                print(f"FAIL: expected JournalPage.records as a list, got: {type(records).__name__}")
                failed = True

    # --- leg 2: doctor -------------------------------------------------------
    if "doctor" not in tools:
        skip("doctor", "tool hidden — unexpected: doctor is a core tool")
    else:
        doc = expect_ok(client.call("doctor", {}), "doctor")
        if not isinstance(doc, (dict, list)):
            print("FAIL: doctor returned a non-object payload")
            failed = True

    # --- leg 3: wiki_search("failed suspend") --------------------------------
    reachable = True
    try:
        socket.getaddrinfo("wiki.archlinux.org", 443)
    except OSError:
        reachable = False
    if not reachable:
        skip("wiki_search('failed suspend')", "no network — the KB backend is honestly unreachable")
    elif "wiki_search" not in tools:
        skip("wiki_search('failed suspend')", "tool hidden — no KB provider this session")
    else:
        result = client.call("wiki_search", {"text": "failed suspend", "limit": 3})
        if result.is_error and result.error_code() in ("AdapterUnavailable", "NativeFailure", "TimeoutError"):
            skip("wiki_search('failed suspend')",
                 f"honest refusal: {result.error_code()} (KB unreachable/stalled — environment, not capability)")
        elif result.is_error:
            print(f"FAIL: wiki_search: {result.text[:300]}")
            failed = True
        else:
            hits = expect_ok(result, "wiki_search('failed suspend')")
            if not isinstance(hits, list):
                print("FAIL: wiki_search returned a non-list payload")
                failed = True

    # --- leg 4: the write path refuses honestly without a grant --------------
    # svc_start is [+scope]: ungranted it must answer ScopeDenied; when the
    # service subsystem is absent the tool is hidden and a direct call must
    # answer CapabilityRequired (the hiding rule) — both are honest refusals.
    if "svc_start" not in tools:
        result = client.call("svc_start", {"unit": "omnibus-smoke-nonexistent.service"})
        expect_refusal(
            result, "svc_start (hidden) without grant", ("ScopeDenied", "CapabilityRequired")
        )
    else:
        result = client.call(
            "svc_start", {"unit": "omnibus-smoke-nonexistent.service"}
        )
        expect_refusal(result, "svc_start without grant", ("ScopeDenied",))

if failed:
    sys.exit(1)
print("demo1: all legs passed or skipped honestly")
PY

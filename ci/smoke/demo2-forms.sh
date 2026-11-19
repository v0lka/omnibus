#!/usr/bin/env bash
# Smoke demo 2 — "Forms" (roadmap 1.8.2): fs_read -> desktop.tree
# -> set_value (only ever refused honestly in CI: no grant is issued).
#
# Read-only by default: the only file touched is a throwaway marker file the
# script itself seeds under $HOME/.cache/omnibus-smoke/ (inside the default
# fs read scope) and removes on exit. The daemon runs with a throwaway
# XDG_CONFIG_HOME (no grants) and XDG_STATE_HOME (audit fallback); the Python
# client owns the daemon process (MCP stdio pipe).
#
# SKIP (exit 0, "SKIP:" on stderr) when no a11y app is present in the session
# or the file subsystem is absent. Exit codes: 0 pass-or-skip, 1 failure.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

TMPDIR_SMOKE="$(mktemp -d "${TMPDIR:-/tmp}/omnibus-smoke-demo2.XXXXXX")"
SEED_DIR="$HOME/.cache/omnibus-smoke"
SEED_FILE="$SEED_DIR/demo2-forms-$$.txt"
MARKER="omnibus-smoke demo2 form field value ${RANDOM}"

cleanup() {
  rm -f "$SEED_FILE"
  rmdir "$SEED_DIR" 2>/dev/null || true
  rm -rf "$TMPDIR_SMOKE"
}
trap cleanup EXIT

echo "== building the workspace (demo2) =="
source "$REPO_ROOT/ci/smoke/build-workspace.sh"

echo "== seeding the demo file under \$HOME =="
mkdir -p "$SEED_DIR"
printf 'field: %s\n' "$MARKER" > "$SEED_FILE"

echo "== running demo2 against a fresh omnibusd (read-only session) =="
XDG_CONFIG_HOME="$TMPDIR_SMOKE/config" \
XDG_STATE_HOME="$TMPDIR_SMOKE/state" \
SMOKE_DIR="$REPO_ROOT/ci/smoke" \
SEED_FILE="$SEED_FILE" \
MARKER="$MARKER" \
python3 - <<'PY'
import base64
import os
import sys

sys.path.insert(0, os.environ["SMOKE_DIR"])
from mcp_client import (  # noqa: E402
    McpClient,
    capabilities_present,
    expect_ok,
    expect_refusal,
)

seed_file = os.environ["SEED_FILE"]
marker = os.environ["MARKER"]

def skip(leg: str, why: str) -> None:
    print(f"SKIP: {leg} — {why}", file=sys.stderr)

failed = False

with McpClient(["./target/debug/omnibusd"]) as client:
    client.initialize()
    tools = client.list_tools()

    # --- leg 1: fs_read of the seeded file -----------------------------------
    if "fs_read" not in tools:
        skip("fs_read", "tool hidden — no file scope backend this session")
    else:
        result = client.call("fs_read", {"path": seed_file})
        if result.is_error:
            print(f"FAIL: fs_read: {result.text[:300]}")
            failed = True
        else:
            content = expect_ok(result, "fs_read(seed)")
            text = content.get("text") or ""
            decoded = text
            if marker not in decoded:
                # data_base64 is always carried; decode defensively.
                decoded = base64.b64decode(content.get("data_base64", "")).decode(
                    "utf-8", "replace"
                )
            if marker not in decoded:
                print("FAIL: seeded marker not found in fs_read content")
                failed = True

    # --- leg 2: desktop.tree of an accessible app ----------------------------
    candidates = [
        a.strip()
        for a in os.environ.get(
            "OMNIBUS_SMOKE_A11Y_APPS",
            "gnome-shell,gnome-calculator,nautilus,org.gnome.Calculator,konsole,dolphin",
        ).split(",")
        if a.strip()
    ]
    a11y_ready = False
    if "desktop.tree" not in tools:
        skip("desktop.tree", "tool hidden — no a11y backend in this session")
    else:
        caps = expect_ok(client.call("capabilities", {}), "capabilities()")
        a11y_ready = capabilities_present(caps, "desktop.tree")
        if not a11y_ready:
            skip("desktop.tree", "desktop.tree capability not present in this session")
    if a11y_ready:
        tree_result = None
        found = None
        for app in candidates:
            attempt = client.call("desktop.tree", {"app": app})
            if not attempt.is_error:
                tree_result = attempt
                found = app
                break
            if attempt.error_code() not in ("AdapterUnavailable", "TimeoutError"):
                # CapabilityRequired-style answers cannot improve for later apps.
                break
        if tree_result is None:
            skip("desktop.tree", "no a11y-accessible app present in this session")
        else:
            expect_ok(tree_result, f"desktop.tree({found})")

    # --- leg 3: the write path refuses honestly without a grant --------------
    # set_value is [+scope]: ungranted it must answer ScopeDenied; when the a11y
    # write capability is absent the tool is hidden and a direct call must
    # answer CapabilityRequired (the hiding rule); when the platform sits
    # below the subsystem's version floor the tool is hidden as well and the
    # honest answer is FloorUnsupported (the R3 ladder; bookworm's at-spi2
    # 2.46 row) — all three are honest refusals.
    if "desktop.set_value" not in tools:
        result = client.call(
            "desktop.set_value", {"node_ref": "omnibus-smoke:none", "value": "x"}
        )
        expect_refusal(
            result,
            "desktop.set_value (hidden) without grant",
            ("ScopeDenied", "CapabilityRequired", "FloorUnsupported"),
        )
    else:
        result = client.call(
            "desktop.set_value", {"node_ref": "omnibus-smoke:none", "value": "x"}
        )
        expect_refusal(result, "desktop.set_value without grant", ("ScopeDenied",))

if failed:
    sys.exit(1)
print("demo2: all legs passed or skipped honestly")
PY

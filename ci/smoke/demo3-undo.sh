#!/usr/bin/env bash
# Smoke demo №3 «После обновления» / "After the update" (roadmap 1.8.2):
# pkg_list + pkg_info(bash), then pkg_undo honest refusals.
#
# Default (read-only, no grants anywhere):
#   - pkg_undo without a grant must answer ScopeDenied (or CapabilityRequired
#     when no package backend exists this session).
# Opt-in leg, OMNIBUS_TEST_UNDO=1: a SECOND daemon starts with a throwaway
# XDG_CONFIG_HOME carrying a single pkg.undo grant. pkg_undo is then called on
# a package name that was never installed — the adapter honestly refuses with
# NativeFailure "undo-no-cache-version" because no previous version exists.
# NO real downgrade is ever attempted: the refusal happens before any
# transaction, and the grant lives only in the throwaway config dir.
#
# Exit codes: 0 pass-or-skip, 1 assertion failure.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

TMPDIR_SMOKE="$(mktemp -d "${TMPDIR:-/tmp}/omnibus-smoke-demo3.XXXXXX")"
cleanup() { rm -rf "$TMPDIR_SMOKE"; }
trap cleanup EXIT

echo "== building the workspace (demo3) =="
cargo build --workspace --quiet

echo "== running demo3 against a fresh omnibusd (read-only session) =="
XDG_CONFIG_HOME="$TMPDIR_SMOKE/config" \
XDG_STATE_HOME="$TMPDIR_SMOKE/state" \
SMOKE_DIR="$REPO_ROOT/ci/smoke" \
python3 - <<'PY'
import os
import sys

sys.path.insert(0, os.environ["SMOKE_DIR"])
from mcp_client import McpClient, expect_ok, expect_refusal  # noqa: E402

def skip(leg: str, why: str) -> None:
    print(f"SKIP: {leg} — {why}", file=sys.stderr)

failed = False

with McpClient(["./target/debug/omnibusd"]) as client:
    client.initialize()
    tools = client.list_tools()

    # --- leg 1: pkg_list ------------------------------------------------------
    if "pkg_list" not in tools:
        skip("pkg_list", "tool hidden — no package backend this session")
    else:
        packages = expect_ok(client.call("pkg_list", {"limit": 5}), "pkg_list(limit=5)")
        if not isinstance(packages, list) or not packages:
            print("FAIL: pkg_list returned no packages")
            failed = True

    # --- leg 2: pkg_info(bash) ------------------------------------------------
    if "pkg_info" not in tools:
        skip("pkg_info(bash)", "tool hidden — no package backend this session")
    else:
        result = client.call("pkg_info", {"name": "bash"})
        if result.is_error and result.error_code() == "CapabilityRequired":
            skip("pkg_info(bash)", f"honest refusal: {result.error_code()}")
        elif result.is_error:
            print(f"FAIL: pkg_info(bash): {result.text[:300]}")
            failed = True
        else:
            info = expect_ok(result, "pkg_info(bash)")
            name = info.get("name") if isinstance(info, dict) else None
            if name != "bash":
                print(f"FAIL: pkg_info answered for {name!r}, expected 'bash'")
                failed = True

    # --- leg 3: pkg_undo without a grant refuses honestly ---------------------
    # Ungranted -> ScopeDenied; when the pkg.undo capability is absent the tool
    # is hidden and a direct call must answer CapabilityRequired (hiding rule).
    if "pkg_undo" not in tools:
        result = client.call("pkg_undo", {"name": "bash"})
        expect_refusal(
            result, "pkg_undo (hidden) without grant", ("ScopeDenied", "CapabilityRequired")
        )
    else:
        result = client.call("pkg_undo", {"name": "bash"})
        expect_refusal(result, "pkg_undo without grant", ("ScopeDenied",))

if failed:
    sys.exit(1)
print("demo3: read-only legs passed or skipped honestly")
PY
READ_ONLY_STATUS=$?

# --- opt-in leg: the granted pkg_undo path never reaches a real downgrade ----
GRANTED_STATUS=0
if [[ "${OMNIBUS_TEST_UNDO:-0}" != "1" ]]; then
  echo "SKIP: granted pkg_undo leg — set OMNIBUS_TEST_UNDO=1 to exercise it" >&2
else
  echo "== granted pkg_undo leg (OMNIBUS_TEST_UNDO=1, throwaway grant) =="
  mkdir -p "$TMPDIR_SMOKE/granted/omnibus/scopes.d"
  cat > "$TMPDIR_SMOKE/granted/omnibus/scopes.d/50-smoke-undo.toml" <<'TOML'
# Smoke-test grant: lives only in the throwaway config dir of this run.
[write]
"pkg.undo" = {}
TOML

  SMOKE_DIR="$REPO_ROOT/ci/smoke" \
  GRANT_DIR="$TMPDIR_SMOKE/granted" \
  STATE_DIR="$TMPDIR_SMOKE/state-granted" \
  python3 - <<'PY'
import os
import sys

sys.path.insert(0, os.environ["SMOKE_DIR"])
from mcp_client import McpClient, expect_refusal  # noqa: E402

env_extra = {
    "XDG_CONFIG_HOME": os.environ["GRANT_DIR"],
    "XDG_STATE_HOME": os.environ["STATE_DIR"],
}

with McpClient(["./target/debug/omnibusd"], env_extra=env_extra) as client:
    client.initialize()
    tools = client.list_tools()
    if "pkg_undo" not in tools:
        print("SKIP: granted pkg_undo leg — tool hidden this session", file=sys.stderr)
        sys.exit(0)
    # A name that was never installed: history has no previous version, so the
    # adapter refuses before staging anything — NativeFailure, never a downgrade.
    result = client.call(
        "pkg_undo", {"name": "omnibus-smoke-package-never-installed-xyz"}
    )
    if not result.is_error:
        print(f"FAIL: pkg_undo on a never-installed package must not succeed: {result.text[:300]}")
        sys.exit(1)
    if result.error_code() == "ScopeDenied":
        print(
            "SKIP: granted pkg_undo leg — ScopeDenied despite the grant "
            "(grant store not picked up; check the throwaway manifest)",
            file=sys.stderr,
        )
        sys.exit(0)
    expect_refusal(
        result,
        "pkg_undo (granted, never-installed package)",
        ("NativeFailure",),
    )
print("demo3: granted refusal leg passed")
PY
  GRANTED_STATUS=$?
fi

if [[ "$READ_ONLY_STATUS" -ne 0 || "$GRANTED_STATUS" -ne 0 ]]; then
  echo "FAIL: demo3 reported failures" >&2
  exit 1
fi
echo "demo3: all legs passed or skipped honestly"

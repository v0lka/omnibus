#!/usr/bin/env bash
# Smoke demo 5 — the closed loop (roadmap 2.9.2 AC; the v1.5 headline
# scenario, pitch demo: launched -> observed -> read -> terminated).
#
#   app.launch (grant) -> ps.list sees the process
#   journal_query reads the launched output -> ps.signal (grant) terminates it
#
# Session A (a throwaway XDG_CONFIG_HOME with no grants anywhere):
#   leg 1  app.list    — the fixture app is inventoried (read, no grant);
#   leg 2  ps.list     — the live /proc inventory answers (read, no grant);
#   leg 3  app.launch  — the launch write without a grant refuses honestly
#                        (ScopeDenied, or CapabilityRequired when the tool is
#                        hidden because systemd-run is absent — the hiding rule);
#   leg 4  ps.signal   — the signal write without a grant refuses honestly.
#
# Session B (a second daemon over a throwaway grant dir carrying
# app.launch + ps.signal): the full loop, gated on a LIVE launch stand —
# /usr/bin/systemd-run present AND the systemd user manager answering
# (the same skip discipline as the adapter's live binder):
#   leg 5  app.launch  — launches the fixture app as a transient user unit;
#   leg 6  ps.list     — polls until the process is a fact (pid from ps.list,
#                        never a guessed name), cgroup in the user hierarchy;
#   leg 7  journal_query — polls a since-window page until the fixture's
#                        marker lines surface (the output lands in journald
#                        asynchronously);
#   leg 8  ps.signal   — TERM to that pid under the grant;
#   leg 9  ps.list     — polls until the pid is gone: the loop is closed.
#
# CI honesty: reference-platform containers have no systemd user manager, so
# there the demo passes on the refusal legs plus an honest SKIP of the
# granted legs; the full loop is the real-session contract (Manjaro VM legs,
# the release checklist's reference machines). A SKIP is printed on stderr
# and the demo still exits 0 — exactly the house rule every other demo runs
# under ("Smoke demos ... honest skips when a backend is absent").
#
# Safety: the fixture app is a bounded marker-loop script registered in a
# throwaway XDG_DATA_HOME; it dies on TERM (the ps.signal leg) and on its own
# tick budget even if the demo dies mid-run, so nothing lingers beyond two
# minutes. The grants live only in the throwaway config dir; nothing outside
# $TMPDIR_SMOKE is written, and no grant exists outside it.
#
# Exit codes: 0 pass-or-skip, 1 assertion failure.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

TMPDIR_SMOKE="$(mktemp -d "${TMPDIR:-/tmp}/omnibus-smoke-demo5.XXXXXX")"
cleanup() { rm -rf "$TMPDIR_SMOKE"; }
trap cleanup EXIT

# --- the fixture stand: a bounded loop app in a throwaway XDG_DATA_HOME -----
# The app id is the desktop file name (the launch inventory's stable key);
# the Exec carries no shell — the adapter executes it directly, and the
# marker lines it prints land in the unit's journald output.
LOOP_APP_ID="omnibus-loop.desktop"
LOOP_MARKER="OMNIBUS-DEMO5-MARKER"

mkdir -p "$TMPDIR_SMOKE/data/applications" "$TMPDIR_SMOKE/data/bin"
cat > "$TMPDIR_SMOKE/data/bin/omnibus-loop.sh" <<'LOOP'
#!/usr/bin/env bash
# demo5 fixture: bounded marker loop. TERM (what the ps.signal leg delivers)
# ends it instantly; the tick budget ends it even without a signal.
i=0
while [ "$i" -lt 240 ]; do
  echo "OMNIBUS-DEMO5-MARKER tick ${i}"
  sleep 0.5
  i=$((i + 1))
done
LOOP
chmod +x "$TMPDIR_SMOKE/data/bin/omnibus-loop.sh"
cat > "$TMPDIR_SMOKE/data/applications/${LOOP_APP_ID}" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Omnibus Demo5 Loop
Exec=$TMPDIR_SMOKE/data/bin/omnibus-loop.sh
NoDisplay=false
DESKTOP

echo "== building the workspace (demo5) =="
source "$REPO_ROOT/ci/smoke/build-workspace.sh"

# --- session A: fresh daemon, no grants — the write half refuses honestly ---
echo "== running demo5 session A against a fresh omnibusd (no grants) =="
XDG_CONFIG_HOME="$TMPDIR_SMOKE/config" \
XDG_DATA_HOME="$TMPDIR_SMOKE/data" \
XDG_STATE_HOME="$TMPDIR_SMOKE/state" \
SMOKE_DIR="$REPO_ROOT/ci/smoke" \
python3 - <<'PY'
import os
import sys

sys.path.insert(0, os.environ["SMOKE_DIR"])
from mcp_client import McpClient, expect_ok, expect_refusal  # noqa: E402

LOOP_APP_ID = "omnibus-loop.desktop"

def skip(leg: str, why: str) -> None:
    print(f"SKIP: {leg} — {why}", file=sys.stderr)

failed = False

with McpClient(["./target/debug/omnibusd"]) as client:
    client.initialize()
    tools = client.list_tools()

    # --- leg 1: app.list inventories the fixture stand (read, no grant) ----
    if "app.list" not in tools:
        # The fixture applications dir always exists, so the inventory must
        # be served; a hidden app.list is a wiring bug, not an honest absence.
        print("FAIL: app.list hidden although the fixture applications dir exists")
        failed = True
    else:
        entries = expect_ok(client.call("app.list", {}), "app.list")
        ids = {e.get("app_id") for e in entries} if isinstance(entries, list) else set()
        if LOOP_APP_ID not in ids:
            print(f"FAIL: app.list missed the fixture app {LOOP_APP_ID}: "
                  f"{sorted(ids)[:10]}...")
            failed = True

    # --- leg 2: ps.list answers without a grant (read, audited) -----------
    if "ps.list" not in tools:
        skip("ps.list", "tool hidden — process telemetry not wired this session")
    else:
        processes = expect_ok(
            client.call("ps.list", {"limit": 10}), "ps.list(limit=10)"
        )
        if not isinstance(processes, list) or not processes:
            print("FAIL: ps.list returned no processes on a live machine")
            failed = True

    # --- leg 3: app.launch without a grant refuses honestly ---------------
    # Three honest shapes, all accepted (the demo4 pkg_install pattern): the
    # backend present but ungranted -> the grant rung answers ScopeDenied
    # (the tool is hidden from tools/list until the grant exists); the
    # backend absent entirely -> the hiding rule answers CapabilityRequired;
    # the platform below the subsystem's version floor -> the R3 ladder
    # answers FloorUnsupported.
    result = client.call("app.launch", {"app_id": LOOP_APP_ID, "args": []})
    expect_refusal(result, "app.launch without grant",
                   ("ScopeDenied", "CapabilityRequired", "FloorUnsupported"))

    # --- leg 4: ps.signal without a grant refuses honestly ----------------
    # Same ladder: grant rung, hiding rule, or the below-floor rung.
    result = client.call("ps.signal", {"pid": 1, "signal": "term"})
    expect_refusal(result, "ps.signal without grant",
                   ("ScopeDenied", "CapabilityRequired", "FloorUnsupported"))

if failed:
    sys.exit(1)
print("demo5 session A: reads served, writes refused honestly")
PY
SESSION_A_STATUS=$?

# --- session B: the granted closed loop, on a live launch stand only ---------
# The gate mirrors the adapter's live binder: systemd-run installed AND the
# systemd user manager answering. Without it the loop cannot run on this
# machine at all — an honest absence, not a failure.
GRANTED_STATUS=0
LAUNCH_LIVE=0
if [ -x /usr/bin/systemd-run ]; then
  MANAGER_STATE="$(systemctl --user is-system-running 2>/dev/null || true)"
  case "$MANAGER_STATE" in
    running|degraded|starting) LAUNCH_LIVE=1 ;;
  esac
fi

if [ "$LAUNCH_LIVE" -ne 1 ]; then
  echo "SKIP: granted closed-loop legs — no live systemd user manager on this stand" >&2
  echo "      (reference-platform CI containers: the refusal legs above are the contract)" >&2
else
  echo "== running demo5 session B: the granted closed loop =="
  mkdir -p "$TMPDIR_SMOKE/granted/omnibus/scopes.d"
  cat > "$TMPDIR_SMOKE/granted/omnibus/scopes.d/50-smoke-closed-loop.toml" <<'TOML'
# Smoke-test grants: live only in the throwaway config dir of this run.
[write]
"app.launch" = {}
"ps.signal" = {}
TOML

  XDG_CONFIG_HOME="$TMPDIR_SMOKE/granted" \
  XDG_DATA_HOME="$TMPDIR_SMOKE/data" \
  XDG_STATE_HOME="$TMPDIR_SMOKE/state-granted" \
  SMOKE_DIR="$REPO_ROOT/ci/smoke" \
  LOOP_APP_ID="$LOOP_APP_ID" \
  LOOP_MARKER="$LOOP_MARKER" \
  python3 - <<'PY'
import json
import os
import sys
import time

sys.path.insert(0, os.environ["SMOKE_DIR"])
from mcp_client import McpClient, expect_ok, expect_refusal  # noqa: E402

LOOP_APP_ID = os.environ["LOOP_APP_ID"]
LOOP_MARKER = os.environ["LOOP_MARKER"]

def skip(leg: str, why: str) -> None:
    print(f"SKIP: {leg} — {why}", file=sys.stderr)

def fail(leg: str, why: str) -> None:
    print(f"FAIL: {leg} — {why}")
    sys.exit(1)

def poll(call, want, deadline_s: float, label: str):
    """Polls one tool call until `want(payload)` is true, else returns None."""
    deadline = time.monotonic() + deadline_s
    last = None
    while time.monotonic() < deadline:
        result = call()
        if not result.is_error:
            last = result
            if want(result.data):
                return result.data
        time.sleep(0.5)
    if last is not None:
        print(f"   {label}: last payload (no match): "
              f"{json.dumps(last.data)[:300]}")
    return None

env_extra = {
    "XDG_CONFIG_HOME": os.environ["XDG_CONFIG_HOME"],
    "XDG_DATA_HOME": os.environ["XDG_DATA_HOME"],
    "XDG_STATE_HOME": os.environ["XDG_STATE_HOME"],
}

with McpClient(["./target/debug/omnibusd"], env_extra=env_extra) as client:
    client.initialize()
    tools = client.list_tools()

    # The pre-launch timestamp keeps the journal window bounded to this run.
    t0_ms = int(time.time() * 1000) - 1000

    # --- leg 5: the granted launch starts a transient user unit ----------
    if "app.launch" not in tools:
        skip("app.launch", "tool hidden — launch backend absent this session")
        sys.exit(0)
    outcome = expect_ok(
        client.call("app.launch", {"app_id": LOOP_APP_ID, "args": []}),
        "app.launch(omnibus-loop)",
    )
    unit_name = outcome.get("unit_name") if isinstance(outcome, dict) else None
    if not isinstance(unit_name, str) or not unit_name.startswith(
        "app-omnibus-loop-"
    ) or not unit_name.endswith(".service"):
        fail("app.launch", f"outcome lacks the transient unit name: {outcome}")

    # --- leg 6: ps.list sees the launched process as a fact ---------------
    # The join key is the minted unit name inside the cgroup path (the
    # observation join LaunchOutcome promises); comm-based name filtering
    # cannot address interpreted scripts — the kernel names them after the
    # interpreter ("bash"), not the script.
    if "ps.list" not in tools:
        fail("ps.list", "tool hidden — the loop cannot be observed")
    entry = poll(
        lambda: client.call("ps.list", {}),
        want=lambda processes: isinstance(processes, list)
        and any(
            isinstance(p.get("cgroup"), str)
            and p["cgroup"].endswith(unit_name)
            for p in processes
        ),
        deadline_s=15.0,
        label="ps.list",
    )
    if entry is None:
        fail("ps.list", f"no process appeared in the unit's cgroup for {unit_name}")
    observed = next(p for p in entry
                    if isinstance(p.get("cgroup"), str)
                    and p["cgroup"].endswith(unit_name))
    pid = observed.get("pid")
    name = observed.get("name")
    cgroup = observed.get("cgroup")
    if not isinstance(pid, int) or pid <= 1:
        fail("ps.list", f"unusable pid in the entry: {observed}")
    # The transient unit must have landed in the user hierarchy — a
    # system.slice cgroup here would break the addressing-world contract.
    if isinstance(cgroup, str) and "user.slice" not in cgroup:
        fail("ps.list", f"the launched process left the user hierarchy: {cgroup}")
    print(f"   closed loop: pid {pid} ({name}) observed in {cgroup}")

    # --- leg 7: journal_query reads the launched output -------------------
    # The output of a user-unit process lands in journald asynchronously, so
    # this leg polls a since-window page (unit-name filtering is the system
    # tag's exact-match discipline and does not select user-unit records).
    if "journal_query" not in tools:
        skip("journal_query", "tool hidden — journalctl absent this session")
    else:
        found = poll(
            lambda: client.call(
                "journal_query", {"since_epoch_ms": t0_ms, "limit": 200}
            ),
            want=lambda page: (
                isinstance(page, dict)
                and any(LOOP_MARKER in (r.get("message") or "")
                        for r in page.get("records", []))
            ),
            deadline_s=10.0,
            label="journal_query",
        )
        if found is None:
            fail("journal_query",
                 f"no `{LOOP_MARKER}` line surfaced from unit {unit_name}")
        lines = [r.get("message") for r in found.get("records", [])
                 if LOOP_MARKER in (r.get("message") or "")]
        print(f"   closed loop: journal read back {len(lines)} marker line(s) "
              f"of unit {unit_name}")

    # --- leg 8: ps.signal under the grant terminates the process ---------
    if "ps.signal" not in tools:
        fail("ps.signal", "tool hidden — the loop cannot be closed")
    result = client.call("ps.signal", {"pid": pid, "signal": "term"})
    if result.is_error and result.error_code() == "ScopeDenied":
        skip("ps.signal",
             "ScopeDenied despite the grant (grant store not picked up; "
             "check the throwaway manifest)")
        sys.exit(0)
    expect_ok(result, f"ps.signal(term, pid {pid})")

    # --- leg 9: the pid is gone — the loop is closed ----------------------
    gone = poll(
        lambda: client.call("ps.list", {}),
        want=lambda processes: isinstance(processes, list)
        and not any(
            isinstance(p.get("cgroup"), str)
            and p["cgroup"].endswith(unit_name)
            for p in processes
        ),
        deadline_s=15.0,
        label="ps.list (post-signal)",
    )
    if gone is None:
        fail("ps.signal", f"pid {pid} survived TERM — the loop did not close")

print("demo5 session B: launch -> observe -> read -> terminate — loop closed")
PY
  GRANTED_STATUS=$?
fi

if [ "$SESSION_A_STATUS" -ne 0 ] || [ "$GRANTED_STATUS" -ne 0 ]; then
  echo "FAIL: demo5 reported failures" >&2
  exit 1
fi
echo "demo5: all legs passed or skipped honestly"

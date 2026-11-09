#![allow(clippy::expect_used)] // test-only file: tests may expect; the seam must not
//! Binder for the `ProcessTelemetry` contract suite (R2) against two honest
//! stands:
//!
//! - the **live stand** — this machine's real `/proc`: the fixture-free suite
//!   runs against it, and the with-fixture gates bind a process the binder
//!   itself spawned (a `sleep` child; `ps.signal` TERM must actually end it —
//!   the integration proof that a granted signal reaches a test process,
//!   roadmap 2.8.3 AC);
//! - the **synthetic stand** — a fixture `/proc` tree in a temp directory with
//!   300+ processes, PID 1, a system-unit process and a user transient-unit
//!   process, so the ACs that a host cannot promise deterministically are
//!   gated for real: the 300+-process inventory inside the read budget, the
//!   PID 1 / systemd-unit exclusion, the filters and limits traveling in the
//!   DTO, and the numeric-signal rejection at the seam.
//!
//! Skips are honest by construction: the with-fixture live arm runs only when
//! the spawned child is signalable from this daemon's view (its cgroup shows
//! it outside the system-manager hierarchy); a system-service CI runner skips
//! with a note instead of failing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use omnibus_adapter_proc::ProcAdapter;
use omnibus_contracts::dto::process_telemetry::{ProcessListQuery, Signal, SignalRequest};
use omnibus_contracts::spi::process_telemetry::ProcessTelemetry;
use omnibus_contracts::suite::process_telemetry::{
    ProcessTelemetryFixture, SENTINEL_PID_MISSING, SENTINEL_PID_SLOW,
};
use omnibus_contracts::suite::process_telemetry::{
    process_telemetry_suite, process_telemetry_suite_with_fixture,
};

/// The core's read budget (limits.rs `TOOL_CALL_BUDGET`): the wall clock the
/// 300+-process inventory must stay inside (roadmap 2.8.2 AC).
const READ_BUDGET: Duration = Duration::from_secs(10);
/// How many synthetic processes the stand carries: comfortably above the
/// 300+ the AC demands.
const STAND_SIZE: u32 = 340;

/// A synthetic `/proc/<pid>` entry. `stat` fields: state=S, utime=5, stime=3,
/// starttime=`start_ticks`, per the parser's post-paren indices (11, 12, 19).
struct StandProc {
    pid: u32,
    comm: &'static str,
    cgroup: &'static str,
    cmdline: Option<&'static str>,
    start_ticks: u64,
}

/// Writes one synthetic process directory.
fn write_proc_dir(root: &Path, proc_entry: &StandProc) {
    let dir = root.join(proc_entry.pid.to_string());
    std::fs::create_dir_all(&dir).expect("stand pid dir");
    let stat = format!(
        "{pid} ({comm}) S 1 1 1 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 {start} 2000000 150 18446744073709551615 1 1 0 0 0 0 65536 0 0 0 0 17 4 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        pid = proc_entry.pid,
        comm = proc_entry.comm,
        start = proc_entry.start_ticks
    );
    std::fs::write(dir.join("stat"), stat).expect("stand stat");
    // 300 resident pages — the RSS arithmetic scales by the page size.
    std::fs::write(dir.join("statm"), "400 300 120 5 0 100 0\n").expect("stand statm");
    std::fs::write(dir.join("cgroup"), format!("0::{}\n", proc_entry.cgroup))
        .expect("stand cgroup");
    if let Some(cmdline) = proc_entry.cmdline {
        std::fs::write(dir.join("cmdline"), format!("{cmdline}\0")).expect("stand cmdline");
    }
}

/// Builds the synthetic stand: the system files, PID 1, a system-unit
/// process, a user transient-unit process, and `count` plain user processes.
/// Returns (keep-alive dir, the user transient unit's pid).
fn fixture_stand(count: u32) -> (tempfile::TempDir, u32) {
    let dir = tempfile::tempdir().expect("stand tempdir");
    let root = dir.path();
    // /proc/stat: aggregate cpu line + btime (the snapshot epoch source).
    std::fs::write(
        root.join("stat"),
        "cpu  100 0 50 900 10 0 20 0 0 0\ncpu0 100 0 50 900 10 0 20 0 0 0\nbtime 1700000000\n",
    )
    .expect("stand stat");
    std::fs::write(root.join("uptime"), "1000.50 0.00\n").expect("stand uptime");
    std::fs::write(root.join("loadavg"), "0.42 0.35 0.30 2/500 4242\n").expect("stand loadavg");
    std::fs::write(
        root.join("meminfo"),
        "MemTotal:       8000000 kB\nMemFree:        3000000 kB\nMemAvailable:   5000000 kB\nBuffers:         200000 kB\nCached:         1800000 kB\nSwapTotal:      2000000 kB\nSwapFree:       1900000 kB\n",
    )
    .expect("stand meminfo");
    // PID 1 itself (excluded by the domain invariant), a system service
    // (excluded — services go through svc_*), and the user's transient app
    // unit (stays listed — the launch → observe closed loop).
    let fixed = [
        StandProc {
            pid: 1,
            comm: "systemd",
            cgroup: "/init.scope",
            cmdline: Some("/sbin/init splash"),
            start_ticks: 0,
        },
        StandProc {
            pid: 777,
            comm: "nginx",
            cgroup: "/system.slice/nginx.service",
            cmdline: Some("nginx: master process"),
            start_ticks: 900,
        },
        StandProc {
            pid: 4242,
            comm: "suite-echo",
            cgroup: "/user.slice/user-1000.slice/user@1000.service/app.slice/app-suite.service",
            cmdline: Some("suite-echo --idle"),
            start_ticks: 950,
        },
    ];
    for proc_entry in &fixed {
        write_proc_dir(root, proc_entry);
    }
    for i in 0..count {
        write_proc_dir(
            root,
            &StandProc {
                pid: 10_000 + i,
                comm: "plainproc",
                cgroup: "/user.slice/user-1000.slice/user@1000.service/session-1.scope",
                cmdline: None,
                start_ticks: 100 + u64::from(i),
            },
        );
    }
    (dir, 4242)
}

/// Whether the live stand may run the with-fixture gates: the spawned child
/// must sit outside the system-manager hierarchy (a system-service CI runner
/// would make even our own child unaddressable — honest skip, not failure).
fn live_child_is_signalable(pid: u32) -> bool {
    let Some(text) = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok() else {
        return true; // no cgroup view at all: nothing suggests exclusion
    };
    let top = text
        .lines()
        .find_map(|line| line.split_once(':'))
        .and_then(|(_, rest)| rest.split_once(':'))
        .map(|(_, path)| path.trim_start_matches('/'))
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("");
    !(top == "system.slice" || top == "init.scope")
}

#[tokio::test(flavor = "multi_thread")]
async fn fixture_free_suite_is_green_on_the_live_proc_stand() {
    // upower off: the power arm answers CapabilityRequired — the honest shape
    // the suite accepts regardless of what this particular machine has.
    let adapter = ProcAdapter::from_probe(false);
    let report =
        process_telemetry_suite(move || Box::new(adapter.clone()) as Box<dyn ProcessTelemetry>)
            .await;
    assert!(report.is_clean(), "fixture-free suite: {report}");
}

/// The integration stand: a process we spawned is listed, snapshotted and
/// actually terminated by the granted `ps.signal` TERM (roadmap 2.8.3 AC:
/// "with the grant, the signal reaches the test process").
#[tokio::test(flavor = "multi_thread")]
async fn fixture_suite_signals_a_real_test_process() {
    let child = tokio::process::Command::new("/usr/bin/sleep")
        .args(["30"])
        .spawn()
        .expect("spawn the suite sleeper");
    let known_pid = child.id().expect("sleeper pid");
    if !live_child_is_signalable(known_pid) {
        eprintln!(
            "note: the suite sleeper sits in the system-manager cgroup hierarchy; \
             the with-fixture live arm is skipped (honest skip, not a failure)"
        );
        return;
    }
    let adapter = ProcAdapter::from_probe(false);
    let report = process_telemetry_suite_with_fixture(
        move || Box::new(adapter.clone()) as Box<dyn ProcessTelemetry>,
        ProcessTelemetryFixture { known_pid },
    )
    .await;
    assert!(report.is_clean(), "with-fixture suite: {report}");
    // The delivery proof: the TERM really ended the child (the suite only
    // proves kill(2) returned success; this closes the loop on the outcome).
    let mut child = child;
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("the signalled sleeper exits promptly")
        .expect("wait succeeds");
    assert!(
        !status.success(),
        "TERM ended the sleeper (signal exit, not a clean zero)"
    );
}

/// Roadmap 2.8.2 AC: `ps.list` over 300+ processes lands inside the read
/// budget, the inventory excludes PID 1 and system-unit processes, and the
/// user's transient-unit process stays observable (the launch → observe
/// closed loop).
#[tokio::test(flavor = "multi_thread")]
async fn stand_lists_300_processes_inside_the_budget_with_exclusions() {
    let (stand, transient_pid) = fixture_stand(STAND_SIZE);
    let adapter = ProcAdapter::at(
        stand.path().to_path_buf(),
        PathBuf::from("/nonexistent-sys"),
        false,
    );
    let started = Instant::now();
    let entries = adapter
        .list(ProcessListQuery {
            name_contains: None,
            limit: None,
        })
        .await
        .expect("stand list");
    let elapsed = started.elapsed();
    assert!(
        elapsed < READ_BUDGET,
        "{STAND_SIZE} synthetic processes inventoried in {elapsed:?}, over budget"
    );
    assert!(
        entries.len() >= 300,
        "the AC demands 300+ processes: got {}",
        entries.len()
    );
    // Exclusions: PID 1 and the system-unit process never appear; the user
    // transient unit does.
    assert!(!entries.iter().any(|e| e.pid == 1), "PID 1 stays outside");
    assert!(
        !entries.iter().any(|e| e.pid == 777),
        "system-unit processes stay outside"
    );
    assert!(
        entries.iter().any(|e| e.pid == transient_pid),
        "the user transient-unit process stays listed (launch → observe loop)"
    );
    // Every entry carries the full DTO fact set.
    for entry in &entries {
        assert!(entry.pid > 1);
        assert!(!entry.name.is_empty());
        assert!(!entry.state.is_empty());
        assert!(entry.rss_bytes.0 > 0, "RSS is a real fact, not a zero");
    }
    // The transient unit's snapshot is consistent, with its cmdline.
    let snapshot = adapter.info(transient_pid).await.expect("stand info");
    assert_eq!(snapshot.pid, transient_pid);
    assert_eq!(snapshot.name, "suite-echo");
    assert_eq!(
        snapshot.command_line.as_deref(),
        Some("suite-echo --idle"),
        "cmdline joins NUL-separated argv with spaces"
    );
    assert!(
        snapshot.start_time_epoch_ms.0 > 0,
        "btime + starttime reconstruct the epoch start"
    );
    // PID 1 and the system unit refuse info with the svc hint.
    for refused in [1u32, 777] {
        let err = adapter
            .info(refused)
            .await
            .expect_err("excluded pids refuse info");
        let omnibus_contracts::errors::ContractError::Unsupported { reason } = err else {
            panic!("expected Unsupported for pid {refused}, got {err:?}");
        };
        assert!(reason.contains("svc_"), "{reason}");
    }
}

/// The DTO's filters and limits do the `journal_query` discipline: they travel
/// in the query, and the adapter honors both.
#[tokio::test(flavor = "multi_thread")]
async fn stand_filters_and_limits_travel_in_the_dto() {
    let (stand, _transient) = fixture_stand(40);
    let adapter = ProcAdapter::at(
        stand.path().to_path_buf(),
        PathBuf::from("/nonexistent-sys"),
        false,
    );
    let filtered = adapter
        .list(ProcessListQuery {
            name_contains: Some(String::from("suite-echo")),
            limit: None,
        })
        .await
        .expect("filtered list");
    assert_eq!(filtered.len(), 1, "only the named process matches");
    assert_eq!(filtered[0].pid, 4242);
    let limited = adapter
        .list(ProcessListQuery {
            name_contains: Some(String::from("plainproc")),
            limit: Some(7),
        })
        .await
        .expect("limited list");
    assert_eq!(limited.len(), 7, "the DTO limit is honored exactly");
}

/// `sys.stats` on the stand: finite load averages, total ≥ used, swap math,
/// uptime carried; on the static fixture stat the honest sample answers a
/// bounded CPU percent.
#[tokio::test(flavor = "multi_thread")]
async fn stand_sys_stats_are_sane() {
    let (stand, _transient) = fixture_stand(5);
    let adapter = ProcAdapter::at(
        stand.path().to_path_buf(),
        PathBuf::from("/nonexistent-sys"),
        false,
    );
    let stats = adapter.sys_stats().await.expect("stand sys_stats");
    assert!(stats.load_avg_1m.is_finite());
    assert_eq!(stats.load_avg_1m, 0.42);
    assert!(stats.mem_total_bytes.0 >= stats.mem_used_bytes.0);
    assert_eq!(stats.mem_total_bytes.0, 8_000_000 * 1_024);
    assert_eq!(stats.swap_used_bytes.0, 100_000 * 1_024);
    assert_eq!(stats.uptime_seconds.0, 1_000);
    assert!(stats.cpu_percent.0 <= 100);
}

/// Roadmap 2.8.3 AC, seam half: an arbitrary numeric signal cannot cross the
/// seam at all — the closed enum is the only representable shape. The parse
/// direction is the proof (the core-level MCP rejection has its own test).
#[test]
fn a_numeric_signal_is_unrepresentable_at_the_seam() {
    let numeric = serde_json::json!({ "pid": 4321, "signal": 9 });
    let parsed = serde_json::from_value::<SignalRequest>(numeric);
    assert!(
        parsed.is_err(),
        "a raw signal number must not parse into SignalRequest"
    );
    let off_registry = serde_json::json!({ "pid": 4321, "signal": "sigkill" });
    assert!(
        serde_json::from_value::<SignalRequest>(off_registry).is_err(),
        "an off-registry signal name must not parse"
    );
    // The six wire forms parse — the closed set, exactly.
    for wire in ["term", "int", "hup", "usr1", "usr2", "kill"] {
        let request = serde_json::json!({ "pid": 4321, "signal": wire });
        assert!(
            serde_json::from_value::<SignalRequest>(request).is_ok(),
            "wire form {wire} is representable"
        );
    }
    let _ = Signal::Kill; // keep the import honest when only wire strings assert
}

/// The suite's missing-pid sentinel answers the typed failure on the live
/// stand: a nonexistent pid is a contract error, never a panic (2.8.1 AC,
/// re-proven here so the binder pins its own stand).
#[tokio::test(flavor = "multi_thread")]
async fn nonexistent_pid_is_a_contract_error_on_the_live_stand() {
    let adapter = ProcAdapter::from_probe(false);
    let err = adapter
        .info(SENTINEL_PID_MISSING)
        .await
        .expect_err("no pid 2000000000 exists");
    assert!(matches!(
        err,
        omnibus_contracts::errors::ContractError::NativeFailure { .. }
    ));
    let slow = adapter
        .signal(SignalRequest {
            pid: SENTINEL_PID_SLOW,
            signal: Signal::Term,
        })
        .await;
    assert!(
        matches!(
            slow,
            Err(omnibus_contracts::errors::ContractError::TimeoutError)
        ),
        "the slow sentinel binding answers TimeoutError"
    );
}

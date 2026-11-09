#![allow(clippy::expect_used)] // test-only file: tests may expect; the seam must not
//! Binder for the AppLauncher contract suite (R2) against a live stand. The
//! stand is honest by construction: a synthetic `.desktop` fixture in a
//! temp XDG data directory (no reliance on what the host happens to have
//! installed) plus this machine's systemd user manager for the launch arm.
//!
//! Every leg decides its flag before it runs:
//! - the launch arm runs only when `/usr/bin/systemd-run` exists AND the
//!   user manager answers `is-system-running`-shaped liveness; a headless
//!   or non-systemd box skips with a note instead of failing (the same
//!   skip discipline as the portals binder);
//! - the open leg of the fixture suite stays unconfigured on purpose: a
//!   real `xdg-open` pops a GUI window on the developer's desktop, which is
//!   not a test the suite may perform — the open surface is proven by the
//!   sentinel bindings (adapter + suite) and the core's hidden-tool gates;
//! - the metacharacter leg is the closed-loop proof of the domain
//!   invariant: the fixture app's arguments (`;`, `|`, backtick inside
//!   them) are read back verbatim from the launched unit's journald output —
//!   a shell anywhere in the path would have split, piped or substituted
//!   them instead.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use omnibus_adapter_launch::LaunchAdapter;
use omnibus_contracts::dto::app_launcher::AppSpec;
use omnibus_contracts::spi::app_launcher::AppLauncher as _;
use omnibus_contracts::suite::app_launcher::{
    AppLauncherFixture, app_launcher_suite, app_launcher_suite_with_fixture,
};

/// The fixture application inventory: one launchable app plus one
/// `NoDisplay` entry, so `list` proves it serves the full DTO fact set.
const SUITE_DESKTOP: &str = "[Desktop Entry]\n\
     Type=Application\n\
     Name=Omnibus Suite Echo\n\
     Icon=utilities-terminal\n\
     Exec=/usr/bin/echo\n\
     NoDisplay=false\n";
const HIDDEN_DESKTOP: &str = "[Desktop Entry]\n\
     Type=Application\n\
     Name=Omnibus Suite Hidden\n\
     Exec=/usr/bin/true\n\
     NoDisplay=true\n";
/// The desktop-file id the fixture suite gates the known app on.
const KNOWN_APP: &str = "omnibus-suite-echo.desktop";

/// The arguments whose verbatim arrival in journald proves no shell ever
/// interpreted them (roadmap 2.7.2 AC: `;`, `|`, backtick are data).
const METACHAR_ARGS: [&str; 3] = ["arg;one", "arg|two", "arg`three"];
/// The exact line `/usr/bin/echo` must have written to the unit's stdout.
fn metachar_line() -> String {
    METACHAR_ARGS.join(" ")
}

/// Builds the fixture stand: a temp dir with the two desktop entries. The
/// `TempDir` must outlive every adapter call, so it is returned alongside.
fn fixture_stand() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("fixture tempdir");
    let applications = dir.path().join("applications");
    std::fs::create_dir_all(&applications).expect("fixture applications dir");
    std::fs::write(applications.join(KNOWN_APP), SUITE_DESKTOP).expect("suite desktop entry");
    std::fs::write(
        applications.join("omnibus-suite-hidden.desktop"),
        HIDDEN_DESKTOP,
    )
    .expect("hidden desktop entry");
    (dir, applications)
}

/// Runs one child to completion with a budget, returning its stdout (None on
/// any failure or elapse). Direct execve, argument list, kill_on_drop — the
/// binder never shells out either.
async fn probe_command(program: &str, args: &[&str], budget: Duration) -> Option<String> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let out = tokio::time::timeout(budget, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The launch backend answer for this machine: `systemd-run` installed AND
/// the user manager alive (a `--user` transient unit needs both).
async fn launch_backend_live() -> bool {
    if !std::path::Path::new("/usr/bin/systemd-run").exists() {
        eprintln!("note: /usr/bin/systemd-run is not installed; the launch arm is skipped");
        return false;
    }
    let state = probe_command(
        "/usr/bin/systemctl",
        &["--user", "is-system-running"],
        Duration::from_secs(3),
    )
    .await
    .map(|out| out.trim().to_string());
    match state.as_deref() {
        // `is-system-running` answers `degraded` for a healthy manager with
        // one failed unit — still a live user manager (the probe doctrine:
        // state is not presence).
        Some("running") | Some("degraded") | Some("starting") => true,
        Some(other) => {
            eprintln!("note: the systemd user manager answers {other}; the launch arm is skipped");
            false
        }
        None => {
            eprintln!("note: no reachable systemd user manager; the launch arm is skipped");
            false
        }
    }
}

/// The metacharacter closed loop: launch the fixture app with `;`, `|` and
/// backtick inside its typed arguments, then read the transient unit's
/// journald output back and demand the exact bytes. Runs only on a live
/// stand; polls because the unit's output lands in the journal asynchronously.
async fn metacharacters_arrive_verbatim(adapter: &LaunchAdapter) -> bool {
    let outcome = adapter
        .launch(AppSpec {
            app_id: String::from(KNOWN_APP),
            args: METACHAR_ARGS.iter().map(|s| String::from(*s)).collect(),
        })
        .await
        .expect("fixture launch on a live stand");
    let expected = metachar_line();
    let unit_selector = format!("--unit={}", outcome.unit_name);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        let journal = probe_command(
            "/usr/bin/journalctl",
            &[
                "--user",
                &unit_selector,
                "--output=cat",
                "--no-pager",
                "-n",
                "50",
            ],
            Duration::from_secs(3),
        )
        .await;
        if journal.is_some_and(|journal| journal.contains(&expected)) {
            println!(
                "closed loop: the unit {} received the arguments verbatim (`{expected}`)",
                outcome.unit_name
            );
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    eprintln!(
        "the unit {} never logged `{expected}` — an argument was not delivered verbatim",
        outcome.unit_name
    );
    false
}

#[tokio::test]
async fn app_launcher_contract_suites_against_the_live_stand() {
    let (_dir, applications) = fixture_stand();
    // The backend bits mirror what the composition root passes: the probe
    // facts (`systemd-run`, `xdg-open`), with the user-manager liveness the
    // binder verifies because a `--user` launch needs the manager, not just
    // the binary.
    let mut backends: Vec<String> = Vec::new();
    if launch_backend_live().await {
        backends.push(String::from("systemd-run"));
    }
    if std::path::Path::new("/usr/bin/xdg-open").exists() {
        backends.push(String::from("xdg-open"));
    }
    let adapter = LaunchAdapter::with_data_dirs(&backends, vec![applications]);

    // The fixture-free suite always runs: inventory shape and the sentinel
    // bindings need no live backend (the adapter refuses honestly before any
    // native work).
    let report = app_launcher_suite(|| Box::new(adapter.clone())).await;
    println!("{report}");
    assert!(report.is_clean(), "{report}");

    // The fixture suite gates the known app on the live launch arm only:
    // without a user manager the launch-success gate would be a lie about
    // this machine.
    if backends.iter().any(|b| b == "systemd-run") {
        let fixture = AppLauncherFixture {
            known_app: String::from(KNOWN_APP),
            // A real xdg-open opens a GUI window — not a test the suite may
            // perform; the open leg stays honestly unconfigured here.
            openable_target: String::new(),
        };
        let report = app_launcher_suite_with_fixture(|| Box::new(adapter.clone()), fixture).await;
        println!("{report}");
        assert!(report.is_clean(), "{report}");
        assert!(
            metacharacters_arrive_verbatim(&adapter).await,
            "the launched unit must receive `;`, `|` and backtick arguments verbatim"
        );
    } else {
        eprintln!(
            "skipped app_launcher_suite_with_fixture: no live systemd user manager on this machine"
        );
    }
}

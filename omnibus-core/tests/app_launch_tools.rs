//! Integration tests: the `app.list` / `app.launch` / `app.open` tool surface
//! (roadmap task 2.7.2, specs/domains/app-launch.md). The fake launcher (P6:
//! fakes near the core) carries the same capability facts the real adapter
//! reports, so these tests pin the core-side half of the task ACs:
//!
//! - `app.launch` without the out-of-band write grant answers `ScopeDenied`
//!   and leaves exactly one audit event (R5);
//! - with the grant the call runs, and its audit event carries the grant
//!   provenance as `scope_ref` (the full R5 schema);
//! - on a stand whose probe report lacks `xdg-open` the `app.open` tool is
//!   hidden from `tools/list` (R3 rung 2), a direct call answers
//!   `CapabilityRequired`, and `capabilities()` explains the reason;
//! - the typed argument list crosses the seam verbatim — `;`, `|` and
//!   backtick inside `AppSpec.args` reach the adapter unmodified (the shell
//!   would have interpreted them; the adapter-side argv layer is proven in
//!   the adapter crate's own tests and live binder).

use omnibus_contracts::capabilities as cap;
use omnibus_contracts::errors::ContractError;
use omnibus_core::audit::FileSink;
use omnibus_core::grants::{Grant, GrantProvenance, GrantRegistry};
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;
use omnibus_core::tools::{self, GateDecision};

mod common;

use common::{args, build_core, fact_with, met_floor};

/// The app facts the real launch adapter reports on a full stand: the read
/// fact and the launch fact, the latter noting the served arms (and, on a
/// stand without an opener, naming the hidden `app.open` — the R3 reason).
fn app_facts(opener_note: Option<String>) -> Vec<omnibus_contracts::dto::probe::CapabilityFact> {
    let mut facts = vec![common::fact(cap::APP_READ, "present")];
    facts.push(fact_with(
        cap::APP_LAUNCH,
        "present",
        None,
        opener_note,
        None,
    ));
    facts
}

/// A platform whose probe found both app backends plus a met systemd floor
/// (`app.launch` rides the systemd floor; without it the ladder answers
/// `FloorUnsupported` before any grant check).
fn platform_with_backends(backends: &[&str]) -> omnibus_contracts::dto::probe::PlatformReport {
    let mut platform = common::platform(vec![met_floor("systemd", "261")]);
    platform.backends_present = backends.iter().map(|b| String::from(*b)).collect();
    platform
}

fn launch_core(
    facts: Vec<omnibus_contracts::dto::probe::CapabilityFact>,
    backends: &[&str],
    grants: GrantRegistry,
    audit_path: &std::path::Path,
    seed: u8,
) -> omnibus_core::Core {
    let registry = Registry {
        app_launcher: Some(common::FakeAppLauncher::new(facts)),
        ..Registry::default()
    };
    build_core(
        registry,
        grants,
        ScopeManifests::default(),
        common::pipeline_for(&common::session(seed), audit_path),
        platform_with_backends(backends),
        seed,
    )
}

/// AC 2.7.2: `app.launch` without the write scope answers `ScopeDenied` and
/// leaves one audit event naming the tool (R5: a refused call is still a call).
#[tokio::test]
async fn app_launch_without_the_write_grant_is_scope_denied_and_audited() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = launch_core(
        app_facts(None),
        &["systemd-run", "xdg-open"],
        GrantRegistry::new(),
        &dir.path().join("audit.jsonl"),
        1,
    );
    let outcome = tools::dispatch(
        &core,
        "app.launch",
        args(&[
            ("app_id", "demo.desktop".into()),
            ("args", serde_json::json!([])),
        ]),
    )
    .await;
    assert!(
        matches!(outcome, Err(ContractError::ScopeDenied)),
        "the grantless launch answers ScopeDenied"
    );
    // Zero adapter calls: the gate refuses before the seam.
    // (The fake counts calls; assert via a fresh count would need the Arc, so
    // the audit is the durable proof here.)
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1, "the denial is audited exactly once");
    assert_eq!(events[0].result, "ScopeDenied");
    assert_eq!(events[0].tool, "app.launch");
    assert_eq!(events[0].adapter, "core", "the adapter was never reached");
}

/// The grantless `app.open` behaves identically (same write capability, same
/// audit shape).
#[tokio::test]
async fn app_open_without_the_write_grant_is_scope_denied_and_audited() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = launch_core(
        app_facts(None),
        &["systemd-run", "xdg-open"],
        GrantRegistry::new(),
        &dir.path().join("audit.jsonl"),
        2,
    );
    let outcome = tools::dispatch(
        &core,
        "app.open",
        args(&[("target", "https://demo.invalid/".into())]),
    )
    .await;
    assert!(
        matches!(outcome, Err(ContractError::ScopeDenied)),
        "the grantless open answers ScopeDenied"
    );
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].result, "ScopeDenied");
    assert_eq!(events[0].tool, "app.open");
}

/// AC 2.7.2: with the out-of-band grant the writes run, and the audit event
/// carries the full R5 schema — including `scope_ref` naming the grant
/// provenance (`cli:app.launch`).
#[tokio::test]
async fn granted_app_writes_run_and_audit_with_the_grant_scope_ref() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::APP_LAUNCH, GrantProvenance::Cli));
    let launcher = common::FakeAppLauncher::new(app_facts(None));
    let registry = Registry {
        app_launcher: Some(launcher.clone()),
        ..Registry::default()
    };
    let core = build_core(
        registry,
        grants,
        ScopeManifests::default(),
        common::pipeline_for(&common::session(3), &dir.path().join("audit.jsonl")),
        platform_with_backends(&["systemd-run", "xdg-open"]),
        3,
    );
    let launch = tools::dispatch(
        &core,
        "app.launch",
        args(&[
            ("app_id", "demo.desktop".into()),
            (
                "args",
                serde_json::json!(["--flag;pipe", "back`tick", "|piped|"]),
            ),
        ]),
    )
    .await
    .expect("the granted launch runs");
    assert!(
        launch.value["unit_name"]
            .as_str()
            .is_some_and(|unit| unit.starts_with("app-demo.desktop-")),
        "the outcome names the transient unit: {:?}",
        launch.value
    );
    let open = tools::dispatch(
        &core,
        "app.open",
        args(&[("target", "https://demo.invalid/".into())]),
    )
    .await
    .expect("the granted open runs");
    assert_eq!(open.value["opened"], serde_json::json!(true));

    // THE metacharacter invariant at the core seam: the typed list the agent
    // sent is what the adapter received — byte-for-byte, no shell rewriting.
    assert_eq!(
        launcher.last_launch_args(),
        vec![
            String::from("--flag;pipe"),
            String::from("back`tick"),
            String::from("|piped|"),
        ],
        "the typed argument list crosses the seam verbatim"
    );

    // Both writes are audited with the grant provenance as scope_ref (R5).
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 2, "one event per write");
    assert!(events.iter().all(|e| e.result == "ok"));
    assert!(events.iter().all(|e| e.adapter == "fake-launch"));
    for event in &events {
        assert_eq!(
            event.scope_ref.as_deref(),
            Some("cli:app.launch"),
            "grant provenance rides every write event: {event:?}"
        );
    }
    assert_eq!(
        launcher
            .launch_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        launcher
            .open_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

/// AC 2.7.2 (R3): on a stand without `xdg-open` the `app.open` tool is hidden
/// from tools/list, a direct call answers `CapabilityRequired` (audited), and
/// `capabilities()` explains the reason through the adapter's fact note —
/// while `app.list` and `app.launch` stay served.
#[tokio::test]
async fn app_open_hides_without_xdg_open_and_capabilities_explain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let note = String::from(
        "launch as transient user units (systemd-run --user); app.open is hidden: \
         xdg-open is not installed (no association opener on this machine)",
    );
    // The write grant is ISSUED so the grant gate is out of the picture:
    // the only difference between the two write tools is the opener absence —
    // that is what makes this the discriminating hidden-tool proof.
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::APP_LAUNCH, GrantProvenance::Cli));
    let core = launch_core(
        app_facts(Some(note.clone())),
        // The probe found systemd-run but NOT xdg-open.
        &["systemd-run"],
        grants,
        &dir.path().join("audit.jsonl"),
        4,
    );
    // The tool list: app.open gone, the other two app tools visible.
    let visible: Vec<String> = tools::visible_tools(&core)
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert!(
        !visible.iter().any(|name| name == "app.open"),
        "app.open is hidden on a stand without xdg-open"
    );
    assert!(visible.iter().any(|name| name == "app.list"));
    assert!(visible.iter().any(|name| name == "app.launch"));
    // The gate agrees with the list.
    assert!(matches!(
        tools::gate(&core, "app.open"),
        GateDecision::Hidden
    ));
    // A direct call answers the typed rung-2 error (never a runtime surprise).
    let outcome = tools::dispatch(
        &core,
        "app.open",
        args(&[("target", "https://demo.invalid/".into())]),
    )
    .await;
    assert!(
        matches!(outcome, Err(ContractError::CapabilityRequired)),
        "a hidden app.open answers CapabilityRequired"
    );
    // The refusal is audited (R5), and capabilities() carries the reason.
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].result, "CapabilityRequired");
    let document = &core.caps.document;
    let launch_entry = document
        .capabilities
        .iter()
        .find(|entry| entry.name == cap::APP_LAUNCH)
        .expect("the app.launch entry exists on this stand");
    assert_eq!(launch_entry.status, "present");
    assert!(
        launch_entry
            .note
            .as_deref()
            .is_some_and(|note| note.contains("app.open is hidden")),
        "capabilities() explains the hidden tool: {:?}",
        launch_entry.note
    );
}

/// The mirror case: with `xdg-open` in the probe report and the grant issued,
/// the tool is listed and runnable — the opener refinement never fires.
#[test]
fn app_open_is_visible_when_the_opener_is_present() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::APP_LAUNCH, GrantProvenance::Cli));
    let core = launch_core(
        app_facts(None),
        &["systemd-run", "xdg-open"],
        grants,
        &dir.path().join("audit.jsonl"),
        5,
    );
    let visible: Vec<String> = tools::visible_tools(&core)
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert!(visible.iter().any(|name| name == "app.open"));
    assert!(matches!(tools::gate(&core, "app.open"), GateDecision::Run));
}

/// A stand without the launch backend (no `systemd-run` fact at all — the
/// provider-absence shape the real adapter reports) hides both write tools
/// through the ladder even with the grant issued, while the inventory read
/// stays.
#[test]
fn missing_launch_backend_hides_both_writes_but_keeps_the_inventory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let facts = vec![common::fact(cap::APP_READ, "present")];
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::APP_LAUNCH, GrantProvenance::Cli));
    let core = launch_core(facts, &[], grants, &dir.path().join("audit.jsonl"), 6);
    let visible: Vec<String> = tools::visible_tools(&core)
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert!(visible.iter().any(|name| name == "app.list"));
    assert!(!visible.iter().any(|name| name == "app.launch"));
    assert!(!visible.iter().any(|name| name == "app.open"));
}

/// Below the systemd floor `app.launch` answers `FloorUnsupported` (the floor
/// rung wins over the present fact) while the floorless arms keep running.
#[test]
fn below_the_systemd_floor_the_launch_arm_is_floor_unsupported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let floors = vec![omnibus_contracts::dto::probe::FloorStatus {
        component: String::from("systemd"),
        actual: Some(String::from("247")),
        floor: String::from("252"),
        met: false,
        reason: Some(String::from("systemd 247 < 252")),
    }];
    let registry = Registry {
        app_launcher: Some(common::FakeAppLauncher::new(app_facts(None))),
        ..Registry::default()
    };
    let mut platform = common::platform(floors);
    platform.backends_present = vec![String::from("systemd-run"), String::from("xdg-open")];
    // The grant is issued: the floor rung wins over a granted capability.
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::APP_LAUNCH, GrantProvenance::Cli));
    let core = build_core(
        registry,
        grants,
        ScopeManifests::default(),
        common::pipeline_for(&common::session(7), &dir.path().join("audit.jsonl")),
        platform,
        7,
    );
    assert!(matches!(
        tools::gate(&core, "app.launch"),
        GateDecision::BelowFloor
    ));
    assert!(matches!(tools::gate(&core, "app.list"), GateDecision::Run));
    assert!(matches!(tools::gate(&core, "app.open"), GateDecision::Run));
}

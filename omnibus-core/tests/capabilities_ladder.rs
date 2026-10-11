//! Integration tests for roadmap 1.3.5 (capabilities registry and tool gating):
//! the four ladder states end to end through the assembled document and the gate
//! — degraded (visible, limitation marked), absent (hidden, `CapabilityRequired`
//! with a doctor hint), below-floor (adapter not activated, reason in the
//! document), opt-in-ungranted (hidden until the grant) — plus fresh-session
//! pickup: a new instance after a grant change sees the new tool list.

mod common;

use common::{FakeJournal, FakePackages, FakeServices, args, build_core, met_floor, platform};
use omnibus_contracts::capabilities as cap;
use omnibus_contracts::dto::probe::FloorStatus;
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::floors::{PlatformVersion, SYSTEMD_FLOOR, floor_reason};
use omnibus_core::doctor;
use omnibus_core::error_map::{capability_for_tool, render};
use omnibus_core::grants::{Grant, GrantProvenance, GrantRegistry};
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;
use omnibus_core::tools::{self, GateDecision};

fn below_floor_systemd() -> Vec<FloorStatus> {
    vec![FloorStatus {
        component: String::from("systemd"),
        actual: Some(String::from("247.3")),
        floor: String::from("252"),
        met: false,
        reason: Some(floor_reason(
            "systemd",
            SYSTEMD_FLOOR,
            PlatformVersion::new(247, 3, 0),
        )),
    }]
}

/// Rung 1 — degraded: the tool stays visible and runnable, the document marks the
/// limitation on the capability entry.
#[tokio::test]
async fn degraded_capability_keeps_the_tool_visible_with_the_limitation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let note = String::from("metadata partial on rolling mirrors");
    let packages = FakePackages::new(
        vec![common::fact_with(
            cap::PKG_READ,
            "degraded",
            None,
            Some(note.clone()),
            None,
        )],
        std::time::Duration::ZERO,
    );
    let core = build_core(
        Registry {
            packages: Some(packages),
            ..Registry::default()
        },
        GrantRegistry::new(),
        ScopeManifests::default(),
        common::pipeline_for(&common::session(1), &dir.path().join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        1,
    );

    assert!(matches!(tools::gate(&core, "pkg_list"), GateDecision::Run));
    let visible = tools::visible_tools(&core);
    let names: Vec<&str> = visible.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"pkg_list"), "degraded tool stays listed");

    let entry = core.caps.entry(cap::PKG_READ).expect("document entry");
    assert_eq!(entry.status, "degraded");
    assert_eq!(entry.note.as_deref(), Some(note.as_str()));

    let outcome = tools::dispatch(&core, "pkg_list", args(&[])).await;
    assert!(outcome.is_ok(), "the degraded read still runs");
}

/// Rung 2 — absent: the tool is hidden, a direct call yields `CapabilityRequired`,
/// and the rendered error names the doctor.
#[tokio::test]
async fn absent_capability_hides_the_tool_and_hints_the_doctor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = build_core(
        Registry {
            journal: Some(FakeJournal::new(vec![common::fact(
                cap::JOURNAL_QUERY,
                "present",
            )])),
            ..Registry::default()
        },
        GrantRegistry::new(),
        ScopeManifests::default(),
        common::pipeline_for(&common::session(2), &dir.path().join("audit.jsonl")),
        platform(vec![]),
        2,
    );

    let visible = tools::visible_tools(&core);
    let names: Vec<&str> = visible.iter().map(|s| s.name.as_str()).collect();
    assert!(
        !names.contains(&"svc_list"),
        "absent capability: tool hidden"
    );

    assert!(matches!(
        tools::gate(&core, "svc_list"),
        GateDecision::Hidden
    ));
    let outcome = tools::dispatch(&core, "svc_list", args(&[])).await;
    assert!(matches!(outcome, Err(ContractError::CapabilityRequired)));

    let rendered = render(
        &ContractError::CapabilityRequired,
        capability_for_tool("svc_list"),
    );
    assert!(
        rendered.contains("doctor"),
        "the error hints the doctor: {rendered}"
    );
}

/// Rung 3 — below floor: the subsystem stays deactivated even though the adapter
/// report claims presence; dispatches yield `FloorUnsupported` and the document
/// explains the exact reason ("systemd 247.3 < 252.0" shape, from the contract).
#[tokio::test]
async fn below_floor_deactivates_the_subsystem_with_the_reason_in_the_document() {
    let dir = tempfile::tempdir().expect("tempdir");
    let floors = below_floor_systemd();
    let services = FakeServices::new(vec![
        common::fact(cap::SERVICE_READ, "present"),
        common::fact(cap::SERVICE_WRITE, "present"),
    ]);
    let core = build_core(
        Registry {
            services: Some(services),
            ..Registry::default()
        },
        GrantRegistry::new(),
        ScopeManifests::default(),
        common::pipeline_for(&common::session(3), &dir.path().join("audit.jsonl")),
        platform(floors.clone()),
        3,
    );

    // Rung 3 wins over the reported capability status.
    assert!(matches!(
        tools::gate(&core, "svc_list"),
        GateDecision::BelowFloor
    ));
    assert!(matches!(
        tools::gate(&core, "svc_status"),
        GateDecision::BelowFloor
    ));
    assert!(matches!(
        tools::gate(&core, "svc_start"),
        GateDecision::BelowFloor
    ));
    let outcome = tools::dispatch(&core, "svc_list", args(&[])).await;
    assert!(matches!(outcome, Err(ContractError::FloorUnsupported)));

    // The document explains: the unmet floor and its contract-rendered reason.
    let reason = floor_reason("systemd", SYSTEMD_FLOOR, PlatformVersion::new(247, 3, 0));
    assert_eq!(core.caps.floors, floors);
    assert_eq!(
        core.caps.floor_reason("systemd").as_deref(),
        Some(reason.as_str())
    );
    assert!(!core.caps.floor_met("systemd"));
    let entry = core.caps.entry(cap::SERVICE_READ);
    assert!(
        entry.is_some(),
        "the report is still assembled honestly; the floor decides the gate"
    );

    // Doctor names the failing floor.
    let report = doctor::diagnose(&core).await;
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.component == "floor" && f.severity == "fail"),
        "doctor reports the unmet floor"
    );
}

/// Rung 4 — opt-in ungranted: hidden (direct call: `CapabilityRequired`); with the
/// grant issued the tool is runnable and the write goes through.
#[tokio::test]
async fn opt_in_ungranted_is_hidden_and_granted_is_runnable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let facts = vec![
        common::fact(cap::PKG_READ, "present"),
        common::fact_with(
            cap::PKG_WRITE,
            "opt-in-required",
            None,
            None,
            Some(format!("omnibus grant {}", cap::PKG_WRITE)),
        ),
    ];
    let mk = |grants: GrantRegistry, session_num: u8, path: &std::path::Path| {
        build_core(
            Registry {
                packages: Some(FakePackages::new(facts.clone(), std::time::Duration::ZERO)),
                ..Registry::default()
            },
            grants,
            ScopeManifests::default(),
            common::pipeline_for(&common::session(session_num), path),
            platform(vec![met_floor("systemd", "261")]),
            session_num,
        )
    };

    let ungranted = mk(GrantRegistry::new(), 4, &dir.path().join("a.jsonl"));
    assert!(matches!(
        tools::gate(&ungranted, "pkg_install"),
        GateDecision::Hidden
    ));
    let outcome =
        tools::dispatch(&ungranted, "pkg_install", args(&[("name", "demo".into())])).await;
    assert!(matches!(outcome, Err(ContractError::CapabilityRequired)));

    let mut granted_registry = GrantRegistry::new();
    granted_registry.issue(Grant::new(cap::PKG_WRITE, GrantProvenance::Cli));
    let granted = mk(granted_registry, 5, &dir.path().join("b.jsonl"));
    assert!(matches!(
        tools::gate(&granted, "pkg_install"),
        GateDecision::Run
    ));
    let outcome = tools::dispatch(&granted, "pkg_install", args(&[("name", "demo".into())])).await;
    assert!(outcome.is_ok(), "the issued grant unlocks the write");
}

/// AC 1.3.5 (fresh session pickup): two consecutive instances built from the same
/// probe but different grant manifests — the second one, started after the doctor
/// CLI wrote the grant, exposes a different tool list.
#[tokio::test]
async fn fresh_session_picks_up_the_new_grant_manifests() {
    let dir = tempfile::tempdir().expect("tempdir");
    let facts = vec![
        common::fact(cap::PKG_READ, "present"),
        common::fact(cap::JOURNAL_QUERY, "present"),
        common::fact_with(cap::PKG_WRITE, "opt-in-required", None, None, None),
    ];
    let registry = || Registry {
        journal: Some(FakeJournal::new(vec![common::fact(
            cap::JOURNAL_QUERY,
            "present",
        )])),
        packages: Some(FakePackages::new(facts.clone(), std::time::Duration::ZERO)),
        ..Registry::default()
    };

    let before = build_core(
        registry(),
        GrantRegistry::new(),
        ScopeManifests::default(),
        common::pipeline_for(&common::session(6), &dir.path().join("a.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        6,
    );

    std::fs::write(
        dir.path().join("90-agent.toml"),
        format!("[write]\n\"{}\" = {{}}\n", cap::PKG_WRITE),
    )
    .expect("write manifest");
    let (grants, manifests) =
        ScopeManifests::load(&[dir.path().to_path_buf()]).into_grant_registry();
    let after = build_core(
        registry(),
        grants,
        manifests,
        common::pipeline_for(&common::session(7), &dir.path().join("b.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        7,
    );

    let names = |core: &omnibus_core::Core| -> Vec<String> {
        let mut names: Vec<String> = tools::visible_tools(core)
            .iter()
            .map(|s| s.name.clone())
            .collect();
        names.sort();
        names
    };
    let before_names = names(&before);
    let after_names = names(&after);
    assert_ne!(before_names, after_names, "the new instance sees the grant");
    assert!(!before_names.contains(&"pkg_install".to_string()));
    assert!(after_names.contains(&"pkg_install".to_string()));
    // Unchanged read surface survives the new session untouched.
    assert!(before_names.contains(&"pkg_list".to_string()));
    assert!(after_names.contains(&"pkg_list".to_string()));
}

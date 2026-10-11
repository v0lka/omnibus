//! Integration tests for roadmap 1.3.3 (scopes): a grantless write is denied with
//! `ScopeDenied` and audited, in-band "confirmation" fields cannot unlock anything,
//! manifest grants surface in doctor, a broken manifest never takes the daemon
//! down, and the default install exposes exactly the read-only tool set.

mod common;

use std::path::PathBuf;

use common::{FakePackages, args, build_core, met_floor, pipeline_for, platform};
use omnibus_contracts::capabilities as cap;
use omnibus_contracts::errors::ContractError;
use omnibus_core::audit::FileSink;
use omnibus_core::doctor;
use omnibus_core::grants::{Grant, GrantProvenance, GrantRegistry};
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;
use omnibus_core::tools::{self, GateDecision};

fn pkg_report() -> Vec<omnibus_contracts::dto::probe::CapabilityFact> {
    vec![
        common::fact(cap::PKG_READ, "present"),
        common::fact(cap::PKG_WRITE, "present"),
    ]
}

fn core_with_packages(
    dir: &std::path::Path,
    grants: GrantRegistry,
    seed: u8,
) -> omnibus_core::Core {
    let packages = FakePackages::new(pkg_report(), std::time::Duration::ZERO);
    build_core(
        Registry {
            packages: Some(packages),
            ..Registry::default()
        },
        grants,
        ScopeManifests::default(),
        pipeline_for(&common::session(seed), &dir.join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        seed,
    )
}

/// AC 1.3.3: a write tool without a grant is refused with `ScopeDenied` and the
/// refusal itself is written to the audit sink — exactly one event.
#[tokio::test]
async fn grantless_write_is_denied_and_audited() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_with_packages(dir.path(), GrantRegistry::new(), 1);

    let decision = tools::gate(&core, "pkg_install");
    assert!(matches!(decision, GateDecision::Denied));

    let outcome = tools::dispatch(&core, "pkg_install", args(&[("name", "demo".into())])).await;
    assert!(
        matches!(outcome, Err(ContractError::ScopeDenied)),
        "expected ScopeDenied for the grantless write"
    );

    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1, "the denial is audited exactly once");
    assert_eq!(events[0].result, "ScopeDenied");
    assert_eq!(events[0].tool, "pkg_install");
}

/// AC 1.3.3 (no-confirm-API invariant): "confirmation" fields in the arguments —
/// nothing in the core reads them — change nothing: the grantless call is denied
/// like any other, and the audit shows the denial.
#[tokio::test]
async fn in_band_confirmation_fields_cannot_unlock_a_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = core_with_packages(dir.path(), GrantRegistry::new(), 2);

    let consenting = args(&[
        ("name", "demo".into()),
        ("confirm", true.into()),
        ("user_confirmed", "yes".into()),
        ("consent", "granted".into()),
    ]);
    let outcome = tools::dispatch(&core, "pkg_install", consenting).await;
    assert!(
        matches!(outcome, Err(ContractError::ScopeDenied)),
        "an agent-supplied confirmation is always denied without a grant"
    );

    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].result, "ScopeDenied");
}

/// The mirror side of the invariant: with an out-of-band grant the same consent
/// fields are simply unread — the action proceeds because of the grant, and the
/// fields neither help nor hurt (no code path inspects them).
#[tokio::test]
async fn consent_fields_are_ignored_when_the_grant_exists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::PKG_WRITE, GrantProvenance::Config));
    let core = core_with_packages(dir.path(), grants, 3);

    let consenting = args(&[
        ("name", "demo".into()),
        ("confirm", true.into()),
        ("user_confirmed", "yes".into()),
        ("consent", "granted".into()),
    ]);
    let outcome = tools::dispatch(&core, "pkg_install", consenting).await;
    assert!(outcome.is_ok(), "the grant alone decides; fields are noise");
}

/// AC 1.3.3: manifest grants carry provenance `config` (the single grant store);
/// doctor reports a granted opt-in as satisfied and a missing one with a
/// copy-paste `omnibus grant` remedy.
#[tokio::test]
async fn manifest_grants_drive_the_doctor_opt_in_findings() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("90-agent.toml"),
        format!(
            "[write]\n\"{PKG}\" = {{}}\n\n[optin]\n\"{PERSISTENT}\" = {{}}\n",
            PKG = cap::PKG_WRITE,
            PERSISTENT = cap::JOURNAL_PERSISTENT,
        ),
    )
    .expect("write manifest");
    let manifests = ScopeManifests::load(&[dir.path().to_path_buf()]);
    let (grants, manifests) = manifests.into_grant_registry();
    assert!(grants.has_grant(cap::PKG_WRITE));
    assert!(grants.has_grant(cap::JOURNAL_PERSISTENT));

    let facts = vec![
        common::fact(cap::JOURNAL_QUERY, "present"),
        common::fact_with(
            cap::JOURNAL_PERSISTENT,
            "opt-in-required",
            Some(serde_json::json!(false)),
            None,
            Some(format!("omnibus grant {}", cap::JOURNAL_PERSISTENT)),
        ),
    ];
    let core = build_core(
        Registry {
            journal: Some(common::FakeJournal::new(facts)),
            ..Registry::default()
        },
        grants,
        manifests,
        pipeline_for(&common::session(4), &dir.path().join("audit.jsonl")),
        platform(vec![]),
        4,
    );

    // Granted: no grant finding for the opt-in capability.
    let report = doctor::diagnose(&core).await;
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.component == "grant" && f.detail.contains(cap::JOURNAL_PERSISTENT)),
        "a granted opt-in produces no grant finding"
    );

    // A fresh instance without the opt-in manifest sees the gap, with remedy.
    let (empty_grants, empty_manifests) = ScopeManifests::default().into_grant_registry();
    let second = build_core(
        Registry {
            journal: Some(common::FakeJournal::new(vec![common::fact_with(
                cap::JOURNAL_PERSISTENT,
                "opt-in-required",
                None,
                None,
                None,
            )])),
            ..Registry::default()
        },
        empty_grants,
        empty_manifests,
        pipeline_for(&common::session(5), &dir.path().join("audit2.jsonl")),
        platform(vec![]),
        5,
    );
    let report = doctor::diagnose(&second).await;
    let finding = report
        .findings
        .iter()
        .find(|f| f.component == "grant" && f.detail.contains(cap::JOURNAL_PERSISTENT))
        .expect("missing opt-in produces a grant finding");
    let remedy = finding.remedy.as_deref().expect("remedy present");
    assert!(remedy.contains("omnibus grant"));
}

/// AC 1.3.3: a malformed manifest never takes the daemon down — exactly one
/// recorded error naming the bad file, the healthy grant keeps working, and the
/// assembled capabilities state + grant check still function.
#[tokio::test]
async fn broken_manifest_is_recorded_and_healthy_scopes_keep_working() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("10-bad.toml"), "[write\nbroken ==").expect("write bad");
    std::fs::write(
        dir.path().join("20-good.toml"),
        format!("[write]\n\"{}\" = {{}}\n", cap::PKG_WRITE),
    )
    .expect("write good");
    let manifests = ScopeManifests::load(&[dir.path().to_path_buf()]);
    assert_eq!(manifests.errors.len(), 1, "exactly one manifest error");
    assert!(
        manifests.errors[0].path.ends_with("10-bad.toml"),
        "the error names the broken file"
    );
    assert!(manifests.write.contains_key(cap::PKG_WRITE));

    let (grants, manifests) = manifests.into_grant_registry();
    let packages = FakePackages::new(pkg_report(), std::time::Duration::ZERO);
    let core = build_core(
        Registry {
            packages: Some(packages),
            ..Registry::default()
        },
        grants,
        manifests,
        pipeline_for(&common::session(6), &dir.path().join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        6,
    );
    // The healthy grant authorizes the write end to end; the capabilities state
    // assembled from the adapters reports pkg.write as present.
    let outcome = tools::dispatch(&core, "pkg_install", args(&[("name", "demo".into())])).await;
    assert!(outcome.is_ok(), "the healthy grant still works");
    let entry = core
        .caps
        .entry(cap::PKG_WRITE)
        .expect("capability document entry");
    assert_eq!(entry.status, "present");
}

/// AC 1.3.3: the default install (no manifests) exposes exactly the read-only
/// tool set — every write spec is Denied/Hidden, every read spec is Run/Hidden
/// (capability-dependent), and no write tool appears in the visible list.
#[tokio::test]
async fn default_install_is_read_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Registry {
        journal: Some(common::FakeJournal::new(vec![common::fact(
            cap::JOURNAL_QUERY,
            "present",
        )])),
        packages: Some(FakePackages::new(pkg_report(), std::time::Duration::ZERO)),
        services: Some(common::FakeServices::new(vec![
            common::fact(cap::SERVICE_READ, "present"),
            common::fact(cap::SERVICE_WRITE, "present"),
        ])),
        ..Registry::default()
    };
    let manifests = ScopeManifests::load(&[PathBuf::from("/nonexistent/omnibus/scopes.d")]);
    assert!(manifests.write.is_empty() && manifests.optin.is_empty());
    let core = build_core(
        registry,
        GrantRegistry::new(),
        manifests,
        pipeline_for(&common::session(7), &dir.path().join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        7,
    );

    for spec in tools::all_specs() {
        let decision = tools::gate(&core, &spec.name);
        if spec.grant_key.is_some() {
            assert!(
                matches!(decision, GateDecision::Denied | GateDecision::Hidden),
                "write tool {} must not be runnable without a grant",
                spec.name
            );
        } else {
            assert!(
                matches!(decision, GateDecision::Run | GateDecision::Hidden),
                "read tool {} is Run or capability-hidden",
                spec.name
            );
        }
    }

    let visible = tools::visible_tools(&core);
    for spec in &visible {
        assert!(
            spec.grant_key.is_none(),
            "write tool {} leaked into the read-only default",
            spec.name
        );
    }
    let names: Vec<&str> = visible.iter().map(|s| s.name.as_str()).collect();
    for expected in [
        "journal_query",
        "pkg_list",
        "pkg_info",
        "svc_list",
        "svc_status",
    ] {
        assert!(
            names.contains(&expected),
            "read tool {expected} must be visible"
        );
    }
}

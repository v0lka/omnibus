#![allow(clippy::expect_used)] // test-only file: tests may expect; the seam must not
//! Integration tests for roadmap 1.3.2 (sessions): the read budget retries exactly
//! once, writes are never retried, concurrent writes serialize through the write
//! gate with the audit recording the acquisition order, element tokens do not
//! cross instances, and an unregistered adapter slot is a typed error — the
//! `probe → register → serve` FSM keeps adapters unreachable before registration.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{FakeJournal, FakePackages, args, met_floor, pipeline_for, platform};
use omnibus_contracts::capabilities as cap;
use omnibus_contracts::dto::desktop::{A11yElement, WriteOutcome};
use omnibus_contracts::errors::ContractError;
use omnibus_core::audit::FileSink;
use omnibus_core::error_map::join_tool;
use omnibus_core::grants::{Grant, GrantProvenance, GrantRegistry};
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;
use omnibus_core::tools;

fn journal_report() -> Vec<omnibus_contracts::dto::probe::CapabilityFact> {
    vec![
        common::fact(cap::JOURNAL_QUERY, "present"),
        common::fact(cap::JOURNAL_PERSISTENT, "present"),
    ]
}

fn pkg_report() -> Vec<omnibus_contracts::dto::probe::CapabilityFact> {
    vec![
        common::fact(cap::PKG_READ, "present"),
        common::fact(cap::PKG_WRITE, "present"),
    ]
}

fn pkg_grants() -> GrantRegistry {
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::PKG_WRITE, GrantProvenance::Cli));
    grants
}

/// AC 1.3.2: a read past its budget is retried exactly once — two adapter calls
/// total — and then surfaces the typed `TimeoutError`. Virtual time keeps the
/// 10 s budget instant in the test.
#[tokio::test(start_paused = true)]
async fn read_timeout_retries_once_then_times_out() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = FakeJournal::slow(journal_report(), Duration::from_secs(11));
    let registry = Registry {
        journal: Some(journal.clone()),
        ..Registry::default()
    };
    let core = common::build_core(
        registry,
        GrantRegistry::new(),
        ScopeManifests::default(),
        pipeline_for(&common::session(1), &dir.path().join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        1,
    );

    let outcome = tools::dispatch(&core, "journal_query", args(&[("limit", 5.into())])).await;
    assert!(matches!(outcome, Err(ContractError::TimeoutError)));
    assert_eq!(
        journal
            .query_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "idempotent reads retry exactly once: two adapter calls total"
    );

    // The timeout is audited like every terminal outcome.
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].result, "TimeoutError");
    assert_eq!(events[0].tool, "journal_query");
}

/// AC 1.3.2: a write is never retried — exactly one adapter call — and the
/// write's own error surfaces untouched and audited. The write's only
/// timeout layer is its adapter budget (`tools::dispatch` never timers
/// writes), so the honest indeterminate outcome — not a bare core
/// `TimeoutError` — is what the agent and the audit see.
#[tokio::test]
async fn write_error_is_never_retried() {
    let dir = tempfile::tempdir().expect("tempdir");
    let packages = FakePackages::failing_install(pkg_report());
    let registry = Registry {
        packages: Some(packages.clone()),
        ..Registry::default()
    };
    let core = common::build_core(
        registry,
        pkg_grants(),
        ScopeManifests::default(),
        pipeline_for(&common::session(2), &dir.path().join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        2,
    );

    let outcome = tools::dispatch(&core, "pkg_install", args(&[("name", "demo".into())])).await;
    assert!(
        matches!(outcome, Err(ContractError::NativeFailure { code: -7, .. })),
        "the adapter's indeterminate write outcome surfaces untouched"
    );
    assert_eq!(
        packages
            .install_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "writes are never retried"
    );
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].result, "NativeFailure");
}

/// The prev-boot rung is enforced at the dispatch arm, not only in the
/// helper's truth table: a volatile-journal fact set (`journal.prev_boot`
/// `opt-in-required`, no grant) refuses `boot: previous` with
/// `CapabilityRequired` before the adapter is reached, and the refusal is
/// audited like every terminal outcome.
#[tokio::test]
async fn prev_boot_query_refused_until_persistent_granted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let facts = vec![
        common::fact(cap::JOURNAL_QUERY, "present"),
        common::fact(cap::JOURNAL_PERSISTENT, "opt-in-required"),
        common::fact(cap::JOURNAL_PREV_BOOT, "opt-in-required"),
    ];
    let registry = Registry {
        journal: Some(FakeJournal::new(facts)),
        ..Registry::default()
    };
    let core = common::build_core(
        registry,
        GrantRegistry::new(),
        ScopeManifests::default(),
        pipeline_for(&common::session(7), &dir.path().join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        7,
    );

    let outcome = tools::dispatch(
        &core,
        "journal_query",
        args(&[("boot", "previous".into()), ("limit", 5.into())]),
    )
    .await;
    assert!(
        matches!(outcome, Err(ContractError::CapabilityRequired)),
        "an ungranted volatile prev_boot refuses with CapabilityRequired"
    );
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1, "the refusal is audited like every outcome");
    assert_eq!(events[0].result, "CapabilityRequired");
}

/// The grant unlocks the rung: with `journal.persistent` issued, the same
/// `boot: previous` query is dispatched to the adapter instead of refused.
#[tokio::test]
async fn prev_boot_query_runs_once_persistent_is_granted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let facts = vec![
        common::fact(cap::JOURNAL_QUERY, "present"),
        common::fact(cap::JOURNAL_PERSISTENT, "opt-in-required"),
        common::fact(cap::JOURNAL_PREV_BOOT, "opt-in-required"),
    ];
    let journal = FakeJournal::new(facts);
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::JOURNAL_PERSISTENT, GrantProvenance::Cli));
    let registry = Registry {
        journal: Some(journal.clone()),
        ..Registry::default()
    };
    let core = common::build_core(
        registry,
        grants,
        ScopeManifests::default(),
        pipeline_for(&common::session(8), &dir.path().join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        8,
    );

    let outcome = tools::dispatch(
        &core,
        "journal_query",
        args(&[("boot", "previous".into()), ("limit", 5.into())]),
    )
    .await;
    assert!(outcome.is_ok(), "the granted prev_boot query runs");
    assert_eq!(
        journal
            .query_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the adapter was reached"
    );
}

/// AC 1.3.2: two concurrent writes serialize through the gate (no overlap), both
/// complete, and the audit records the factual acquisition order.
#[tokio::test]
async fn concurrent_writes_serialize_and_audit_records_acquisition_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let packages = FakePackages::new(pkg_report(), Duration::from_millis(100));
    let registry = Registry {
        packages: Some(packages.clone()),
        ..Registry::default()
    };
    let core = Arc::new(common::build_core(
        registry,
        pkg_grants(),
        ScopeManifests::default(),
        pipeline_for(&common::session(3), &dir.path().join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        3,
    ));

    let first = tokio::spawn({
        let core = core.clone();
        async move { tools::dispatch(&core, "pkg_install", args(&[("name", "alpha".into())])).await }
    });
    let second = tokio::spawn({
        let core = core.clone();
        async move { tools::dispatch(&core, "pkg_install", args(&[("name", "beta".into())])).await }
    });
    first.await.expect("join").expect("first write ok");
    second.await.expect("join").expect("second write ok");

    // No overlap: the trace strictly alternates enter/leave per writer, and the
    // second writer enters only after the first one left.
    fn entered(line: &str) -> &str {
        line.strip_prefix("enter:").expect("enter record")
    }
    fn left(line: &str) -> &str {
        line.strip_prefix("leave:").expect("leave record")
    }
    let trace = packages.trace();
    assert_eq!(trace.len(), 4, "two writers, enter+leave each");
    assert_eq!(
        entered(&trace[0]),
        left(&trace[1]),
        "first critical section"
    );
    assert_eq!(
        entered(&trace[2]),
        left(&trace[3]),
        "second critical section"
    );
    assert_ne!(entered(&trace[0]), entered(&trace[2]), "both writers ran");

    // The audit events (written while each permit is held) follow the same
    // acquisition order: identify writers by the keyed hash of their arguments.
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 2, "one audited write per dispatch");
    assert!(events.iter().all(|e| e.result == "ok"));
    let hash_of = |name: &str| {
        core.audit
            .hash_params(&serde_json::Value::Object(args(&[("name", name.into())])))
    };
    let first_writer = entered(&trace[0]);
    let second_writer = entered(&trace[2]);
    assert_eq!(events[0].params_hash, hash_of(first_writer));
    assert_eq!(events[1].params_hash, hash_of(second_writer));
}

/// AC 1.3.2: a token minted by one instance never resolves in another — through
/// the dispatch path: `desktop.element` with a foreign token yields the typed
/// `ElementStale` without touching the adapter.
#[tokio::test]
async fn element_token_of_one_instance_is_stale_in_another() {
    let dir = tempfile::tempdir().expect("tempdir");
    let facts = vec![common::fact(cap::DESKTOP_TREE, "present")];
    let tree_tool = join_tool("desktop", "tree");
    let element_tool = join_tool("desktop", "element");

    let first = common::build_core(
        Registry {
            a11y: Some(common::FakeA11y::new(facts.clone())),
            ..Registry::default()
        },
        GrantRegistry::new(),
        ScopeManifests::default(),
        pipeline_for(&common::session(4), &dir.path().join("a.jsonl")),
        platform(vec![]),
        4,
    );
    let second = common::build_core(
        Registry {
            a11y: Some(common::FakeA11y::new(facts)),
            ..Registry::default()
        },
        GrantRegistry::new(),
        ScopeManifests::default(),
        pipeline_for(&common::session(5), &dir.path().join("b.jsonl")),
        platform(vec![]),
        5,
    );

    // The tree of instance one arrives with a core-minted element token.
    let tree = tools::dispatch(&first, &tree_tool, args(&[("app", "demo".into())]))
        .await
        .expect("tree dispatch");
    let token = tree.value["root"]["node_ref"]
        .as_str()
        .expect("minted token in the tree")
        .to_string();
    assert!(!token.is_empty());

    // The same instance resolves the token (the honest fake answers
    // AdapterUnavailable — the call REACHED the adapter, so resolution passed).
    let own = tools::dispatch(
        &first,
        &element_tool,
        args(&[("element_id", token.clone().into())]),
    )
    .await;
    assert!(
        matches!(own, Err(ContractError::AdapterUnavailable)),
        "a valid token resolves and reaches the adapter"
    );

    // A second instance does not resolve the foreign token: typed ElementStale.
    let foreign = tools::dispatch(
        &second,
        &element_tool,
        args(&[("element_id", token.into())]),
    )
    .await;
    assert!(matches!(foreign, Err(ContractError::ElementStale)));
}

/// AC 1.2.2: a FORGED token — never minted by any instance, fabricated from
/// thin air in the exact wire shape the core mints (`elem-` + 16 hex chars) —
/// is rejected with the typed `ElementStale` through the dispatch path, before
/// any adapter call. The negative compile test on the token types
/// (`omnibus-contracts/src/tokens.rs`) pins that a raw string cannot become a
/// token value; this test pins the runtime half: a well-shaped forgery that
/// arrives as a deserialized MCP argument resolves to nothing.
#[tokio::test]
async fn forged_token_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let facts = vec![common::fact(cap::DESKTOP_TREE, "present")];
    let element_tool = join_tool("desktop", "element");

    let core = common::build_core(
        Registry {
            a11y: Some(common::FakeA11y::new(facts)),
            ..Registry::default()
        },
        GrantRegistry::new(),
        ScopeManifests::default(),
        pipeline_for(&common::session(6), &dir.path().join("forged.jsonl")),
        platform(vec![]),
        6,
    );

    // Well-shaped fabrications: the mint-shape prefix and length, including the
    // all-zeros degenerate value. None of these was ever minted by this core.
    for forged in ["elem-0000000000000000", "elem-deadbeefdeadbeef"] {
        let rejected = tools::dispatch(
            &core,
            &element_tool,
            args(&[("element_id", String::from(forged).into())]),
        )
        .await;
        assert!(
            matches!(rejected, Err(ContractError::ElementStale)),
            "forged token {forged} must answer ElementStale"
        );
    }
}

/// AC 1.3.2 (FSM note): registration is structural — a slot is `Option<Arc<dyn
/// Trait>>`, so a tool call cannot even name an unregistered adapter; the empty
/// slot resolves to the typed `AdapterUnavailable`, never a panic. The runtime
/// half below simulates the pre-register state: the session document still
/// advertises the capability, but the slot is absent.
#[tokio::test]
async fn call_with_unregistered_slot_yields_adapter_unavailable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = FakeJournal::new(journal_report());
    let registry = Registry {
        journal: Some(journal.clone()),
        ..Registry::default()
    };
    let mut core = common::build_core(
        registry,
        GrantRegistry::new(),
        ScopeManifests::default(),
        pipeline_for(&common::session(6), &dir.path().join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        6,
    );

    // Sanity: with the slot registered the call goes through.
    let ok = tools::dispatch(&core, "journal_query", args(&[("limit", 1.into())])).await;
    assert!(ok.is_ok(), "registered slot serves the call");

    // The pre-register state: empty slot -> typed AdapterUnavailable.
    core.registry.journal = None;
    let refused = tools::dispatch(&core, "journal_query", args(&[("limit", 1.into())])).await;
    assert!(matches!(refused, Err(ContractError::AdapterUnavailable)));
    assert_eq!(
        journal
            .query_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the unregistered adapter was never invoked"
    );
}

/// Compile-time half of the FSM note: adapter slots are `Option`, so a composition
/// root cannot serve a tool before registration. (Structural check — the absence
/// of a non-Optional accessor is proven by the compiler.)
#[test]
fn adapter_slots_are_optional_by_type() {
    let registry = Registry::default();
    assert!(registry.journal.is_none(), "fresh registry: nothing served");
    assert!(registry.a11y.is_none());
    assert!(registry.packages.is_none());
}

/// The core always mints (dto/desktop.rs): `desktop.element` and the typed
/// write outcomes surface core-minted element tokens — the raw adapter-owned
/// node ref never reaches the MCP surface.
#[tokio::test]
async fn element_and_write_outcomes_surface_core_tokens_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let element = A11yElement {
        node_ref: String::from("app#1/entry#1"),
        role: String::from("entry"),
        name: String::from("Name field"),
        attributes: vec![],
        states: vec![],
        actions: vec![],
        child_refs: vec![],
    };
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::DESKTOP_WRITE_TYPED, GrantProvenance::Cli));
    let core = common::build_core(
        Registry {
            a11y: Some(common::FakeA11y::with_writes(
                vec![
                    common::fact(cap::DESKTOP_TREE, "present"),
                    common::fact(cap::DESKTOP_WRITE_TYPED, "present"),
                ],
                element,
                WriteOutcome {
                    node_ref: String::new(),
                    applied: true,
                },
            )),
            ..Registry::default()
        },
        grants,
        ScopeManifests::default(),
        pipeline_for(&common::session(9), &dir.path().join("audit.jsonl")),
        platform(vec![]),
        9,
    );
    let tree_tool = join_tool("desktop", "tree");
    let element_tool = join_tool("desktop", "element");
    let set_value_tool = join_tool("desktop", "set_value");

    let tree = tools::dispatch(&core, &tree_tool, args(&[("app", "demo".into())]))
        .await
        .expect("tree dispatch");
    let tree_token = tree.value["root"]["node_ref"]
        .as_str()
        .expect("minted token")
        .to_string();

    let element = tools::dispatch(
        &core,
        &element_tool,
        args(&[("element_id", tree_token.clone().into())]),
    )
    .await
    .expect("element dispatch");
    let surfaced = element.value["node_ref"].as_str().expect("surfaced ref");
    assert_ne!(
        surfaced, "app#1/entry#1",
        "the raw adapter ref never reaches the surface"
    );
    assert_ne!(surfaced, tree_token, "each delivery mints a fresh token");

    let outcome = tools::dispatch(
        &core,
        &set_value_tool,
        args(&[
            ("node_ref", surfaced.to_string().into()),
            ("value", "x".into()),
        ]),
    )
    .await
    .expect("set_value dispatch");
    let written = outcome.value["node_ref"].as_str().expect("written ref");
    assert_ne!(
        written, "app#1/entry#1",
        "the typed-write outcome also surfaces a core token"
    );
}

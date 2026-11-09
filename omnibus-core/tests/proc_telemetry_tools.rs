//! Integration tests: the `ps.list` / `ps.info` / `sys.stats` / `power.status`
//! / `ps.signal` tool surface (roadmap tasks 2.8.2 / 2.8.3, specs/domains/
//! process-telemetry.md). The fake telemetry adapter (P6: fakes near the
//! core) carries the same capability facts the real adapter reports, so these
//! tests pin the core-side half of the task ACs:
//!
//! - `ps.signal` without the out-of-band write grant answers `ScopeDenied`,
//!   leaves exactly one audit event (R5), and never reaches the adapter;
//! - with the grant the signal runs, its audit event carries the grant
//!   provenance as `scope_ref` (`cli:ps.signal`), and the typed
//!   `SignalRequest` crosses the seam intact;
//! - an arbitrary numeric signal is rejected at the MCP surface — the closed
//!   contract enum is the only representable shape (the adapter-side proof
//!   lives in the adapter crate's seam test);
//! - on a stand whose adapter report lacks the `power.read` fact the
//!   `power.status` tool is hidden from tools/list (R3 rung 2), a direct call
//!   answers `CapabilityRequired`, and `capabilities()` carries the reason in
//!   the `ps.read` note — while the other four tools stay served;
//! - the reads run without any grant and audit their outcomes.

use omnibus_contracts::capabilities as cap;
use omnibus_contracts::errors::ContractError;
use omnibus_core::audit::FileSink;
use omnibus_core::grants::{Grant, GrantProvenance, GrantRegistry};
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;
use omnibus_core::tools::{self, GateDecision};

mod common;

use common::{args, build_core, fact, fact_with};

/// The facts the real proc adapter reports on a full stand: the three always
/// present facts plus `power.read` (UPower + battery), the ps.read note
/// naming the served power surface.
fn proc_facts(power_note: Option<String>) -> Vec<omnibus_contracts::dto::probe::CapabilityFact> {
    let ps_read_note = power_note.unwrap_or_else(|| {
        String::from("process and system facts from /proc; power facts via UPower")
    });
    vec![
        fact_with(cap::PS_READ, "present", None, Some(ps_read_note), None),
        fact(cap::SYS_READ, "present"),
        fact(cap::PS_SIGNAL, "present"),
    ]
}

/// The full stand (UPower + battery present): `power.read` is served too.
fn proc_facts_with_power() -> Vec<omnibus_contracts::dto::probe::CapabilityFact> {
    let mut facts = proc_facts(None);
    facts.push(fact_with(
        cap::POWER_READ,
        "present",
        None,
        Some(String::from(
            "battery and supply facts through UPower (display device)",
        )),
        None,
    ));
    facts
}

fn proc_core(
    facts: Vec<omnibus_contracts::dto::probe::CapabilityFact>,
    grants: GrantRegistry,
    audit_path: &std::path::Path,
    seed: u8,
) -> (
    omnibus_core::Core,
    std::sync::Arc<common::FakeProcessTelemetry>,
) {
    let telemetry = common::FakeProcessTelemetry::new(facts);
    let registry = Registry {
        process_telemetry: Some(telemetry.clone()),
        ..Registry::default()
    };
    (
        build_core(
            registry,
            grants,
            ScopeManifests::default(),
            common::pipeline_for(&common::session(seed), audit_path),
            common::platform(vec![]),
            seed,
        ),
        telemetry,
    )
}

/// AC 2.8.3: `ps.signal` without the write scope answers `ScopeDenied`, is
/// audited exactly once (R5: a refused call is still a call), and the adapter
/// is never reached — the pid argument is a fact the core refuses to act on
/// before the seam.
#[tokio::test]
async fn ps_signal_without_the_write_grant_is_scope_denied_and_audited() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (core, telemetry) = proc_core(
        proc_facts_with_power(),
        GrantRegistry::new(),
        &dir.path().join("audit.jsonl"),
        1,
    );
    let outcome = tools::dispatch(
        &core,
        "ps.signal",
        args(&[
            ("pid", serde_json::json!(4321)),
            ("signal", serde_json::json!("term")),
        ]),
    )
    .await;
    assert!(
        matches!(outcome, Err(ContractError::ScopeDenied)),
        "the grantless signal answers ScopeDenied"
    );
    assert_eq!(telemetry.signal_calls(), 0, "the adapter was never reached");
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1, "the denial is audited exactly once");
    assert_eq!(events[0].result, "ScopeDenied");
    assert_eq!(events[0].tool, "ps.signal");
    assert_eq!(events[0].adapter, "core", "the refusal is core-attributed");
}

/// AC 2.8.3: with the out-of-band grant the signal runs, and the audit event
/// carries the full R5 schema — including `scope_ref` naming the grant
/// provenance (`cli:ps.signal`).
#[tokio::test]
async fn granted_ps_signal_runs_and_audits_with_the_grant_scope_ref() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::PS_SIGNAL, GrantProvenance::Cli));
    let (core, telemetry) = proc_core(
        proc_facts_with_power(),
        grants,
        &dir.path().join("audit.jsonl"),
        2,
    );
    let outcome = tools::dispatch(
        &core,
        "ps.signal",
        args(&[
            ("pid", serde_json::json!(4321)),
            ("signal", serde_json::json!("term")),
        ]),
    )
    .await;
    assert!(outcome.is_ok(), "the granted signal runs");
    assert_eq!(telemetry.signal_calls(), 1, "exactly one adapter call");
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].result, "ok");
    assert_eq!(events[0].tool, "ps.signal");
    assert_eq!(
        events[0].adapter, "fake-proc",
        "the adapter that served the write is the audit adapter"
    );
    assert_eq!(
        events[0].scope_ref.as_deref(),
        Some("cli:ps.signal"),
        "the grant provenance travels as the audit scope_ref"
    );
}

/// AC 2.8.3 (negative): an arbitrary numeric signal cannot cross the MCP
/// surface at all — the tool schema carries the closed contract enum, so the
/// call is a schema violation answered `Unsupported` and audited, with the
/// adapter never reached. No grant can repair it; only the enum is a signal.
#[tokio::test]
async fn numeric_signal_is_rejected_at_the_mcp_surface() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::PS_SIGNAL, GrantProvenance::Cli));
    let (core, telemetry) = proc_core(
        proc_facts_with_power(),
        grants,
        &dir.path().join("audit.jsonl"),
        3,
    );
    let outcome = tools::dispatch(
        &core,
        "ps.signal",
        args(&[
            ("pid", serde_json::json!(4321)),
            ("signal", serde_json::json!(9)),
        ]),
    )
    .await;
    assert!(
        matches!(outcome, Err(ContractError::Unsupported { .. })),
        "a raw signal number is a schema violation, not a deliverable signal"
    );
    assert_eq!(telemetry.signal_calls(), 0, "nothing reached the kernel");
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].result, "Unsupported");
    // The off-registry name is equally unrepresentable.
    let off_registry = tools::dispatch(
        &core,
        "ps.signal",
        args(&[
            ("pid", serde_json::json!(4321)),
            ("signal", serde_json::json!("sigkill")),
        ]),
    )
    .await;
    assert!(matches!(
        off_registry,
        Err(ContractError::Unsupported { .. })
    ));
}

/// AC 2.8.2 (R3/R7): on a stand without UPower/battery the `power.status`
/// tool is hidden from tools/list (no `power.read` fact), a direct call
/// answers `CapabilityRequired` and is audited, and `capabilities()` carries
/// the reason in the adapter's ps.read note — while the other four tools stay
/// served. No error appears anywhere in the log path: absence is the fact.
#[tokio::test]
async fn power_status_hides_without_upower_and_capabilities_explain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let note = String::from(
        "process and system facts from /proc; power.status is hidden: this machine \
         has no UPower daemon or no battery (R3/R7: the machine is never promised \
         what it lacks)",
    );
    // The write grant is ISSUED so the grant gate is out of the picture:
    // the only difference between the five tools is the missing power.read
    // fact — that is what makes this the discriminating hidden-tool proof
    // (the same design as the app.open test).
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::PS_SIGNAL, GrantProvenance::Cli));
    let (core, telemetry) = proc_core(
        proc_facts(Some(note.clone())),
        grants,
        &dir.path().join("audit.jsonl"),
        4,
    );
    // The tool list: power.status gone, the other four visible.
    let visible: Vec<String> = tools::visible_tools(&core)
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert!(
        !visible.iter().any(|name| name == "power.status"),
        "power.status is hidden on a stand without UPower/battery"
    );
    for served in ["ps.list", "ps.info", "sys.stats", "ps.signal"] {
        assert!(
            visible.iter().any(|name| name == served),
            "{served} stays served on the same stand"
        );
    }
    // The gate agrees with the list.
    assert!(matches!(
        tools::gate(&core, "power.status"),
        GateDecision::Hidden
    ));
    // A direct call answers the typed rung-2 error, audited once (R5).
    let outcome = tools::dispatch(&core, "power.status", args(&[])).await;
    assert!(
        matches!(outcome, Err(ContractError::CapabilityRequired)),
        "a hidden power.status answers CapabilityRequired, never a hollow report"
    );
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].result, "CapabilityRequired");
    // capabilities() explains: the power.read entry is absent, and the
    // ps.read note names the reason (the R3 reason that travels).
    let document = &core.caps.document;
    assert!(
        !document
            .capabilities
            .iter()
            .any(|entry| entry.name == cap::POWER_READ),
        "a missing provider is provider absence, not a present-but-degraded row"
    );
    let ps_read = document
        .capabilities
        .iter()
        .find(|entry| entry.name == cap::PS_READ)
        .expect("ps.read is served");
    assert_eq!(ps_read.status, "present");
    assert!(
        ps_read
            .note
            .as_deref()
            .is_some_and(|n| n.contains("power.status is hidden")),
        "the ps.read note names the hidden power.status: {:?}",
        ps_read.note
    );
    let _ = telemetry; // the fake stays untouched on the hidden-tool path
}

/// AC 2.8.2: the reads run without any grant (reads are audited, not gated)
/// and their audit events name the serving adapter.
#[tokio::test]
async fn process_reads_run_ungated_and_audit_their_outcomes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (core, _telemetry) = proc_core(
        proc_facts_with_power(),
        GrantRegistry::new(),
        &dir.path().join("audit.jsonl"),
        5,
    );
    for (tool, arguments) in [
        ("ps.list", args(&[])),
        ("ps.info", args(&[("pid", serde_json::json!(4321))])),
        ("sys.stats", args(&[])),
        ("power.status", args(&[])),
    ] {
        let outcome = tools::dispatch(&core, tool, arguments).await;
        assert!(outcome.is_ok(), "{tool} is a read: it runs ungated");
    }
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 4, "every read is audited (R5 audit is total)");
    for event in &events {
        assert_eq!(event.result, "ok");
        assert_eq!(event.adapter, "fake-proc");
        assert_eq!(event.scope_ref, None, "reads carry no grant scope_ref");
    }
    // A nonexistent pid on the reads: the typed contract error surfaces
    // through dispatch and is audited under the serving adapter.
    let missing = tools::dispatch(
        &core,
        "ps.info",
        args(&[("pid", serde_json::json!(2_000_000_000))]),
    )
    .await;
    assert!(matches!(missing, Err(ContractError::NativeFailure { .. })));
    let events = FileSink::new(dir.path().join("audit.jsonl")).read_all();
    assert_eq!(events.len(), 5);
    assert_eq!(events[4].result, "NativeFailure");
}

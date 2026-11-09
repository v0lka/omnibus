#![allow(clippy::expect_used)] // test-only file: tests may expect; the seam must not
//! Doctor opt-in integration tests (roadmap 2.5.1, MS2): on a machine whose
//! probe reports a volatile journald config, doctor reports `journal.persistent`
//! as `opt-in-required` (honest `warn`, never `ok`) and hands the human BOTH
//! levers — the out-of-band grant and the persistent-storage command — while the
//! agent channel itself stays mutation-free. Every applied doctor mutation is
//! one audit event through the same pipeline the daemon uses; a declined opt-in
//! changes nothing: the diagnosis is a pure function of the facts (no nagging
//! state, no escalation between calls).

mod common;

use common::{FakeJournal, build_core, pipeline_for};
use omnibus_contracts::capabilities as cap;
use omnibus_core::doctor;
use omnibus_core::grants::GrantRegistry;
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;

/// The volatile-journald shape the journald adapter emits (facts per 1.1.1):
/// the fact set is always present, persistence is `opt-in-required` and
/// prev_boot is hidden behind the same grant.
fn volatile_journal() -> std::sync::Arc<FakeJournal> {
    FakeJournal::new(vec![
        common::fact(cap::JOURNAL_QUERY, "present"),
        common::fact_with(
            cap::JOURNAL_PERSISTENT,
            "opt-in-required",
            None,
            None,
            Some(format!("omnibusd grant {}", cap::JOURNAL_PERSISTENT)),
        ),
        common::fact_with(
            cap::JOURNAL_PREV_BOOT,
            "opt-in-required",
            Some(serde_json::json!(false)),
            Some(String::from(
                "the journal is volatile; past boots appear after persistent storage is granted",
            )),
            Some(format!("omnibusd grant {}", cap::JOURNAL_PERSISTENT)),
        ),
    ])
}

fn volatile_core(dir: &std::path::Path, session_num: u8) -> omnibus_core::Core {
    build_core(
        Registry {
            journal: Some(volatile_journal()),
            ..Registry::default()
        },
        GrantRegistry::new(),
        ScopeManifests::default(),
        pipeline_for(&common::session(session_num), &dir.join("audit.jsonl")),
        common::platform(vec![]),
        session_num,
    )
}

/// The honest statuses: the journal finding is a `warn` naming the
/// `opt-in-required` rung (never the `ok` a persistent machine gets), the
/// opt-in list carries the capability, and BOTH human levers are ready — the
/// grant (out-of-band consent) and the storage command. The grant alone never
/// creates the storage; the storage command alone never issues the grant.
#[tokio::test]
async fn volatile_journal_doctor_reports_opt_in_with_both_human_levers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = volatile_core(dir.path(), 20);

    let report = doctor::diagnose(&core).await;

    // Honest status: the journal finding is `warn` and names the rung.
    let journal = report
        .findings
        .iter()
        .find(|f| f.component == "journal")
        .expect("the volatile journal is diagnosed");
    assert_eq!(journal.severity, "warn", "opt-in-required is never ok");
    assert!(
        journal.detail.contains("opt-in-required"),
        "the finding names the ladder rung: {}",
        journal.detail
    );

    // The capability appears in the opt-in list.
    assert!(
        report.opt_ins.iter().any(|c| c == cap::JOURNAL_PERSISTENT),
        "journal.persistent is on the opt-in list"
    );

    // Lever 1 — the grant (consent is out of band, human-executed).
    let grant = report
        .ready_commands
        .iter()
        .find(|c| {
            c.command
                .contains(&format!("omnibusd grant {}", cap::JOURNAL_PERSISTENT))
        })
        .expect("the grant lever is a ready command");
    assert_eq!(grant.executed_by, "human");

    // Lever 2 — the storage mutation (mkdir + restart journald), human-run.
    let storage = report
        .ready_commands
        .iter()
        .find(|c| c.name == "enable journal persistence")
        .expect("the storage lever is a ready command");
    assert_eq!(storage.executed_by, "human");
    assert!(
        storage.command.contains("mkdir -p /var/log/journal")
            && storage
                .command
                .contains("systemctl restart systemd-journald"),
        "the storage command creates the directory and restarts journald: {}",
        storage.command
    );
    // The journal finding's remedy points at both halves.
    let remedy = journal.remedy.as_deref().unwrap_or_default();
    assert!(
        remedy.contains("grant") && remedy.contains("storage"),
        "the remedy names grant AND storage: {remedy}"
    );
}

/// No repeat-nagging policy hook (roadmap 2.5.1 AC): a declined opt-in leaves
/// the read-only status quo — the diagnosis carries no state, so repeated
/// doctor calls answer identically instead of escalating.
#[tokio::test]
async fn declined_opt_in_leaves_the_status_quo_without_nagging() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = volatile_core(dir.path(), 21);

    let first = doctor::diagnose(&core).await;
    let second = doctor::diagnose(&core).await;
    assert_eq!(
        first, second,
        "the diagnosis is a pure function of the machine facts — no nag counter, no escalation"
    );
    // Nothing was granted or mutated by diagnosing.
    assert!(
        !core.grants.has_grant(cap::JOURNAL_PERSISTENT),
        "diagnosing never self-issues the opt-in grant"
    );
}

/// The fallback sink renders the doctor event shape (doctor.md invariant, the
/// pipeline half): the event recorded here goes through the SAME
/// record→fallback pipeline the daemon's delivery helper uses — tool `doctor`,
/// op `doctor-apply:<name>`, provenance `doctor:<name>` in `scope_ref` — and
/// lands in the fallback sink of a volatile-journald machine (no journald
/// primary on such a shape). This test does NOT execute the doctor CLI's apply
/// path: the producer side — that `omnibusd doctor --apply` emits exactly this
/// op/params/result/scope_ref shape — is pinned at the emitting code in
/// `omnibusd/src/doctor_cli.rs`
/// (`doctor_audit_event_pins_the_producer_shape`), so a producer regression
/// cannot hide behind this test.
#[tokio::test]
async fn applied_storage_mutation_is_one_audit_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = volatile_core(dir.path(), 22);
    let report = doctor::diagnose(&core).await;
    let storage = report
        .ready_commands
        .iter()
        .find(|c| c.name == "enable journal persistence")
        .expect("the storage lever is a ready command");

    // Recorded through the same record→fallback pipeline the CLI delivery
    // goes through; the CLI's own emission and its event shape are pinned at
    // the producer (omnibusd/src/doctor_cli.rs, doctor_audit_event).
    let audited = core
        .audit
        .record(
            "doctor",
            "core",
            &format!("doctor-apply:{}", storage.name),
            &serde_json::json!({ "op": storage.name }),
            "ok",
            Some(format!("doctor:{}", storage.name)),
        )
        .await;
    assert!(
        audited || core.audit.fallback_healthy(),
        "the event is delivered"
    );

    let trail = std::fs::read_to_string(core.audit.fallback_path()).expect("audit trail");
    let event = trail
        .lines()
        .find(|line| line.contains("doctor-apply:enable journal persistence"))
        .expect("the doctor mutation is one audit event");
    assert!(event.contains(r#""tool":"doctor""#), "tool=doctor: {event}");
    assert!(
        event.contains(r#""scope_ref":"doctor:enable journal persistence""#),
        "the provenance travels in scope_ref: {event}"
    );
    assert!(
        event.contains(r#""result":"ok""#),
        "the outcome is recorded: {event}"
    );
}

/// The persistent machine keeps the honest `ok`: no storage command, no
/// journal warn — doctor proposes nothing for a fact that already holds.
#[tokio::test]
async fn persistent_journal_doctor_reports_ok_without_commands() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = build_core(
        Registry {
            journal: Some(FakeJournal::new(vec![
                common::fact(cap::JOURNAL_QUERY, "present"),
                common::fact_with(
                    cap::JOURNAL_PERSISTENT,
                    "present",
                    Some(serde_json::json!(true)),
                    None,
                    None,
                ),
            ])),
            ..Registry::default()
        },
        GrantRegistry::new(),
        ScopeManifests::default(),
        pipeline_for(&common::session(23), &dir.path().join("audit.jsonl")),
        common::platform(vec![]),
        23,
    );

    let report = doctor::diagnose(&core).await;
    let journal = report
        .findings
        .iter()
        .find(|f| f.component == "journal")
        .expect("the persistent journal is diagnosed");
    assert_eq!(journal.severity, "ok");
    assert!(
        journal.detail.contains("true"),
        "the probe fact value is rendered: {}",
        journal.detail
    );
    assert!(
        !report
            .ready_commands
            .iter()
            .any(|c| c.name == "enable journal persistence"),
        "a persistent machine needs no storage mutation"
    );
}

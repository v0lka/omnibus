//! Integration tests for roadmap 1.3.4 (audit pipeline): exactly one structured
//! event per tool call (reads included) with the full normative schema,
//! irreversibility of `params_hash`, and sink failure resilience — a broken sink
//! degrades to the file fallback without failing the tool call.

mod common;

use std::sync::Arc;

use common::{FakeJournal, FakePackages, FakeServices, args, build_core, met_floor, platform};
use omnibus_contracts::capabilities as cap;
use omnibus_contracts::dto::audit::AuditEvent;
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::audit_sink::AuditSink;
use omnibus_core::audit::{AuditPipeline, FileSink};
use omnibus_core::grants::GrantRegistry;
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;
use omnibus_core::tools;

struct BrokenSink;

#[async_trait::async_trait]
impl AuditSink for BrokenSink {
    fn kind(&self) -> &'static str {
        "broken"
    }

    async fn write(&self, _event: AuditEvent) -> Result<(), ContractError> {
        Err(ContractError::NativeFailure {
            code: 1,
            payload: String::from("sink down"),
        })
    }
}

fn full_registry() -> Registry {
    Registry {
        journal: Some(FakeJournal::new(vec![common::fact(
            cap::JOURNAL_QUERY,
            "present",
        )])),
        packages: Some(FakePackages::new(
            vec![common::fact(cap::PKG_READ, "present")],
            std::time::Duration::ZERO,
        )),
        services: Some(FakeServices::new(vec![common::fact(
            cap::SERVICE_READ,
            "present",
        )])),
        ..Registry::default()
    }
}

/// AC 1.3.4: every action of any tool — reads included — yields exactly one event
/// with the complete schema {ts, session, actor, tool, adapter, op, params_hash,
/// result, scope_ref}.
#[tokio::test]
async fn exactly_one_full_schema_event_per_dispatch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("audit.jsonl");
    let core = build_core(
        full_registry(),
        GrantRegistry::new(),
        ScopeManifests::default(),
        common::pipeline_for(&common::session(1), &path),
        platform(vec![met_floor("systemd", "261")]),
        1,
    );

    let journal_args = args(&[("limit", 5.into())]);
    let pkg_args = args(&[]);
    let svc_args = args(&[]);
    let journal = tools::dispatch(&core, "journal_query", journal_args.clone()).await;
    let packages = tools::dispatch(&core, "pkg_list", pkg_args.clone()).await;
    let services = tools::dispatch(&core, "svc_list", svc_args.clone()).await;
    assert!(journal.is_ok() && packages.is_ok() && services.is_ok());

    let events = FileSink::new(path.clone()).read_all();
    assert_eq!(events.len(), 3, "exactly one event per dispatch");

    let expected: Vec<(&str, &str, &serde_json::Map<String, serde_json::Value>)> = vec![
        ("journal_query", "fake-journald", &journal_args),
        ("pkg_list", "fake-alpm", &pkg_args),
        ("svc_list", "fake-systemd1", &svc_args),
    ];
    for (event, (tool, adapter, arguments)) in events.iter().zip(expected) {
        assert_eq!(event.tool, tool);
        assert_eq!(event.op, tool, "the audited op is the tool call itself");
        assert_eq!(event.adapter, adapter);
        assert_eq!(event.session, core.session.id.as_str());
        assert!(event.ts_epoch_ms > 0, "the event carries a real timestamp");
        assert_eq!(event.actor.uid, 1000);
        assert_eq!(event.actor.pid, 4242);
        assert_eq!(event.actor.client_name.as_deref(), Some("integration-test"));
        assert_eq!(
            event.params_hash,
            core.audit
                .hash_params(&serde_json::Value::Object(arguments.clone())),
            "the event stores the keyed hash of the arguments"
        );
        assert_eq!(event.result, "ok");
        assert_eq!(event.scope_ref, None);
        // The serialized shape carries exactly the normative fields — no more.
        let wire = serde_json::to_value(event).expect("event serializes");
        let mut keys: Vec<&str> = wire
            .as_object()
            .expect("event is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "actor",
                "adapter",
                "op",
                "params_hash",
                "result",
                "scope_ref",
                "session",
                "tool",
                "ts_epoch_ms"
            ]
        );
    }
}

/// AC 1.3.4: `params_hash` never carries raw parameters — a distinctive needle in
/// the arguments appears in no event field, only its hash does.
#[tokio::test]
async fn params_hash_never_carries_raw_parameters() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("audit.jsonl");
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
        common::pipeline_for(&common::session(2), &path),
        platform(vec![]),
        2,
    );

    let needle = "needle-v3ry-s3cret-42";
    let arguments = args(&[("unit", needle.into())]);
    let outcome = tools::dispatch(&core, "journal_query", arguments.clone()).await;
    assert!(outcome.is_ok());

    let events = FileSink::new(path.clone()).read_all();
    assert_eq!(events.len(), 1);
    for event in &events {
        let wire = serde_json::to_string(event).expect("event serializes");
        assert!(
            !wire.contains(needle),
            "raw parameters never reach the audit sink"
        );
        assert_eq!(
            event.params_hash,
            core.audit
                .hash_params(&serde_json::Value::Object(arguments.clone())),
            "the event stores the keyed hash of the arguments"
        );
    }
}

/// audit.md: parameters are never reversibly *restorable*. A candidate list of
/// low-entropy arguments (package names, unit names) hashed with the unkeyed
/// construction would leak exactly which one was used; the per-install key
/// stands between the journal and the parameters — two installs with distinct
/// state directories produce distinct digests for the same arguments, while one
/// install stays stable across its sessions.
#[tokio::test]
async fn params_hash_is_keyed_per_install() {
    let first = tempfile::tempdir().expect("tempdir");
    let second = tempfile::tempdir().expect("tempdir");
    let mk = |dir: &std::path::Path, seed: u8| {
        let session = common::session(seed);
        AuditPipeline::new(
            session.id.as_str().to_string(),
            session.actor.clone(),
            None,
            dir.join("audit.jsonl"),
        )
    };
    let arguments = serde_json::json!({ "name": "ripgrep" });
    let install_one_session_a = mk(first.path(), 1).hash_params(&arguments);
    let install_one_session_b = mk(first.path(), 2).hash_params(&arguments);
    let install_two = mk(second.path(), 3).hash_params(&arguments);
    assert_eq!(
        install_one_session_a, install_one_session_b,
        "one install, one key: stable across sessions"
    );
    assert_ne!(
        install_one_session_a, install_two,
        "distinct installs cannot be dictionary-attacked jointly"
    );
}

/// AC 1.3.4: a failing (or absent) audit sink never fails the tool call — the
/// event degrades to the file fallback and the dispatch still returns Ok.
#[tokio::test]
async fn sink_failure_degrades_to_fallback_without_failing_the_tool() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Case 1: no primary sink at all (journald absent) — degradation is visible.
    let session = common::session(3);
    let absent_path = dir.path().join("absent.jsonl");
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
        AuditPipeline::new(
            session.id.as_str().to_string(),
            session.actor.clone(),
            None,
            absent_path.clone(),
        ),
        platform(vec![]),
        3,
    );
    assert!(!core.audit.primary_present());
    let outcome = tools::dispatch(&core, "journal_query", args(&[("limit", 1.into())])).await;
    assert!(outcome.is_ok(), "the tool result is unaffected by the sink");
    let value = outcome.expect("ok dispatch").value;
    assert_eq!(value["records"].as_array().map(Vec::len), Some(1));
    assert_eq!(FileSink::new(absent_path.clone()).read_all().len(), 1);

    // Case 2: a primary sink that fails on write — same degradation, same Ok.
    let session = common::session(4);
    let fallback_path = dir.path().join("fallback.jsonl");
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
        AuditPipeline::new(
            session.id.as_str().to_string(),
            session.actor.clone(),
            Some(Arc::new(BrokenSink)),
            fallback_path.clone(),
        ),
        platform(vec![]),
        4,
    );
    assert!(core.audit.primary_present());
    let outcome = tools::dispatch(&core, "journal_query", args(&[("limit", 1.into())])).await;
    assert!(outcome.is_ok(), "a broken sink never fails the tool");
    let events = FileSink::new(fallback_path.clone()).read_all();
    assert_eq!(events.len(), 1, "the event reached the file fallback");
    assert_eq!(events[0].result, "ok");
}

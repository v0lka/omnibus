#![allow(clippy::expect_used)] // test-only file: tests may expect; the seam must not
//! The schema↔serde round-trip guard (schema.rs module doc, tool-registry.md
//! Invariants: "JSON schemas are always generated from DTOs; hand-written schemas
//! fail the build").
//!
//! For every DTO the suite builds a sample value from its *schema*, serde-
//! deserializes it back into the DTO type, and re-serializes it: a schema that
//! disagrees with serde (a drifted `snake_case`, a mismatched scalar, a rendered
//! shape serde rejects) fails the test run. Recursive DTOs (`A11yNode`) get an
//! explicit sample — the schema deliberately breaks the recursion with a
//! shape-open `Any`, which cannot carry typed children.

use omnibus_contracts::dto::{
    Bytes, Milliseconds, NamedValue, OffsetBytes, Percent, Seconds, Severity,
    app_launcher::{AppEntry, AppSpec, LaunchOutcome, OpenRequest},
    audit::{AuditActor, AuditEvent},
    caps::{
        CapabilitiesDocument, CapabilityEntry, DoctorCommand, DoctorFinding, DoctorReport,
        LimitsDocument, SessionInfo,
    },
    desktop::{
        A11yElement, ElementRequest, InvokeRequest, SetValueRequest, TreeSelector, WriteOutcome,
    },
    files::{
        DirEntry, DirListing, EntryKind, FileContent, FileReadRequest, FileWriteRequest,
        FileWriteResult, PathQuery, PathResolveRequest, ResolvedPath,
    },
    journal::{BootSelector, JournalPage, JournalQuery, JournalRecord, TailRequest},
    kb::{KbArticle, KbArticleRef, KbPageRequest, KbQuery},
    net::{NetDevice, NetStatusReport, WifiNetwork},
    packages::{
        PackageAction, PackageBrief, PackageHistoryEntry, PackageHistoryQuery, PackageInfo,
        PackageListRequest, PackageNameRequest, PackageSearch, TransactionResult, UndoResult,
        UndoSource,
    },
    portals::{
        ClipboardContent, ImageFormat, NotifyRequest, NotifyResult, ScreenshotRequest,
        ScreenshotResult, Urgency,
    },
    probe::{
        AdapterReport, CapabilityFact, DaemonLiveness, DistroFamily, DistroInfo, FloorStatus,
        ManjaroFacts, PlatformReport,
    },
    process_telemetry::{
        PowerStatus, ProcessEntry, ProcessListQuery, ProcessSnapshot, Signal, SignalRequest,
        SysStats,
    },
    services::{
        ServiceAction, ServiceActionRequest, ServiceActionResult, ServiceFilter,
        ServiceNameRequest, ServiceStatus, ServiceUnit,
    },
};
use omnibus_contracts::schema::{DtoSchema, SchemaNode, json_schema, sample};

/// One schema↔serde round trip: schema → sample → typed DTO → value → compare.
fn roundtrip<T>(tag: &'static str)
where
    T: DtoSchema + serde::de::DeserializeOwned + serde::Serialize,
{
    let node = T::schema();
    let rendered = json_schema(&node);
    if matches!(node, SchemaNode::Object(_)) {
        assert_eq!(
            rendered["type"], "object",
            "{tag}: objects render as objects"
        );
        assert_eq!(
            rendered["additionalProperties"], false,
            "{tag}: object schemas are strict"
        );
    }
    let value = sample(&node, 0);
    let back: T = serde_json::from_value(value.clone()).unwrap_or_else(|error| {
        panic!("{tag}: schema sample does not deserialize into the DTO: {error}")
    });
    let re = serde_json::to_value(&back)
        .unwrap_or_else(|error| panic!("{tag}: DTO failed to re-serialize: {error}"));
    assert_eq!(re, value, "{tag}: schema and serde disagree");
}

#[test]
fn every_dto_schema_sample_deserializes_into_the_dto() {
    // Units and shared leaf DTOs (dto/mod.rs).
    roundtrip::<Milliseconds>("Milliseconds");
    roundtrip::<Bytes>("Bytes");
    roundtrip::<Severity>("Severity");
    roundtrip::<OffsetBytes>("OffsetBytes");
    roundtrip::<Percent>("Percent");
    roundtrip::<Seconds>("Seconds");
    roundtrip::<NamedValue>("NamedValue");

    // Application launch (specs/domains/app-launch.md).
    roundtrip::<AppEntry>("AppEntry");
    roundtrip::<AppSpec>("AppSpec");
    roundtrip::<LaunchOutcome>("LaunchOutcome");
    roundtrip::<OpenRequest>("OpenRequest");

    // Process telemetry (specs/domains/process-telemetry.md).
    roundtrip::<Signal>("Signal");
    roundtrip::<ProcessListQuery>("ProcessListQuery");
    roundtrip::<ProcessEntry>("ProcessEntry");
    roundtrip::<ProcessSnapshot>("ProcessSnapshot");
    roundtrip::<SysStats>("SysStats");
    roundtrip::<PowerStatus>("PowerStatus");
    roundtrip::<SignalRequest>("SignalRequest");

    // Audit (specs/domains/audit.md).
    roundtrip::<AuditActor>("AuditActor");
    roundtrip::<AuditEvent>("AuditEvent");

    // Capabilities and doctor (caps.rs).
    roundtrip::<CapabilityEntry>("CapabilityEntry");
    roundtrip::<CapabilitiesDocument>("CapabilitiesDocument");
    roundtrip::<LimitsDocument>("LimitsDocument");
    roundtrip::<SessionInfo>("SessionInfo");
    roundtrip::<DoctorFinding>("DoctorFinding");
    roundtrip::<DoctorCommand>("DoctorCommand");
    roundtrip::<DoctorReport>("DoctorReport");

    // Files (specs/domains/files.md).
    roundtrip::<PathQuery>("PathQuery");
    roundtrip::<EntryKind>("EntryKind");
    roundtrip::<DirEntry>("DirEntry");
    roundtrip::<DirListing>("DirListing");
    roundtrip::<FileReadRequest>("FileReadRequest");
    roundtrip::<FileContent>("FileContent");
    roundtrip::<FileWriteRequest>("FileWriteRequest");
    roundtrip::<FileWriteResult>("FileWriteResult");
    roundtrip::<PathResolveRequest>("PathResolveRequest");
    roundtrip::<ResolvedPath>("ResolvedPath");

    // Journal (specs/domains/journal.md).
    roundtrip::<BootSelector>("BootSelector");
    roundtrip::<JournalQuery>("JournalQuery");
    roundtrip::<JournalRecord>("JournalRecord");
    roundtrip::<JournalPage>("JournalPage");
    roundtrip::<TailRequest>("TailRequest");

    // Knowledge base (specs/domains/knowledge.md).
    roundtrip::<KbQuery>("KbQuery");
    roundtrip::<KbArticleRef>("KbArticleRef");
    roundtrip::<KbPageRequest>("KbPageRequest");
    roundtrip::<KbArticle>("KbArticle");

    // Network status (specs/domains/network.md).
    roundtrip::<NetDevice>("NetDevice");
    roundtrip::<WifiNetwork>("WifiNetwork");
    roundtrip::<NetStatusReport>("NetStatusReport");

    // Packages (specs/domains/packages.md).
    roundtrip::<PackageBrief>("PackageBrief");
    roundtrip::<PackageInfo>("PackageInfo");
    roundtrip::<PackageSearch>("PackageSearch");
    roundtrip::<PackageListRequest>("PackageListRequest");
    roundtrip::<PackageAction>("PackageAction");
    roundtrip::<PackageHistoryEntry>("PackageHistoryEntry");
    roundtrip::<PackageHistoryQuery>("PackageHistoryQuery");
    roundtrip::<PackageNameRequest>("PackageNameRequest");
    roundtrip::<TransactionResult>("TransactionResult");
    roundtrip::<UndoSource>("UndoSource");
    roundtrip::<UndoResult>("UndoResult");

    // Portals (specs/domains/clipboard.md, notifications.md, screen-capture.md).
    roundtrip::<ClipboardContent>("ClipboardContent");
    roundtrip::<ImageFormat>("ImageFormat");
    roundtrip::<ScreenshotRequest>("ScreenshotRequest");
    roundtrip::<ScreenshotResult>("ScreenshotResult");
    roundtrip::<Urgency>("Urgency");
    roundtrip::<NotifyRequest>("NotifyRequest");
    roundtrip::<NotifyResult>("NotifyResult");

    // Platform probe (specs/architecture/platform-matrix.md).
    roundtrip::<DistroFamily>("DistroFamily");
    roundtrip::<DistroInfo>("DistroInfo");
    roundtrip::<DaemonLiveness>("DaemonLiveness");
    roundtrip::<FloorStatus>("FloorStatus");
    roundtrip::<ManjaroFacts>("ManjaroFacts");
    roundtrip::<PlatformReport>("PlatformReport");
    roundtrip::<CapabilityFact>("CapabilityFact");
    roundtrip::<AdapterReport>("AdapterReport");

    // Services (specs/domains/services.md).
    roundtrip::<ServiceUnit>("ServiceUnit");
    roundtrip::<ServiceFilter>("ServiceFilter");
    roundtrip::<ServiceStatus>("ServiceStatus");
    roundtrip::<ServiceAction>("ServiceAction");
    roundtrip::<ServiceNameRequest>("ServiceNameRequest");
    roundtrip::<ServiceActionRequest>("ServiceActionRequest");
    roundtrip::<ServiceActionResult>("ServiceActionResult");

    // Desktop a11y, minus the recursive A11yNode/A11yTree (see below).
    roundtrip::<TreeSelector>("TreeSelector");
    roundtrip::<ElementRequest>("ElementRequest");
    roundtrip::<A11yElement>("A11yElement");
    roundtrip::<SetValueRequest>("SetValueRequest");
    roundtrip::<InvokeRequest>("InvokeRequest");
    roundtrip::<WriteOutcome>("WriteOutcome");
}

/// Builds a real (serde-shaped) `A11yNode` value; the schema cannot sample this
/// generically because nested children are declared shape-open (`Any`).
fn a11y_node_value(depth: usize) -> serde_json::Value {
    let children = if depth < 2 {
        vec![a11y_node_value(depth + 1)]
    } else {
        vec![]
    };
    serde_json::json!({
        "node_ref": format!("raw#{depth}"),
        "role": "push button",
        "name": format!("node {depth}"),
        "pid": 7,
        "children": children,
    })
}

#[test]
fn a11y_node_schema_is_shape_open_and_serde_roundtrips() {
    use omnibus_contracts::dto::desktop::{A11yNode, A11yTree};

    // The recursion break renders genuinely shape-open: nested children are an
    // array whose `items` is the empty schema, so any child object passes.
    let rendered = json_schema(&A11yNode::schema());
    assert_eq!(
        rendered["properties"]["children"]["items"],
        serde_json::json!({}),
        "nested children must render shape-open, rejecting no real child"
    );

    let leaf = a11y_node_value(0);
    let node: A11yNode =
        serde_json::from_value(leaf.clone()).expect("real child array deserializes");
    assert_eq!(
        serde_json::to_value(&node).expect("A11yNode re-serializes"),
        leaf
    );

    let tree = serde_json::json!({
        "app": "sample",
        "pid": 7,
        "root": a11y_node_value(0),
        "node_count": 3,
        "truncated": false,
    });
    let typed: A11yTree = serde_json::from_value(tree.clone()).expect("tree deserializes");
    assert_eq!(
        serde_json::to_value(&typed).expect("A11yTree re-serializes"),
        tree
    );
}

#[test]
fn optional_fields_render_null_accepting_schemas() {
    // serde maps an explicit JSON null on Option<T> to None; the rendered schema
    // must accept null too, or tools/list clients reject calls serde would serve.
    let rendered = json_schema(&SchemaNode::Optional(Box::new(SchemaNode::Scalar(
        omnibus_contracts::schema::ScalarType::U32,
    ))));
    assert_eq!(
        rendered,
        serde_json::json!({ "anyOf": [{ "type": "integer" }, { "type": "null" }] })
    );
    let none: Option<u32> =
        serde_json::from_value(serde_json::Value::Null).expect("null deserializes as None");
    assert_eq!(none, None);
}

#[test]
fn optional_struct_keeps_the_nested_required_fields() {
    // `Option<Struct>` (probe.rs `manjaro: Option<ManjaroFacts>`): the field may
    // be absent or null, but when it IS present serde still requires the nested
    // struct's own fields — the rendered schema must say the same, or tools/list
    // clients accept calls the round-trip rejects.
    let rendered = json_schema(&PlatformReport::schema());
    let manjaro = rendered["properties"]["manjaro"]["anyOf"][0]
        .as_object()
        .expect("the object branch of the Optional wrapper");
    assert!(
        manjaro
            .get("required")
            .is_some_and(|r| !r.as_array().expect("array").is_empty()),
        "the nested struct keeps its required fields: {manjaro:?}"
    );
    assert_eq!(
        manjaro.get("additionalProperties"),
        Some(&serde_json::Value::Bool(false)),
        "the nested struct stays a strict object"
    );
    // And the schema disagrees-with-serde direction is visible: with every other
    // field present and valid, a hollow nested object fails serde exactly as the
    // schema's `required` predicts — the negative control is the hollow manjaro.
    let empty: Result<PlatformReport, _> = serde_json::from_value(serde_json::json!({
        "distro": { "id": "sample", "name": "sample", "release": "1", "family": "other" },
        "kernel": "k",
        "desktop": null,
        "session_kind": null,
        "systemd_version": null,
        "atspi_version": null,
        "liveness": [],
        "backends_present": [],
        "floors": [],
        "adapters_selected": [],
        "manjaro": {},
    }));
    assert!(
        empty.is_err(),
        "serde rejects a present but hollow nested struct"
    );
}

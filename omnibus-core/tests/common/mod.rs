//! Shared fakes and builders for the omnibus-core integration suites (P6: fakes
//! live near the core, never inside adapters). Every fake implements only the
//! trait methods its scenarios need and answers the rest with the honest
//! `AdapterUnavailable`. Capability facts always come from the
//! `omnibus_contracts::capabilities` constants (R4-CON), never literals.

#![allow(dead_code)] // each suite links the module and uses its own subset

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use omnibus_contracts::dto::audit::AuditActor;
use omnibus_contracts::dto::desktop::{
    A11yElement, A11yNode, A11yTree, InvokeRequest, SetValueRequest, TreeSelector, WriteOutcome,
};
use omnibus_contracts::dto::journal::{JournalPage, JournalQuery, JournalRecord, TailRequest};
use omnibus_contracts::dto::packages::{
    PackageBrief, PackageHistoryEntry, PackageHistoryQuery, PackageInfo, PackageListRequest,
    PackageNameRequest, PackageSearch, TransactionResult, UndoResult,
};
use omnibus_contracts::dto::probe::{
    AdapterReport, CapabilityFact, DistroFamily, DistroInfo, FloorStatus, PlatformReport,
};
use omnibus_contracts::dto::services::{
    ServiceActionRequest, ServiceActionResult, ServiceFilter, ServiceStatus, ServiceUnit,
};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::a11y::DesktopA11y;
use omnibus_contracts::spi::journal::Journal;
use omnibus_contracts::spi::packages::PackageManager;
use omnibus_contracts::spi::services::ServiceManager;
use omnibus_contracts::version::CONTRACT_VERSION;
use omnibus_core::Core;
use omnibus_core::audit::AuditPipeline;
use omnibus_core::grants::GrantRegistry;
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;
use omnibus_core::session::SessionContext;
use omnibus_core::write_gate::WriteGate;

// --- Builders ---------------------------------------------------------------

/// Deterministic entropy for a session (the composition root generates it
/// securely in production; tests need only distinctness between instances).
pub fn entropy(seed: u8) -> [u8; 32] {
    [seed; 32]
}

/// The audit actor of the test agent.
pub fn actor() -> AuditActor {
    AuditActor {
        uid: 1000,
        pid: 4242,
        client_name: Some(String::from("integration-test")),
    }
}

/// A fresh session context with the given seed (distinct per instance).
pub fn session(seed: u8) -> SessionContext {
    SessionContext::new(entropy(seed), actor())
}

/// The audit pipeline with the file fallback at `path` and no primary sink
/// (journald is absent in tests; degradation is asserted explicitly where needed).
pub fn pipeline_for(session: &SessionContext, path: &std::path::Path) -> AuditPipeline {
    AuditPipeline::new(
        session.id.as_str().to_string(),
        session.actor.clone(),
        None,
        path.to_path_buf(),
    )
}

/// One capability fact as an adapter would report it (ver = contract version).
pub fn fact(name: &str, status: &str) -> CapabilityFact {
    CapabilityFact {
        name: name.to_string(),
        ver: CONTRACT_VERSION.to_string(),
        status: status.to_string(),
        value: None,
        note: None,
        remedy: None,
    }
}

/// A capability fact with the optional fields filled.
pub fn fact_with(
    name: &str,
    status: &str,
    value: Option<serde_json::Value>,
    note: Option<String>,
    remedy: Option<String>,
) -> CapabilityFact {
    CapabilityFact {
        name: name.to_string(),
        ver: CONTRACT_VERSION.to_string(),
        status: status.to_string(),
        value,
        note,
        remedy,
    }
}

/// An adapter report named `adapter`, carrying `facts`.
pub fn report(adapter: &str, subsystem: &str, facts: Vec<CapabilityFact>) -> AdapterReport {
    AdapterReport {
        adapter: adapter.to_string(),
        subsystem: subsystem.to_string(),
        contract_version: CONTRACT_VERSION.to_string(),
        capabilities: facts,
        notes: vec![],
    }
}

/// A met floor row for the platform report.
pub fn met_floor(component: &str, actual: &str) -> FloorStatus {
    FloorStatus {
        component: component.to_string(),
        actual: Some(actual.to_string()),
        floor: String::from("252"),
        met: true,
        reason: None,
    }
}

/// A literal platform report (all fields are public by contract).
pub fn platform(floors: Vec<FloorStatus>) -> PlatformReport {
    PlatformReport {
        distro: DistroInfo {
            id: String::from("integration"),
            name: String::from("Integration Linux"),
            release: String::from("rolling"),
            family: DistroFamily::Other,
        },
        kernel: String::from("test-kernel"),
        desktop: None,
        session_kind: None,
        systemd_version: None,
        atspi_version: None,
        liveness: vec![],
        backends_present: vec![],
        floors,
        adapters_selected: vec![],
        manjaro: None,
    }
}

/// Wires one core instance the way the composition root would: session, registry,
/// grants, scopes, audit pipeline, process-local write gate, platform report.
pub fn build_core(
    registry: Registry,
    grants: GrantRegistry,
    scopes: ScopeManifests,
    audit: AuditPipeline,
    platform: PlatformReport,
    seed: u8,
) -> Core {
    Core::new(
        session(seed),
        registry,
        grants,
        scopes,
        audit,
        WriteGate::process_only(),
        platform,
    )
}

/// Builds a JSON argument map for `tools::dispatch`.
pub fn args(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_string(), value.clone());
    }
    map
}

// --- Fakes ------------------------------------------------------------------

/// Journal fake: counts `query` calls, optionally sleeps past the budget, and
/// returns a one-record page. `tail` stays honest (`AdapterUnavailable`).
pub struct FakeJournal {
    pub facts: Vec<CapabilityFact>,
    pub delay: Duration,
    pub query_calls: AtomicUsize,
}

impl FakeJournal {
    pub fn new(facts: Vec<CapabilityFact>) -> Arc<Self> {
        Arc::new(Self {
            facts,
            delay: Duration::ZERO,
            query_calls: AtomicUsize::new(0),
        })
    }

    pub fn slow(facts: Vec<CapabilityFact>, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            facts,
            delay,
            query_calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Journal for FakeJournal {
    fn report(&self) -> AdapterReport {
        report("fake-journald", "journal", self.facts.clone())
    }

    async fn query(&self, _filter: JournalQuery) -> Result<JournalPage, ContractError> {
        self.query_calls.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(JournalPage {
            records: vec![JournalRecord {
                ts_epoch_ms: 1,
                boot_id: None,
                unit: Some(String::from("unit-a")),
                severity: None,
                message: String::from("integration record"),
            }],
            truncated: false,
        })
    }

    async fn tail(&self, _request: TailRequest) -> Result<Vec<JournalRecord>, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }
}

/// Package manager fake: `list` returns one brief; `install` counts calls,
/// records enter/leave trace lines (for write-gate serialization checks) and
/// optionally sleeps past the budget. Everything else is `AdapterUnavailable`.
pub struct FakePackages {
    pub facts: Vec<CapabilityFact>,
    pub install_delay: Duration,
    pub install_calls: AtomicUsize,
    trace: std::sync::Mutex<Vec<String>>,
}

impl FakePackages {
    pub fn new(facts: Vec<CapabilityFact>, install_delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            facts,
            install_delay,
            install_calls: AtomicUsize::new(0),
            trace: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The enter/leave trace of `install` in push order (critical sections push
    /// under the same mutex, so the vector order is the factual order).
    pub fn trace(&self) -> Vec<String> {
        self.trace.lock().expect("trace lock").clone()
    }
}

#[async_trait]
impl PackageManager for FakePackages {
    fn report(&self) -> AdapterReport {
        report("fake-alpm", "packages", self.facts.clone())
    }

    async fn list(&self, _request: PackageListRequest) -> Result<Vec<PackageBrief>, ContractError> {
        Ok(vec![PackageBrief {
            name: String::from("demo"),
            version: String::from("1.0"),
            description: None,
            repo: None,
        }])
    }

    async fn info(&self, _name: String) -> Result<PackageInfo, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }

    async fn search(&self, _query: PackageSearch) -> Result<Vec<PackageBrief>, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }

    async fn install(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        self.install_calls.fetch_add(1, Ordering::SeqCst);
        self.trace
            .lock()
            .expect("trace lock")
            .push(format!("enter:{}", request.name));
        if !self.install_delay.is_zero() {
            tokio::time::sleep(self.install_delay).await;
        }
        self.trace
            .lock()
            .expect("trace lock")
            .push(format!("leave:{}", request.name));
        Ok(TransactionResult {
            name: request.name,
            version_applied: Some(String::from("1.0")),
        })
    }

    async fn remove(
        &self,
        _request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }

    async fn history(
        &self,
        _query: PackageHistoryQuery,
    ) -> Result<Vec<PackageHistoryEntry>, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }

    async fn undo(&self, _request: PackageNameRequest) -> Result<UndoResult, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }
}

/// Service manager fake: `list` returns one unit; `status`/`apply` stay honest.
pub struct FakeServices {
    pub facts: Vec<CapabilityFact>,
}

impl FakeServices {
    pub fn new(facts: Vec<CapabilityFact>) -> Arc<Self> {
        Arc::new(Self { facts })
    }
}

#[async_trait]
impl ServiceManager for FakeServices {
    fn report(&self) -> AdapterReport {
        report("fake-systemd1", "services", self.facts.clone())
    }

    async fn list(&self, _filter: ServiceFilter) -> Result<Vec<ServiceUnit>, ContractError> {
        Ok(vec![ServiceUnit {
            name: String::from("demo.service"),
            unit_type: String::from("service"),
            description: String::from("Demo unit"),
            active_state: String::from("active"),
            sub_state: String::from("running"),
            enabled: Some(true),
        }])
    }

    async fn status(&self, _unit: String) -> Result<ServiceStatus, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }

    async fn apply(
        &self,
        _request: ServiceActionRequest,
    ) -> Result<ServiceActionResult, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }
}

/// Desktop a11y fake: `tree` returns a one-node tree whose `node_ref` the core
/// swaps for a minted token; every other method stays `AdapterUnavailable` (the
/// element-stale scenario never reaches it).
pub struct FakeA11y {
    pub facts: Vec<CapabilityFact>,
}

impl FakeA11y {
    pub fn new(facts: Vec<CapabilityFact>) -> Arc<Self> {
        Arc::new(Self { facts })
    }
}

#[async_trait]
impl DesktopA11y for FakeA11y {
    fn report(&self) -> AdapterReport {
        report("fake-atspi", "desktop", self.facts.clone())
    }

    async fn tree(&self, _selector: TreeSelector) -> Result<A11yTree, ContractError> {
        Ok(A11yTree {
            app: String::from("demo"),
            pid: Some(11),
            root: A11yNode {
                node_ref: String::from("app#1/entry#1"),
                role: String::from("entry"),
                name: String::from("Name field"),
                pid: Some(11),
                children: vec![],
            },
            node_count: 1,
            truncated: false,
        })
    }

    async fn element(&self, _node_ref: String) -> Result<A11yElement, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }

    async fn read_attr(
        &self,
        _node_ref: String,
        _attribute: String,
    ) -> Result<String, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }

    async fn set_value(&self, _request: SetValueRequest) -> Result<WriteOutcome, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }

    async fn invoke(&self, _request: InvokeRequest) -> Result<WriteOutcome, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }
}

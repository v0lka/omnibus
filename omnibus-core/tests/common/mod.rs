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
use omnibus_contracts::capabilities as cap;
use omnibus_contracts::dto::audit::AuditActor;
use omnibus_contracts::dto::desktop::{
    A11yElement, A11yNode, A11yTree, InvokeRequest, SetValueRequest, TreeSelector, WriteOutcome,
};
use omnibus_contracts::dto::files::{
    DirListing, FileContent, FileReadRequest, FileWriteRequest, FileWriteResult, PathQuery,
    PathResolveRequest, ResolvedPath,
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
use omnibus_contracts::spi::files::FileScope;
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
    SessionContext::try_new(entropy(seed), actor())
        .expect("a seeded non-zero key is a valid mint key")
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
    /// When set, `install` answers the honest write-path indeterminate
    /// failure — the shape real adapters report when their own (only) write
    /// budget elapses; the core does not timer writes (`tools::dispatch`).
    pub install_fails: bool,
    pub install_calls: AtomicUsize,
    trace: std::sync::Mutex<Vec<String>>,
}

impl FakePackages {
    pub fn new(facts: Vec<CapabilityFact>, install_delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            facts,
            install_delay,
            install_fails: false,
            install_calls: AtomicUsize::new(0),
            trace: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// An `install` that fails with the honest indeterminate-write outcome.
    pub fn failing_install(facts: Vec<CapabilityFact>) -> Arc<Self> {
        Arc::new(Self {
            facts,
            install_delay: Duration::ZERO,
            install_fails: true,
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
            source: omnibus_contracts::dto::packages::PackageSource::Native,
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
        if self.install_fails {
            self.trace
                .lock()
                .expect("trace lock")
                .push(format!("leave:{}", request.name));
            return Err(ContractError::NativeFailure {
                code: -7,
                payload: String::from(
                    "timeout-indeterminate: budget elapsed; the write may still have landed",
                ),
            });
        }
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
    /// When set, `element` answers with this element echoing the raw ref;
    /// otherwise it stays honest (`AdapterUnavailable`).
    pub element_response: Option<A11yElement>,
    /// When set, `set_value`/`invoke` answer with this outcome echoing the raw
    /// ref; otherwise `AdapterUnavailable`.
    pub write_outcome: Option<WriteOutcome>,
}

impl FakeA11y {
    pub fn new(facts: Vec<CapabilityFact>) -> Arc<Self> {
        Arc::new(Self {
            facts,
            element_response: None,
            write_outcome: None,
        })
    }

    /// A fake that answers `desktop.element` (echoing the resolved raw ref).
    pub fn with_element(facts: Vec<CapabilityFact>, element: A11yElement) -> Arc<Self> {
        Arc::new(Self {
            facts,
            element_response: Some(element),
            write_outcome: None,
        })
    }

    /// A fake that also answers the typed writes (echoing the resolved raw ref).
    pub fn with_writes(
        facts: Vec<CapabilityFact>,
        element: A11yElement,
        outcome: WriteOutcome,
    ) -> Arc<Self> {
        Arc::new(Self {
            facts,
            element_response: Some(element),
            write_outcome: Some(outcome),
        })
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

    async fn element(&self, node_ref: String) -> Result<A11yElement, ContractError> {
        match &self.element_response {
            Some(element) => {
                let mut element = element.clone();
                element.node_ref = node_ref;
                Ok(element)
            }
            None => Err(ContractError::AdapterUnavailable),
        }
    }

    async fn read_attr(
        &self,
        _node_ref: String,
        _attribute: String,
    ) -> Result<String, ContractError> {
        Err(ContractError::AdapterUnavailable)
    }

    async fn set_value(&self, request: SetValueRequest) -> Result<WriteOutcome, ContractError> {
        match &self.write_outcome {
            Some(outcome) => Ok(WriteOutcome {
                node_ref: request.node_ref,
                applied: outcome.applied,
            }),
            None => Err(ContractError::AdapterUnavailable),
        }
    }

    async fn invoke(&self, request: InvokeRequest) -> Result<WriteOutcome, ContractError> {
        match &self.write_outcome {
            Some(outcome) => Ok(WriteOutcome {
                node_ref: request.node_ref,
                applied: outcome.applied,
            }),
            None => Err(ContractError::AdapterUnavailable),
        }
    }
}

/// Files fake: answers list/read/write/resolve for any path it is handed — the
/// fs suites exercise the core's scope decisions, so the fake never refuses.
pub struct FakeFiles {
    pub facts: Vec<CapabilityFact>,
}

impl FakeFiles {
    pub fn new(facts: Vec<CapabilityFact>) -> Arc<Self> {
        Arc::new(Self { facts })
    }
}

#[async_trait]
impl FileScope for FakeFiles {
    fn report(&self) -> AdapterReport {
        report("fake-fs-xdg", "files", self.facts.clone())
    }

    async fn list(&self, query: PathQuery) -> Result<DirListing, ContractError> {
        Ok(DirListing {
            path: query.path,
            entries: vec![],
            truncated: false,
        })
    }

    async fn read(&self, request: FileReadRequest) -> Result<FileContent, ContractError> {
        Ok(FileContent {
            path: request.path,
            size_bytes: 4,
            data_base64: String::from("ZGF0YQ=="),
            text: Some(String::from("data")),
            truncated: false,
        })
    }

    async fn write(&self, request: FileWriteRequest) -> Result<FileWriteResult, ContractError> {
        Ok(FileWriteResult {
            path: request.path,
            bytes_written: 4,
        })
    }

    async fn resolve(&self, request: PathResolveRequest) -> Result<ResolvedPath, ContractError> {
        Ok(ResolvedPath {
            path: request.path.clone(),
            canonical: request.path,
            inside_home: true,
            read_allowed: true,
            write_allowed: true,
        })
    }
}

/// App-launcher fake: `list` returns one entry; `launch` counts calls and
/// echoes the `app_id` in the transient-unit name (the observation join key);
/// `open` counts calls. The received `AppSpec.args` of the last launch are
/// kept verbatim so the tests can assert the typed list crossed the seam
/// unmodified. The facts mirror the real adapter's report (including the R3
/// note naming a hidden `app.open` when the opener is absent).
pub struct FakeAppLauncher {
    pub facts: Vec<CapabilityFact>,
    pub launch_calls: AtomicUsize,
    pub open_calls: AtomicUsize,
    pub last_launch_args: std::sync::Mutex<Vec<String>>,
}

impl FakeAppLauncher {
    pub fn new(facts: Vec<CapabilityFact>) -> Arc<Self> {
        Arc::new(Self {
            facts,
            launch_calls: AtomicUsize::new(0),
            open_calls: AtomicUsize::new(0),
            last_launch_args: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The last launch's argument list, verbatim.
    pub fn last_launch_args(&self) -> Vec<String> {
        self.last_launch_args.lock().expect("args lock").clone()
    }
}

#[async_trait]
impl omnibus_contracts::spi::app_launcher::AppLauncher for FakeAppLauncher {
    fn report(&self) -> AdapterReport {
        report("fake-launch", "app-launch", self.facts.clone())
    }

    async fn list(
        &self,
    ) -> Result<Vec<omnibus_contracts::dto::app_launcher::AppEntry>, ContractError> {
        Ok(vec![omnibus_contracts::dto::app_launcher::AppEntry {
            app_id: String::from("demo.desktop"),
            name: String::from("Demo Application"),
            exec: String::from("/usr/bin/demo"),
            icon: None,
            no_display: false,
        }])
    }

    async fn launch(
        &self,
        spec: omnibus_contracts::dto::app_launcher::AppSpec,
    ) -> Result<omnibus_contracts::dto::app_launcher::LaunchOutcome, ContractError> {
        self.launch_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_launch_args.lock().expect("args lock") = spec.args.clone();
        Ok(omnibus_contracts::dto::app_launcher::LaunchOutcome {
            unit_name: format!("app-{}-0000000000000000.service", spec.app_id),
        })
    }

    async fn open(
        &self,
        _request: omnibus_contracts::dto::app_launcher::OpenRequest,
    ) -> Result<(), ContractError> {
        self.open_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Process-telemetry fake (roadmap 2.8.2/2.8.3): the reads return one honest
/// process and one sys-stats document; `signal` counts calls and succeeds —
/// the core's grant gate is the surface under test, so the fake stays on the
/// happy path beyond it. The facts mirror the real adapter's report shape
/// (ps.read / sys.read / ps.signal always, power.read only when the stand
/// has UPower + a battery).
pub struct FakeProcessTelemetry {
    pub facts: Vec<CapabilityFact>,
    pub signal_calls: AtomicUsize,
    pub last_signalled_pid: std::sync::Mutex<Option<u32>>,
}

impl FakeProcessTelemetry {
    pub fn new(facts: Vec<CapabilityFact>) -> Arc<Self> {
        Arc::new(Self {
            facts,
            signal_calls: AtomicUsize::new(0),
            last_signalled_pid: std::sync::Mutex::new(None),
        })
    }

    pub fn signal_calls(&self) -> usize {
        self.signal_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl omnibus_contracts::spi::process_telemetry::ProcessTelemetry for FakeProcessTelemetry {
    fn report(&self) -> AdapterReport {
        report("fake-proc", "process-telemetry", self.facts.clone())
    }

    async fn list(
        &self,
        _query: omnibus_contracts::dto::process_telemetry::ProcessListQuery,
    ) -> Result<Vec<omnibus_contracts::dto::process_telemetry::ProcessEntry>, ContractError> {
        use omnibus_contracts::dto::{Bytes, Percent};
        Ok(vec![
            omnibus_contracts::dto::process_telemetry::ProcessEntry {
                pid: 4321,
                name: String::from("demo-proc"),
                state: String::from("S"),
                cpu_percent: Percent(1),
                rss_bytes: Bytes(4096),
                cgroup: Some(String::from(
                    "/user.slice/user-1000.slice/user@1000.service/session-1.scope",
                )),
            },
        ])
    }

    async fn info(
        &self,
        pid: u32,
    ) -> Result<omnibus_contracts::dto::process_telemetry::ProcessSnapshot, ContractError> {
        use omnibus_contracts::dto::{Bytes, Milliseconds, Percent};
        if pid == 4321 {
            return Ok(omnibus_contracts::dto::process_telemetry::ProcessSnapshot {
                pid,
                name: String::from("demo-proc"),
                state: String::from("S"),
                cpu_percent: Percent(1),
                rss_bytes: Bytes(4096),
                cgroup: None,
                command_line: Some(String::from("demo-proc --idle")),
                start_time_epoch_ms: Milliseconds(1_700_000_000_000),
            });
        }
        Err(ContractError::NativeFailure {
            code: 1,
            payload: String::from("no-such-process"),
        })
    }

    async fn sys_stats(
        &self,
    ) -> Result<omnibus_contracts::dto::process_telemetry::SysStats, ContractError> {
        use omnibus_contracts::dto::{Bytes, Percent, Seconds};
        Ok(omnibus_contracts::dto::process_telemetry::SysStats {
            cpu_percent: Percent(12),
            load_avg_1m: 0.42,
            load_avg_5m: 0.4,
            load_avg_15m: 0.36,
            mem_total_bytes: Bytes(8_000_000_000),
            mem_used_bytes: Bytes(4_000_000_000),
            swap_total_bytes: Bytes(2_000_000_000),
            swap_used_bytes: Bytes(0),
            uptime_seconds: Seconds(1_000),
        })
    }

    async fn power_status(
        &self,
    ) -> Result<omnibus_contracts::dto::process_telemetry::PowerStatus, ContractError> {
        // Present only when the stand's facts carry power.read — the same
        // honesty the real adapter applies.
        if self.facts.iter().any(|f| f.name == cap::POWER_READ) {
            Ok(omnibus_contracts::dto::process_telemetry::PowerStatus {
                on_battery: false,
                battery_percent: Some(omnibus_contracts::dto::Percent(88)),
                seconds_until_empty: None,
                seconds_until_full: Some(omnibus_contracts::dto::Seconds(1_800)),
            })
        } else {
            Err(ContractError::CapabilityRequired)
        }
    }

    async fn signal(
        &self,
        request: omnibus_contracts::dto::process_telemetry::SignalRequest,
    ) -> Result<(), ContractError> {
        self.signal_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_signalled_pid.lock().expect("pid lock") = Some(request.pid);
        Ok(())
    }
}

// --- Packages-aggregation fakes (v1.5, roadmap 2.9.1) -----------------------

/// The native manager fake in the two aggregate fixture shapes (the roadmap's
/// "alpm+flatpak" and "apt+flatpak" stands): list/info/search/history answer
/// from a fixed table; writes count calls (the write-routing assertion) and
/// `install`/`remove` succeed; `undo` is honest per shape — alpm answers the
/// package error, apt the no-undo ladder refusal. A name outside the table is
/// the honest `pkg-not-found` failure, the fall-through trigger of the
/// aggregated `pkg_info`.
pub struct FakeNativePackages {
    adapter: String,
    undo_expected: bool,
    packages: Vec<PackageBrief>,
    pub install_calls: AtomicUsize,
    pub remove_calls: AtomicUsize,
    pub undo_calls: AtomicUsize,
}

impl FakeNativePackages {
    /// The alpm aggregate shape: `pkg.undo = cache-downgrade`.
    pub fn alpm_shape(entries: &[(&str, &str)]) -> Arc<Self> {
        Arc::new(Self::new("alpm", true, entries))
    }

    /// The apt aggregate shape: `pkg.undo = none` (the ladder hides the tool).
    pub fn apt_shape(entries: &[(&str, &str)]) -> Arc<Self> {
        Arc::new(Self::new("apt", false, entries))
    }

    fn new(adapter: &str, undo_expected: bool, entries: &[(&str, &str)]) -> Self {
        Self {
            adapter: String::from(adapter),
            undo_expected,
            packages: entries
                .iter()
                .map(|(name, version)| PackageBrief {
                    name: (*name).to_string(),
                    version: (*version).to_string(),
                    description: Some(format!("native package {name}")),
                    repo: None,
                    source: omnibus_contracts::dto::packages::PackageSource::Native,
                })
                .collect(),
            install_calls: AtomicUsize::new(0),
            remove_calls: AtomicUsize::new(0),
            undo_calls: AtomicUsize::new(0),
        }
    }

    fn brief_of(&self, name: &str) -> Option<&PackageBrief> {
        self.packages.iter().find(|p| p.name == name)
    }
}

#[async_trait]
impl PackageManager for FakeNativePackages {
    fn report(&self) -> AdapterReport {
        let undo_fact = if self.undo_expected {
            common_undo_fact(
                "present",
                Some(omnibus_contracts::capabilities::PkgUndoValue::CacheDowngrade.to_json()),
            )
        } else {
            // The honest no-undo shape: an unrecognized ladder status, so the
            // core hides pkg_undo (the apt adapter's convention).
            common_undo_fact(
                "none",
                Some(omnibus_contracts::capabilities::PkgUndoValue::None.to_json()),
            )
        };
        report(
            &self.adapter,
            "packages",
            vec![
                fact(cap::PKG_READ, "present"),
                fact(cap::PKG_WRITE, "present"),
                undo_fact,
            ],
        )
    }

    async fn list(&self, _request: PackageListRequest) -> Result<Vec<PackageBrief>, ContractError> {
        Ok(self.packages.clone())
    }

    async fn info(&self, name: String) -> Result<PackageInfo, ContractError> {
        match self.brief_of(&name) {
            Some(brief) => Ok(PackageInfo {
                name: brief.name.clone(),
                version: brief.version.clone(),
                description: brief.description.clone(),
                repo: None,
                size_bytes: Some(1024),
                install_reason: Some(String::from("explicit")),
                depends: vec![],
                required_by: vec![],
                version_available: None,
            }),
            None => Err(ContractError::NativeFailure {
                code: 4,
                payload: format!("pkg-not-found: {name} is neither installed nor in a sync db"),
            }),
        }
    }

    async fn search(&self, query: PackageSearch) -> Result<Vec<PackageBrief>, ContractError> {
        let limit = query
            .limit
            .and_then(|l| usize::try_from(l).ok())
            .unwrap_or(usize::MAX);
        Ok(self
            .packages
            .iter()
            .filter(|p| p.name.contains(&query.query))
            .take(limit)
            .cloned()
            .collect())
    }

    async fn install(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        self.install_calls.fetch_add(1, Ordering::SeqCst);
        Ok(TransactionResult {
            name: request.name,
            version_applied: Some(String::from("9.9")),
        })
    }

    async fn remove(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        self.remove_calls.fetch_add(1, Ordering::SeqCst);
        Ok(TransactionResult {
            name: request.name,
            version_applied: None,
        })
    }

    async fn history(
        &self,
        _query: PackageHistoryQuery,
    ) -> Result<Vec<PackageHistoryEntry>, ContractError> {
        Ok(vec![PackageHistoryEntry {
            ts_epoch_ms: 1,
            action: omnibus_contracts::dto::packages::PackageAction::Upgrade,
            name: self
                .packages
                .first()
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            from_version: Some(String::from("0.9")),
            to_version: Some(String::from("1.0")),
        }])
    }

    async fn undo(&self, _request: PackageNameRequest) -> Result<UndoResult, ContractError> {
        self.undo_calls.fetch_add(1, Ordering::SeqCst);
        if self.undo_expected {
            Err(ContractError::NativeFailure {
                code: 3,
                payload: String::from("undo-no-cache-version: previous version not in cache"),
            })
        } else {
            // The apt no-undo refusal: before any native work, never
            // mid-transaction (specs/domains/packages.md).
            Err(ContractError::CapabilityRequired)
        }
    }
}

/// The inventory fake (the flatpak/snap aggregate shape): mirrors the real
/// adapter's contract — source-stamped list rows, exact-match `info`,
/// `Unsupported` writes — with one counter across the write-shaped methods
/// for the never-invoked assertion.
pub struct FakeInventoryPackages {
    packages: Vec<PackageBrief>,
    pub write_attempts: AtomicUsize,
}

impl FakeInventoryPackages {
    /// Rows shaped `(store, name, version)`: store "flatpak" or "snap".
    pub fn new(entries: &[(&str, &str, &str)]) -> Arc<Self> {
        Arc::new(Self {
            packages: entries
                .iter()
                .map(|(store, name, version)| PackageBrief {
                    name: (*name).to_string(),
                    version: (*version).to_string(),
                    description: None,
                    repo: (*store == "flatpak").then(|| String::from("flathub")),
                    source: if *store == "flatpak" {
                        omnibus_contracts::dto::packages::PackageSource::Flatpak
                    } else {
                        omnibus_contracts::dto::packages::PackageSource::Snap
                    },
                })
                .collect(),
            write_attempts: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl PackageManager for FakeInventoryPackages {
    fn report(&self) -> AdapterReport {
        report(
            "flatpak",
            "packages",
            vec![fact_with(
                cap::PKG_READ,
                "present",
                None,
                Some(String::from(
                    "read-only flatpak/snap inventory (list/info only); install/remove/undo are \
                     not declared for these sources — writes belong to the native manager",
                )),
                None,
            )],
        )
    }

    async fn list(&self, _request: PackageListRequest) -> Result<Vec<PackageBrief>, ContractError> {
        Ok(self.packages.clone())
    }

    async fn info(&self, name: String) -> Result<PackageInfo, ContractError> {
        match self.packages.iter().find(|p| p.name == name) {
            Some(brief) => Ok(PackageInfo {
                name: brief.name.clone(),
                version: brief.version.clone(),
                description: None,
                repo: brief.repo.clone(),
                size_bytes: None,
                install_reason: None,
                depends: vec![],
                required_by: vec![],
                version_available: None,
            }),
            None => Err(ContractError::NativeFailure {
                code: 2,
                payload: format!("pkg-not-found: {name} is not in the flatpak/snap inventory"),
            }),
        }
    }

    async fn search(&self, _query: PackageSearch) -> Result<Vec<PackageBrief>, ContractError> {
        self.write_attempts.fetch_add(1, Ordering::SeqCst);
        Err(ContractError::Unsupported {
            reason: String::from("the flatpak/snap inventory implements list/info only"),
        })
    }

    async fn install(
        &self,
        _request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        self.write_attempts.fetch_add(1, Ordering::SeqCst);
        Err(ContractError::Unsupported {
            reason: String::from("flatpak/snap sources declare no write operations"),
        })
    }

    async fn remove(
        &self,
        _request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        self.write_attempts.fetch_add(1, Ordering::SeqCst);
        Err(ContractError::Unsupported {
            reason: String::from("flatpak/snap sources declare no write operations"),
        })
    }

    async fn history(
        &self,
        _query: PackageHistoryQuery,
    ) -> Result<Vec<PackageHistoryEntry>, ContractError> {
        self.write_attempts.fetch_add(1, Ordering::SeqCst);
        Err(ContractError::Unsupported {
            reason: String::from("the flatpak/snap inventory has no transaction log"),
        })
    }

    async fn undo(&self, _request: PackageNameRequest) -> Result<UndoResult, ContractError> {
        self.write_attempts.fetch_add(1, Ordering::SeqCst);
        Err(ContractError::Unsupported {
            reason: String::from("flatpak/snap sources declare no write operations"),
        })
    }
}

/// A pkg.undo capability fact with the typed value (the native shapes report
/// it; the suite reads the value back through `PkgUndoValue::from_json`).
fn common_undo_fact(status: &str, value: Option<serde_json::Value>) -> CapabilityFact {
    fact_with(cap::PKG_UNDO, status, value, None, None)
}

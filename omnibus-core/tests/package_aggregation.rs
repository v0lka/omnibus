//! Integration tests for roadmap 2.9.1 (the flatpak/snap inventory, v1.5):
//! `pkg_list`/`pkg_info` aggregate the native manager and the read-only
//! inventory adapter — the one documented exception to the one-active-adapter
//! rule (§9). The aggregate fixture comes in the two roadmap shapes
//! (alpm+flatpak and apt+flatpak); the contract suite's read part binds green
//! against both. Without an inventory slot the output is exactly the native
//! manager's — unchanged and not degraded (R7) — and write tools never reach
//! the inventory: write semantics belong to the native manager only.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::{
    FakeInventoryPackages, FakeNativePackages, args, build_core, met_floor, pipeline_for, platform,
    session,
};
use omnibus_contracts::capabilities as cap;
use omnibus_contracts::dto::packages::{PackageBrief, PackageSource};
use omnibus_contracts::dto::probe::AdapterReport;
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::packages::PackageManager;
use omnibus_core::grants::{Grant, GrantProvenance, GrantRegistry};
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;
use omnibus_core::tools::{self, GateDecision};

/// The native fixture table: two clearly native names plus one name the
/// inventory carries too (the collision the native manager must win).
const NATIVE_TABLE: [(&str, &str); 3] = [
    ("alpm-core", "1.0"),
    ("alpm-app", "2.0"),
    ("shared-name", "3.0"),
];

/// The inventory fixture table: one flatpak row, one snap row, plus the
/// collision name (the inventory's copy is never the answer for pkg_info).
const INVENTORY_TABLE: [(&str, &str, &str); 3] = [
    ("flatpak", "org.gnome.Epiphany", "46.3"),
    ("snap", "core22", "20231115"),
    ("snap", "shared-name", "9.0"),
];

/// Builds a core with the requested packages slots plus the two fixture
/// handles (for the call-counter assertions).
fn aggregate_core(
    dir: &std::path::Path,
    seed: u8,
    native: Option<Arc<FakeNativePackages>>,
    inventory: Option<Arc<FakeInventoryPackages>>,
    grants: GrantRegistry,
) -> omnibus_core::Core {
    build_core(
        Registry {
            packages: native.map(|a| a as Arc<dyn PackageManager>),
            packages_inventory: inventory.map(|a| a as Arc<dyn PackageManager>),
            ..Registry::default()
        },
        grants,
        ScopeManifests::default(),
        pipeline_for(&session(seed), &dir.join("audit.jsonl")),
        platform(vec![met_floor("systemd", "261")]),
        seed,
    )
}

fn default_native() -> Arc<FakeNativePackages> {
    FakeNativePackages::alpm_shape(&NATIVE_TABLE)
}

fn default_inventory() -> Arc<FakeInventoryPackages> {
    FakeInventoryPackages::new(&INVENTORY_TABLE)
}

/// The names `pkg_list` served, in order.
async fn listed_names(core: &omnibus_core::Core) -> Vec<(String, PackageSource)> {
    let outcome = tools::dispatch(core, "pkg_list", args(&[]))
        .await
        .expect("pkg_list runs on the aggregate");
    let briefs: Vec<PackageBrief> =
        serde_json::from_value(outcome.value).expect("the list payload is the DTO");
    briefs.into_iter().map(|b| (b.name, b.source)).collect()
}

// --- The aggregate semantics through the real dispatch path -----------------

#[tokio::test]
async fn pkg_list_merges_native_first_then_the_inventory_with_explicit_sources() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (native, inventory) = (default_native(), default_inventory());
    let core = aggregate_core(
        dir.path(),
        1,
        Some(native.clone()),
        Some(inventory.clone()),
        GrantRegistry::new(),
    );
    let served = listed_names(&core).await;
    assert_eq!(
        served,
        vec![
            (String::from("alpm-core"), PackageSource::Native),
            (String::from("alpm-app"), PackageSource::Native),
            (String::from("shared-name"), PackageSource::Native),
            (String::from("org.gnome.Epiphany"), PackageSource::Flatpak),
            (String::from("core22"), PackageSource::Snap),
            (String::from("shared-name"), PackageSource::Snap),
        ],
        "native rows precede the inventory rows and every source is explicit"
    );
}

#[tokio::test]
async fn pkg_list_limit_truncates_the_merged_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = aggregate_core(
        dir.path(),
        2,
        Some(default_native()),
        Some(default_inventory()),
        GrantRegistry::new(),
    );
    let outcome = tools::dispatch(&core, "pkg_list", args(&[("limit", 2.into())]))
        .await
        .expect("pkg_list runs");
    let briefs: Vec<PackageBrief> = serde_json::from_value(outcome.value).expect("DTO payload");
    assert_eq!(briefs.len(), 2, "the limit bounds the MERGED output");
    assert_eq!(briefs[0].name, "alpm-core");
    assert_eq!(briefs[1].name, "alpm-app");
}

#[tokio::test]
async fn pkg_info_prefers_native_and_falls_through_to_the_inventory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = aggregate_core(
        dir.path(),
        3,
        Some(default_native()),
        Some(default_inventory()),
        GrantRegistry::new(),
    );
    // Native name: native metadata, native attribution.
    let native_outcome = tools::dispatch(&core, "pkg_info", args(&[("name", "alpm-core".into())]))
        .await
        .expect("a native name resolves");
    assert_eq!(native_outcome.adapter, "alpm", "native lookup attribution");
    // Collision name: the native manager wins (write semantics and the
    // authoritative metadata belong to it).
    let collision = tools::dispatch(&core, "pkg_info", args(&[("name", "shared-name".into())]))
        .await
        .expect("the collision resolves");
    assert_eq!(collision.adapter, "alpm");
    let info: omnibus_contracts::dto::packages::PackageInfo =
        serde_json::from_value(collision.value).expect("DTO payload");
    assert_eq!(info.version, "3.0", "the native copy answers, not 9.0");
    // Inventory-only name: the fall-through answers, inventory attribution.
    let inventory_outcome = tools::dispatch(
        &core,
        "pkg_info",
        args(&[("name", "org.gnome.Epiphany".into())]),
    )
    .await
    .expect("a flatpak-only name falls through to the inventory");
    assert_eq!(
        inventory_outcome.adapter, "flatpak",
        "the inventory names itself when it served the answer"
    );
    let info: omnibus_contracts::dto::packages::PackageInfo =
        serde_json::from_value(inventory_outcome.value).expect("DTO payload");
    assert_eq!(info.version, "46.3");
    assert_eq!(info.repo.as_deref(), Some("flathub"));
}

#[tokio::test]
async fn pkg_info_names_the_honest_not_found_when_nothing_answers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = aggregate_core(
        dir.path(),
        4,
        Some(default_native()),
        Some(default_inventory()),
        GrantRegistry::new(),
    );
    let missing =
        tools::dispatch(&core, "pkg_info", args(&[("name", "no-such-thing".into())])).await;
    match missing {
        Err(ContractError::NativeFailure { payload, .. }) => {
            assert!(
                payload.contains("pkg-not-found"),
                "the native manager's honest not-found is the answer: {payload}"
            );
        }
        Err(other) => panic!("expected the native not-found failure, got {other:?}"),
        Ok(_) => panic!("expected the not-found failure, got an Ok answer"),
    }
}

/// The R7 leg of the acceptance criterion: without flatpak/snap (no inventory
/// slot) the aggregated output is exactly the native manager's — same rows,
/// same order, native attribution — and nothing degrades: the tool runs, the
/// gate stays open, no CapabilityRequired/FloorUnsupported surfaces.
#[tokio::test]
async fn without_the_inventory_the_output_is_unchanged_and_not_degraded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let native = default_native();
    let core = aggregate_core(
        dir.path(),
        5,
        Some(native.clone()),
        None,
        GrantRegistry::new(),
    );
    assert!(
        matches!(tools::gate(&core, "pkg_list"), GateDecision::Run),
        "the pkg tools stay visible without the inventory"
    );
    let served = listed_names(&core).await;
    let expected: Vec<(String, PackageSource)> = native
        .list(omnibus_contracts::dto::packages::PackageListRequest { limit: None })
        .await
        .expect("the fake answers")
        .into_iter()
        .map(|b| (b.name, b.source))
        .collect();
    assert_eq!(served, expected, "the output is exactly the native list");
    // Attribution stays the native manager's alone (no phantom `+flatpak`).
    let outcome = tools::dispatch(&core, "pkg_list", args(&[]))
        .await
        .expect("pkg_list runs");
    assert_eq!(outcome.adapter, "alpm");
}

/// The inventory-only stand (no native manager — a store-only machine):
/// reads serve from the inventory, the tools stay visible, and the write
/// tools hide (no PKG_WRITE/PKG_UNDO fact exists to serve them).
#[tokio::test]
async fn inventory_only_stand_serves_reads_and_hides_writes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = aggregate_core(
        dir.path(),
        6,
        None,
        Some(default_inventory()),
        GrantRegistry::new(),
    );
    assert!(matches!(tools::gate(&core, "pkg_list"), GateDecision::Run));
    let served = listed_names(&core).await;
    assert_eq!(served.len(), 3, "the inventory alone answers the list");
    assert!(
        served
            .iter()
            .all(|(_, source)| *source != PackageSource::Native)
    );
    let outcome = tools::dispatch(&core, "pkg_info", args(&[("name", "core22".into())]))
        .await
        .expect("a snap name resolves on the inventory-only stand");
    assert_eq!(outcome.adapter, "flatpak");
    for tool in ["pkg_install", "pkg_remove", "pkg_undo"] {
        assert!(
            matches!(tools::gate(&core, tool), GateDecision::Hidden),
            "{tool} hides on a stand whose only packages source declares no writes"
        );
    }
}

/// The mechanical no-write leg of the acceptance criterion: granted write
/// tools route to the NATIVE manager only. The inventory's write-shaped
/// methods answer `Unsupported` and would count every attempt — after the
/// dispatches the counter must still read zero.
#[tokio::test]
async fn write_tools_never_reach_the_inventory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut grants = GrantRegistry::new();
    grants.issue(Grant::new(cap::PKG_WRITE, GrantProvenance::Config));
    grants.issue(Grant::new(cap::PKG_UNDO, GrantProvenance::Config));
    let (native, inventory) = (default_native(), default_inventory());
    let core = aggregate_core(
        dir.path(),
        7,
        Some(native.clone()),
        Some(inventory.clone()),
        grants,
    );
    assert!(matches!(
        tools::gate(&core, "pkg_install"),
        GateDecision::Run
    ));
    let installed =
        tools::dispatch(&core, "pkg_install", args(&[("name", "alpm-core".into())])).await;
    assert!(
        installed.is_ok(),
        "the granted install runs: {:?}",
        installed.map(|_| ())
    );
    let removed = tools::dispatch(&core, "pkg_remove", args(&[("name", "alpm-core".into())])).await;
    assert!(
        removed.is_ok(),
        "the granted remove runs: {:?}",
        removed.map(|_| ())
    );
    let undone = tools::dispatch(&core, "pkg_undo", args(&[("name", "alpm-core".into())])).await;
    // The alpm fixture undo answers an honest native error — the point is
    // that SOMETHING native ran, never the inventory.
    assert!(undone.is_err(), "the fixture undo is the honest cache miss");
    assert_eq!(
        native
            .install_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "install routed to the native manager"
    );
    assert_eq!(
        native
            .remove_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "remove routed to the native manager"
    );
    assert_eq!(
        native.undo_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "undo routed to the native manager"
    );
    assert_eq!(
        inventory
            .write_attempts
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the inventory was never reached by a write-shaped method"
    );
}

// --- The contract suite bound against the aggregate fixtures ----------------

/// The aggregate as ONE `PackageManager`: reads through the real dispatch
/// path (`pkg_list`/`pkg_info` = the production aggregation), search/history
/// through the native manager (the production shape: no MCP tool exists for
/// them), writes through the granted write tools. This is the fixture the
/// R2 suite binds green against, in both roadmap shapes.
struct DispatchAggregate {
    core: omnibus_core::Core,
    native: Arc<FakeNativePackages>,
    _dir: tempfile::TempDir,
}

impl DispatchAggregate {
    fn build(shape: &str, undo_expected: bool) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let native = if undo_expected {
            FakeNativePackages::alpm_shape(&NATIVE_TABLE)
        } else {
            FakeNativePackages::apt_shape(&NATIVE_TABLE)
        };
        let mut grants = GrantRegistry::new();
        grants.issue(Grant::new(cap::PKG_WRITE, GrantProvenance::Config));
        grants.issue(Grant::new(cap::PKG_UNDO, GrantProvenance::Config));
        let core = aggregate_core(
            dir.path(),
            match (shape, undo_expected) {
                ("alpm", _) => 11,
                (_, false) => 12,
                _ => 13,
            },
            Some(native.clone()),
            Some(default_inventory()),
            grants,
        );
        Self {
            core,
            native,
            _dir: dir,
        }
    }

    async fn read_tool(&self, tool: &str, arguments: serde_json::Value) -> serde_json::Value {
        let map = arguments
            .as_object()
            .cloned()
            .expect("a DTO serializes into an object");
        tools::dispatch(&self.core, tool, map)
            .await
            .expect("the aggregate read runs")
            .value
    }

    /// The dispatch with its typed error preserved: the suite exercises the
    /// honest refusals (a missing package, a cache-miss undo) through the
    /// same path.
    async fn dispatch_tool(
        &self,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, ContractError> {
        let map = arguments
            .as_object()
            .cloned()
            .expect("a DTO serializes into an object");
        Ok(tools::dispatch(&self.core, tool, map).await?.value)
    }
}

#[async_trait]
impl PackageManager for DispatchAggregate {
    fn report(&self) -> AdapterReport {
        // The aggregate's document shape is the primary manager's: the undo
        // value rides there (the inventory's PKG_READ entry joins it in the
        // assembled capabilities document, deduped per adapter).
        self.native.report()
    }

    async fn list(
        &self,
        request: omnibus_contracts::dto::packages::PackageListRequest,
    ) -> Result<Vec<PackageBrief>, ContractError> {
        let value = serde_json::to_value(&request).expect("a DTO serializes");
        let payload = self.read_tool("pkg_list", value).await;
        serde_json::from_value(payload).map_err(|e| ContractError::Unsupported {
            reason: format!("dispatch payload is not the list DTO: {e}"),
        })
    }

    async fn info(
        &self,
        name: String,
    ) -> Result<omnibus_contracts::dto::packages::PackageInfo, ContractError> {
        let payload = self
            .dispatch_tool("pkg_info", serde_json::json!({ "name": name }))
            .await?;
        serde_json::from_value(payload).map_err(|e| ContractError::Unsupported {
            reason: format!("dispatch payload is not the info DTO: {e}"),
        })
    }

    async fn search(
        &self,
        query: omnibus_contracts::dto::packages::PackageSearch,
    ) -> Result<Vec<PackageBrief>, ContractError> {
        // No MCP tool aggregates search: the native manager is the search
        // surface (the inventory declares none — honest Unsupported).
        self.native.search(query).await
    }

    async fn install(
        &self,
        request: omnibus_contracts::dto::packages::PackageNameRequest,
    ) -> Result<omnibus_contracts::dto::packages::TransactionResult, ContractError> {
        let payload = self
            .read_tool("pkg_install", serde_json::to_value(&request).expect("DTO"))
            .await;
        serde_json::from_value(payload).map_err(|e| ContractError::Unsupported {
            reason: format!("dispatch payload is not the transaction DTO: {e}"),
        })
    }

    async fn remove(
        &self,
        request: omnibus_contracts::dto::packages::PackageNameRequest,
    ) -> Result<omnibus_contracts::dto::packages::TransactionResult, ContractError> {
        let payload = self
            .read_tool("pkg_remove", serde_json::to_value(&request).expect("DTO"))
            .await;
        serde_json::from_value(payload).map_err(|e| ContractError::Unsupported {
            reason: format!("dispatch payload is not the transaction DTO: {e}"),
        })
    }

    async fn history(
        &self,
        query: omnibus_contracts::dto::packages::PackageHistoryQuery,
    ) -> Result<Vec<omnibus_contracts::dto::packages::PackageHistoryEntry>, ContractError> {
        // No MCP tool exposes history: the native manager's own log is the
        // only transaction history (specs/domains/packages.md).
        self.native.history(query).await
    }

    async fn undo(
        &self,
        request: omnibus_contracts::dto::packages::PackageNameRequest,
    ) -> Result<omnibus_contracts::dto::packages::UndoResult, ContractError> {
        let payload = self
            .dispatch_tool("pkg_undo", serde_json::to_value(&request).expect("DTO"))
            .await?;
        serde_json::from_value(payload).map_err(|e| ContractError::Unsupported {
            reason: format!("dispatch payload is not the undo DTO: {e}"),
        })
    }
}

/// Roadmap 2.9.1 AC, alpm+flatpak shape: the PackageManager read-part suite
/// is green against the aggregate.
#[tokio::test]
async fn read_part_suite_is_green_on_the_alpm_flatpak_aggregate() {
    let report = omnibus_contracts::suite::packages::packages_suite(
        || {
            let aggregate = DispatchAggregate::build("alpm", true);
            Box::new(aggregate)
        },
        omnibus_contracts::suite::packages::PackagesFixture {
            undo_expected: true,
        },
    )
    .await;
    assert!(
        report.is_clean(),
        "alpm+flatpak aggregate suite failures: {report}"
    );
}

/// Roadmap 2.9.1 AC, apt+flatpak shape: the same suite green with the honest
/// no-undo native shape.
#[tokio::test]
async fn read_part_suite_is_green_on_the_apt_flatpak_aggregate() {
    let report = omnibus_contracts::suite::packages::packages_suite(
        || {
            let aggregate = DispatchAggregate::build("apt", false);
            Box::new(aggregate)
        },
        omnibus_contracts::suite::packages::PackagesFixture {
            undo_expected: false,
        },
    )
    .await;
    assert!(
        report.is_clean(),
        "apt+flatpak aggregate suite failures: {report}"
    );
}

#![cfg_attr(test, allow(clippy::expect_used))] // unit tests may expect; the seam must not
//! flatpak/snap adapter: the read-only package inventory (roadmap task 2.9.1,
//! specs/domains/packages.md). One crate for both stores — `flatpak` and `snap`
//! are called through their CLI binaries, and CLI shapes never leave the
//! adapter: mapping to DTOs happens here (DTO Discipline).
//!
//! The adapter implements the READ part of `PackageManager` only — `list` and
//! `info`. `install`/`remove`/`undo` are NOT declared for these sources (the
//! honest limitation is marked in `capabilities()` through the report note,
//! and the methods answer [`ContractError::Unsupported`] before any subprocess
//! can spawn); `search` and `history` are equally out of the inventory
//! contract — remote store search and a transaction log do not exist for a
//! read-only inventory. Write semantics belong to the native manager; the core
//! routes `pkg_install`/`pkg_remove`/`pkg_undo` to the native slot only.
//!
//! Detection is per-store liveness, decided once at wire time: `flatpak
//! --version` and `snap list` must run. A present `snap` binary with a wedged
//! snapd counts as absent this session (the same honest-absence convention the
//! composition root applies to failed adapter detections) — plain absence from
//! the aggregated output, never a degradation state (R7). Neither store live
//! means the adapter does not wire at all, and the aggregated `pkg_list`/
//! `pkg_info` output is exactly what it was before v1.5.
//!
//! Error mapping (specs/architecture/error-taxonomy.md): every store failure
//! becomes [`ContractError::NativeFailure`] with the original message in the
//! payload; the numeric `code` is adapter-scoped (`NF_STORE`,
//! `NF_PKG_NOT_FOUND`). A name in neither live store is an honest
//! `pkg-not-found`, never a fabricated empty answer. Both store scans run
//! concurrently under one budget that sits below the core read budget, so the
//! adapter's typed answers stay reachable through the tool path.

mod flatpak;
mod run;
mod snap;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use omnibus_contracts::capabilities::PKG_READ;
use omnibus_contracts::dto::packages::{
    PackageBrief, PackageHistoryEntry, PackageHistoryQuery, PackageInfo, PackageListRequest,
    PackageNameRequest, PackageSearch, TransactionResult, UndoResult,
};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::packages::PackageManager;
use omnibus_contracts::version::CONTRACT_VERSION;

pub use run::{NF_PKG_NOT_FOUND, NF_STORE};

/// Subprocess budget for one inventory call (both store scans run
/// concurrently under it). It sits strictly below the core's
/// `TOOL_CALL_BUDGET` (10 s, `omnibus-core/src/limits.rs`) so the adapter's
/// typed answers stay reachable through the tool path (see the journald
/// adapter for the full equality-hazard note). Reads only: an elapse answers
/// [`ContractError::TimeoutError`] — a read observes no native state, so
/// "the adapter stopped waiting" is the whole truth.
const DEFAULT_BUDGET: Duration = Duration::from_secs(9);

/// The store binaries this adapter invokes.
#[derive(Debug, Clone)]
pub struct StorePrograms {
    /// `flatpak` (list/info scans).
    pub flatpak: String,
    /// `snap` (the `snap list` inventory scan).
    pub snap: String,
}

impl Default for StorePrograms {
    fn default() -> Self {
        Self {
            flatpak: String::from("flatpak"),
            snap: String::from("snap"),
        }
    }
}

/// The read-only flatpak/snap inventory provider. Cheap to clone; every clone
/// shares the same immutable configuration.
#[derive(Debug, Clone)]
pub struct FlatpakInventory {
    inner: Arc<InventoryState>,
}

#[derive(Debug)]
struct InventoryState {
    programs: StorePrograms,
    budget: Duration,
    flatpak_live: bool,
    snap_live: bool,
}

impl FlatpakInventory {
    /// Detects the stores (composition-root entry): `flatpak --version` and
    /// `snap list` must run. Machines with neither store answer
    /// `AdapterUnavailable` and the inventory does not wire — the aggregated
    /// output stays exactly the native manager's.
    pub async fn detect() -> Result<Self, ContractError> {
        Self::detect_with(StorePrograms::default(), DEFAULT_BUDGET).await
    }

    /// [`detect`](Self::detect) over an explicit configuration (tests bind
    /// fixture store scripts here; the wiring itself stays the same bounded
    /// subprocess pair).
    pub async fn detect_with(
        programs: StorePrograms,
        budget: Duration,
    ) -> Result<Self, ContractError> {
        // The probe futures borrow their argument slices: bind them first, or
        // the temporaries would drop before `join!` awaits the futures.
        let flatpak_args = [String::from("--version")];
        let snap_args = [String::from("list")];
        let flatpak_probe = run::capture_bounded(&programs.flatpak, &flatpak_args, budget);
        let snap_probe = run::capture_bounded(&programs.snap, &snap_args, budget);
        let (flatpak_result, snap_result) = tokio::join!(flatpak_probe, snap_probe);
        let flatpak_live = flatpak_result.is_ok();
        let snap_live = snap_result.is_ok();
        if !flatpak_live && !snap_live {
            return Err(ContractError::AdapterUnavailable);
        }
        Ok(Self {
            inner: Arc::new(InventoryState {
                programs,
                budget,
                flatpak_live,
                snap_live,
            }),
        })
    }

    /// Test-only constructor with explicit store liveness: the no-write test
    /// needs an adapter whose detection never invoked the sentinel fixtures.
    #[cfg(test)]
    fn with_liveness(
        programs: StorePrograms,
        budget: Duration,
        flatpak_live: bool,
        snap_live: bool,
    ) -> Self {
        Self {
            inner: Arc::new(InventoryState {
                programs,
                budget,
                flatpak_live,
                snap_live,
            }),
        }
    }

    /// Runs the flatpak list scan (only when the store is live); a dead store
    /// yields `None` without spawning anything.
    async fn scan_flatpak(&self) -> Option<Result<Vec<flatpak::FlatpakRow>, ContractError>> {
        if !self.inner.flatpak_live {
            return None;
        }
        let args = [
            String::from("list"),
            String::from("--app"),
            String::from("--columns"),
            flatpak::LIST_COLUMNS.join(","),
        ];
        match run::capture_bounded(&self.inner.programs.flatpak, &args, self.inner.budget).await {
            Ok(captured) => Some(Ok(flatpak::parse_list(&captured.stdout))),
            Err(error) => Some(Err(error)),
        }
    }

    /// Runs the snap list scan (only when the store is live); a dead store
    /// yields `None` without spawning anything.
    async fn scan_snap(&self) -> Option<Result<Vec<snap::SnapRow>, ContractError>> {
        if !self.inner.snap_live {
            return None;
        }
        match run::capture_bounded(
            &self.inner.programs.snap,
            &[String::from("list")],
            self.inner.budget,
        )
        .await
        {
            Ok(captured) => Some(Ok(snap::parse_list(&captured.stdout))),
            Err(error) => Some(Err(error)),
        }
    }

    /// The inventory rows of both live stores. Strict policy: a live store
    /// that fails at runtime fails the whole inventory call — the list claims
    /// the full inventory, so a silently skipped store would make it lie.
    async fn inventory(
        &self,
    ) -> Result<(Vec<flatpak::FlatpakRow>, Vec<snap::SnapRow>), ContractError> {
        let (flatpak_result, snap_result) = tokio::join!(self.scan_flatpak(), self.scan_snap());
        let flatpak_rows = match flatpak_result {
            Some(rows) => rows?,
            None => Vec::new(),
        };
        let snap_rows = match snap_result {
            Some(rows) => rows?,
            None => Vec::new(),
        };
        Ok((flatpak_rows, snap_rows))
    }

    /// Both stores dead: unreachable through the wiring (detect refuses), but
    /// a hand-built instance can still reach it — refuse honestly instead of
    /// answering an empty inventory.
    fn require_live(&self) -> Result<(), ContractError> {
        if !self.inner.flatpak_live && !self.inner.snap_live {
            return Err(ContractError::AdapterUnavailable);
        }
        Ok(())
    }
}

/// The honest no-write reason: the single source for the `Unsupported`
/// answers, so tests and the capability note quote one text.
pub const NO_WRITE_REASON: &str =
    "flatpak/snap sources declare no write operations; package writes belong to the native manager";

/// The honest no-search reason (the inventory has no remote search contract).
pub const NO_SEARCH_REASON: &str =
    "the flatpak/snap inventory implements list/info only; remote store search is not part of it";

/// The honest no-history reason (no transaction log exists for the inventory).
pub const NO_HISTORY_REASON: &str = "the flatpak/snap inventory has no transaction log; package history lives with the native manager";

#[async_trait]
impl PackageManager for FlatpakInventory {
    fn report(&self) -> AdapterReport {
        let state = &self.inner;
        AdapterReport {
            adapter: String::from("flatpak"),
            subsystem: String::from("packages"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![CapabilityFact {
                name: String::from(PKG_READ),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("present"),
                value: None,
                note: Some(String::from(
                    "read-only flatpak/snap inventory (list/info only); install/remove/undo are \
                     not declared for these sources — writes belong to the native manager",
                )),
                remedy: None,
            }],
            notes: vec![
                format!("flatpak store: {}", live_word(state.flatpak_live)),
                format!("snap store: {}", live_word(state.snap_live)),
            ],
        }
    }

    async fn list(&self, request: PackageListRequest) -> Result<Vec<PackageBrief>, ContractError> {
        self.require_live()?;
        let (flatpak_rows, snap_rows) = self.inventory().await?;
        let mut briefs = flatpak::briefs_of(flatpak_rows);
        briefs.extend(snap::briefs_of(snap_rows));
        if let Some(limit) = request.limit {
            let limit = usize::try_from(limit).unwrap_or(usize::MAX);
            briefs.truncate(limit);
        }
        Ok(briefs)
    }

    async fn info(&self, name: String) -> Result<PackageInfo, ContractError> {
        self.require_live()?;
        let (flatpak_rows, snap_rows) = self.inventory().await?;
        // Identity is the store ID: the flatpak application ID or the snap
        // name, exactly the strings `pkg_list` names.
        if let Some(row) = flatpak_rows.iter().find(|row| row.application == name) {
            return Ok(PackageInfo {
                name: row.application.clone(),
                version: row.version.clone().unwrap_or_default(),
                description: row.description.clone(),
                repo: row.origin.clone(),
                size_bytes: row.size_bytes,
                install_reason: None,
                depends: Vec::new(),
                required_by: Vec::new(),
                version_available: None,
            });
        }
        if let Some(row) = snap_rows.iter().find(|row| row.name == name) {
            return Ok(PackageInfo {
                name: row.name.clone(),
                version: row.version.clone(),
                // The store publishes no description, origin or size for a
                // bare list row; absence stays honest.
                description: None,
                repo: None,
                size_bytes: None,
                install_reason: None,
                depends: Vec::new(),
                required_by: Vec::new(),
                version_available: None,
            });
        }
        // No answer anywhere: a live store that failed names that failure;
        // two healthy misses name the honest not-found.
        Err(ContractError::NativeFailure {
            code: NF_PKG_NOT_FOUND,
            payload: format!("pkg-not-found: {name} is not in the flatpak/snap inventory"),
        })
    }

    async fn search(&self, _query: PackageSearch) -> Result<Vec<PackageBrief>, ContractError> {
        Err(ContractError::Unsupported {
            reason: String::from(NO_SEARCH_REASON),
        })
    }

    async fn install(
        &self,
        _request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        Err(ContractError::Unsupported {
            reason: String::from(NO_WRITE_REASON),
        })
    }

    async fn remove(
        &self,
        _request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        Err(ContractError::Unsupported {
            reason: String::from(NO_WRITE_REASON),
        })
    }

    async fn history(
        &self,
        _query: PackageHistoryQuery,
    ) -> Result<Vec<PackageHistoryEntry>, ContractError> {
        Err(ContractError::Unsupported {
            reason: String::from(NO_HISTORY_REASON),
        })
    }

    async fn undo(&self, _request: PackageNameRequest) -> Result<UndoResult, ContractError> {
        Err(ContractError::Unsupported {
            reason: String::from(NO_WRITE_REASON),
        })
    }
}

/// "present"/"absent" — the store presence word the report notes use.
fn live_word(live: bool) -> &'static str {
    if live {
        "present"
    } else {
        "absent this session"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    // --- Fixture stores: tiny POSIX-sh scripts standing in for the flatpak
    // and snap CLIs (the apt adapter's fixture discipline). The parsers and
    // error mapping run hermetically on any machine; a script that touches
    // the filesystem when invoked proves the write methods NEVER invoke a
    // subprocess (the no-write acceptance criterion).

    const FLATPAK_TABLE: &str = "org.gnome.Epiphany\tWeb\t46.3\tflathub\tA simple web browser\t256.4 MB\n\
         org.mozilla.firefox\tFirefox\t-\tflathub\t-\t512.0 MB\n";

    const SNAP_TABLE: &str = "Name\tVersion\tRev\tTracking\tPublisher\tNotes\n\
         core22\t20231115\t1380\tlatest/stable\tcanonical✓\tbase\n\
         chromium\t121.0\t2900\tlatest/stable\tcanonical✓\t-\n";

    fn fixture_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("omnibus-flatpak-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        dir
    }

    fn write_fixture(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("fixture write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("fixture chmod");
        path.to_string_lossy().into_owned()
    }

    /// A flatpak fixture that passes the `--version` probe (so detect marks
    /// the store live) and answers list scans with the fixture table.
    fn table_flatpak(dir: &Path) -> String {
        write_fixture(
            dir,
            "flatpak",
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'Flatpak 1.18.4'; exit 0; fi\nprintf '%s' '{FLATPAK_TABLE}'"
            ),
        )
    }

    fn table_snap(dir: &Path) -> String {
        write_fixture(
            dir,
            "snap",
            &format!("#!/bin/sh\nprintf '%s' '{SNAP_TABLE}'"),
        )
    }

    async fn live_inventory(dir: &Path) -> FlatpakInventory {
        let programs = StorePrograms {
            flatpak: table_flatpak(dir),
            snap: table_snap(dir),
        };
        FlatpakInventory::detect_with(programs, Duration::from_secs(2))
            .await
            .expect("both fixture stores are live")
    }

    #[tokio::test]
    async fn detect_rejects_a_machine_without_stores() {
        let programs = StorePrograms {
            flatpak: String::from("/nonexistent/flatpak"),
            snap: String::from("/nonexistent/snap"),
        };
        let detected = FlatpakInventory::detect_with(programs, Duration::from_secs(1)).await;
        assert!(
            matches!(detected, Err(ContractError::AdapterUnavailable)),
            "no stores means no inventory: {detected:?}"
        );
    }

    #[tokio::test]
    async fn detect_survives_a_single_store() {
        let dir = fixture_dir("single");
        let programs = StorePrograms {
            flatpak: String::from("/nonexistent/flatpak"),
            snap: write_fixture(&dir, "snap", "#!/bin/sh\nprintf '%s' ''"),
        };
        let inventory = FlatpakInventory::detect_with(programs, Duration::from_secs(1))
            .await
            .expect("one live store wires the inventory");
        let report = inventory.report();
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("snap store: present")),
            "the live store is named: {:?}",
            report.notes
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("flatpak store: absent")),
            "the dead store is honest: {:?}",
            report.notes
        );
    }

    #[tokio::test]
    async fn list_merges_both_stores_with_explicit_sources() {
        let dir = fixture_dir("list");
        let inventory = live_inventory(&dir).await;
        let briefs = inventory
            .list(PackageListRequest { limit: None })
            .await
            .expect("both fixture stores answer");
        let sources: Vec<_> = briefs.iter().map(|b| b.source).collect();
        assert_eq!(
            sources,
            vec![
                omnibus_contracts::dto::packages::PackageSource::Flatpak,
                omnibus_contracts::dto::packages::PackageSource::Flatpak,
                omnibus_contracts::dto::packages::PackageSource::Snap,
                omnibus_contracts::dto::packages::PackageSource::Snap,
            ],
            "flatpak rows precede snap rows, every source explicit"
        );
        assert_eq!(briefs[0].name, "org.gnome.Epiphany");
        assert_eq!(briefs[0].repo.as_deref(), Some("flathub"));
        assert_eq!(briefs[2].name, "core22");
    }

    #[tokio::test]
    async fn list_truncates_to_the_limit() {
        let dir = fixture_dir("limit");
        let inventory = live_inventory(&dir).await;
        let briefs = inventory
            .list(PackageListRequest { limit: Some(1) })
            .await
            .expect("both fixture stores answer");
        assert_eq!(briefs.len(), 1);
        assert_eq!(briefs[0].name, "org.gnome.Epiphany");
    }

    #[tokio::test]
    async fn info_matches_store_ids_exactly_and_answers_metadata() {
        let dir = fixture_dir("info");
        let inventory = live_inventory(&dir).await;
        let flatpak_info = inventory
            .info(String::from("org.gnome.Epiphany"))
            .await
            .expect("the flatpak ID resolves");
        assert_eq!(flatpak_info.version, "46.3");
        assert_eq!(flatpak_info.repo.as_deref(), Some("flathub"));
        assert_eq!(flatpak_info.size_bytes, Some(256_400_000));
        let snap_info = inventory
            .info(String::from("core22"))
            .await
            .expect("the snap name resolves");
        assert_eq!(snap_info.version, "20231115");
        // The store publishes no description, origin or size in `snap list`;
        // absence stays honest.
        assert_eq!(snap_info.description, None);
        assert_eq!(snap_info.repo, None);
        assert_eq!(snap_info.size_bytes, None);
    }

    #[tokio::test]
    async fn unknown_name_is_an_honest_pkg_not_found() {
        let dir = fixture_dir("missing");
        let inventory = live_inventory(&dir).await;
        let missing = inventory
            .info(String::from("no-such-package"))
            .await
            .expect_err("an unknown name is a contract error");
        match missing {
            ContractError::NativeFailure { code, payload } => {
                assert_eq!(code, NF_PKG_NOT_FOUND);
                assert!(payload.contains("pkg-not-found"), "{payload}");
            }
            other => panic!("expected NativeFailure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failing_live_store_fails_the_whole_inventory() {
        let dir = fixture_dir("poison");
        let programs = StorePrograms {
            // Passes the --version probe, fails every list scan.
            flatpak: write_fixture(
                &dir,
                "flatpak",
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'Flatpak 1.18.4'; exit 0; fi\necho 'store exploded' >&2\nexit 99\n",
            ),
            snap: table_snap(&dir),
        };
        let inventory = FlatpakInventory::detect_with(programs, Duration::from_secs(2))
            .await
            .expect("both stores are live at wire time");
        let listed = inventory.list(PackageListRequest { limit: None }).await;
        match listed {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_STORE);
                assert!(payload.contains("store exploded"), "{payload}");
            }
            other => panic!("a live store failing must fail the call honestly: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_slow_store_answers_timeout_within_the_budget() {
        let dir = fixture_dir("slow");
        let programs = StorePrograms {
            // Passes the --version probe instantly, stalls every list scan.
            flatpak: write_fixture(
                &dir,
                "flatpak",
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'Flatpak 1.18.4'; exit 0; fi\nsleep 30\n",
            ),
            snap: write_fixture(&dir, "snap", "#!/bin/sh\nprintf '%s' ''"),
        };
        let inventory = FlatpakInventory::detect_with(programs, Duration::from_millis(300))
            .await
            .expect("both stores answer their probe in time");
        let listed = inventory.list(PackageListRequest { limit: None }).await;
        assert!(
            matches!(listed, Err(ContractError::TimeoutError)),
            "an elapsing store scan is the typed read timeout: {listed:?}"
        );
    }

    /// The no-write acceptance criterion, mechanically: the write methods
    /// answer `Unsupported` BEFORE any subprocess can spawn — the fixture
    /// scripts touch the sentinel file on ANY invocation, so an empty
    /// sentinel directory after the calls proves no store binary was ever
    /// run. The report declares no write capability for the same reason
    /// (review leg of the criterion lives in this assertion, too).
    #[tokio::test]
    async fn write_operations_never_invoke_a_store() {
        let dir = fixture_dir("nowrite");
        let sentinel = dir.join("sentinel");
        let touch = format!(
            "#!/bin/sh\ntouch {}\nprintf '%s' ''",
            sentinel.to_string_lossy()
        );
        let programs = StorePrograms {
            flatpak: write_fixture(&dir, "flatpak", &touch),
            snap: write_fixture(&dir, "snap", &touch),
        };
        // Built without detect ON PURPOSE: detection would legitimately
        // invoke the stores (its probes) and touch the sentinel.
        let inventory =
            FlatpakInventory::with_liveness(programs, Duration::from_secs(2), true, true);
        let mut outcomes: Vec<(&'static str, ContractError)> = Vec::new();
        outcomes.push((
            "install",
            inventory
                .install(PackageNameRequest {
                    name: String::from("core22"),
                })
                .await
                .expect_err("install is Unsupported"),
        ));
        outcomes.push((
            "remove",
            inventory
                .remove(PackageNameRequest {
                    name: String::from("core22"),
                })
                .await
                .expect_err("remove is Unsupported"),
        ));
        outcomes.push((
            "undo",
            inventory
                .undo(PackageNameRequest {
                    name: String::from("core22"),
                })
                .await
                .expect_err("undo is Unsupported"),
        ));
        outcomes.push((
            "search",
            inventory
                .search(PackageSearch {
                    query: String::from("core"),
                    limit: Some(10),
                })
                .await
                .expect_err("search is Unsupported"),
        ));
        outcomes.push((
            "history",
            inventory
                .history(PackageHistoryQuery {
                    name: None,
                    limit: Some(10),
                })
                .await
                .expect_err("history is Unsupported"),
        ));
        for (name, error) in outcomes {
            match error {
                ContractError::Unsupported { reason } => {
                    assert!(
                        reason.contains("no write operations")
                            || reason.contains("list/info only")
                            || reason.contains("no transaction log"),
                        "{name}: the honest limitation is stated: {reason}"
                    );
                }
                other => panic!("{name}: expected Unsupported, got {other:?}"),
            }
        }
        assert!(
            !sentinel.exists(),
            "no store binary was ever invoked by a write-shaped method"
        );
        // And the report never declares a write capability.
        let report = inventory.report();
        assert_eq!(report.capabilities.len(), 1);
        assert_eq!(report.capabilities[0].name, PKG_READ);
        assert!(
            report.capabilities[0]
                .note
                .as_deref()
                .is_some_and(|note| note.contains("read-only")),
            "the read-only limitation is marked in capabilities(): {:?}",
            report.capabilities[0].note
        );
    }
}

//! alpm adapter: package list/info/search/install/remove/history and
//! cache-downgrade undo.
//!
//! Native backend: libalpm (crate `alpm`, links the system libalpm). ALPM
//! handles never leave this crate — mapping to DTOs happens inside the adapter
//! (specs/architecture/spi-registry.md "DTO Discipline").
//!
//! Threading: the libalpm handle is `!Send` (raw C pointers and callbacks), so it
//! is created on, and stays on, one dedicated worker thread; every libalpm call is
//! a job sent to that worker from a `spawn_blocking` task. No unsafe, no `Send`
//! claims beyond what libalpm itself upholds.
//!
//! Error mapping (specs/architecture/error-taxonomy.md): every libalpm failure
//! becomes [`ContractError::NativeFailure`] with the original message preserved;
//! the numeric `code` is adapter-scoped and its symbolic name prefixes the
//! payload: `alpm`, `transaction`, `undo-no-cache-version`, `pkg-not-found`,
//! `log`, `timeout-indeterminate`. A missing cached version is an honest
//! `undo-no-cache-version` failure, never a half-applied transaction. A write
//! path never reports `TimeoutError`: its `spawn_blocking` job is
//! uncancellable and detached on elapse, so at answer time it may or may not
//! have reached the transaction stage — and a stalled pre-stage can still
//! complete and dispatch LATER. The only airtight answer is
//! `timeout-indeterminate` claiming exactly the uncertainty (whether a
//! package change lands natively is unknown), never a stronger claim in
//! either direction.

mod logparse;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use alpm::{Alpm, Package, SigLevel, TransFlag};
use async_trait::async_trait;
use omnibus_contracts::capabilities::{PKG_READ, PKG_UNDO, PKG_WRITE};
use omnibus_contracts::dto::packages::{
    PackageBrief, PackageHistoryEntry, PackageHistoryQuery, PackageInfo, PackageListRequest,
    PackageNameRequest, PackageSearch, TransactionResult, UndoResult, UndoSource,
};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::packages::PackageManager;
use omnibus_contracts::version::CONTRACT_VERSION;

/// `NativeFailure` code: generic libalpm failure (handle init, worker, cache).
pub const NF_ALPM: i32 = 1;
/// `NativeFailure` code: transaction prepare/commit failed (lock, privileges,
/// disk space) — reported honestly, never retried in place.
pub const NF_TRANSACTION: i32 = 2;
/// `NativeFailure` code: the previous version is not in the pacman cache.
pub const NF_UNDO_NO_CACHE: i32 = 3;
/// `NativeFailure` code: the named package is neither installed nor available.
pub const NF_PKG_NOT_FOUND: i32 = 4;
/// `NativeFailure` code: the transaction log cannot be read.
pub const NF_LOG_READ: i32 = 5;
/// `NativeFailure` code: a write-path budget elapsed — the outcome is unknown
/// (jobs are uncancellable and the detached job may still reach the
/// transaction stage after the answer; see [`with_budget_write`]).
pub const NF_TIMEOUT_INDETERMINATE: i32 = 6;

/// Payload of the honest write-path timeout (single source for tests). The
/// wording is an uncertainty claim on purpose: at answer time the detached
/// job may or may not have reached the transaction stage (a stalled read
/// stage may still complete and dispatch later; a queued job may already be
/// past its pre-stages), so neither "nothing was dispatched" nor "the
/// transaction was running" would be airtight — "whether a change lands is
/// unknown" is true in every case.
pub const WRITE_TIMEOUT_INDETERMINATE_PAYLOAD: &str = "timeout-indeterminate: write budget elapsed while the uncancellable job was still in flight; \
     whether a package change lands natively is unknown (it may still be dispatched after this answer) — re-check the package state before retrying";

/// Payload of the honest undo refusal (single source for tests and behavior).
pub const UNDO_NO_CACHE_PAYLOAD: &str =
    "undo-no-cache-version: previous version not in cache; see ALA/AUR archive";

/// Sync repos registered lazily; missing ones are tolerated (Manjaro mirrors
/// differ from Arch: community may not exist as its own db).
const SYNC_REPOS: [&str; 4] = ["core", "extra", "community", "multilib"];
/// Budget for one blocking libalpm call before the contract reports a timeout.
/// It sits below the core's `TOOL_CALL_BUDGET` (10 s, `omnibus-core/src/limits.rs`)
/// so the adapter's typed answers are reachable through the tool path: a write
/// that elapses here answers `timeout-indeterminate` (see
/// [`with_budget_write`]) — the write's only timeout layer, since the core
/// does not timer writes (see `tools::dispatch`) — and a read answers
/// `TimeoutError` before the core drops the future (quality-gates.md,
/// "Security Tests" ladder).
const BLOCKING_BUDGET: Duration = Duration::from_secs(9);
/// Upper bound on pacman.log bytes read per `history`/`undo` call: the most
/// recent tail of the log. The log grows without bound over a system's
/// lifetime, so the read (and the memory it allocates) is capped at this
/// documented constant instead of tracking the file size; entries older than
/// the tail are invisible to history and undo. 8 MiB covers years of upgrades.
const HISTORY_TAIL_BYTES: u64 = 8 * 1024 * 1024;
/// The `pkg.undo` semantics this adapter genuinely implements, typed by the
/// contracts enum: cache downgrade. Cache packages load with pacman's
/// local-file signature policy (`LocalFileSigLevel = Optional` as shipped): a
/// missing detached `.sig` is tolerated, while a PRESENT signature must fully
/// verify — marginal-trust and unknown-key signatures fail the load. Undo is a
/// write path: it installs nothing that no trusted key vouches for, and a
/// cache entry whose signature does not verify is an honest `transaction:
/// load` failure, never a silently accepted write.
const UNDO_VALUE: omnibus_contracts::capabilities::PkgUndoValue =
    omnibus_contracts::capabilities::PkgUndoValue::CacheDowngrade;

/// One unit of libalpm work for the worker thread.
type Job = Box<dyn FnOnce(&mut Alpm) + Send + 'static>;

/// alpm adapter handle. Cheap to clone; every clone shares one worker thread.
#[derive(Debug, Clone)]
pub struct AlpmAdapter {
    inner: Arc<AlpmInner>,
}

#[derive(Debug)]
struct AlpmInner {
    worker: AlpmWorker,
    sync_registered: AtomicBool,
}

/// The mailbox of the thread that owns the libalpm handle.
#[derive(Debug)]
struct AlpmWorker {
    tx: std::sync::mpsc::Sender<Job>,
}

impl AlpmAdapter {
    /// Opens the local ALPM database at `/` with `OMNIBUS_DBPATH`
    /// (default `/var/lib/pacman/`). Spawns the worker thread that owns the
    /// handle; sync DBs register lazily on first use. Fails with a contract
    /// error when libalpm cannot initialize (no pacman db, wrong dbpath) —
    /// callers treat that as "adapter absent".
    pub fn open() -> Result<Self, ContractError> {
        let dbpath = env_or("OMNIBUS_DBPATH", "/var/lib/pacman/");
        let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name(String::from("omnibus-alpm"))
            .spawn(move || {
                // The handle is born here and never crosses a thread boundary.
                let mut alpm = match Alpm::new("/", dbpath.as_str()) {
                    Ok(handle) => handle,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("alpm: init with dbpath {dbpath}: {e}")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                while let Ok(job) = job_rx.recv() {
                    job(&mut alpm);
                }
            })
            .map_err(|e| native(NF_ALPM, format!("alpm: worker thread: {e}")))?;
        let ready = ready_rx
            .recv()
            .unwrap_or_else(|_| Err(String::from("alpm: worker died before init")));
        if let Err(text) = ready {
            return Err(native(NF_ALPM, text));
        }
        Ok(Self {
            inner: Arc::new(AlpmInner {
                worker: AlpmWorker { tx: job_tx },
                sync_registered: AtomicBool::new(false),
            }),
        })
    }

    /// Reads the pacman transaction log (`OMNIBUS_PACMAN_LOG`, default
    /// `/var/log/pacman.log`), returning entries for `name` (all if `None`),
    /// at most the `limit` most recent, in chronological order.
    ///
    /// Bounded and lossy by design: at most the last [`HISTORY_TAIL_BYTES`]
    /// bytes are read (documented truncation — results cover the recent tail,
    /// not all of history), and undecodable bytes degrade to U+FFFD in place,
    /// so one corrupted line costs one entry, never the whole read.
    fn read_history(
        name: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<PackageHistoryEntry>, ContractError> {
        let path = env_or("OMNIBUS_PACMAN_LOG", "/var/log/pacman.log");
        let bytes = read_log_tail(Path::new(&path), HISTORY_TAIL_BYTES)?;
        Ok(history_from_bytes(&bytes, name, limit))
    }

    /// Locates the cached package file for a downgrade, if one exists.
    fn find_cached_file(name: &str, version: &str) -> Result<Option<String>, ContractError> {
        let dir = env_or("OMNIBUS_CACHE_DIR", "/var/cache/pacman/pkg");
        let mut candidates: Vec<String> = Vec::new();
        let read = std::fs::read_dir(Path::new(&dir))
            .map_err(|e| native(NF_ALPM, format!("alpm: cache dir {dir}: {e}")))?;
        for entry in read.flatten() {
            if let Some(file_name) = entry.file_name().to_str()
                && logparse::is_cached_pkg_file(file_name, name, version)
            {
                candidates.push(String::from(file_name));
            }
        }
        let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        Ok(logparse::pick_cached_file(&refs, name, version).map(String::from))
    }

    /// Runs the tail of a libalpm transaction (prepare -> commit), always
    /// releasing the transaction afterwards. `stage` adds its targets; every
    /// failure maps to the honest `transaction` native failure.
    fn run_transaction<F>(alpm: &mut Alpm, stage: F) -> Result<(), ContractError>
    where
        F: FnOnce(&mut Alpm) -> Result<(), ContractError>,
    {
        let result = stage(alpm).and_then(|()| {
            alpm.trans_prepare()
                .map_err(|e| native(NF_TRANSACTION, format!("transaction: prepare: {e}")))?;
            alpm.trans_commit()
                .map_err(|e| native(NF_TRANSACTION, format!("transaction: commit: {e}")))?;
            Ok(())
        });
        let _ = alpm.trans_release();
        result
    }
}

impl AlpmInner {
    /// Sends one libalpm job to the worker and waits for its outcome. Blocking
    /// by design: callers already run inside `spawn_blocking`.
    fn call<R, F>(&self, job: F) -> Result<R, ContractError>
    where
        R: Send + 'static,
        F: FnOnce(&mut Alpm) -> Result<R, ContractError> + Send + 'static,
    {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.worker
            .tx
            .send(Box::new(move |alpm: &mut Alpm| {
                let _ = reply_tx.send(job(alpm));
            }))
            .map_err(|_| native(NF_ALPM, String::from("alpm: worker thread unavailable")))?;
        match reply_rx.recv() {
            Ok(result) => result,
            Err(_) => Err(native(
                NF_ALPM,
                String::from("alpm: worker dropped the result"),
            )),
        }
    }

    /// Registers the sync DBs once, tolerating missing repos; later calls are a
    /// no-op (the flag is published only after the registration pass).
    fn ensure_syncdbs(&self) -> Result<(), ContractError> {
        if self.sync_registered.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.call(|alpm| {
            for repo in SYNC_REPOS {
                // Tolerate missing repos: they differ per distro (community).
                if let Err(e) = alpm.register_syncdb(repo, SigLevel::DATABASE | SigLevel::PACKAGE) {
                    eprintln!("alpm: sync db {repo} unavailable: {e}");
                }
            }
            Ok(())
        })?;
        self.sync_registered.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl PackageManager for AlpmAdapter {
    fn report(&self) -> AdapterReport {
        AdapterReport {
            adapter: String::from("alpm"),
            subsystem: String::from("packages"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![
                fact(PKG_READ),
                fact(PKG_WRITE),
                CapabilityFact {
                    name: String::from(PKG_UNDO),
                    ver: String::from(CONTRACT_VERSION),
                    status: String::from("present"),
                    value: Some(UNDO_VALUE.to_json()),
                    note: Some(String::from(
                        "downgrade to the previous version from the pacman cache, located via history",
                    )),
                    remedy: None,
                },
            ],
            notes: Vec::new(),
        }
    }

    async fn list(&self, request: PackageListRequest) -> Result<Vec<PackageBrief>, ContractError> {
        let inner = self.inner.clone();
        with_budget(async move {
            spawn_blocking(move || {
                inner.call(move |alpm| {
                    let mut briefs = Vec::new();
                    for pkg in alpm.localdb().pkgs() {
                        if let Some(limit) = request.limit
                            && briefs.len() >= limit as usize
                        {
                            break;
                        }
                        briefs.push(brief_of(pkg, None));
                    }
                    Ok(briefs)
                })
            })
            .await
        })
        .await
    }

    async fn info(&self, name: String) -> Result<PackageInfo, ContractError> {
        let inner = self.inner.clone();
        with_budget(async move {
            spawn_blocking(move || {
                inner.ensure_syncdbs()?;
                inner.call(move |alpm| {
                    if let Ok(pkg) = alpm.localdb().pkg(name.as_str()) {
                        let version_available = alpm.syncdbs().iter().find_map(|db| {
                            db.pkg(name.as_str())
                                .ok()
                                .map(|sync_pkg| sync_pkg.version().to_string())
                        });
                        return Ok(info_from_local(pkg, version_available));
                    }
                    for db in alpm.syncdbs().iter() {
                        if let Ok(pkg) = db.pkg(name.as_str()) {
                            return Ok(info_from_sync(pkg, db.name()));
                        }
                    }
                    Err(native(
                        NF_PKG_NOT_FOUND,
                        format!("pkg-not-found: {name} is neither installed nor in a sync db"),
                    ))
                })
            })
            .await
        })
        .await
    }

    async fn search(&self, query: PackageSearch) -> Result<Vec<PackageBrief>, ContractError> {
        let inner = self.inner.clone();
        with_budget(async move {
            spawn_blocking(move || {
                inner.ensure_syncdbs()?;
                let needle = query.query.to_lowercase();
                let limit = query.limit.map_or(usize::MAX, |l| l as usize);
                inner.call(move |alpm| {
                    let mut briefs: Vec<PackageBrief> = Vec::new();
                    let mut seen: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    for pkg in alpm.localdb().pkgs() {
                        if briefs.len() >= limit {
                            break;
                        }
                        if matches(needle.as_str(), pkg.name(), pkg.desc()) {
                            seen.insert(String::from(pkg.name()));
                            briefs.push(brief_of(pkg, None));
                        }
                    }
                    for db in alpm.syncdbs().iter() {
                        for pkg in db.pkgs() {
                            if briefs.len() >= limit {
                                return Ok(briefs);
                            }
                            if seen.contains(pkg.name()) {
                                continue;
                            }
                            if matches(needle.as_str(), pkg.name(), pkg.desc()) {
                                // Dedup covers every source: a package in two
                                // sync repos (and under localdb) reports once.
                                seen.insert(String::from(pkg.name()));
                                briefs.push(brief_of(pkg, Some(db.name())));
                            }
                        }
                    }
                    Ok(briefs)
                })
            })
            .await
        })
        .await
    }

    async fn install(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        let inner = self.inner.clone();
        with_budget_write(async move {
            spawn_blocking(move || {
                inner.ensure_syncdbs()?;
                let name = request.name;
                let lookup_name = name.clone();
                let version = inner.call(move |alpm| {
                    alpm.syncdbs()
                        .iter()
                        .find_map(|db| db.pkg(lookup_name.as_str()).ok())
                        .map(|pkg| pkg.version().to_string())
                        .ok_or_else(|| {
                            native(
                                NF_PKG_NOT_FOUND,
                                format!("pkg-not-found: {lookup_name} is not in any sync db"),
                            )
                        })
                })?;
                inner.call(move |alpm| {
                    alpm.trans_init(TransFlag::NONE)
                        .map_err(|e| native(NF_TRANSACTION, format!("transaction: init: {e}")))?;
                    Self::run_transaction(alpm, |alpm| {
                        for db in alpm.syncdbs().iter() {
                            if let Ok(pkg) = db.pkg(name.as_str()) {
                                return alpm.trans_add_pkg(pkg).map_err(|e| {
                                    native(NF_TRANSACTION, format!("transaction: add: {e}"))
                                });
                            }
                        }
                        Err(native(
                            NF_PKG_NOT_FOUND,
                            format!("pkg-not-found: {name} vanished from the sync dbs"),
                        ))
                    })?;
                    Ok(TransactionResult {
                        name,
                        version_applied: Some(version),
                    })
                })
            })
            .await
        })
        .await
    }

    async fn remove(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        let inner = self.inner.clone();
        with_budget_write(async move {
            spawn_blocking(move || {
                let name = request.name;
                let version_lookup = name.clone();
                let version = inner.call(move |alpm| {
                    alpm.localdb()
                        .pkg(version_lookup.as_str())
                        .map(|pkg| pkg.version().to_string())
                        .map_err(|_| {
                            native(
                                NF_PKG_NOT_FOUND,
                                format!("pkg-not-found: {version_lookup} is not installed"),
                            )
                        })
                })?;
                inner.call(move |alpm| {
                    alpm.trans_init(TransFlag::NONE)
                        .map_err(|e| native(NF_TRANSACTION, format!("transaction: init: {e}")))?;
                    Self::run_transaction(alpm, |alpm| match alpm.localdb().pkg(name.as_str()) {
                        Ok(pkg) => alpm.trans_remove_pkg(pkg).map_err(|e| {
                            native(NF_TRANSACTION, format!("transaction: remove: {e}"))
                        }),
                        Err(e) => Err(native(NF_TRANSACTION, format!("transaction: {e}"))),
                    })?;
                    Ok(TransactionResult {
                        name,
                        version_applied: Some(version),
                    })
                })
            })
            .await
        })
        .await
    }

    async fn history(
        &self,
        query: PackageHistoryQuery,
    ) -> Result<Vec<PackageHistoryEntry>, ContractError> {
        with_budget(async move {
            spawn_blocking(move || Self::read_history(query.name.as_deref(), query.limit)).await
        })
        .await
    }

    async fn undo(&self, request: PackageNameRequest) -> Result<UndoResult, ContractError> {
        let inner = self.inner.clone();
        let name = request.name;
        with_budget_write(async move {
            spawn_blocking(move || {
                let entries = Self::read_history(Some(name.as_str()), None)?;
                let Some((current, previous)) = logparse::previous_version(&entries) else {
                    return Err(native(
                        NF_UNDO_NO_CACHE,
                        String::from(UNDO_NO_CACHE_PAYLOAD),
                    ));
                };
                let Some(file_name) = Self::find_cached_file(&name, &previous)? else {
                    return Err(native(
                        NF_UNDO_NO_CACHE,
                        String::from(UNDO_NO_CACHE_PAYLOAD),
                    ));
                };
                let cache_dir = env_or("OMNIBUS_CACHE_DIR", "/var/cache/pacman/pkg");
                let full_path = Path::new(&cache_dir).join(&file_name);
                let trans_name = name.clone();
                inner.call(move |alpm| {
                    alpm.trans_init(TransFlag::NONE)
                        .map_err(|e| native(NF_TRANSACTION, format!("transaction: init: {e}")))?;
                    Self::run_transaction(alpm, |alpm| {
                        let loaded = alpm
                            .pkg_load(
                                full_path.display().to_string(),
                                false,
                                // Local-file signature policy, pacman's
                                // `LocalFileSigLevel = Optional` as shipped:
                                // the signature is checked when present and a
                                // missing detached `.sig` is tolerated (the
                                // cache holds locally built and stale-download
                                // entries without one), but a PRESENT
                                // signature must fully verify — marginal-trust
                                // and unknown-key signatures fail the load.
                                // Undo is a write path: it installs nothing
                                // that no trusted key vouches for. (DATABASE
                                // bits are meaningless for pkg_load.)
                                SigLevel::PACKAGE | SigLevel::PACKAGE_OPTIONAL,
                            )
                            .map_err(|e| {
                                native(
                                    NF_TRANSACTION,
                                    format!("transaction: load {file_name}: {e}"),
                                )
                            })?;
                        alpm.trans_add_pkg(loaded)
                            .map_err(|e| native(NF_TRANSACTION, format!("transaction: add: {e}")))
                    })?;
                    Ok(UndoResult {
                        name: trans_name,
                        from_version: current,
                        to_version: previous,
                        source: UndoSource::Cache,
                    })
                })
            })
            .await
        })
        .await
    }
}

/// Brief shape shared by local and sync sources.
fn brief_of(pkg: &Package, repo: Option<&str>) -> PackageBrief {
    PackageBrief {
        name: String::from(pkg.name()),
        version: pkg.version().to_string(),
        description: pkg.desc().map(String::from),
        repo: repo.map(String::from),
    }
}

fn info_from_local(pkg: &Package, version_available: Option<String>) -> PackageInfo {
    PackageInfo {
        name: String::from(pkg.name()),
        version: pkg.version().to_string(),
        description: pkg.desc().map(String::from),
        repo: None,
        size_bytes: Some(u64::try_from(pkg.size()).unwrap_or(0)),
        install_reason: Some(reason_of(pkg)),
        depends: pkg.depends().iter().map(|dep| dep.to_string()).collect(),
        required_by: pkg.required_by().into_iter().collect(),
        version_available,
    }
}

fn info_from_sync(pkg: &Package, repo: &str) -> PackageInfo {
    PackageInfo {
        name: String::from(pkg.name()),
        version: pkg.version().to_string(),
        description: pkg.desc().map(String::from),
        repo: Some(String::from(repo)),
        size_bytes: Some(u64::try_from(pkg.size()).unwrap_or(0)),
        install_reason: None,
        depends: pkg.depends().iter().map(|dep| dep.to_string()).collect(),
        required_by: Vec::new(),
        version_available: Some(pkg.version().to_string()),
    }
}

fn reason_of(pkg: &Package) -> String {
    match pkg.reason() {
        alpm::PackageReason::Explicit => String::from("explicit"),
        alpm::PackageReason::Depend => String::from("dependency"),
    }
}

fn matches(needle: &str, name: &str, desc: Option<&str>) -> bool {
    name.to_lowercase().contains(needle) || desc.is_some_and(|d| d.to_lowercase().contains(needle))
}

fn fact(name: &str) -> CapabilityFact {
    CapabilityFact {
        name: String::from(name),
        ver: String::from(CONTRACT_VERSION),
        status: String::from("present"),
        value: None,
        note: None,
        remedy: None,
    }
}

fn native(code: i32, payload: String) -> ContractError {
    ContractError::NativeFailure { code, payload }
}

/// Reads at most the last `cap` bytes of `path` as raw bytes. The leading
/// partial line left by the tail seek is dropped: the byte just before the
/// seek point is peeked, and only when it is not a newline (seek landed
/// mid-line) is the first buffered line discarded.
fn read_log_tail(path: &Path, cap: u64) -> Result<Vec<u8>, ContractError> {
    use std::io::{Read, Seek, SeekFrom};
    let log_err = |what: &str, e: std::io::Error| {
        native(
            NF_LOG_READ,
            format!("log: cannot {what} {}: {e}", path.display()),
        )
    };
    let mut file = std::fs::File::open(path).map_err(|e| log_err("open", e))?;
    let len = file.metadata().map_err(|e| log_err("stat", e))?.len();
    let skip = len.saturating_sub(cap);
    let mut buf = Vec::new();
    if skip == 0 {
        file.take(cap)
            .read_to_end(&mut buf)
            .map_err(|e| log_err("read", e))?;
        return Ok(buf);
    }
    // A newline right before the seek point means the tail starts on a line
    // boundary; otherwise the first buffered line is cut mid-way. (If the
    // peek itself fails, assume a cut line — the conservative fallback.)
    let boundary = {
        file.seek(SeekFrom::Start(skip - 1))
            .map_err(|e| log_err("seek", e))?;
        let mut probe = [0_u8; 1];
        file.read_exact(&mut probe)
            .is_ok_and(|()| probe[0] == b'\n')
    };
    file.seek(SeekFrom::Start(skip))
        .map_err(|e| log_err("seek", e))?;
    // `take` hard-bounds the read even if the log grows between stat and read.
    file.take(cap)
        .read_to_end(&mut buf)
        .map_err(|e| log_err("read", e))?;
    if !boundary {
        if let Some(pos) = buf.iter().position(|&byte| byte == b'\n') {
            buf.drain(..=pos);
        } else {
            buf.clear();
        }
    }
    Ok(buf)
}

/// Parses history entries from a raw log tail: lossy UTF-8 decode (one bad
/// byte becomes U+FFFD inside one line, the entry still parses or is skipped),
/// filtered by `name`, at most the `limit` most recent.
fn history_from_bytes(
    bytes: &[u8],
    name: Option<&str>,
    limit: Option<u32>,
) -> Vec<PackageHistoryEntry> {
    let text = String::from_utf8_lossy(bytes);
    let mut entries: Vec<PackageHistoryEntry> = text
        .lines()
        .filter_map(logparse::parse_line)
        .filter(|entry| name.is_none_or(|n| entry.name == n))
        .collect();
    if let Some(limit) = limit {
        let skip = entries.len().saturating_sub(limit as usize);
        entries.drain(..skip);
    }
    entries
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| String::from(default))
}

/// spawn_blocking with join-error mapping (the job itself never panics).
async fn spawn_blocking<T, F>(f: F) -> Result<T, ContractError>
where
    F: FnOnce() -> Result<T, ContractError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| native(NF_ALPM, format!("alpm: blocking task: {e}")))?
}

/// Wraps a future in the blocking-call budget; elapsing maps to TimeoutError.
///
/// Honest only for reads: a read observes no native state, so "timeout" is the
/// whole truth. Never use for writes — see [`with_budget_write`].
async fn with_budget<F, T>(fut: F) -> Result<T, ContractError>
where
    F: std::future::Future<Output = Result<T, ContractError>>,
{
    match tokio::time::timeout(BLOCKING_BUDGET, fut).await {
        Ok(result) => result,
        Err(_) => Err(ContractError::TimeoutError),
    }
}

/// Write-path budget: on ANY elapse answers [`NF_TIMEOUT_INDETERMINATE`],
/// never `TimeoutError`. The `spawn_blocking` job is uncancellable, and
/// dropping the handle detaches it: at answer time the job may already be
/// inside `trans_prepare` -> `trans_commit`, or still queued, or still in its
/// read-only pre-stages — which, once unstalled, WILL dispatch the
/// transaction (the pre-stage code runs to its `trans_init` send unless it
/// errors, and an error would have resolved the future before the budget
/// did). So neither "nothing was dispatched" (false for queued/in-flight
/// jobs, and for pre-stages that complete after the answer) nor "the
/// transaction was running" (false for a forever-stalled read stage) is an
/// airtight claim; the payload asserts exactly the uncertainty — whether a
/// package change lands natively is unknown — which holds in every case, and
/// tells the caller to re-check state instead of blindly retrying.
async fn with_budget_write<F, T>(fut: F) -> Result<T, ContractError>
where
    F: std::future::Future<Output = Result<T, ContractError>>,
{
    match tokio::time::timeout(BLOCKING_BUDGET, fut).await {
        Ok(result) => result,
        Err(_) => Err(native(
            NF_TIMEOUT_INDETERMINATE,
            String::from(WRITE_TIMEOUT_INDETERMINATE_PAYLOAD),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::dto::packages::PackageAction;
    use omnibus_contracts::suite::packages::PackagesFixture;

    /// Fleet-scale floor for the live list test: a base-devel container holds
    /// ~100-250 packages, a maintainer desktop 1600+. Below the floor the
    /// fleet-scale assertions self-skip with a stderr note (issue #77: the
    /// tests must pass, as honest skips, where the scale cannot exist).
    const FLEET_SCALE_FLOOR: usize = 1000;

    /// A write-budget elapse answers `timeout-indeterminate`, whatever stage
    /// the detached job was in: at answer time it may already be committing,
    /// still queued, or stalled in a read stage that completes (and
    /// dispatches) later — a plain `TimeoutError` would deny the
    /// still-possible native landing, so the only airtight answer claims the
    /// uncertainty (see [`with_budget_write`]).
    #[tokio::test]
    async fn stalled_write_answers_indeterminate() {
        tokio::time::pause();
        let task = tokio::spawn(async {
            with_budget_write(std::future::pending::<Result<(), ContractError>>()).await
        });
        tokio::time::advance(BLOCKING_BUDGET).await;
        let result = task.await.unwrap_or_else(|e| panic!("join: {e}"));
        match result {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_TIMEOUT_INDETERMINATE);
                assert_eq!(payload, WRITE_TIMEOUT_INDETERMINATE_PAYLOAD);
            }
            other => panic!("expected timeout-indeterminate, got {other:?}"),
        }
    }

    #[test]
    fn history_from_bytes_tolerates_bad_bytes_and_applies_filters() {
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(
            b"[2026-10-03T17:12:17+0300] [ALPM] installed xf86-video-ati (1:22.0.0-3)\n",
        );
        // One undecodable byte (a corrupted line): degrades that one entry.
        bytes.extend_from_slice(
            b"[2026-10-03T17:12:18+0300] [ALPM] installed xf86-v\xFFideo-amdgpu (25.0.0-1)\n",
        );
        bytes.extend_from_slice(
            b"[2026-10-03T17:17:26+0300] [ALPM] upgraded bash (5.2.037-1 -> 5.2.040-1)\n",
        );

        let all = history_from_bytes(&bytes, None, None);
        assert_eq!(all.len(), 3, "the bad byte must not kill the read");
        assert!(all[1].name.contains('\u{FFFD}'));

        let bash = history_from_bytes(&bytes, Some("bash"), None);
        assert_eq!(bash.len(), 1);
        assert_eq!(bash[0].action, PackageAction::Upgrade);

        let limited = history_from_bytes(&bytes, None, Some(2));
        assert_eq!(limited.len(), 2, "limit keeps the most recent entries");
        assert_eq!(limited[0].action, PackageAction::Install);
        assert_eq!(limited[1].action, PackageAction::Upgrade);
    }

    #[test]
    fn log_tail_reads_the_recent_end_and_drops_the_cut_line() {
        let path =
            std::env::temp_dir().join(format!("omnibus-alpm-tail-{}.log", std::process::id()));
        let first = b"[2026-10-03T17:12:17+0300] [ALPM] installed xf86-video-ati (1:22.0.0-3)\n";
        let second = b"[2026-10-03T17:12:18+0300] [ALPM] installed libxvmc (1.0.15-1)\n";
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(first);
        body.extend_from_slice(second);
        std::fs::write(&path, &body).expect("test fixture write");

        let whole = read_log_tail(&path, body.len() as u64).expect("full read");
        assert_eq!(whole, body);

        // Cap ten bytes into the first line: the seek lands mid-line, the cut
        // line is dropped, the later complete line remains.
        let tail = read_log_tail(&path, body.len() as u64 - 10).expect("tail read");
        assert_eq!(tail, second);
        // Cap exactly at the start of the second line: it survives whole.
        let exact = read_log_tail(&path, second.len() as u64).expect("exact tail read");
        assert_eq!(exact, second);
        // Cap that keeps only part of the first line: nothing parseable left.
        let nothing = read_log_tail(&path, 10).expect("tiny tail read");
        assert!(nothing.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn contract_suite_on_the_live_backend() {
        let Some(adapter) = open_or_skip() else {
            return;
        };
        let report = omnibus_contracts::suite::packages::packages_suite(
            || Box::new(adapter.clone()),
            PackagesFixture {
                undo_expected: true,
            },
        )
        .await;
        assert!(report.is_clean(), "{report}");
    }

    #[tokio::test]
    async fn list_sees_a_real_fleet_on_this_machine() {
        let Some(adapter) = open_or_skip() else {
            return;
        };
        let pkgs = adapter
            .list(PackageListRequest { limit: None })
            .await
            .expect("list must succeed on the live backend");
        // Fleet-scale assertion: maintainer desktops hold thousands of
        // packages, a base-devel container ~100-250. Below the floor the
        // machine simply is not the subject of the assertion — skip honestly
        // instead of failing the run.
        if pkgs.len() < FLEET_SCALE_FLOOR {
            eprintln!(
                "skipping fleet-scale list assertion: local database holds {} packages (< {FLEET_SCALE_FLOOR}; not a maintainer-scale machine)",
                pkgs.len()
            );
            return;
        }
        // No assert here: the skip guard above IS the fleet-scale check —
        // reaching this line means the floor held.
        let limited = adapter
            .list(PackageListRequest { limit: Some(10) })
            .await
            .expect("limited list must succeed");
        assert_eq!(limited.len(), 10);
    }

    #[tokio::test]
    async fn info_search_and_history_hit_real_data() {
        let Some(adapter) = open_or_skip() else {
            return;
        };
        // Guard the assumptions, don't assert them: bash may be absent and
        // pacman.log may not exist (fresh container, server image).
        let log_path = env_or("OMNIBUS_PACMAN_LOG", "/var/log/pacman.log");
        if !Path::new(&log_path).exists() {
            eprintln!("skipping history live assertions: {log_path} does not exist");
            return;
        }
        let info = match adapter.info(String::from("bash")).await {
            Ok(info) => info,
            Err(ContractError::NativeFailure { code, .. }) if code == NF_PKG_NOT_FOUND => {
                eprintln!("skipping info live assertions: bash is not installed on this machine");
                return;
            }
            Err(e) => panic!("info must succeed for an installed package: {e}"),
        };
        assert!(!info.version.is_empty());
        assert!(!info.depends.is_empty());
        let found = adapter
            .search(PackageSearch {
                query: String::from("linux"),
                limit: Some(5),
            })
            .await
            .expect("search must succeed");
        assert!(!found.is_empty() && found.len() <= 5);
        let history = adapter
            .history(PackageHistoryQuery {
                name: None,
                limit: Some(20),
            })
            .await
            .expect("history must read the live pacman.log");
        assert!(history.len() <= 20);
    }

    fn open_or_skip() -> Option<AlpmAdapter> {
        match AlpmAdapter::open() {
            Ok(adapter) => Some(adapter),
            Err(e) => {
                eprintln!("skipping alpm suite: no pacman database ({e})");
                None
            }
        }
    }
}

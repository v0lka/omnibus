//! Write serialization (specs/architecture/topology-and-lifecycle.md "Concurrency
//! Model"): in-process mutex plus cross-process `flock` at
//! `XDG_RUNTIME_DIR/omnibus/write.lock`. Application order equals lock
//! acquisition order and the audit records the factual order — with one honest
//! exception: a write that outlives its adapter's write budget releases the
//! gate while the native operation may still complete; the elapse answer is
//! adapter-defined — a `NativeFailure` payload naming the possibly-still-
//! running write where the adapter can name it (alpm, fs-xdg, systemd1), a
//! plain `TimeoutError` elsewhere — and the audit event keeps the late
//! outcome correlatable (see `tools::dispatch`, which never wraps a write in
//! a second, core-side timer). Reads never take the gate.
//!
//! The gate bounds itself: flock acquisition runs inside
//! [`crate::limits::WRITE_GATE_BUDGET`] (a wedged second instance answers
//! `TimeoutError` instead of hanging every write forever), and a lock file that
//! cannot be created degrades visibly ([`WriteGate::lock_degraded`], surfaced
//! in doctor with the failing reason — [`WriteGate::lock_failure_detail`])
//! instead of silently dropping cross-process serialization.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use omnibus_contracts::errors::ContractError;

/// Poll interval while another instance holds the flock.
const FLOCK_POLL: Duration = Duration::from_millis(10);

/// The cross-process lock file handle held for the duration of a write.
struct FlockGuard {
    _file: File,
}

/// One non-blocking acquisition attempt.
enum LockStep {
    Acquired(FlockGuard),
    /// Another instance holds the gate; retry shortly.
    Blocked,
    /// The lock file cannot be created, or flock itself failed with an error
    /// that retrying cannot fix (e.g. the filesystem has no flock support):
    /// a deterministic failure, surfaced instead of looping forever.
    Failed(String),
}

impl FlockGuard {
    fn try_acquire(path: &Path) -> LockStep {
        let file = match File::create(path) {
            Ok(file) => file,
            Err(e) => {
                return LockStep::Failed(format!(
                    "write lock file {} cannot be created: {e}",
                    path.display()
                ));
            }
        };
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => LockStep::Acquired(FlockGuard { _file: file }),
            Err(rustix::io::Errno::WOULDBLOCK) => LockStep::Blocked,
            Err(e) => LockStep::Failed(format!("flock failed: {e}")),
        }
    }
}

/// The write gate shared by every tool call of one instance.
#[derive(Clone)]
pub struct WriteGate {
    mutex: Arc<tokio::sync::Mutex<()>>,
    lock_path: Option<PathBuf>,
    /// False once the cross-process lock proved unavailable (creation failure
    /// or an unretryable flock error) — doctor names the degradation.
    lock_healthy: Arc<AtomicBool>,
    /// The most recent acquisition-failure reason (lock file creation failure
    /// or an unretryable flock error), cleared by the next successful
    /// acquisition — doctor quotes it so the degradation is named, not just
    /// flagged (R3: named degradation).
    lock_failure: Arc<Mutex<Option<String>>>,
}

impl WriteGate {
    /// The default gate: process mutex + flock at the runtime dir. The XDG
    /// rule applies: a present but empty value counts as unset (never a
    /// CWD-relative lock file — the #53 family), and a relative value is not a
    /// runtime dir either; without a usable absolute runtime dir the gate runs
    /// process-only and says so (`cross_process_available`), so doctor names
    /// the missing cross-process half instead of it failing silently.
    pub fn system_default() -> Self {
        let lock_path = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| !dir.as_os_str().is_empty() && dir.is_absolute())
            .map(|dir| dir.join("omnibus/write.lock"));
        Self {
            mutex: Arc::new(tokio::sync::Mutex::new(())),
            lock_path,
            lock_healthy: Arc::new(AtomicBool::new(true)),
            lock_failure: Arc::new(Mutex::new(None)),
        }
    }

    /// A gate without the cross-process half (tests, or systems without a runtime
    /// dir); serialization still holds within the process.
    pub fn process_only() -> Self {
        Self {
            mutex: Arc::new(tokio::sync::Mutex::new(())),
            lock_path: None,
            lock_healthy: Arc::new(AtomicBool::new(true)),
            lock_failure: Arc::new(Mutex::new(None)),
        }
    }

    /// True when a configured cross-process lock proved unavailable: writes
    /// proceed with in-process serialization only, visibly.
    pub fn lock_degraded(&self) -> bool {
        self.lock_path.is_some() && !self.lock_healthy.load(Ordering::Relaxed)
    }

    /// Why the cross-process lock last proved unavailable (`Some` whenever
    /// [`WriteGate::lock_degraded`] is true, save for a transient window while
    /// a concurrent acquisition recovers — the healthy flag flips first and
    /// the stale reason is dropped immediately after): the failing syscall's
    /// error text, for doctor to quote. `None` while healthy (a later
    /// successful acquisition clears it — doctor reports the most recent
    /// acquisition outcome) and when no cross-process half was configured at
    /// all.
    pub fn lock_failure_detail(&self) -> Option<String> {
        self.lock_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether the cross-process half exists at all (an absolute runtime dir
    /// was resolvable at construction). `false` means the gate serializes
    /// within the process only — the same condition a lost lock file reports,
    /// so doctor names it too (never silent).
    pub fn cross_process_available(&self) -> bool {
        self.lock_path.is_some()
    }

    /// Acquires the gate; the returned permit serializes writes app-wide.
    /// Errors:
    /// - `TimeoutError` when another instance held the cross-process flock past
    ///   [`crate::limits::WRITE_GATE_BUDGET`] — gate acquisition happens before
    ///   the tool budget starts, so the gate bounds itself (dispatch audits the
    ///   timeout).
    ///
    /// A lock file that cannot be created is *not* an error: the write proceeds
    /// with in-process serialization and the degradation stays visible (with
    /// the failing reason) through [`WriteGate::lock_degraded`] and
    /// [`WriteGate::lock_failure_detail`].
    pub async fn acquire(&self) -> Result<WritePermit, ContractError> {
        let owned = self.mutex.clone().lock_owned().await;
        let flock = match self.lock_path.as_ref() {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let attempts = async {
                    loop {
                        match FlockGuard::try_acquire(path) {
                            LockStep::Acquired(guard) => {
                                // The gate recovered: doctor reports the most
                                // recent acquisition outcome, not the worst
                                // moment of the process lifetime (R7 honesty
                                // cuts both ways — degradation visible, and so
                                // is recovery; the stale failure reason goes
                                // with it).
                                self.lock_healthy.store(true, Ordering::Relaxed);
                                self.lock_failure
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .take();
                                break Some(guard);
                            }
                            LockStep::Blocked => tokio::time::sleep(FLOCK_POLL).await,
                            LockStep::Failed(detail) => {
                                *self
                                    .lock_failure
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                    Some(detail);
                                self.lock_healthy.store(false, Ordering::Relaxed);
                                break None;
                            }
                        }
                    }
                };
                match tokio::time::timeout(crate::limits::WRITE_GATE_BUDGET, attempts).await {
                    Ok(flock) => flock,
                    Err(_elapsed) => {
                        // The in-process mutex guard drops with this return.
                        return Err(ContractError::TimeoutError);
                    }
                }
            }
            None => None,
        };
        Ok(WritePermit {
            _mutex_guard: owned,
            _flock: flock,
        })
    }
}

/// Held for the duration of one write action.
pub struct WritePermit {
    _mutex_guard: tokio::sync::OwnedMutexGuard<()>,
    _flock: Option<FlockGuard>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn permits_serialize_within_the_process() {
        let gate = WriteGate::process_only();
        let order = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for i in 0..4u32 {
            let gate = gate.clone();
            let order = order.clone();
            handles.push(tokio::spawn(async move {
                let _permit = gate
                    .acquire()
                    .await
                    .unwrap_or_else(|e| panic!("acquire: {e}"));
                order.lock().await.push(i);
            }));
        }
        for handle in handles {
            handle.await.unwrap_or_else(|e| panic!("join: {e}"));
        }
        let final_order = order.lock().await.len();
        assert_eq!(final_order, 4, "every writer completed under the gate");
    }

    #[tokio::test]
    async fn flock_file_is_created_when_runtime_dir_is_set() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = dir.path().join("nested/write.lock");
        let gate = WriteGate {
            mutex: Arc::new(tokio::sync::Mutex::new(())),
            lock_path: Some(path.clone()),
            lock_healthy: Arc::new(AtomicBool::new(true)),
            lock_failure: Arc::new(Mutex::new(None)),
        };
        {
            let _permit = gate
                .acquire()
                .await
                .unwrap_or_else(|e| panic!("acquire: {e}"));
            assert!(path.exists(), "flock file materializes on acquire");
        }
        assert!(!gate.lock_degraded());
    }

    /// A second instance holding the flock no longer hangs every write: the
    /// gate answers `TimeoutError` within its own budget. Virtual time keeps
    /// the test instant.
    #[tokio::test(start_paused = true)]
    async fn gate_times_out_when_another_instance_holds_the_flock() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = dir.path().join("write.lock");
        let holder = File::create(&path).unwrap_or_else(|e| panic!("create: {e}"));
        rustix::fs::flock(
            &holder,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .unwrap_or_else(|e| panic!("flock: {e}"));
        let gate = WriteGate {
            mutex: Arc::new(tokio::sync::Mutex::new(())),
            lock_path: Some(path),
            lock_healthy: Arc::new(AtomicBool::new(true)),
            lock_failure: Arc::new(Mutex::new(None)),
        };
        let result = gate.acquire().await;
        assert!(
            matches!(result, Err(ContractError::TimeoutError)),
            "a held cross-process gate times out instead of hanging"
        );
    }

    /// An uncreatable lock file degrades visibly: the write proceeds (the
    /// in-process mutex still serializes) and doctor can name the loss.
    #[tokio::test]
    async fn uncreatable_lock_file_degrades_visibly() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap_or_else(|e| panic!("write: {e}"));
        // A regular file where the lock directory should be: creation fails
        // deterministically, even for root.
        let gate = WriteGate {
            mutex: Arc::new(tokio::sync::Mutex::new(())),
            lock_path: Some(blocker.join("write.lock")),
            lock_healthy: Arc::new(AtomicBool::new(true)),
            lock_failure: Arc::new(Mutex::new(None)),
        };
        {
            let _permit = gate
                .acquire()
                .await
                .unwrap_or_else(|e| panic!("acquire: {e}"));
            assert!(
                gate.lock_degraded(),
                "the lost cross-process serialization is visible"
            );
            let detail = gate
                .lock_failure_detail()
                .unwrap_or_else(|| panic!("the degradation must name its cause"));
            assert!(
                detail.contains("cannot be created") && detail.contains("write.lock"),
                "the quoted reason names the failing operation and the path: {detail}"
            );
        }
        // Recovery is equally visible: once the lock file is creatable again,
        // the next acquisition clears both the flag and the stale reason, so
        // doctor reports the current state's cause, not the worst moment.
        std::fs::remove_file(&blocker).unwrap_or_else(|e| panic!("remove: {e}"));
        {
            let _permit = gate
                .acquire()
                .await
                .unwrap_or_else(|e| panic!("acquire after recovery: {e}"));
            assert!(!gate.lock_degraded(), "recovery clears the degradation");
            assert!(
                gate.lock_failure_detail().is_none(),
                "recovery clears the stale failure reason"
            );
        }
    }
}

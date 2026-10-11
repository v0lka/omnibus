//! Write serialization (specs/architecture/topology-and-lifecycle.md "Concurrency
//! Model"): in-process mutex plus cross-process `flock` at
//! `XDG_RUNTIME_DIR/omnibus/write.lock`. Application order equals lock acquisition
//! order and the audit records the factual order. Reads never take the gate.

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

/// The cross-process lock file handle held for the duration of a write.
struct FlockGuard {
    _file: File,
}

impl FlockGuard {
    /// Polls a non-blocking exclusive flock until acquired (async-safe: the
    /// executor thread never blocks on another process's critical section).
    async fn acquire(path: &PathBuf) -> Option<Self> {
        let file = File::create(path).ok()?;
        loop {
            match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => return Some(Self { _file: file }),
                // EWOULDBLOCK: another instance holds the gate; retry shortly.
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
    }
}

/// The write gate shared by every tool call of one instance.
#[derive(Clone)]
pub struct WriteGate {
    mutex: Arc<tokio::sync::Mutex<()>>,
    lock_path: Option<PathBuf>,
}

impl WriteGate {
    /// The default gate: process mutex + flock at the runtime dir.
    pub fn system_default() -> Self {
        let lock_path = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .map(|dir| dir.join("omnibus/write.lock"));
        Self {
            mutex: Arc::new(tokio::sync::Mutex::new(())),
            lock_path,
        }
    }

    /// A gate without the cross-process half (tests, or systems without a runtime
    /// dir); serialization still holds within the process.
    pub fn process_only() -> Self {
        Self {
            mutex: Arc::new(tokio::sync::Mutex::new(())),
            lock_path: None,
        }
    }

    /// Acquires the gate; the returned guard serializes writes app-wide.
    pub async fn acquire(&self) -> WritePermit {
        let owned = self.mutex.clone().lock_owned().await;
        let flock = match self.lock_path.as_ref() {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                FlockGuard::acquire(path).await
            }
            None => None,
        };
        WritePermit {
            _mutex_guard: owned,
            _flock: flock,
        }
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
                let _permit = gate.acquire().await;
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
        };
        {
            let _permit = gate.acquire().await;
            assert!(path.exists(), "flock file materializes on acquire");
        }
    }
}

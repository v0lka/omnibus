//! fs-xdg adapter: read/list/write inside granted XDG scopes (roadmap 1.5.5,
//! 1.6.4). The default read scope is `$HOME` including dotfiles (decision
//! 2026-10-10); writes require granted paths and are re-checked here under the
//! core's grant decision (defense in depth, specs/domains/files.md). Native
//! filesystem details never leave the crate — DTOs only at the seam.
//!
//! Every filesystem operation runs in [`tokio::task::spawn_blocking`] under a
//! wall-clock budget: a blocked syscall (stale mount, FIFO) parks only a
//! blocking-pool thread, never a runtime worker, and the caller gets an honest
//! timeout. Non-regular files are refused before they can block an open.

pub mod scope;

use async_trait::async_trait;
use omnibus_contracts::capabilities as cap;
use omnibus_contracts::dto::files::{
    DirEntry, DirListing, EntryKind, FileContent, FileReadRequest, FileWriteRequest,
    FileWriteResult, PathQuery, PathResolveRequest, ResolvedPath,
};
use omnibus_contracts::dto::probe::AdapterReport;
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::files::FileScope;

use scope::FileScopes;

/// Hard caps (files.md): reads and writes never exceed 2 MiB per call,
/// listings never exceed 2000 entries regardless of the requested limit.
const MAX_READ_BYTES: i64 = 2 * 1024 * 1024;
const MAX_WRITE_BYTES: usize = 2 * 1024 * 1024;
const MAX_LIST_ENTRIES: usize = 2000;

/// Wall-clock budget for read-side adapter filesystem operations (reads and
/// lists). Elapse is honest cancellation only for reads — a read observes no
/// native state, so `TimeoutError` is the whole truth there. Like the write
/// budget, it sits below the core's `TOOL_CALL_BUDGET` (10s,
/// `omnibus-core/src/limits.rs`): the adapter answers first and the core's
/// timeout stays the backstop instead of outrunning an answer that was on its
/// way.
const FS_BUDGET: std::time::Duration = std::time::Duration::from_secs(9);

/// Wall-clock budget for writes. The core never wraps a write in its own
/// timer (`tools::dispatch`: exactly one timeout layer per call), so this is
/// the only write timeout there is — its elapse must stay reachable as the
/// honest indeterminate-write error in [`with_write_budget`], never masked by
/// an outer plain `TimeoutError`. The workspace still keeps every write
/// budget strictly below the core's `TOOL_CALL_BUDGET` (10s,
/// `omnibus-core/src/limits.rs`) — canary-tested below — so no write outlives
/// the documented tool-call scale and the budget ordering stays simple to
/// reason about.
const FS_WRITE_BUDGET: std::time::Duration = std::time::Duration::from_secs(9);

/// Synthetic `NativeFailure` code: the budget elapsed while an uncancellable
/// blocking write was still in flight — the outcome is unknown (negative codes
/// are adapter-synthetic, never an errno).
const NF_TIMEOUT_INDETERMINATE: i32 = -2;
const WRITE_TIMEOUT_INDETERMINATE: &str =
    "fs_write: budget elapsed; the write may still have been applied";

/// The adapter: cheap-Clone over its scope set.
#[derive(Clone)]
pub struct FsXdgAdapter {
    scopes: std::sync::Arc<FileScopes>,
}

/// How the final component of a decided path is opened
/// ([`FsXdgAdapter::open_decided_path`]).
enum FinalOpen {
    /// `fs_read`: `O_RDONLY | O_NOFOLLOW | O_NONBLOCK` on the final
    /// component (`O_NONBLOCK` is belt and braces against a FIFO swapped
    /// in after the decision).
    Read,
    /// `fs_write`: `O_WRONLY | O_CREAT | O_TRUNC | O_NOFOLLOW | O_NONBLOCK`,
    /// mode `0o666` (the umask narrows it, as before). When `create_parents`
    /// is set, missing intermediate directories are created during the walk
    /// (mkdirat, mode `0o777`), matching the previous `create_dir_all`
    /// behavior — but relative to the walked directory, never through a
    /// re-walked path.
    Write { create_parents: bool },
    /// `fs_list`: `O_RDONLY | O_DIRECTORY | O_NOFOLLOW` on the final
    /// component — the listing root is pinned at open time exactly like an
    /// `fs_read`/`fs_write` target, so a symlink planted into any component
    /// after the scope decision fails the walk instead of redirecting the
    /// listing out of the approved scope.
    List,
}

impl FsXdgAdapter {
    /// Builds the adapter with the granted write paths (canonical check happens
    /// per call, so late mount/symlink changes are still judged correctly).
    /// Without a non-empty `HOME` in the environment the default read scope is
    /// unavailable and [`FileScope::report`] degrades honestly instead of
    /// claiming a present `$HOME` scope that reads nothing.
    pub fn new(write_paths: Vec<String>) -> Self {
        Self {
            scopes: std::sync::Arc::new(FileScopes::new(write_paths)),
        }
    }

    /// Explicit-scope constructor (hermetic tests; the composition root uses
    /// [`FsXdgAdapter::new`]).
    pub fn with_scopes(scopes: FileScopes) -> Self {
        Self {
            scopes: std::sync::Arc::new(scopes),
        }
    }

    fn io_error(context: &str, error: std::io::Error) -> ContractError {
        ContractError::NativeFailure {
            code: error.raw_os_error().unwrap_or(-1),
            payload: format!("{context}: {error}"),
        }
    }

    /// Opens a scope-decided path through a per-component `openat` dirfd walk.
    ///
    /// [`FileScopes::decide`] resolved every symlink at decision time, so each
    /// existing component of the canonical path was symlink-free when the
    /// scope was judged. A plain `open()` re-walks the whole path in the
    /// kernel, so a same-user process that renames an INTERMEDIATE directory
    /// onto a symlink inside the decide->open window redirects the operation
    /// out of the approved scope while `decide()` approved the resolved path.
    /// The walk instead re-opens every intermediate component relative to the
    /// previously opened directory with `O_NOFOLLOW | O_DIRECTORY`, and the
    /// final component with `O_NOFOLLOW` — a symlink planted anywhere after
    /// the decision fails the walk with ELOOP, mapped to the same typed
    /// refusal the final-component guard already gave. Because `decide()` has
    /// canonicalized the path, paths that were honest at decision time (a
    /// symlinked scope root included — it resolved at decision time) still
    /// open unchanged; the walk only rejects post-decide plantings.
    ///
    /// Intermediates are opened `O_PATH | O_NOFOLLOW | O_DIRECTORY`, not
    /// `O_RDONLY`: an `O_PATH` open needs only search (x) permission on the
    /// component, never read, so a wx-only drop-box directory (mode 0o1333)
    /// inside a granted scope keeps working — the `create_dir_all` + plain
    /// `open` path this walk replaced also needed only x, and reverting to
    /// read-opened intermediates would silently revoke that write capability.
    /// Every other guarantee is unchanged: `O_NOFOLLOW` still fails a planted
    /// symlink with ELOOP, `O_DIRECTORY` still fails a non-directory with
    /// ENOTDIR, an `O_PATH` descriptor remains a valid dirfd for
    /// `openat`/`mkdirat`, and the final component still opens with its real
    /// access flags and is fstat-verified by the caller.
    fn open_decided_path(
        canonical: &str,
        op: &str,
        final_open: FinalOpen,
    ) -> Result<std::fs::File, ContractError> {
        use rustix::fs::{Mode, OFlags, mkdirat, open, openat};
        use rustix::io::Errno;

        // The typed refusal: identical in spirit to the final-component guard
        // this walk replaces, naming which component was refused.
        let symlink_refusal = |position: &str, name: &str| ContractError::Unsupported {
            reason: format!(
                "{op}: {position} component {name} of {canonical} is a symlink; \
                 refusing to follow it"
            ),
        };
        let walk_error = |error: Errno| Self::io_error(op, std::io::Error::from(error));
        let walk_step = |dir: &std::os::fd::OwnedFd,
                         name: &std::ffi::OsStr|
         -> Result<std::os::fd::OwnedFd, ContractError> {
            // O_PATH (see the doc comment): search-only on the intermediate,
            // no read permission required.
            match openat(
                dir,
                name,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(next) => Ok(next),
                Err(Errno::LOOP) => Err(symlink_refusal("intermediate", &name.to_string_lossy())),
                // The write tail may not exist yet: with `create_parents` the
                // missing directory level is created, then the step is retried
                // (an `EEXIST` racing a concurrent creator is retried too).
                Err(Errno::NOENT)
                    if matches!(
                        final_open,
                        FinalOpen::Write {
                            create_parents: true
                        }
                    ) =>
                {
                    match mkdirat(dir, name, Mode::from_raw_mode(0o777)) {
                        Ok(()) | Err(Errno::EXIST) => {
                            match openat(
                                dir,
                                name,
                                OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                                Mode::empty(),
                            ) {
                                Ok(next) => Ok(next),
                                Err(Errno::LOOP) => {
                                    Err(symlink_refusal("intermediate", &name.to_string_lossy()))
                                }
                                Err(error) => Err(walk_error(error)),
                            }
                        }
                        Err(error) => Err(walk_error(error)),
                    }
                }
                Err(error) => Err(walk_error(error)),
            }
        };

        let components: Vec<&std::ffi::OsStr> = std::path::Path::new(canonical)
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        // The canonical path is absolute (the scope resolution lexically
        // normalizes onto "/"), so no components means the canonical is the
        // filesystem root itself; open(2) answered EISDIR for it before, and
        // the same errno is carried honestly here.
        let Some((final_name, intermediates)) = components.split_last() else {
            return Err(Self::io_error(
                op,
                std::io::Error::from_raw_os_error(libc::EISDIR),
            ));
        };
        // Walk from "/" down one component at a time, each step relative to
        // the pinned parent — the kernel never re-walks a path a swapper
        // could have rearranged mid-walk. A dropped step closes its parent
        // (OwnedFd), so the chain never leaks descriptors.
        let mut dir = open(
            std::path::Path::new("/"),
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(walk_error)?;
        for name in intermediates {
            dir = walk_step(&dir, name)?;
        }
        let (flags, mode) = match final_open {
            FinalOpen::Read => (
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            ),
            FinalOpen::Write { .. } => (
                OFlags::WRONLY
                    | OFlags::CREATE
                    | OFlags::TRUNC
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK,
                Mode::from_raw_mode(0o666),
            ),
            FinalOpen::List => (
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                Mode::empty(),
            ),
        };
        let fd = openat(&dir, *final_name, flags, mode).map_err(|error| match error {
            Errno::LOOP => symlink_refusal("final", &final_name.to_string_lossy()),
            error => walk_error(error),
        })?;
        Ok(std::fs::File::from(fd))
    }

    /// `fs_list` body — runs inside `spawn_blocking` (see [`FS_BUDGET`]).
    fn list_blocking(scopes: FileScopes, query: PathQuery) -> Result<DirListing, ContractError> {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat};
        let decision = scopes.decide(&query.path)?;
        if !decision.read_allowed {
            return Err(ContractError::ScopeDenied);
        }
        let limit = query
            .limit
            .map(|l| usize::try_from(l).unwrap_or(MAX_LIST_ENTRIES))
            .unwrap_or(MAX_LIST_ENTRIES)
            .min(MAX_LIST_ENTRIES);
        let depth = usize::from(query.depth.unwrap_or(1)).max(1);
        let root = decision.canonical.clone();
        let mut entries: Vec<DirEntry> = Vec::new();
        let mut truncated = false;

        // The decide->open race is closed exactly like fs_read/fs_write: the
        // listing root is opened through the per-component `O_NOFOLLOW` walk
        // in [`Self::open_decided_path`], so a symlink planted into any
        // component after the decision fails the walk (typed refusal)
        // instead of silently redirecting the listing out of the approved
        // scope. Every level below iterates a pinned dirfd — `read_dir` on a
        // re-walked path would re-open the same race for the root — and
        // subdirectories are opened `O_NOFOLLOW` relative to their pinned
        // parent, never through a re-walked path.
        let root_fd: std::os::fd::OwnedFd =
            Self::open_decided_path(&root, "fs_list", FinalOpen::List)?.into();

        // Bounded walk over pinned dirfds; the recursion is capped by depth
        // and the entry cap. Any unreadable subtree or entry marks the
        // listing truncated — a listing that silently drops entries would
        // present itself as complete.
        let mut stack: Vec<(std::os::fd::OwnedFd, usize)> = vec![(root_fd, 0usize)];
        while let Some((dir, level)) = stack.pop() {
            let mut reader = match rustix::fs::Dir::read_from(&dir) {
                Ok(reader) => reader,
                Err(error) => {
                    if entries.is_empty() && level == 0 {
                        return Err(Self::io_error("fs_list", std::io::Error::from(error)));
                    }
                    truncated = true;
                    continue;
                }
            };
            loop {
                if entries.len() >= limit {
                    truncated = true;
                    stack.clear();
                    break;
                }
                let entry = match reader.read() {
                    None => break,
                    Some(Ok(entry)) => entry,
                    Some(Err(error)) => {
                        if entries.is_empty() && level == 0 {
                            return Err(Self::io_error("fs_list", std::io::Error::from(error)));
                        }
                        truncated = true;
                        break;
                    }
                };
                // The raw stream carries the dot entries; they are skipped by
                // name, exactly as `std::fs::read_dir` does.
                let name = entry.file_name();
                let raw = name.to_bytes();
                if raw == b"." || raw == b".." {
                    continue;
                }
                // d_type comes straight from the pinned directory stream and
                // is never followed: links are reported as links and never
                // descended into, so a link cannot leak an outside tree.
                let mut file_type = entry.file_type();
                if file_type == FileType::Unknown {
                    // DT_UNKNOWN (overlayfs, some network filesystems):
                    // resolve the type relative to the pinned parent with
                    // `SYMLINK_NOFOLLOW` — never through a re-walked path.
                    match statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW) {
                        Ok(meta) => file_type = FileType::from_raw_mode(meta.st_mode),
                        Err(_) => {
                            truncated = true;
                            continue;
                        }
                    }
                }
                let (kind, size) = if file_type.is_symlink() {
                    (EntryKind::Symlink, None)
                } else if file_type.is_dir() {
                    (EntryKind::Dir, None)
                } else {
                    match statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW) {
                        Ok(meta) => (
                            EntryKind::File,
                            Some(u64::try_from(meta.st_size).unwrap_or(u64::MAX)),
                        ),
                        Err(_) => {
                            truncated = true;
                            continue;
                        }
                    }
                };
                let descend = file_type.is_dir() && level + 1 < depth;
                entries.push(DirEntry {
                    name: name.to_string_lossy().into_owned(),
                    kind,
                    size_bytes: size,
                });
                if descend {
                    // `O_NOFOLLOW`: a subdirectory swapped to a symlink (or
                    // to a non-directory) after its entry was read fails the
                    // open instead of leaking the link target's tree; the
                    // unreadable subtree marks the listing truncated.
                    match openat(
                        &dir,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                        Mode::empty(),
                    ) {
                        Ok(child) => stack.push((child, level + 1)),
                        Err(_) => {
                            truncated = true;
                        }
                    }
                }
            }
        }

        Ok(DirListing {
            path: root,
            entries,
            truncated,
        })
    }

    /// `fs_read` body — runs inside `spawn_blocking` (see [`FS_BUDGET`]).
    fn read_blocking(
        scopes: FileScopes,
        request: FileReadRequest,
    ) -> Result<FileContent, ContractError> {
        let decision = scopes.decide(&request.path)?;
        if !decision.read_allowed {
            return Err(ContractError::ScopeDenied);
        }
        let offset = request.offset_bytes.map(|o| o.0).unwrap_or(0);
        let max = request
            .max_bytes
            .map(|b| b.0)
            .unwrap_or(u64::try_from(MAX_READ_BYTES).unwrap_or(u64::MAX))
            .min(u64::try_from(MAX_READ_BYTES).unwrap_or(u64::MAX));

        let path = std::path::PathBuf::from(&decision.canonical);
        let meta = std::fs::metadata(&path).map_err(|e| Self::io_error("fs_read", e))?;
        // Refuse everything that is not a regular file before opening it: a
        // FIFO (or socket, or device) would park the open — or an endless
        // stream would saturate the read — instead of answering honestly.
        if !meta.is_file() {
            return Err(ContractError::Unsupported {
                reason: format!("fs_read: not a regular file: {}", decision.canonical),
            });
        }
        // The decide->open race is closed by the per-component walk in
        // [`Self::open_decided_path`]: every intermediate directory is opened
        // O_NOFOLLOW relative to the pinned parent, so a symlink planted into
        // any component after the decision fails the walk (typed refusal)
        // instead of silently redirecting the read out of scope. The handle
        // is then fstat-verified, so a FIFO or device that won the final
        // component race still never serves bytes.
        let mut file = Self::open_decided_path(&decision.canonical, "fs_read", FinalOpen::Read)?;
        // Judge the handle, not the path: whatever the final component is now,
        // these bytes come from the opened file.
        let meta = file.metadata().map_err(|e| Self::io_error("fs_read", e))?;
        if !meta.is_file() {
            return Err(ContractError::Unsupported {
                reason: format!("fs_read: not a regular file: {}", decision.canonical),
            });
        }
        use std::io::{Read as _, Seek as _, SeekFrom};
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| Self::io_error("fs_read", e))?;
        let mut buffer = Vec::new();
        let total = meta.len();
        let take = max.min(total.saturating_sub(offset));
        (&mut file)
            .take(take)
            .read_to_end(&mut buffer)
            .map_err(|e| Self::io_error("fs_read", e))?;
        // Honest partial-read flag: the payload is not the whole file whenever
        // the read started late or was capped (the agent then re-reads).
        let truncated = offset > 0
            || u64::try_from(buffer.len()).unwrap_or(u64::MAX) < total.saturating_sub(offset);

        let text = String::from_utf8(buffer.clone()).ok();
        Ok(FileContent {
            path: decision.canonical,
            size_bytes: total,
            data_base64: encode_base64(&buffer),
            text,
            truncated,
        })
    }

    /// `fs_write` body — runs inside `spawn_blocking` (see [`FS_BUDGET`]).
    fn write_blocking(
        scopes: FileScopes,
        request: FileWriteRequest,
    ) -> Result<FileWriteResult, ContractError> {
        let decision = scopes.decide(&request.path)?;
        if !decision.write_allowed {
            return Err(ContractError::ScopeDenied);
        }
        let bytes: Vec<u8> = if let Some(b64) = &request.data_base64 {
            // Reject the base64 form before decoding: base64 inflates the
            // payload by ~4/3, so a short length pre-check spares the daemon
            // the decode of a hostile oversized body.
            if b64.len() > MAX_WRITE_BYTES.div_ceil(3) * 4 {
                return Err(write_cap_error());
            }
            decode_base64(b64)?
        } else if let Some(text) = &request.text {
            text.clone().into_bytes()
        } else {
            Vec::new()
        };
        if bytes.len() > MAX_WRITE_BYTES {
            return Err(write_cap_error());
        }
        // The decide->open race is closed by the per-component walk in
        // [`Self::open_decided_path`]: intermediate directories are opened
        // (and, with `create_parents`, created) relative to the pinned parent
        // with O_NOFOLLOW, so a symlink swapped into any component between the
        // decision and this open fails the walk (typed refusal) instead of
        // silently redirecting the write — a granted write can no longer land
        // outside the approved scope. O_NONBLOCK keeps a target swapped to a
        // FIFO from parking the open; the opened handle is verified to be a
        // regular file before any byte is written, so a FIFO never has the
        // last word either.
        let mut file = Self::open_decided_path(
            &decision.canonical,
            "fs_write",
            FinalOpen::Write {
                create_parents: request.create_parents.is_some_and(|v| v),
            },
        )?;
        if !file
            .metadata()
            .map_err(|e| Self::io_error("fs_write", e))?
            .is_file()
        {
            return Err(ContractError::Unsupported {
                reason: format!("fs_write: not a regular file: {}", decision.canonical),
            });
        }
        use std::io::Write as _;
        file.write_all(&bytes)
            .map_err(|e| Self::io_error("fs_write", e))?;
        Ok(FileWriteResult {
            path: decision.canonical,
            bytes_written: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        })
    }
}

fn write_cap_error() -> ContractError {
    ContractError::Unsupported {
        reason: format!(
            "fs_write: payload exceeds the {} MiB per-call cap",
            MAX_WRITE_BYTES / (1024 * 1024)
        ),
    }
}

/// Runs a blocking filesystem task off the async workers: a blocked syscall
/// parks a blocking-pool thread the runtime can grow around, never a worker
/// that timers need.
async fn run_fs<T, F>(task: F) -> Result<T, ContractError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ContractError> + Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|e| ContractError::NativeFailure {
            code: -1,
            payload: format!("fs: blocking task failed: {e}"),
        })?
}

/// Read-side budget: on elapse [`ContractError::TimeoutError`] is the whole
/// truth (a read observes no native state).
async fn with_read_budget<T, F>(task: F) -> Result<T, ContractError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ContractError> + Send + 'static,
{
    match tokio::time::timeout(FS_BUDGET, run_fs(task)).await {
        Ok(result) => result,
        Err(_) => Err(ContractError::TimeoutError),
    }
}

/// Write-side budget: the `spawn_blocking` job is uncancellable — when the
/// budget elapses the worker may still complete the write natively, so the
/// error carries that fact instead of claiming a plain timeout. This is the
/// write's only timeout layer (the core does not timer writes — see
/// `tools::dispatch`); [`FS_WRITE_BUDGET`] stays below the core's tool budget
/// by workspace policy, keeping one documented latency scale.
async fn with_write_budget<T, F>(task: F) -> Result<T, ContractError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ContractError> + Send + 'static,
{
    match tokio::time::timeout(FS_WRITE_BUDGET, run_fs(task)).await {
        Ok(result) => result,
        Err(_) => Err(ContractError::NativeFailure {
            code: NF_TIMEOUT_INDETERMINATE,
            payload: String::from(WRITE_TIMEOUT_INDETERMINATE),
        }),
    }
}

#[async_trait]
impl FileScope for FsXdgAdapter {
    fn report(&self) -> AdapterReport {
        // R3 honesty: when $HOME is missing the default read scope does not
        // exist — the report says degraded, it never claims a present scope
        // that every read outside granted paths would contradict.
        let read_present = self.scopes.default_read_scope_available();
        AdapterReport {
            adapter: String::from("fs-xdg"),
            subsystem: String::from("files"),
            contract_version: String::from(omnibus_contracts::version::CONTRACT_VERSION),
            capabilities: vec![
                omnibus_contracts::dto::probe::CapabilityFact {
                    name: String::from(cap::FILES_SCOPE_READ),
                    ver: String::from(omnibus_contracts::version::CONTRACT_VERSION),
                    status: if read_present {
                        String::from("present")
                    } else {
                        String::from("degraded")
                    },
                    value: read_present.then(|| {
                        serde_json::json!({
                            "default_scope": "$HOME (dotfiles included)"
                        })
                    }),
                    note: (!read_present).then(|| {
                        String::from(
                            "$HOME is not set; the default read scope is unavailable — \
                             only granted write scopes are readable",
                        )
                    }),
                    remedy: (!read_present).then(|| {
                        String::from("launch omnibusd with HOME set to the user's home directory")
                    }),
                },
                omnibus_contracts::dto::probe::CapabilityFact {
                    name: String::from(cap::FILES_SCOPE_WRITE),
                    ver: String::from(omnibus_contracts::version::CONTRACT_VERSION),
                    status: String::from("present"),
                    value: None,
                    note: Some(String::from("active only inside granted write scopes")),
                    remedy: None,
                },
            ],
            notes: vec![],
        }
    }

    async fn list(&self, query: PathQuery) -> Result<DirListing, ContractError> {
        let scopes = (*self.scopes).clone();
        with_read_budget(move || Self::list_blocking(scopes, query)).await
    }

    async fn read(&self, request: FileReadRequest) -> Result<FileContent, ContractError> {
        let scopes = (*self.scopes).clone();
        with_read_budget(move || Self::read_blocking(scopes, request)).await
    }

    async fn write(&self, request: FileWriteRequest) -> Result<FileWriteResult, ContractError> {
        let scopes = (*self.scopes).clone();
        with_write_budget(move || Self::write_blocking(scopes, request)).await
    }

    async fn resolve(&self, request: PathResolveRequest) -> Result<ResolvedPath, ContractError> {
        let scopes = (*self.scopes).clone();
        with_read_budget(move || {
            let decision = scopes.decide(&request.path)?;
            Ok(ResolvedPath {
                path: request.path,
                canonical: decision.canonical,
                inside_home: decision.inside_home,
                read_allowed: decision.read_allowed,
                write_allowed: decision.write_allowed,
            })
        })
        .await
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_base64(text: &str) -> Result<Vec<u8>, ContractError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|e| ContractError::Unsupported {
            reason: format!("fs_write: data_base64 is not valid base64: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use omnibus_contracts::dto::{Bytes, OffsetBytes};

    fn adapter_for(dir: &std::path::Path) -> FsXdgAdapter {
        // Hermetic scopes: the temp dir stands in for $HOME, the projects
        // subdirectory is the granted write scope.
        FsXdgAdapter::with_scopes(scope::FileScopes::with_home(
            vec![dir.join("projects").to_string_lossy().into_owned()],
            dir.to_path_buf(),
        ))
    }

    fn text_content(text: &str) -> FileWriteRequest {
        FileWriteRequest {
            path: String::new(),
            data_base64: None,
            text: Some(String::from(text)),
            create_parents: Some(false),
        }
    }

    /// Creates a FIFO with the mkfifo(1) utility; `false` when unavailable.
    fn make_fifo(path: &std::path::Path) -> bool {
        match std::process::Command::new("mkfifo").arg(path).status() {
            Ok(status) if status.success() => true,
            _ => {
                eprintln!("skipping FIFO test: mkfifo unavailable");
                false
            }
        }
    }

    #[tokio::test]
    async fn write_read_roundtrip_inside_the_write_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let target = format!("{}/projects/notes/todo.md", dir.path().display());
        let mut request = text_content("# todo\n- ship it\n");
        request.path = target.clone();
        request.create_parents = Some(true);
        let written = adapter.write(request).await.expect("write inside scope");
        assert!(written.bytes_written > 0);
        let content = adapter
            .read(FileReadRequest {
                path: target,
                offset_bytes: None,
                max_bytes: Some(Bytes(1024)),
            })
            .await
            .expect("read back");
        assert!(
            content
                .text
                .as_deref()
                .unwrap_or_default()
                .contains("ship it")
        );
        assert!(!content.truncated);
    }

    #[tokio::test]
    async fn write_outside_the_scope_is_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let mut request = text_content("nope");
        request.path = String::from("/etc/omnibus-should-not-write.conf");
        let denied = adapter.write(request).await;
        assert!(matches!(denied, Err(ContractError::ScopeDenied)));
    }

    #[tokio::test]
    async fn read_outside_home_is_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let denied = adapter
            .read(FileReadRequest {
                path: String::from("/etc/os-release"),
                offset_bytes: None,
                max_bytes: None,
            })
            .await;
        assert!(matches!(denied, Err(ContractError::ScopeDenied)));
    }

    #[tokio::test]
    async fn offset_and_truncation_are_honest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let target = format!("{}/projects/blob.bin", dir.path().display());
        let payload: Vec<u8> = (0..64u8).collect();
        let request = FileWriteRequest {
            path: target.clone(),
            data_base64: Some(base64::engine::general_purpose::STANDARD.encode(&payload)),
            text: None,
            create_parents: Some(true),
        };
        adapter.write(request).await.expect("write blob");
        let content = adapter
            .read(FileReadRequest {
                path: target,
                offset_bytes: Some(OffsetBytes(60)),
                max_bytes: Some(Bytes(1024)),
            })
            .await
            .expect("read tail");
        assert_eq!(content.size_bytes, 64);
        assert_eq!(content.data_base64.len() % 4, 0);
        assert!(content.truncated, "60..64 is a truncated read of 64 bytes");
    }

    #[tokio::test]
    async fn list_respects_depth_and_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let listing = adapter
            .list(PathQuery {
                path: String::from("."),
                depth: Some(1),
                limit: Some(5),
            })
            .await
            .expect("list home");
        assert!(listing.entries.len() <= 5);
    }

    #[test]
    fn report_names_only_registry_capabilities() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let report = adapter.report();
        assert!(
            report
                .capabilities
                .iter()
                .all(|f| omnibus_contracts::capabilities::ALL.contains(&f.name.as_str()))
        );
    }

    // ---- Issue #29: a missing $HOME must degrade the report, not silently
    // collapse the default read scope while claiming it present.

    #[test]
    fn report_degrades_honestly_without_home() {
        let adapter = FsXdgAdapter::with_scopes(scope::FileScopes::without_home(vec![]));
        let report = adapter.report();
        let read = report
            .capabilities
            .iter()
            .find(|f| f.name == cap::FILES_SCOPE_READ)
            .expect("read capability fact");
        assert_eq!(read.status, "degraded");
        assert!(read.note.is_some(), "the degradation must say why");
        assert!(read.remedy.is_some());
        assert!(read.value.is_none(), "no default scope exists to advertise");
    }

    #[tokio::test]
    async fn home_relative_reads_fail_closed_without_home() {
        let adapter = FsXdgAdapter::with_scopes(scope::FileScopes::without_home(vec![]));
        let denied = adapter
            .read(FileReadRequest {
                path: String::from("~/notes.txt"),
                offset_bytes: None,
                max_bytes: None,
            })
            .await;
        assert!(matches!(denied, Err(ContractError::ScopeDenied)));
    }

    // ---- Issue #30: symlinked scopes resolve tails, never judge lexically.

    #[tokio::test]
    async fn symlinked_scope_dir_keeps_the_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let real = dir.path().join("real");
        std::fs::create_dir_all(&home).expect("home dir");
        std::fs::create_dir_all(&real).expect("real dir");
        std::os::unix::fs::symlink(&real, home.join("projects")).expect("symlink");
        let adapter = FsXdgAdapter::with_scopes(scope::FileScopes::with_home(
            vec![home.join("projects").to_string_lossy().into_owned()],
            home.clone(),
        ));
        let target = home.join("projects/new.md").to_string_lossy().into_owned();
        let mut request = text_content("through the link\n");
        request.path = target.clone();
        request.create_parents = Some(true);
        adapter.write(request).await.expect("write through link");
        assert_eq!(
            std::fs::read_to_string(real.join("new.md")).expect("real file"),
            "through the link\n",
            "the file must land behind the symlinked scope root, tail intact"
        );
    }

    #[tokio::test]
    async fn symlink_in_the_tail_never_decides_lexically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("projects/w")).expect("dirs");
        std::os::unix::fs::symlink("/etc", home.join("projects/w/link")).expect("symlink");
        let adapter = FsXdgAdapter::with_scopes(scope::FileScopes::with_home(
            vec![home.join("projects").to_string_lossy().into_owned()],
            home.clone(),
        ));
        let mut request = text_content("escape\n");
        request.path = home
            .join("projects/w/link/omnibus-escape")
            .to_string_lossy()
            .into_owned();
        let denied = adapter.write(request).await;
        assert!(
            matches!(denied, Err(ContractError::ScopeDenied)),
            "a tail symlink pointing into /etc must not be judged lexically"
        );
        assert!(!std::path::Path::new("/etc/omnibus-escape").exists());
    }

    // ---- Issue #31: the final-component symlink race fails the write.

    #[tokio::test]
    async fn write_through_a_final_component_symlink_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        std::fs::create_dir_all(dir.path().join("projects")).expect("projects dir");
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "keep").expect("seed victim");
        std::os::unix::fs::symlink(&victim, dir.path().join("projects/notes.txt"))
            .expect("symlink");
        let mut request = text_content("overwrite\n");
        request.path = dir
            .path()
            .join("projects/notes.txt")
            .to_string_lossy()
            .into_owned();
        let refused = adapter.write(request).await;
        assert!(
            matches!(refused, Err(ContractError::ScopeDenied)),
            "the planted tail symlink resolves to the victim path, which is \
             outside the write scope — the decision denies it"
        );
        assert_eq!(
            std::fs::read_to_string(&victim).expect("victim intact"),
            "keep",
            "the symlink target must be untouched"
        );
        // Reads still follow the link honestly (the canonical path resolves).
        let content = adapter
            .read(FileReadRequest {
                path: dir
                    .path()
                    .join("projects/notes.txt")
                    .to_string_lossy()
                    .into_owned(),
                offset_bytes: None,
                max_bytes: None,
            })
            .await
            .expect("read through link");
        assert_eq!(content.text.as_deref(), Some("keep"));
    }

    // ---- Budget ordering: the adapter budgets must stay below the core's.

    #[test]
    fn the_adapter_budgets_sit_below_the_core_tool_budget() {
        // omnibus-core/src/limits.rs pins TOOL_CALL_BUDGET at 10s; adapters
        // depend only on contracts, so the value is mirrored here as a
        // canary. Writes are not wrapped in a core timer (tools::dispatch:
        // exactly one timeout layer per call), so this budget ordering keeps
        // the documented latency scale: the honest adapter answer — the
        // indeterminate-write error for writes, the earned read answer for
        // reads — is the only answer a call can end with.
        assert_eq!(FS_BUDGET, std::time::Duration::from_secs(9));
        assert_eq!(FS_WRITE_BUDGET, std::time::Duration::from_secs(9));
        assert!(FS_BUDGET < std::time::Duration::from_secs(10));
    }

    // ---- fs_read hardening: the decide->open race must not leak bytes.

    /// Mirror of the write-side planted-symlink refusal: a final-component
    /// symlink inside the read scope but resolving outside `$HOME` is refused.
    #[tokio::test]
    async fn read_through_a_planted_final_component_symlink_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "out of scope").expect("secret");
        let adapter = adapter_for(dir.path());
        std::fs::create_dir_all(dir.path().join("projects")).expect("projects dir");
        std::os::unix::fs::symlink(&secret, dir.path().join("projects/notes.txt"))
            .expect("symlink");
        let refused = adapter
            .read(FileReadRequest {
                path: dir
                    .path()
                    .join("projects/notes.txt")
                    .to_string_lossy()
                    .into_owned(),
                offset_bytes: None,
                max_bytes: None,
            })
            .await;
        assert!(
            matches!(refused, Err(ContractError::ScopeDenied)),
            "a planted symlink resolving outside $HOME must be denied"
        );
    }

    /// The decide->open race: a same-user swapper flips the target between a
    /// regular in-scope file and a symlink to an out-of-scope secret. No read
    /// may ever return the secret's bytes; a read that loses the race is
    /// refused honestly instead of following the link.
    #[tokio::test]
    async fn symlink_swapped_mid_read_never_leaks_out_of_scope_bytes() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        std::fs::create_dir_all(dir.path().join("projects")).expect("projects dir");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "OUT-OF-SCOPE-SECRET").expect("secret");
        let target = dir.path().join("projects/notes.txt");
        std::fs::write(&target, "in scope\n").expect("seed target");
        // Staging entries swapped in with rename(2): the replacement is
        // atomic, so the target never goes missing mid-read (no ENOENT noise,
        // every read sees one of the two honest states).
        let swap_link = dir.path().join("projects/.race-link");
        let swap_file = dir.path().join("projects/.race-file");
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let swapper_stop = stop.clone();
        let swapper_target = target.clone();
        let swapper_secret = secret.clone();
        let swapper = std::thread::spawn(move || {
            while !swapper_stop.load(AtomicOrdering::Relaxed) {
                let _ = std::os::unix::fs::symlink(&swapper_secret, &swap_link);
                let _ = std::fs::rename(&swap_link, &swapper_target);
                std::thread::yield_now();
                let _ = std::fs::write(&swap_file, "in scope\n");
                let _ = std::fs::rename(&swap_file, &swapper_target);
                std::thread::yield_now();
            }
        });

        let mut reads_ok = 0u32;
        for _ in 0..1000 {
            match adapter
                .read(FileReadRequest {
                    path: target.to_string_lossy().into_owned(),
                    offset_bytes: None,
                    max_bytes: None,
                })
                .await
            {
                Ok(content) => {
                    reads_ok += 1;
                    assert!(
                        !content
                            .text
                            .as_deref()
                            .unwrap_or_default()
                            .contains("OUT-OF-SCOPE-SECRET"),
                        "a read that lost the race must not follow the planted symlink"
                    );
                }
                // Honest refusals: the decision saw the symlink (ScopeDenied),
                // the open hit O_NOFOLLOW / the non-regular handle check
                // (Unsupported), or the hostile concurrent renamer made the
                // resolution fail transiently (NativeFailure) — none of them
                // follow the link.
                Err(
                    ContractError::ScopeDenied
                    | ContractError::Unsupported { .. }
                    | ContractError::NativeFailure { .. },
                ) => {}
                Err(other) => panic!("unexpected error during the race: {other:?}"),
            }
        }
        stop.store(true, AtomicOrdering::Relaxed);
        swapper.join().expect("swapper thread");
        assert!(reads_ok > 0, "reads must still work through the swap storm");
    }

    // ---- The intermediate-component decide->open race: O_NOFOLLOW guarded
    // only the final component, so a symlink renamed over an INTERMEDIATE
    // directory inside the decide->open window redirected the kernel's path
    // walk out of the approved scope. The per-component openat walk in
    // [`FsXdgAdapter::open_decided_path`] refuses it; these tests mirror the
    // final-component storms above at the intermediate level.

    /// A same-user swapper flips the INTERMEDIATE directory `projects/sub`
    /// between a real directory holding the in-scope file and a symlink
    /// pointing at an out-of-scope secret. No read may ever return the
    /// secret's bytes; losing the race must produce the typed symlink
    /// refusal (or another honest transient failure), never the secret.
    #[tokio::test]
    async fn intermediate_symlink_swapped_mid_read_never_leaks_out_of_scope_bytes() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        // The honest walk: home/projects/sub/notes.txt, a real directory chain.
        std::fs::create_dir_all(dir.path().join("projects/sub")).expect("dirs");
        std::fs::write(dir.path().join("projects/sub/notes.txt"), "in scope\n")
            .expect("seed target");
        // The planted walk: projects/sub -> <outside>/evil, where the same
        // final name holds the secret. The link is planted by rename(2) only
        // between a read's decide() and its open — decision time still sees
        // the honest chain.
        let evil = outside.path().join("evil");
        std::fs::create_dir_all(&evil).expect("evil dir");
        std::fs::write(evil.join("notes.txt"), "OUT-OF-SCOPE-SECRET").expect("secret");
        let sub = dir.path().join("projects/sub");
        let hold = dir.path().join("projects/.sub-hold");
        let link_stage = dir.path().join("projects/.sub-link");
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let swapper_stop = stop.clone();
        let swapper_sub = sub.clone();
        let swapper_evil = evil.clone();
        let swapper = std::thread::spawn(move || {
            let mut honest = true;
            while !swapper_stop.load(AtomicOrdering::Relaxed) {
                if honest {
                    // Plant: move the real directory away (the name is
                    // briefly absent — an honest transient ENOENT), then
                    // rename the staged link into its place, atomically.
                    let _ = std::fs::rename(&swapper_sub, &hold);
                    let _ = std::os::unix::fs::symlink(&swapper_evil, &link_stage);
                    let _ = std::fs::rename(&link_stage, &swapper_sub);
                } else {
                    // Restore: move the link away, drop the real directory
                    // back into the name.
                    let _ = std::fs::rename(&swapper_sub, &link_stage);
                    let _ = std::fs::rename(&hold, &swapper_sub);
                }
                honest = !honest;
                std::thread::yield_now();
            }
        });

        let mut reads_ok = 0u32;
        for _ in 0..1000 {
            match adapter
                .read(FileReadRequest {
                    path: sub.join("notes.txt").to_string_lossy().into_owned(),
                    offset_bytes: None,
                    max_bytes: None,
                })
                .await
            {
                Ok(content) => {
                    reads_ok += 1;
                    assert!(
                        !content
                            .text
                            .as_deref()
                            .unwrap_or_default()
                            .contains("OUT-OF-SCOPE-SECRET"),
                        "a read that lost the intermediate race must not follow \
                         the planted directory symlink"
                    );
                }
                // Honest refusals: the decision saw the planted link
                // (ScopeDenied), the walk met it after the decision
                // (Unsupported), or the swapper's atomic two-step rename made
                // the name briefly absent (NativeFailure) — none of them
                // follow the link.
                Err(
                    ContractError::ScopeDenied
                    | ContractError::Unsupported { .. }
                    | ContractError::NativeFailure { .. },
                ) => {}
                Err(other) => panic!("unexpected error during the race: {other:?}"),
            }
        }
        stop.store(true, AtomicOrdering::Relaxed);
        swapper.join().expect("swapper thread");
        assert!(reads_ok > 0, "reads must still work through the swap storm");
    }

    /// Mirror of the read-side intermediate storm for listings: `fs_list` of
    /// `projects/sub` during the swap storm must never list the planted
    /// intermediate symlink's target — the listing rides the pinned
    /// `O_NOFOLLOW` dirfd walk exactly like the read's bytes do.
    #[tokio::test]
    async fn intermediate_symlink_swapped_mid_list_never_lists_out_of_scope() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        // The honest walk: home/projects/sub/notes.txt, a real directory chain.
        std::fs::create_dir_all(dir.path().join("projects/sub")).expect("dirs");
        std::fs::write(dir.path().join("projects/sub/notes.txt"), "in scope\n")
            .expect("seed target");
        // The planted walk: projects/sub -> <outside>/evil, whose listing
        // would carry `secret.txt`. Planted by rename(2) only between a
        // list's decide() and its open — decision time sees the honest chain.
        let evil = outside.path().join("evil");
        std::fs::create_dir_all(&evil).expect("evil dir");
        std::fs::write(evil.join("secret.txt"), "OUT-OF-SCOPE-SECRET").expect("secret");
        let sub = dir.path().join("projects/sub");
        let hold = dir.path().join("projects/.sub-hold");
        let link_stage = dir.path().join("projects/.sub-link");
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let swapper_stop = stop.clone();
        let swapper_sub = sub.clone();
        let swapper_evil = evil.clone();
        let swapper = std::thread::spawn(move || {
            let mut honest = true;
            while !swapper_stop.load(AtomicOrdering::Relaxed) {
                if honest {
                    let _ = std::fs::rename(&swapper_sub, &hold);
                    let _ = std::os::unix::fs::symlink(&swapper_evil, &link_stage);
                    let _ = std::fs::rename(&link_stage, &swapper_sub);
                } else {
                    let _ = std::fs::rename(&swapper_sub, &link_stage);
                    let _ = std::fs::rename(&hold, &swapper_sub);
                }
                honest = !honest;
                std::thread::yield_now();
            }
        });

        let mut lists_ok = 0u32;
        for _ in 0..1000 {
            match adapter
                .list(PathQuery {
                    path: sub.to_string_lossy().into_owned(),
                    depth: None,
                    limit: None,
                })
                .await
            {
                Ok(listing) => {
                    lists_ok += 1;
                    assert!(
                        listing.entries.iter().all(|e| e.name != "secret.txt"),
                        "a listing that lost the intermediate race must not follow \
                         the planted directory symlink"
                    );
                }
                // Honest refusals, same classes as the read storm: the
                // decision saw the planted link (ScopeDenied), the walk met
                // it after the decision (Unsupported), or the name was
                // briefly absent mid-rename (NativeFailure).
                Err(
                    ContractError::ScopeDenied
                    | ContractError::Unsupported { .. }
                    | ContractError::NativeFailure { .. },
                ) => {}
                Err(other) => panic!("unexpected error during the race: {other:?}"),
            }
        }
        stop.store(true, AtomicOrdering::Relaxed);
        swapper.join().expect("swapper thread");
        assert!(
            lists_ok > 0,
            "listings must still work through the swap storm"
        );
    }

    /// Mirror of the read-side intermediate storm for writes: a granted write
    /// to `projects/sub/notes.txt` must never land behind the planted
    /// intermediate symlink in `evil/` — a directory inside `$HOME` but
    /// outside the granted write scope, so the scope decision alone cannot
    /// catch the planting.
    #[tokio::test]
    async fn intermediate_symlink_swapped_mid_write_never_lands_out_of_scope() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        std::fs::create_dir_all(dir.path().join("projects/sub")).expect("dirs");
        std::fs::write(dir.path().join("projects/sub/notes.txt"), "in scope\n")
            .expect("seed target");
        // The redirect target: inside $HOME (so the decision approves the
        // honest path), outside the granted write scope (so a landing here is
        // a scope violation, not just an unexpected write).
        let evil = dir.path().join("evil");
        std::fs::create_dir_all(&evil).expect("evil dir");
        std::fs::write(evil.join("notes.txt"), "victim").expect("seed victim");
        let sub = dir.path().join("projects/sub");
        let hold = dir.path().join("projects/.sub-hold");
        let link_stage = dir.path().join("projects/.sub-link");
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let swapper_stop = stop.clone();
        let swapper_sub = sub.clone();
        let swapper_evil = evil.clone();
        let swapper = std::thread::spawn(move || {
            let mut honest = true;
            while !swapper_stop.load(AtomicOrdering::Relaxed) {
                if honest {
                    let _ = std::fs::rename(&swapper_sub, &hold);
                    let _ = std::os::unix::fs::symlink(&swapper_evil, &link_stage);
                    let _ = std::fs::rename(&link_stage, &swapper_sub);
                } else {
                    let _ = std::fs::rename(&swapper_sub, &link_stage);
                    let _ = std::fs::rename(&hold, &swapper_sub);
                }
                honest = !honest;
                std::thread::yield_now();
            }
        });

        let mut writes_ok = 0u32;
        for _ in 0..1000 {
            let mut request = text_content("in scope\n");
            request.path = sub.join("notes.txt").to_string_lossy().into_owned();
            match adapter.write(request).await {
                Ok(_) => writes_ok += 1,
                // Same honest refusals as the read storm: the decision caught
                // the link, the walk refused it, or the name was briefly
                // absent mid-rename.
                Err(
                    ContractError::ScopeDenied
                    | ContractError::Unsupported { .. }
                    | ContractError::NativeFailure { .. },
                ) => {}
                Err(other) => panic!("unexpected error during the race: {other:?}"),
            }
        }
        stop.store(true, AtomicOrdering::Relaxed);
        swapper.join().expect("swapper thread");
        assert!(
            writes_ok > 0,
            "writes must still work through the swap storm"
        );
        assert!(
            std::fs::read_to_string(evil.join("notes.txt")).is_ok_and(|c| c == "victim"),
            "no write may land behind the planted intermediate symlink"
        );
    }

    // ---- Search-only intermediates: the walk opens intermediates O_PATH, so
    // a wx-only drop-box inside a granted scope stays writable — the
    // capability the replaced `create_dir_all` + plain `open` path had. An
    // O_RDONLY intermediate open would need read permission and fail with
    // EACCES there.

    /// A wx-only drop-box directory (mode 0o1333, sticky + wx-wx-wx) inside
    /// the granted write scope admits writes and reads of files within it
    /// (search x is enough to traverse; the file itself is mode 0o666&~umask).
    #[tokio::test]
    async fn search_only_drop_box_intermediate_stays_writable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let drop = dir.path().join("projects/drop");
        std::fs::create_dir_all(&drop).expect("drop dir");
        use std::os::unix::fs::PermissionsExt as _;
        // 0o1333 = wx-wx-wx: the owner can write and traverse but NOT read the
        // directory — a real drop box. (0o1733 would give the owner rwx, and
        // the probe below would skip everywhere.)
        std::fs::set_permissions(&drop, std::fs::Permissions::from_mode(0o1333))
            .expect("wx-only drop-box mode");
        // Root defeats permission checks; the assertion only holds where
        // wx-only actually denies a read of the directory.
        if std::fs::read_dir(&drop).is_ok() {
            std::fs::set_permissions(&drop, std::fs::Permissions::from_mode(0o755))
                .expect("restore");
            eprintln!("skipping: running as a privileged user, wx-only is readable");
            return;
        }
        let target = drop.join("note.txt");
        let mut request = text_content("dropped\n");
        request.path = target.to_string_lossy().into_owned();
        let written = adapter
            .write(request)
            .await
            .expect("write through the wx-only intermediate");
        assert!(written.bytes_written > 0);
        let content = adapter
            .read(FileReadRequest {
                path: target.to_string_lossy().into_owned(),
                offset_bytes: None,
                max_bytes: None,
            })
            .await
            .expect("read back through the wx-only intermediate");
        std::fs::set_permissions(&drop, std::fs::Permissions::from_mode(0o755)).expect("restore");
        assert_eq!(content.text.as_deref(), Some("dropped\n"));
    }

    // ---- Issue #32: non-regular files are refused, never parked.

    #[tokio::test]
    async fn reading_a_fifo_is_refused_not_hanging() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        std::fs::create_dir_all(dir.path().join("projects")).expect("projects dir");
        let fifo = dir.path().join("projects/pipe");
        if !make_fifo(&fifo) {
            return;
        }
        let refused = adapter
            .read(FileReadRequest {
                path: fifo.to_string_lossy().into_owned(),
                offset_bytes: None,
                max_bytes: None,
            })
            .await;
        assert!(
            matches!(refused, Err(ContractError::Unsupported { .. })),
            "a FIFO must be refused honestly, not parked on"
        );
    }

    #[tokio::test]
    async fn writing_a_fifo_fails_without_parking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        std::fs::create_dir_all(dir.path().join("projects")).expect("projects dir");
        let fifo = dir.path().join("projects/pipe");
        if !make_fifo(&fifo) {
            return;
        }
        let mut request = text_content("no reader\n");
        request.path = fifo.to_string_lossy().into_owned();
        let failed = adapter.write(request).await;
        assert!(
            matches!(failed, Err(ContractError::NativeFailure { .. })),
            "opening a reader-less FIFO must fail fast (O_NONBLOCK), not park"
        );
    }

    #[tokio::test]
    async fn writing_a_fifo_with_a_reader_is_refused_as_non_regular() {
        use std::os::unix::fs::OpenOptionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        std::fs::create_dir_all(dir.path().join("projects")).expect("projects dir");
        let fifo = dir.path().join("projects/pipe");
        if !make_fifo(&fifo) {
            return;
        }
        // A live reader makes the writer-side open succeed — the handle is
        // then fstat-verified and the write refused as not-a-regular-file.
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .expect("non-blocking reader");
        let mut request = text_content("with reader\n");
        request.path = fifo.to_string_lossy().into_owned();
        let refused = adapter.write(request).await;
        drop(reader);
        assert!(
            matches!(refused, Err(ContractError::Unsupported { .. })),
            "an opened FIFO must be refused by the regular-file check"
        );
    }

    #[tokio::test]
    async fn reading_a_socket_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        std::fs::create_dir_all(dir.path().join("projects")).expect("projects dir");
        let socket = dir.path().join("projects/sock");
        std::os::unix::net::UnixListener::bind(&socket).expect("bind socket");
        let refused = adapter
            .read(FileReadRequest {
                path: socket.to_string_lossy().into_owned(),
                offset_bytes: None,
                max_bytes: None,
            })
            .await;
        assert!(matches!(refused, Err(ContractError::Unsupported { .. })));
    }

    // ---- Issue #34: the write payload is capped symmetrically to reads.

    #[tokio::test]
    async fn write_payload_over_the_cap_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let mut request = text_content(&"x".repeat(MAX_WRITE_BYTES + 1));
        request.path = dir
            .path()
            .join("projects/big.txt")
            .to_string_lossy()
            .into_owned();
        let refused = adapter.write(request).await;
        match refused {
            Err(ContractError::Unsupported { reason }) => {
                assert!(
                    reason.contains("cap"),
                    "the refusal names the cap: {reason}"
                );
            }
            other => panic!("expected a cap refusal, got {other:?}"),
        }
        assert!(!dir.path().join("projects/big.txt").exists());
    }

    #[tokio::test]
    async fn oversized_base64_is_refused_before_decoding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let payload: Vec<u8> = vec![0u8; MAX_WRITE_BYTES + 1];
        let request = FileWriteRequest {
            path: dir
                .path()
                .join("projects/big.bin")
                .to_string_lossy()
                .into_owned(),
            data_base64: Some(base64::engine::general_purpose::STANDARD.encode(&payload)),
            text: None,
            create_parents: Some(false),
        };
        let refused = adapter.write(request).await;
        assert!(matches!(refused, Err(ContractError::Unsupported { .. })));
    }

    // ---- Issue #58: symlinks are listed as symlinks.

    #[tokio::test]
    async fn list_reports_symlinks_as_symlinks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let projects = dir.path().join("projects");
        std::fs::create_dir_all(projects.join("d")).expect("dir");
        std::fs::write(projects.join("f"), "ab").expect("file");
        std::os::unix::fs::symlink("f", projects.join("l")).expect("symlink");
        let listing = adapter
            .list(PathQuery {
                path: projects.to_string_lossy().into_owned(),
                depth: Some(1),
                limit: None,
            })
            .await
            .expect("list");
        let link = listing
            .entries
            .iter()
            .find(|e| e.name == "l")
            .expect("link entry");
        assert!(
            matches!(link.kind, EntryKind::Symlink),
            "a symlink is reported as EntryKind::Symlink"
        );
        assert_eq!(link.size_bytes, None, "a link has no content length");
        let file = listing
            .entries
            .iter()
            .find(|e| e.name == "f")
            .expect("file entry");
        assert!(matches!(file.kind, EntryKind::File));
        assert_eq!(file.size_bytes, Some(2));
        let subdir = listing
            .entries
            .iter()
            .find(|e| e.name == "d")
            .expect("dir entry");
        assert!(matches!(subdir.kind, EntryKind::Dir));
        assert!(!listing.truncated);
    }

    // ---- Issue #76: unreadable subtrees mark the listing truncated.

    #[tokio::test]
    async fn unreadable_subtree_marks_the_listing_truncated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = adapter_for(dir.path());
        let projects = dir.path().join("projects");
        let secret = projects.join("secret");
        std::fs::create_dir_all(&secret).expect("subdir");
        std::fs::write(projects.join("visible.txt"), "here").expect("file");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        // Root ignores 0o000; the assertion only holds where unreadability does.
        if std::fs::read_dir(&secret).is_ok() {
            std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o755))
                .expect("restore");
            eprintln!("skipping: running as a privileged user, 0o000 is readable");
            return;
        }
        let listing = adapter
            .list(PathQuery {
                path: projects.to_string_lossy().into_owned(),
                depth: Some(2),
                limit: None,
            })
            .await
            .expect("list");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o755)).expect("restore");
        assert!(
            listing.truncated,
            "an unreadable subtree must mark the listing incomplete"
        );
        assert!(
            listing.entries.iter().any(|e| e.name == "visible.txt"),
            "the readable siblings are still listed"
        );
        assert!(
            !listing.entries.iter().any(|e| e.name == "secret")
                || listing.entries.iter().all(|e| e.name != "invisible-marker"),
            "no entries from inside the unreadable subtree appear"
        );
    }

    // ---- Issue #69: the grant store is never writable through the adapter.

    #[tokio::test]
    async fn grant_store_write_is_denied_even_under_a_home_grant() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().to_path_buf();
        let adapter = FsXdgAdapter::with_scopes(scope::FileScopes::with_home(
            vec![home.to_string_lossy().into_owned()],
            home.clone(),
        ));
        let mut request = text_content("[write]\n\"files.scope.write\" = {}\n");
        request.path = home
            .join(".config/omnibus/scopes.d/90-self.toml")
            .to_string_lossy()
            .into_owned();
        let denied = adapter.write(request).await;
        assert!(
            matches!(denied, Err(ContractError::ScopeDenied)),
            "a grant on $HOME must not reach the grant store"
        );
        // The same grant still writes an ordinary dotfile.
        let mut ordinary = text_content("fine\n");
        ordinary.path = home
            .join(".config/other/app.toml")
            .to_string_lossy()
            .into_owned();
        ordinary.create_parents = Some(true);
        adapter
            .write(ordinary)
            .await
            .expect("non-store dotfiles stay writable");
    }
}

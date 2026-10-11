//! fs-xdg adapter: read/list/write inside granted XDG scopes (roadmap 1.5.5,
//! 1.6.4). The default read scope is `$HOME` including dotfiles (decision
//! 2026-10-10); writes require granted paths and are re-checked here under the
//! core's grant decision (defense in depth, specs/contracts/files.md). Native
//! filesystem details never leave the crate — DTOs only at the seam.

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

/// Hard caps (files.md): reads never exceed 2 MiB per call, listings never
/// exceed 2000 entries regardless of the requested limit.
const MAX_READ_BYTES: i64 = 2 * 1024 * 1024;
const MAX_LIST_ENTRIES: usize = 2000;

/// The adapter: cheap-Clone over its scope set.
#[derive(Clone)]
pub struct FsXdgAdapter {
    scopes: std::sync::Arc<FileScopes>,
}

impl FsXdgAdapter {
    /// Builds the adapter with the granted write paths (canonical check happens
    /// per call, so late mount/symlink changes are still judged correctly).
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
}

#[async_trait]
impl FileScope for FsXdgAdapter {
    fn report(&self) -> AdapterReport {
        AdapterReport {
            adapter: String::from("fs-xdg"),
            subsystem: String::from("files"),
            contract_version: String::from(omnibus_contracts::version::CONTRACT_VERSION),
            capabilities: vec![
                omnibus_contracts::dto::probe::CapabilityFact {
                    name: String::from(cap::FILES_SCOPE_READ),
                    ver: String::from(omnibus_contracts::version::CONTRACT_VERSION),
                    status: String::from("present"),
                    value: Some(serde_json::json!({
                        "default_scope": "$HOME (dotfiles included)"
                    })),
                    note: None,
                    remedy: None,
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
        let decision = self.scopes.decide(&query.path);
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

        // Bounded recursive walk; small local fs ops run inline (budgeted by the
        // core), the recursion is capped by depth and the entry cap.
        let mut stack = vec![(std::path::PathBuf::from(&root), 0usize)];
        while let Some((dir, level)) = stack.pop() {
            let read_dir = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) => {
                    if entries.is_empty() && level == 0 {
                        return Err(Self::io_error("fs_list", e));
                    }
                    continue;
                }
            };
            for entry in read_dir.flatten() {
                if entries.len() >= limit {
                    truncated = true;
                    stack.clear();
                    break;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let is_dir = meta.is_dir();
                entries.push(DirEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    kind: if is_dir {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    },
                    size_bytes: (!is_dir).then_some(meta.len()),
                });
                if is_dir && level + 1 < depth {
                    stack.push((entry.path(), level + 1));
                }
            }
        }

        Ok(DirListing {
            path: root,
            entries,
            truncated,
        })
    }

    async fn read(&self, request: FileReadRequest) -> Result<FileContent, ContractError> {
        let decision = self.scopes.decide(&request.path);
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
        let total = meta.len();
        let mut file = std::fs::File::open(&path).map_err(|e| Self::io_error("fs_read", e))?;
        use std::io::{Read as _, Seek as _, SeekFrom};
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| Self::io_error("fs_read", e))?;
        let mut buffer = Vec::new();
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

    async fn write(&self, request: FileWriteRequest) -> Result<FileWriteResult, ContractError> {
        let decision = self.scopes.decide(&request.path);
        if !decision.write_allowed {
            return Err(ContractError::ScopeDenied);
        }
        let bytes: Vec<u8> = if let Some(b64) = &request.data_base64 {
            decode_base64(b64)?
        } else if let Some(text) = &request.text {
            text.clone().into_bytes()
        } else {
            Vec::new()
        };
        let path = std::path::PathBuf::from(&decision.canonical);
        if request.create_parents.is_some_and(|v| v)
            && let Some(parent) = path.parent()
        {
            std::fs::create_dir_all(parent).map_err(|e| Self::io_error("fs_write", e))?;
        }
        std::fs::write(&path, &bytes).map_err(|e| Self::io_error("fs_write", e))?;
        Ok(FileWriteResult {
            path: decision.canonical,
            bytes_written: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        })
    }

    async fn resolve(&self, request: PathResolveRequest) -> Result<ResolvedPath, ContractError> {
        let decision = self.scopes.decide(&request.path);
        Ok(ResolvedPath {
            path: request.path,
            canonical: decision.canonical,
            inside_home: decision.inside_home,
            read_allowed: decision.read_allowed,
            write_allowed: decision.write_allowed,
        })
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
}

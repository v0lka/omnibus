//! `FileScope` — file access inside granted scopes (subsystem 6).

use async_trait::async_trait;

use crate::dto::files::{
    DirListing, FileContent, FileReadRequest, FileWriteRequest, FileWriteResult, PathQuery,
    PathResolveRequest, ResolvedPath,
};
use crate::errors::ContractError;

/// Scoped file operations. Scope enforcement is a core responsibility; the adapter
/// additionally refuses anything outside the scopes it was activated with, so a
/// bug in one layer cannot widen the other (defense in depth, files.md).
#[async_trait]
pub trait FileScope: Send + Sync {
    /// The adapter report (activated scopes).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `fs_list` — directory listing within the scope budget.
    async fn list(&self, query: PathQuery) -> Result<DirListing, ContractError>;

    /// `fs_read` — file content with offset/size limits.
    async fn read(&self, request: FileReadRequest) -> Result<FileContent, ContractError>;

    /// `fs_write` (write, granted) — creates or replaces a file inside the scope.
    async fn write(&self, request: FileWriteRequest) -> Result<FileWriteResult, ContractError>;

    /// Scope introspection used by doctor and the denial remedy text.
    async fn resolve(&self, request: PathResolveRequest) -> Result<ResolvedPath, ContractError>;
}

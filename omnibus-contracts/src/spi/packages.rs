//! `PackageManager` — package reading, writes, history and undo (subsystem 4).

use async_trait::async_trait;

use crate::dto::packages::{
    PackageBrief, PackageHistoryEntry, PackageHistoryQuery, PackageInfo, PackageListRequest,
    PackageNameRequest, PackageSearch, TransactionResult, UndoResult,
};
use crate::errors::ContractError;

/// Package operations. `undo` semantics are backend-defined and reported by the
/// `pkg.undo` capability value: `none` (tool hidden), `cache-downgrade` (alpm) or
/// `transactional` (dnf). A missing cached version is an honest `NativeFailure`
/// with an explanatory payload, never a half-applied transaction (roadmap 1.6.3).
#[async_trait]
pub trait PackageManager: Send + Sync {
    /// The adapter report (undo value, provider).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `pkg_list` — installed packages, within the time budget.
    async fn list(&self, request: PackageListRequest) -> Result<Vec<PackageBrief>, ContractError>;

    /// `pkg_info` — metadata of one package (installed and/or available).
    async fn info(&self, name: String) -> Result<PackageInfo, ContractError>;

    /// `pkg_search` — search by name/description substring.
    async fn search(&self, query: PackageSearch) -> Result<Vec<PackageBrief>, ContractError>;

    /// `pkg_install` (write, granted).
    async fn install(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError>;

    /// `pkg_remove` (write, granted).
    async fn remove(&self, request: PackageNameRequest)
    -> Result<TransactionResult, ContractError>;

    /// `pkg_history` — the transaction log, basis of the cache-downgrade undo.
    async fn history(
        &self,
        query: PackageHistoryQuery,
    ) -> Result<Vec<PackageHistoryEntry>, ContractError>;

    /// `pkg_undo` (write, granted; hidden when the capability value is `none`).
    async fn undo(&self, request: PackageNameRequest) -> Result<UndoResult, ContractError>;
}

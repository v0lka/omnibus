//! `ServiceManager` — systemd unit reading and lifecycle writes (subsystem 3).

use async_trait::async_trait;

use crate::dto::services::{
    ServiceActionRequest, ServiceActionResult, ServiceFilter, ServiceStatus, ServiceUnit,
};
use crate::errors::ContractError;

/// Unit listing, status, and lifecycle actions. polkit authorizations act as they
/// are; an authorization failure surfaces as an honest contract error (roadmap
/// 1.6.2) — the adapter never intercepts or supersedes system policy.
#[async_trait]
pub trait ServiceManager: Send + Sync {
    /// The adapter report (read/write capabilities, provider).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `svc_list` — units matching the filter, within the time budget.
    async fn list(&self, filter: ServiceFilter) -> Result<Vec<ServiceUnit>, ContractError>;

    /// `svc_status` — one unit's status; unknown units are contract errors.
    async fn status(&self, unit: String) -> Result<ServiceStatus, ContractError>;

    /// `svc_start` / `svc_stop` (write, granted) — lifecycle transitions.
    async fn apply(
        &self,
        request: ServiceActionRequest,
    ) -> Result<ServiceActionResult, ContractError>;
}

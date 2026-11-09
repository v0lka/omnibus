//! `AppLauncher` — application inventory, launch and open (subsystem 11, v1.5
//! Linux), declared just-in-time together with its first implementation,
//! `omnibus-adapter-launch` (roadmap task 2.7.1, ADR-011). `launch` and `open`
//! are write actions: always behind the out-of-band `app.launch` write grant;
//! there is no runtime consent flow (ADR-006, ADR-010).

use async_trait::async_trait;

use crate::dto::app_launcher::{AppEntry, AppSpec, LaunchOutcome, OpenRequest};
use crate::errors::ContractError;

/// Application-launch operations: the inventory behind `app.read`, the write
/// actions (`launch`, `open`) behind `app.launch` (specs/domains/app-launch.md).
#[async_trait]
pub trait AppLauncher: Send + Sync {
    /// The adapter report (provider, subsystem).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `app.list` — installed-application inventory (`Exec`, `Name`, `Icon`,
    /// `NoDisplay` from the `.desktop` files); read-only by contract.
    async fn list(&self) -> Result<Vec<AppEntry>, ContractError>;

    /// `app.launch` (write, granted) — starts the application as a transient
    /// user unit; the process lands in the user's cgroup and journald, so
    /// `ps.list` and `journal_query` observe it. Arguments cross the SPI as a
    /// typed list and reach the target process verbatim.
    async fn launch(&self, spec: AppSpec) -> Result<LaunchOutcome, ContractError>;

    /// `app.open` (write, granted) — opens a URI or a file with the associated
    /// application. Absent an opener on the machine the tool is hidden (R3);
    /// a direct call is an honest contract error.
    async fn open(&self, request: OpenRequest) -> Result<(), ContractError>;
}

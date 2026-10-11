//! `PlatformProbe` — platform detection and adapter selection (subsystem 8).

use async_trait::async_trait;

use crate::dto::probe::PlatformReport;
use crate::errors::ContractError;

/// `detect()` produces the machine truth: distro/release, component versions,
/// daemon liveness, floor enforcement, Manjaro-specific facts where applicable
/// (tasks 1.4.1, 2.1.1, 2.4.1), and the activated adapter set (one per subsystem).
#[async_trait]
pub trait PlatformProbe: Send + Sync {
    /// The probe's own report row (never hidden; the probe is always present).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// Runs the detection; the report feeds the capabilities document and the
    /// adapter registry of the fresh session (topology-and-lifecycle.md).
    async fn detect(&self) -> Result<PlatformReport, ContractError>;
}

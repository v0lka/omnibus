//! `ProcessTelemetry` — process list, per-PID snapshots, system statistics and
//! power status (subsystem 12, v1.5 Linux), declared just-in-time together with
//! its first implementation, `omnibus-adapter-proc` (roadmap task 2.8.1,
//! ADR-011). Reads are audited; `signal` is a write action: always behind the
//! out-of-band `ps.signal` write grant, with no runtime consent flow
//! (ADR-006, ADR-010).

use async_trait::async_trait;

use crate::dto::process_telemetry::{
    PowerStatus, ProcessEntry, ProcessListQuery, ProcessSnapshot, SignalRequest, SysStats,
};
use crate::errors::ContractError;

/// Process and telemetry operations (specs/domains/process-telemetry.md): the
/// reads behind `ps.read` / `sys.read` / `power.read`, the write behind
/// `ps.signal`. PID 1 and systemd-unit processes stay outside every operation —
/// services are addressed only through `ServiceManager` (`svc_*`), and reads
/// reflect exactly what the daemon's uid can see of `/proc`.
#[async_trait]
pub trait ProcessTelemetry: Send + Sync {
    /// The adapter report (provider, subsystem, power-provider presence).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `ps.list` — the process inventory from `/proc` as the daemon's uid sees
    /// it (no escalations); limits and filters travel in the query DTO (the
    /// `journal_query` discipline). Read-only by contract.
    async fn list(&self, query: ProcessListQuery) -> Result<Vec<ProcessEntry>, ContractError>;

    /// `ps.info` — one consistent per-PID snapshot. A nonexistent pid yields a
    /// contract error, never a panic; the agent re-lists (`ps.list`) for fresh
    /// facts instead of guessing pids.
    async fn info(&self, pid: u32) -> Result<ProcessSnapshot, ContractError>;

    /// `sys.stats` — CPU load, load average, memory and swap from `/proc` and
    /// `/sys`. Read-only by contract.
    async fn sys_stats(&self) -> Result<SysStats, ContractError>;

    /// `power.status` — battery and supply facts through UPower, "when present"
    /// (probe auto-detect). Without the provider the tool is hidden (R3): a
    /// direct call answers `CapabilityRequired`, never a hollow report.
    async fn power_status(&self) -> Result<PowerStatus, ContractError>;

    /// `ps.signal` (write, granted) — one signal from the closed contract enum
    /// (`crate::dto::process_telemetry::Signal`, never a raw number) to a pid
    /// taken from `ps.list` as fact. A signal aimed at PID 1 or a
    /// systemd-unit process answers `Unsupported` with a hint to the service
    /// tools (the domain invariant); without the write grant the core answers
    /// `ScopeDenied` and audits the attempt (R5).
    async fn signal(&self, request: SignalRequest) -> Result<(), ContractError>;
}

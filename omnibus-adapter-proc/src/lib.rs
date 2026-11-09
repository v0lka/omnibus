#![cfg_attr(test, allow(clippy::expect_used))] // unit tests may expect; the seam must not
//! The process-telemetry adapter: the Linux [`ProcessTelemetry`] implementation
//! over `/proc`, sysfs and UPower (subsystem 12, roadmap tasks 2.8.2 and 2.8.3,
//! specs/domains/process-telemetry.md).
//!
//! Surfaces:
//!
//! - `ps.list` / `ps.info` — the process inventory and per-PID snapshots read
//!   from `/proc` ([`read`]): pid, name, state, lifetime-average CPU, RSS and
//!   cgroup; limits and filters travel in the [`ProcessListQuery`] DTO (the
//!   `journal_query` discipline). PID 1 and the system-manager unit hierarchy
//!   (`system.slice/…`, `init.scope`) stay outside every operation — services
//!   are addressed only through `svc_*`; the user hierarchy (including the
//!   transient `app-*.service` units `app.launch` mints) stays inside, which
//!   is the launch → observe closed loop.
//! - `sys.stats` — CPU busy percent (a short `/proc/stat` delta sample), load
//!   averages, memory and swap ([`read`]); the subsystem's sysfs read is the
//!   battery-presence cross-check on the power side.
//! - `power.status` — battery and supply facts through UPower D-Bus "when
//!   present" ([`power`]): the probe auto-detects UPower, sysfs decides the
//!   battery half of the presence rule; without either the tool hides (R3/R7)
//!   — no `power.read` fact, the reason lives in `capabilities()`, and a
//!   direct trait call answers `CapabilityRequired`, never a hollow report.
//! - `ps.signal` — the granted write ([`write`]): one closed-enum signal to
//!   one pid taken from `ps.list` as fact. The core owns the write gate;
//!   the adapter refuses PID 1 / system-unit targets with the `svc_*` hint.
//!
//! Error mapping (specs/architecture/error-taxonomy.md): native failures
//! become [`ContractError::NativeFailure`] with the typed numeric code and
//! the symbolic name riding the payload prefix (`no-such-process`,
//! `proc-read`, `signal`, `dbus`). Every method returns a contract error —
//! `anyhow` and panics never cross the seam.

mod power;
mod read;
mod write;

use async_trait::async_trait;
use omnibus_contracts::capabilities::{POWER_READ, PS_READ, PS_SIGNAL, SYS_READ};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::dto::process_telemetry::{
    PowerStatus, ProcessEntry, ProcessListQuery, ProcessSnapshot, SignalRequest, SysStats,
};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::process_telemetry::ProcessTelemetry;
use omnibus_contracts::version::CONTRACT_VERSION;

use crate::power::PowerProvider;
use crate::read::{CPU_SAMPLE_WINDOW, ProcReader};

/// The CPU lines of `/proc/stat`: `(delta_total, delta_busy)` between two
/// reads is the live sample `sys.stats` reports.
fn cpu_ticks(root: &std::path::Path) -> Result<(u64, u64), ContractError> {
    let text =
        std::fs::read_to_string(root.join("stat")).map_err(|e| ContractError::NativeFailure {
            code: read::NF_PROC_READ,
            payload: format!("proc-read: stat: {e}"),
        })?;
    let line = text
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| ContractError::NativeFailure {
            code: read::NF_PROC_READ,
            payload: String::from("proc-read: stat carries no aggregate cpu line"),
        })?;
    let fields = line
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse::<u64>().ok())
        .collect::<Vec<_>>();
    // Aggregate line: user nice system idle iowait irq softirq steal … —
    // "busy" is everything except the idle and iowait columns.
    let total: u64 = fields.iter().sum();
    let idle: u64 = fields.get(3).copied().unwrap_or(0) + fields.get(4).copied().unwrap_or(0);
    Ok((total, total.saturating_sub(idle)))
}

/// The process-telemetry adapter: `/proc` reader + power provider. Cloning
/// is cheap (paths and two flags).
#[derive(Debug, Clone)]
pub struct ProcAdapter {
    reader: ProcReader,
    power: PowerProvider,
}

impl ProcAdapter {
    /// Builds from the probe's facts: UPower liveness decides the power arm;
    /// `/proc` and `/sys` are the live roots. The composition root's entry
    /// point — the read part activates on every Linux stand (no version
    /// floor), the power arm only when the machine has UPower *and* a
    /// battery (R3/R7).
    pub fn from_probe(upower_present: bool) -> Self {
        Self {
            reader: ProcReader::live(),
            power: PowerProvider {
                upower: upower_present,
                sys_root: std::path::PathBuf::from("/sys"),
            },
        }
    }

    /// Builds over explicit roots — the fixture suite's synthetic stand.
    /// The tick rate and page size are the live kernel's facts (`USER_HZ` is
    /// a fixed Linux ABI constant; the page size comes from rustix).
    pub fn at(proc_root: std::path::PathBuf, sys_root: std::path::PathBuf, upower: bool) -> Self {
        Self {
            reader: ProcReader::at(
                proc_root,
                read::USER_HZ,
                u64::try_from(rustix::param::page_size())
                    .unwrap_or(1)
                    .max(1),
            ),
            power: PowerProvider { upower, sys_root },
        }
    }

    /// The `power.read` fact: present only when UPower is live AND the sysfs
    /// view carries a battery. The ps.read note names the hidden
    /// `power.status` when absent — the R3 reason travels into
    /// `capabilities()` without a second registry constant.
    fn power_fact(&self) -> Option<CapabilityFact> {
        if !(self.power.upower && power::battery_present(&self.power.sys_root)) {
            return None;
        }
        Some(CapabilityFact {
            name: String::from(POWER_READ),
            ver: String::from(CONTRACT_VERSION),
            status: String::from("present"),
            value: None,
            note: Some(String::from(
                "battery and supply facts through UPower (display device)",
            )),
            remedy: None,
        })
    }

    /// The note on the always-present `ps.read` fact: the served surface and
    /// — on a machine without UPower/battery — the honest reason the
    /// `power.status` tool is hidden.
    fn read_note(&self) -> String {
        let hidden_power = self.power_fact().is_none();
        if hidden_power {
            String::from(
                "process and system facts from /proc; power.status is hidden: this machine \
                 has no UPower daemon or no battery (R3/R7: the machine is never promised \
                 what it lacks)",
            )
        } else {
            String::from("process and system facts from /proc; power facts via UPower")
        }
    }
}

#[async_trait]
impl ProcessTelemetry for ProcAdapter {
    fn report(&self) -> AdapterReport {
        let mut capabilities = vec![
            CapabilityFact {
                name: String::from(PS_READ),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("present"),
                value: None,
                note: Some(self.read_note()),
                remedy: None,
            },
            CapabilityFact {
                name: String::from(SYS_READ),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("present"),
                value: None,
                note: Some(String::from(
                    "CPU/load/memory/swap from /proc (stat, loadavg, meminfo, uptime)",
                )),
                remedy: None,
            },
            CapabilityFact {
                name: String::from(PS_SIGNAL),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("present"),
                value: None,
                note: Some(String::from(
                    "send one closed-enum signal by pid from ps.list (write grant required; \
                     PID 1 and systemd-unit processes are excluded — services go through svc_*)",
                )),
                remedy: None,
            },
        ];
        if let Some(power) = self.power_fact() {
            capabilities.push(power);
        }
        AdapterReport {
            adapter: String::from("proc"),
            subsystem: String::from("process-telemetry"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities,
            notes: vec![String::from(
                "reads reflect exactly what the daemon's uid can see of /proc — no escalations",
            )],
        }
    }

    async fn list(&self, query: ProcessListQuery) -> Result<Vec<ProcessEntry>, ContractError> {
        // The walk is plain small-file reads (µs each): inline is far inside
        // the read budget even for 300+ processes; the fixture suite gates
        // the budget honestly.
        Ok(self.reader.list(&query))
    }

    async fn info(&self, pid: u32) -> Result<ProcessSnapshot, ContractError> {
        self.reader.info(pid)
    }

    async fn sys_stats(&self) -> Result<SysStats, ContractError> {
        // One live sample: two /proc/stat reads a window apart. A static
        // (fixture) stat answers 0 busy — honest, since nothing consumed CPU
        // in the window.
        let first = cpu_ticks(&self.reader.root)?;
        tokio::time::sleep(CPU_SAMPLE_WINDOW).await;
        let second = cpu_ticks(&self.reader.root)?;
        let delta = (
            second.0.saturating_sub(first.0),
            second.1.saturating_sub(first.1),
        );
        self.reader.sys_stats(delta)
    }

    async fn power_status(&self) -> Result<PowerStatus, ContractError> {
        self.power.status().await
    }

    async fn signal(&self, request: SignalRequest) -> Result<(), ContractError> {
        write::send(&self.reader.root, request)
    }
}

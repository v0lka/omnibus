//! The `/proc` read side (roadmap task 2.8.2, specs/domains/process-telemetry.md
//! Behavior): process enumeration, per-PID snapshots and system statistics.
//!
//! Reading rules the domain pins:
//!
//! - The subsystem reports exactly what the daemon's uid can see of `/proc`
//!   — no escalations, ever (Invariants). Entries whose files are unreadable
//!   or that vanish mid-walk are skipped honestly; a vanished process is a
//!   contract error (`no-such-process`), never a panic.
//! - PID 1 and processes of the system manager's unit hierarchy
//!   (`system.slice/…`, `init.scope`) stay outside every operation — services
//!   are addressed only through the services subsystem (`svc_*`). Processes
//!   under `user.slice/…` stay listed: the user's own session, including the
//!   transient `app-*.service` units `app.launch` mints, is exactly what the
//!   launch → observe closed loop must be able to watch (app-launch.md: the
//!   launched process lands in the user's cgroup so `ps.list` observes it).
//! - Per-process CPU is the process's lifetime average
//!   (`(utime+stime)/CLK_TCK` over the process's own age) — one consistent
//!   number per snapshot, no two-pass sampling in `ps.list` (the 300+-process
//!   inventory must stay far inside the read budget).
//! - `/proc` and sysfs carry no version floor: the read part activates on
//!   every target Linux platform.

use std::path::{Path, PathBuf};

use omnibus_contracts::dto::process_telemetry::{ProcessEntry, ProcessListQuery, ProcessSnapshot};
use omnibus_contracts::dto::{Bytes, Milliseconds, Percent, Seconds};
use omnibus_contracts::errors::ContractError;

/// `NativeFailure` code: the addressed pid does not exist (or vanished).
pub const NF_NO_PROCESS: i32 = 1;
/// `NativeFailure` code: a `/proc` read failed natively (permission, EIO);
/// the original message rides the payload.
pub const NF_PROC_READ: i32 = 2;
/// `NativeFailure` code: `kill(2)` refused; the OS message rides the payload.
pub const NF_SIGNAL: i32 = 3;

/// The system statistics sample window for `sys.stats`' CPU busy percent: two
/// `/proc/stat` reads `CPU_SAMPLE_WINDOW` apart give the live delta. Far below
/// the core's read budget; a static (fixture) `/proc/stat` honestly answers 0.
pub const CPU_SAMPLE_WINDOW: std::time::Duration = std::time::Duration::from_millis(120);

/// One `/proc/<pid>` sample: everything the DTOs are built from, read once.
#[derive(Debug, Clone)]
pub(crate) struct ProcSample {
    pub pid: u32,
    /// The process name (`comm`): parens-quoted in `/proc/<pid>/stat`, which
    /// may itself contain spaces and `)` characters — hence the last-`)` rule.
    pub comm: String,
    /// The single-letter kernel state (`R S D Z T …`).
    pub state: String,
    /// User + system CPU time, in clock ticks (`utime + stime`).
    pub cpu_ticks: u64,
    /// Process start, in clock ticks since boot (`starttime`).
    pub starttime_ticks: u64,
    /// Resident set size in bytes (`statm` resident pages × page size).
    pub rss_bytes: u64,
    /// The cgroup path from `/proc/<pid>/cgroup` (the unified `0::` line when
    /// present, else the last line's path) — `None` when the kernel hides it.
    pub cgroup: Option<String>,
    /// The argv joined with spaces (`cmdline`), `None` for kernel threads.
    pub cmdline: Option<String>,
}

/// Parses one `/proc/<pid>/stat` line: `pid (comm) state …` where `comm` may
/// contain spaces and parentheses — the split is on the LAST `)` (the kernel
/// guarantees the comm field ends there).
fn parse_stat(text: &str) -> Option<ProcSample> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let pid: u32 = text[..open].trim().parse().ok()?;
    let comm = String::from(&text[open + 1..close]);
    // After the closing paren: `state pppid pgrp session …` — overall fields
    // 3.. of /proc/<pid>/stat. With pid=1 and comm=2 counted, the token at
    // post-paren index i is overall field i+3: state(3), utime(14) → 11,
    // stime(15) → 12, starttime(22) → 19.
    let after: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let field = |idx: usize| after.get(idx).and_then(|f| f.parse::<u64>().ok());
    Some(ProcSample {
        pid,
        comm,
        state: String::from(after.first().copied().unwrap_or("?")),
        cpu_ticks: field(11)?.saturating_add(field(12)?),
        starttime_ticks: field(19)?,
        rss_bytes: 0,
        cgroup: None,
        cmdline: None,
    })
}

/// Reads the RSS from `/proc/<pid>/statm` (field 2 = resident pages) scaled
/// by the page size; 0 when the file is unreadable (kernel threads report an
/// honest 0 rather than being dropped).
fn read_rss(pid_dir: &Path, page_size: u64) -> u64 {
    let Ok(text) = std::fs::read_to_string(pid_dir.join("statm")) else {
        return 0;
    };
    text.split_whitespace()
        .nth(1)
        .and_then(|pages| pages.parse::<u64>().ok())
        .map(|pages| pages.saturating_mul(page_size))
        .unwrap_or(0)
}

/// Extracts the cgroup path from a `/proc/<pid>/cgroup` body: prefers the
/// cgroup-v2 unified line (`0::/path`), falls back to the last line's path
/// (cgroup v1 controller lines). The path is carried verbatim — the
/// exclusion rule, not the DTO, decides what it means. Shared by the read
/// side and the `ps.signal` exclusion check.
pub(crate) fn cgroup_path_of(text: &str) -> Option<String> {
    let mut v2: Option<String> = None;
    let mut last: Option<String> = None;
    for line in text.lines() {
        let Some((_, path)) = line.split_once(':') else {
            continue;
        };
        let Some((_, path)) = path.split_once(':') else {
            continue;
        };
        if line.starts_with("0::") {
            v2 = Some(String::from(path));
        }
        last = Some(String::from(path));
    }
    v2.or(last)
}

/// Reads the cgroup path of one pid directory.
fn read_cgroup(pid_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pid_dir.join("cgroup")).ok()?;
    cgroup_path_of(&text)
}

/// Reads `/proc/<pid>/cmdline`: NUL-separated argv joined with single spaces.
/// An empty file (kernel threads) is `None` — the DTO honestly carries no
/// command line rather than an empty one.
fn read_cmdline(pid_dir: &Path) -> Option<String> {
    let bytes = std::fs::read(pid_dir.join("cmdline")).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let joined = bytes
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// The domain exclusion rule (specs/domains/process-telemetry.md Invariants):
/// PID 1 itself, and any process whose cgroup path places it in the system
/// manager's hierarchy — `system.slice/…` (system services) or `init.scope`
/// (the manager's own scope). Those are exactly the units `svc_list`
/// enumerates and `svc_start`/`svc_stop` address; a signal aimed at them
/// answers `Unsupported` with the svc hint. The user hierarchy
/// (`user.slice/…`, including transient `app-*.service` units) stays
/// inside: that is the launch → observe closed loop.
pub(crate) fn excluded(pid: u32, cgroup: Option<&str>) -> bool {
    if pid == 1 {
        return true;
    }
    let Some(path) = cgroup else {
        return false;
    };
    let path = path.trim_start_matches('/');
    let top = path.split('/').next().unwrap_or("");
    top == "system.slice" || top == "init.scope"
}

/// Reads one process sample; `None` when the pid dir or its stat file is
/// unreadable (vanished mid-walk, kernel-hidden) — callers skip honestly.
fn sample_of(pid_dir: &Path, page_size: u64) -> Option<ProcSample> {
    let mut sample = parse_stat(&std::fs::read_to_string(pid_dir.join("stat")).ok()?)?;
    sample.rss_bytes = read_rss(pid_dir, page_size);
    sample.cgroup = read_cgroup(pid_dir);
    sample.cmdline = read_cmdline(pid_dir);
    Some(sample)
}

/// The `/proc` reader over an injectable root (`/proc` live; a synthetic
/// stand for the fixture suite). Holds the clock-tick and page-size facts the
/// arithmetic needs.
#[derive(Debug, Clone)]
pub(crate) struct ProcReader {
    pub root: PathBuf,
    pub clk_tck: u64,
    pub page_size: u64,
}

/// The userspace clock-tick rate the kernel renders `/proc` timing in:
/// Linux fixes `USER_HZ` at 100 on every port — `utime`, `stime` and
/// `starttime` are ticks of this rate (the fact `sysconf(_SC_CLK_TCK)`
/// would report; the value is an ABI constant, not a kernel-config variable).
pub const USER_HZ: u64 = 100;

impl ProcReader {
    /// Builds with the live `/proc`, the fixed Linux `USER_HZ` tick rate and
    /// the kernel's page size (`rustix::param::page_size` — safe, no libc
    /// FFI: the workspace forbids `unsafe` and none is needed).
    pub fn live() -> Self {
        Self::at(
            PathBuf::from("/proc"),
            USER_HZ,
            u64::try_from(rustix::param::page_size())
                .unwrap_or(1)
                .max(1),
        )
    }

    /// Builds over an explicit root — the fixture suite's synthetic stand.
    pub fn at(root: PathBuf, clk_tck: u64, page_size: u64) -> Self {
        Self {
            root,
            clk_tck: clk_tck.max(1),
            page_size: page_size.max(1),
        }
    }

    /// Enumerates the numeric pid directories, newest-last (sorted by pid for
    /// a deterministic delivery).
    pub fn pid_dirs(&self) -> Vec<(u32, PathBuf)> {
        let Ok(read) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut pids = Vec::new();
        for item in read.flatten() {
            let name = item.file_name();
            let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
                continue; // /proc carries non-numeric files too
            };
            pids.push((pid, item.path()));
        }
        pids.sort_by_key(|(pid, _)| *pid);
        pids
    }

    /// The uptime seconds (`/proc/uptime` field 1); 0.0 when unreadable —
    /// every arithmetic consumer clamps honestly rather than dividing by a
    /// negative age.
    pub fn uptime_seconds(&self) -> f64 {
        std::fs::read_to_string(self.root.join("uptime"))
            .ok()
            .and_then(|text| {
                text.split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
            })
            .unwrap_or(0.0)
    }

    /// The boot epoch (`btime`, `/proc/stat`), in whole seconds.
    fn boot_epoch(&self) -> u64 {
        std::fs::read_to_string(self.root.join("stat"))
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("btime ")
                        .and_then(|value| value.trim().parse::<u64>().ok())
                })
            })
            .unwrap_or(0)
    }

    /// `ps.list`: the inventory under the query's limits and filters. The
    /// walk never fails: unreadable or vanished entries are skipped, and an
    /// unreadable `/proc` honestly answers an empty list (nothing is visible
    /// to the daemon's uid — that is the fact).
    pub fn list(&self, query: &ProcessListQuery) -> Vec<ProcessEntry> {
        let uptime = self.uptime_seconds();
        let mut entries = Vec::new();
        for (_pid, dir) in self.pid_dirs() {
            let Some(sample) = sample_of(&dir, self.page_size) else {
                continue;
            };
            if excluded(sample.pid, sample.cgroup.as_deref()) {
                continue;
            }
            if let Some(filter) = &query.name_contains
                && !sample.comm.contains(filter.as_str())
            {
                continue;
            }
            entries.push(ProcessEntry {
                pid: sample.pid,
                name: sample.comm.clone(),
                state: sample.state.clone(),
                cpu_percent: Percent(lifetime_cpu_percent(
                    sample.cpu_ticks,
                    sample.starttime_ticks,
                    uptime,
                    self.clk_tck,
                )),
                rss_bytes: Bytes(sample.rss_bytes),
                cgroup: sample.cgroup.clone(),
            });
        }
        if let Some(limit) = query.limit {
            entries.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        entries
    }

    /// `ps.info`: one consistent per-PID snapshot, or the typed refusal:
    /// PID 1 / system-unit processes answer `Unsupported` with the svc hint,
    /// a nonexistent (or vanished) pid answers `no-such-process`.
    pub fn info(&self, pid: u32) -> Result<ProcessSnapshot, ContractError> {
        let dir = self.root.join(pid.to_string());
        let Some(sample) = sample_of(&dir, self.page_size) else {
            return Err(ContractError::NativeFailure {
                code: NF_NO_PROCESS,
                payload: format!(
                    "no-such-process: pid {pid} is not visible to this daemon's uid — \
                     re-list with ps.list for fresh facts instead of guessing pids"
                ),
            });
        };
        if excluded(sample.pid, sample.cgroup.as_deref()) {
            return Err(excluded_refusal(pid));
        }
        let uptime = self.uptime_seconds();
        let start_epoch_ms = self.boot_epoch().saturating_mul(1_000).saturating_add(
            sample
                .starttime_ticks
                .saturating_mul(1_000)
                .checked_div(self.clk_tck)
                .unwrap_or(0),
        );
        Ok(ProcessSnapshot {
            pid: sample.pid,
            name: sample.comm,
            state: sample.state,
            cpu_percent: Percent(lifetime_cpu_percent(
                sample.cpu_ticks,
                sample.starttime_ticks,
                uptime,
                self.clk_tck,
            )),
            rss_bytes: Bytes(sample.rss_bytes),
            cgroup: sample.cgroup,
            command_line: sample.cmdline,
            start_time_epoch_ms: Milliseconds(i64::try_from(start_epoch_ms).unwrap_or(i64::MAX)),
        })
    }

    /// `sys.stats` arithmetic on a pair of `/proc/stat` CPU lines plus the
    /// load / memory / uptime files. `dcpu` is `(delta_total, delta_busy)`.
    pub fn sys_stats(
        &self,
        dcpu: (u64, u64),
    ) -> Result<omnibus_contracts::dto::process_telemetry::SysStats, ContractError> {
        use omnibus_contracts::dto::process_telemetry::SysStats;
        let loadavg = std::fs::read_to_string(self.root.join("loadavg")).map_err(|e| {
            ContractError::NativeFailure {
                code: NF_PROC_READ,
                payload: format!("proc-read: loadavg: {e}"),
            }
        })?;
        let loads: Vec<f64> = loadavg
            .split_whitespace()
            .take(3)
            .filter_map(|value| value.parse::<f64>().ok())
            .collect();
        let [load_avg_1m, load_avg_5m, load_avg_15m] = loads[..] else {
            return Err(ContractError::NativeFailure {
                code: NF_PROC_READ,
                payload: String::from("proc-read: loadavg carries fewer than three averages"),
            });
        };
        let meminfo = std::fs::read_to_string(self.root.join("meminfo")).map_err(|e| {
            ContractError::NativeFailure {
                code: NF_PROC_READ,
                payload: format!("proc-read: meminfo: {e}"),
            }
        })?;
        let mem_kb = |key: &str| meminfo_kb(&meminfo, key);
        let mem_total_kb = mem_kb("MemTotal").ok_or_else(|| ContractError::NativeFailure {
            code: NF_PROC_READ,
            payload: String::from("proc-read: meminfo has no MemTotal"),
        })?;
        // "Used" is the honest available-based figure (total - available);
        // kernels without MemAvailable fall back to the classic free+buffers+
        // cached approximation.
        let mem_available_kb =
            mem_kb("MemAvailable").unwrap_or_else(|| mem_free_fallback(&meminfo));
        let swap_total_kb = mem_kb("SwapTotal").unwrap_or(0);
        let swap_free_kb = mem_kb("SwapFree").unwrap_or(0);
        let kb = 1_024u64;
        let (dtotal, dbusy) = dcpu;
        let cpu_percent = if dtotal > 0 {
            ((dbusy as f64) / (dtotal as f64) * 100.0).round() as u64
        } else {
            0
        };
        Ok(SysStats {
            cpu_percent: Percent(u32::try_from(cpu_percent).unwrap_or(100)),
            load_avg_1m,
            load_avg_5m,
            load_avg_15m,
            mem_total_bytes: Bytes(mem_total_kb.saturating_mul(kb)),
            mem_used_bytes: Bytes(
                mem_total_kb
                    .saturating_sub(mem_available_kb.min(mem_total_kb))
                    .saturating_mul(kb),
            ),
            swap_total_bytes: Bytes(swap_total_kb.saturating_mul(kb)),
            swap_used_bytes: Bytes(
                swap_total_kb
                    .saturating_sub(swap_free_kb.min(swap_total_kb))
                    .saturating_mul(kb),
            ),
            uptime_seconds: Seconds(self.uptime_seconds() as u64),
        })
    }
}

/// The lifetime-average CPU percent for one process, clamped to 0..=100 and
/// rounded to whole percents (`Percent` is a whole-number newtype): total CPU
/// ticks divided by the process's own age in seconds. A process sampled at
/// its birth (age 0) answers 0 rather than dividing by zero.
fn lifetime_cpu_percent(cpu_ticks: u64, start_ticks: u64, uptime: f64, clk_tck: u64) -> u32 {
    let age = uptime - (start_ticks as f64) / (clk_tck as f64);
    if age <= 0.0 {
        return 0;
    }
    let cpu_seconds = cpu_ticks as f64 / clk_tck as f64;
    ((cpu_seconds / age) * 100.0).round().clamp(0.0, 100.0) as u32
}

/// One `/proc/meminfo` numeric field: the first token after the colon, in kB.
/// `None` when the key is absent — callers decide the honest fallback.
fn meminfo_kb(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let (name, rest) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case(key)
            .then(|| {
                rest.split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .flatten()
    })
}

/// The meminfo fallback for kernels without `MemAvailable`:
/// MemFree + Buffers + Cached (+ SReclaimable when present).
fn mem_free_fallback(meminfo: &str) -> u64 {
    ["MemFree", "Buffers", "Cached", "SReclaimable"]
        .iter()
        .map(|key| meminfo_kb(meminfo, key).unwrap_or(0))
        .fold(0u64, u64::saturating_add)
}

/// The typed refusal for an excluded pid: always `Unsupported` with the svc
/// hint (the domain invariant — services go through the services subsystem).
pub(crate) fn excluded_refusal(pid: u32) -> ContractError {
    ContractError::Unsupported {
        reason: format!(
            "pid {pid} is PID 1 or a systemd-unit process, which stay outside ps.signal: \
             services are addressed only through the services subsystem — use svc_list / \
             svc_status / svc_stop (specs/domains/process-telemetry.md Invariants)"
        ),
    }
}

//! The doctor (roadmap 1.7.1, 2.5.1; specs/architecture/capability-model.md §7.4):
//! diagnosis of Omnibus' own prerequisites over the SPI traits. Called through MCP
//! it only diagnoses and produces copy-paste remediation for a human — it never
//! mutates the system, whatever the agent answers (no consent surface exists).
//! Mutations happen when a human executes `omnibusd doctor` in CLI mode.

use omnibus_contracts::capabilities as cap;
use omnibus_contracts::dto::caps::{DoctorCommand, DoctorFinding, DoctorReport};

use crate::Core;

const EXECUTED_BY_HUMAN: &str = "human";

fn finding(component: &str, severity: &str, detail: String) -> DoctorFinding {
    DoctorFinding {
        component: String::from(component),
        severity: String::from(severity),
        detail,
        remedy: None,
    }
}

fn remedy(mut f: DoctorFinding, remedy: String) -> DoctorFinding {
    f.remedy = Some(remedy);
    f
}

/// Orchestrates the probe report, capability entries, adapter-registry presence,
/// and audit-sink and write-gate facts into one diagnosis. Reads only; every
/// suggested change is a command string for a human to run.
pub async fn diagnose(core: &Core) -> DoctorReport {
    let mut findings: Vec<DoctorFinding> = Vec::new();
    let mut commands: Vec<DoctorCommand> = Vec::new();

    // --- Platform facts and floors (probe report of this session) ---
    let floors = core.caps.floors.clone();
    for floor in &floors {
        if floor.met {
            findings.push(finding(
                "floor",
                "ok",
                format!(
                    "{} {}: floor {} met",
                    floor.component,
                    floor.actual.clone().unwrap_or_else(|| String::from("?")),
                    floor.floor
                ),
            ));
        } else {
            findings.push(remedy(
                finding(
                    "floor",
                    "fail",
                    format!(
                        "{} below floor: {}",
                        floor.component,
                        floor.reason.clone().unwrap_or_else(|| format!(
                            "{} < {}",
                            floor.actual.clone().unwrap_or_default(),
                            floor.floor
                        )),
                    ),
                ),
                String::from(
                    "upgrade the component; tools of the affected subsystem stay hidden until then",
                ),
            ));
        }
    }

    for liveness in &liveness_of(core) {
        if !liveness.alive {
            findings.push(remedy(
                finding(
                    "daemon",
                    "warn",
                    format!(
                        "{} is not reachable: {}",
                        liveness.name,
                        liveness.detail.clone().unwrap_or_default()
                    ),
                ),
                format!(
                    "start/enable the {} service; see the platform knowledge base",
                    liveness.name
                ),
            ));
        }
    }

    // --- Journal persistence (opt-in path on volatile distros; facts per 1.1.1) ---
    let persistent = core.caps.entry(cap::JOURNAL_PERSISTENT);
    match persistent {
        Some(entry) => findings.push(finding(
            "journal",
            "ok",
            format!(
                "journald persistent storage: {} ({})",
                entry
                    .value
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .unwrap_or("true"),
                entry.adapter.clone().unwrap_or_default()
            ),
        )),
        None => {
            // Fact-driven (m0-findings.md): persistence is judged by the probe
            // fact, not by distro family. This storage suggestion fires when the
            // `journal.persistent` entry is absent entirely (journald adapter
            // inactive); a volatile machine with an active adapter reports the
            // entry as `opt-in-required` and is served by the generic opt-in
            // grant suggestion (`omnibusd grant journal.persistent`) instead —
            // surfacing the storage command there remains roadmap 2.5.1 work.
            // Either way the mutation is executed by a human.
            findings.push(remedy(
                finding(
                    "journal",
                    "warn",
                    String::from(
                        "journald volatile: past-boot diagnostics (journal.prev_boot) unavailable",
                    ),
                ),
                String::from(
                    "enable persistent journal storage (command below); next boot gains prev_boot",
                ),
            ));
            commands.push(DoctorCommand {
                name: String::from("enable journal persistence"),
                command: String::from(
                    "sudo mkdir -p /var/log/journal && sudo systemd-tmpfiles --create --prefix /var/log/journal && sudo systemctl restart systemd-journald",
                ),
                executed_by: String::from(EXECUTED_BY_HUMAN),
            });
        }
    }

    // --- Opt-in capabilities without a grant (rung 4 of the ladder) ---
    for capability in omnibus_contracts::capabilities::ALL {
        let entry = core.caps.entry(capability);
        let Some(entry) = entry else { continue };
        if entry.status == "opt-in-required" && !core.grants.has_grant(capability) {
            // The unlocking command is the fact's own remedy when the adapter
            // supplies one: e.g. the volatile-journal prev-boot rung is
            // unlocked by a grant on the persistent-journal capability, not by
            // a grant on the fact itself — a synthesized command naming the
            // fact would be rejected by the grant CLI and the finding could
            // never clear. Synthesize only when the adapter names no lever.
            let command = entry
                .remedy
                .clone()
                .unwrap_or_else(|| format!("omnibusd grant {capability}"));
            let name = command
                .strip_prefix("omnibusd ")
                .unwrap_or(&command)
                .to_string();
            findings.push(remedy(
                finding(
                    "grant",
                    "warn",
                    format!("capability {capability} is opt-in and not granted"),
                ),
                format!("run: {command}"),
            ));
            commands.push(DoctorCommand {
                name,
                command,
                executed_by: String::from(EXECUTED_BY_HUMAN),
            });
        }
    }

    // --- Audit sink health (degraded sink never fails a tool; must be visible) ---
    if !core.audit.fallback_healthy() {
        findings.push(remedy(
            finding(
                "audit",
                "fail",
                format!(
                    "the audit fallback at {} is unwritable: audit events are being lost",
                    core.audit.fallback_path().display()
                ),
            ),
            String::from(
                "make the state directory writable (check XDG_STATE_HOME), then start a new omnibusd instance",
            ),
        ));
    }
    if !core.audit.primary_present() {
        // The completeness claim is honest in all three fallback states: an
        // untried sink must never read as proven (`Unproven` is what a fresh
        // session with zero audited calls reports).
        let completeness = match core.audit.fallback_health() {
            crate::audit::FallbackHealth::Failing => {
                "; the fallback is unwritable too — the audit trail is being lost"
            }
            crate::audit::FallbackHealth::Unproven => {
                "; no audit event has been written yet this session, so the fallback's writability is not yet proven"
            }
            crate::audit::FallbackHealth::Writing => {
                "; the writable fallback keeps the audit trail complete"
            }
        };
        findings.push(remedy(
            finding(
                "audit",
                "warn",
                format!(
                    "primary audit sink unavailable; events go to the file fallback at {}{}",
                    core.audit.fallback_path().display(),
                    completeness
                ),
            ),
            String::from("check journald availability"),
        ));
    }

    // --- Grant manifests that failed to load (the error channel is doctor's) ---
    for error in &core.scopes.errors {
        findings.push(remedy(
            finding(
                "grants",
                "fail",
                format!(
                    "grant manifest unreadable: {} ({})",
                    error.path, error.detail
                ),
            ),
            format!(
                "fix or remove {}; capabilities it grants stay hidden until the file loads",
                error.path
            ),
        ));
    }

    // --- Write gate: a lost or absent cross-process lock is a visible
    // degradation (#9), quoted with the failing syscall's reason when the
    // lock itself failed ---
    if core.write_gate.lock_degraded() || !core.write_gate.cross_process_available() {
        let detail = match core.write_gate.lock_failure_detail() {
            Some(detail) => format!(" ({detail})"),
            None => String::new(),
        };
        findings.push(remedy(
            finding(
                "write-gate",
                "warn",
                format!(
                    "the cross-process write lock is unavailable{detail}: concurrent daemon instances are not serialized (in-process serialization still holds)"
                ),
            ),
            String::from(
                "make XDG_RUNTIME_DIR writable (or set it to a writable directory), then start a new omnibusd instance",
            ),
        ));
    }

    // --- Desktop session (AT-SPI2 presence decides the desktop.* half) ---
    if core.registry.a11y.is_none() {
        findings.push(remedy(
            finding(
                "desktop",
                "warn",
                String::from("no AT-SPI2 accessibility bus in this session; desktop.* tools are hidden"),
            ),
            String::from(
                "install/start at-spi2 (e.g. at-spi2-core package) in this desktop session, then start a new omnibusd instance",
            ),
        ));
    }

    // --- Knowledge base reachability is probed by the KB adapter's report ---
    if core.registry.kb.is_none() {
        findings.push(finding(
            "kb",
            "warn",
            String::from("no knowledge-base adapter activated; wiki_search/wiki_page are hidden"),
        ));
    }

    let opt_ins: Vec<String> = omnibus_contracts::capabilities::ALL
        .iter()
        .filter(|c| {
            core.caps
                .entry(c)
                .map(|e| e.status == "opt-in-required")
                .unwrap_or(false)
        })
        .map(|c| String::from(*c))
        .collect();

    DoctorReport {
        findings,
        ready_commands: commands,
        opt_ins,
    }
}

fn liveness_of(core: &Core) -> Vec<omnibus_contracts::dto::probe::DaemonLiveness> {
    core.platform.liveness.clone()
}

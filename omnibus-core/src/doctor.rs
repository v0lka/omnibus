//! The doctor (roadmap 1.7.1, 2.5.1; specs/architecture/capability-model.md §7.4):
//! diagnosis of Omnibus' own prerequisites over the SPI traits. Called through MCP
//! it only diagnoses and produces copy-paste remediation for a human — it never
//! mutates the system, whatever the agent answers (no consent surface exists).
//! Mutations happen when a human executes `omnibus doctor` in CLI mode.

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

/// Orchestrates probe + journal + services + KB facts into one diagnosis. Reads
/// only; every suggested change is a command string for a human to run.
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
            // fact, not by distro family — a volatile-configured machine of any
            // family gets the same honest suggestion, executed by a human.
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
            findings.push(remedy(
                finding(
                    "grant",
                    "warn",
                    format!("capability {capability} is opt-in and not granted"),
                ),
                format!("run: omnibus grant {capability}"),
            ));
            commands.push(DoctorCommand {
                name: format!("grant {capability}"),
                command: format!("omnibus grant {capability}"),
                executed_by: String::from(EXECUTED_BY_HUMAN),
            });
        }
    }

    // --- Audit sink health (degraded sink never fails a tool; must be visible) ---
    if !core.audit.primary_present() {
        findings.push(remedy(
            finding(
                "audit",
                "warn",
                format!(
                    "primary audit sink unavailable; events go to the file fallback at {}",
                    core.audit.fallback_path().display()
                ),
            ),
            String::from(
                "check journald availability; the fallback keeps the audit trail complete",
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

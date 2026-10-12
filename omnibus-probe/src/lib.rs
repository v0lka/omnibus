//! `omnibus-probe` — platform detection, adapter selection, version floors
//! (specs/architecture/layers-and-crates.md; roadmap tasks 1.4.1, 1.4.2, 2.1.1,
//! 2.4.1). The probe is the runtime detection level (§7.2): every fresh session
//! re-runs `detect()` and the report feeds the capabilities document and the
//! adapter registry — the composition root activates exactly one adapter per
//! subsystem from it.

pub mod detect;
pub mod floors;
pub mod manjaro;
pub mod runner;

use async_trait::async_trait;
use omnibus_contracts::dto::probe::{AdapterReport, DaemonLiveness, DistroFamily, PlatformReport};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::probe::PlatformProbe;

use runner::SharedRunner;

/// The system probe: one instance per process, stateless per run.
#[derive(Clone)]
pub struct Probe {
    runner: SharedRunner,
}

impl Probe {
    /// A probe over the real host.
    pub fn system(runner: SharedRunner) -> Self {
        Self { runner }
    }

    /// Adapter candidates for the platform family (task 2.1.1): exactly one
    /// package backend per family; NM is subsystem-level, not family-level;
    /// knowledge follows the per-distro Knowledge column of
    /// specs/architecture/platform-matrix.md — ArchWiki is the promised source
    /// only on the Arch and Manjaro rows (Debian wiki, Ubuntu docs, Mint
    /// docs/forums and Fedora docs adapters do not exist yet), so other
    /// families get honest absence instead of a source the matrix does not
    /// promise (R7).
    fn adapter_candidates(family: DistroFamily, net_alive: bool) -> Vec<String> {
        let mut adapters: Vec<String> = vec![
            String::from("atspi"),
            String::from("journald"),
            String::from("systemd1"),
        ];
        match family {
            DistroFamily::Arch | DistroFamily::Manjaro => adapters.push(String::from("alpm")),
            DistroFamily::Debian | DistroFamily::Ubuntu | DistroFamily::Mint => {
                adapters.push(String::from("apt"))
            }
            DistroFamily::Fedora => adapters.push(String::from("dnf")),
            DistroFamily::Other => {}
        }
        adapters.push(String::from("xdg-portals"));
        adapters.push(String::from("fs-xdg"));
        if matches!(family, DistroFamily::Arch | DistroFamily::Manjaro) {
            // Platform matrix, Knowledge column: ArchWiki (Arch) and ArchWiki +
            // Manjaro overlay (Manjaro) only; every other family promises its
            // own KB source, not ArchWiki.
            adapters.push(String::from("kb-archwiki"));
        }
        if net_alive {
            adapters.push(String::from("nm"));
        }
        adapters
    }
}

#[async_trait]
impl PlatformProbe for Probe {
    fn report(&self) -> AdapterReport {
        AdapterReport {
            adapter: String::from("probe"),
            subsystem: String::from("probe"),
            contract_version: String::from(omnibus_contracts::version::CONTRACT_VERSION),
            capabilities: vec![],
            notes: vec![],
        }
    }

    /// Runs the detection. Never fails the session: every observation degrades
    /// to `None`/false so the report is always produced (the capabilities doc
    /// is the machine truth, including about absences).
    async fn detect(&self) -> Result<PlatformReport, ContractError> {
        let r = &*self.runner;

        let distro = r
            .read_file("/etc/os-release")
            .map(|text| detect::parse_os_release(&text))
            .unwrap_or_else(|| omnibus_contracts::dto::probe::DistroInfo {
                id: String::from("unknown"),
                name: String::from("unknown"),
                release: String::new(),
                family: DistroFamily::Other,
            });
        let kernel = r
            .read_file("/proc/sys/kernel/osrelease")
            .map(|text| detect::parse_kernel(&text))
            .unwrap_or_else(|| String::from("unknown"));
        let desktop = r.env("XDG_CURRENT_DESKTOP");
        let session_kind = r.env("XDG_SESSION_TYPE");

        // systemd: version + bus liveness.
        let systemd_version = r
            .run("/usr/bin/systemctl", &["--version"])
            .await
            .as_deref()
            .and_then(detect::parse_systemd_version);
        // systemd1 liveness = bus-name ownership, probed like every other
        // daemon (specs/architecture/platform-matrix.md: Services = systemd1 on
        // every Linux row; the only sanctioned hiding reasons are a missing
        // provider and the version floor, never the system state).
        // `systemctl is-system-running` encodes *state*, not presence: its
        // non-zero exit for a healthy-but-"degraded" system (one failed unit
        // suffices) would hide svc_* on the most common real-world state.
        let systemd_alive = r
            .run("/usr/bin/busctl", &["status", "org.freedesktop.systemd1"])
            .await
            .is_some();

        // AT-SPI2: package version (per-family query) + session-bus liveness.
        let atspi_version = atspi_detected_version(r).await;
        let atspi_alive = r
            .run("/usr/bin/busctl", &["--user", "status", "org.a11y.Bus"])
            .await
            .is_some();

        let portal_alive = r
            .run(
                "/usr/bin/busctl",
                &["--user", "status", "org.freedesktop.portal.Desktop"],
            )
            .await
            .is_some();
        let notify_alive = r
            .run(
                "/usr/bin/busctl",
                &["--user", "status", "org.freedesktop.Notifications"],
            )
            .await
            .is_some();
        let net_alive = r
            .run(
                "/usr/bin/busctl",
                &["status", "org.freedesktop.NetworkManager"],
            )
            .await
            .is_some();

        let liveness = vec![
            DaemonLiveness {
                name: String::from("systemd1"),
                alive: systemd_alive,
                detail: None,
            },
            DaemonLiveness {
                name: String::from("at-spi2"),
                alive: atspi_alive,
                detail: Some(String::from(
                    "session a11y bus (org.a11y.Bus); desktop.* tools hide without it",
                )),
            },
            DaemonLiveness {
                name: String::from("xdg-desktop-portal"),
                alive: portal_alive,
                detail: None,
            },
            DaemonLiveness {
                name: String::from("notifications"),
                alive: notify_alive,
                detail: None,
            },
            DaemonLiveness {
                name: String::from("NetworkManager"),
                alive: net_alive,
                detail: None,
            },
        ];

        let mut backends_present: Vec<String> = Vec::new();
        for (backend, path) in [
            ("pacman", "/usr/bin/pacman"),
            ("apt", "/usr/bin/apt"),
            ("dnf", "/usr/bin/dnf"),
        ] {
            if r.file_exists(path) {
                backends_present.push(String::from(backend));
            }
        }

        let mut floors_vec = vec![
            floors::systemd_floor(systemd_version.as_deref()),
            floors::atspi_floor(atspi_version.as_deref()),
        ];
        floors_vec.shrink_to_fit();

        let manjaro = if distro.family == DistroFamily::Manjaro {
            Some(manjaro::collect(r).await)
        } else {
            None
        };

        let adapters_selected = Self::adapter_candidates(distro.family, net_alive);

        Ok(PlatformReport {
            distro,
            kernel,
            desktop,
            session_kind,
            systemd_version,
            atspi_version,
            liveness,
            backends_present,
            floors: floors_vec,
            adapters_selected,
            manjaro,
        })
    }
}

/// AT-SPI2 version detection across package managers (task 2.1.1: the floor is
/// enforced from real versions; the query order matches the family backends).
async fn atspi_detected_version(r: &dyn runner::CommandRunner) -> Option<String> {
    for (prog, args) in [
        ("/usr/bin/pacman", vec!["-Q", "at-spi2-core"]),
        (
            "/usr/bin/dpkg-query",
            vec!["-W", "-f=${Package}\\t${Version}\\n", "at-spi2-core"],
        ),
        ("/usr/bin/rpm", vec!["-q", "at-spi2-core"]),
    ] {
        if let Some(v) = r
            .run(prog, &args)
            .await
            .as_deref()
            .and_then(|out| detect::parse_pkg_query_version(out, "at-spi2-core"))
        {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use omnibus_contracts::floors::ATSPI_FLOOR;
    use runner::CommandRunner;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct FakeRunner {
        files: BTreeMap<String, String>,
        outputs: BTreeMap<String, String>,
        envs: BTreeMap<String, String>,
    }

    impl FakeRunner {
        fn stub(self, prog_args: &str, out: &str) -> Self {
            let mut this = self;
            this.outputs
                .insert(String::from(prog_args), String::from(out));
            this
        }
    }

    #[async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(&self, prog: &str, args: &[&str]) -> Option<String> {
            let key = format!("{prog} {}", args.join(" "));
            self.outputs.get(&key).cloned()
        }
        fn file_exists(&self, path: &str) -> bool {
            self.files.contains_key(path)
        }
        fn env(&self, name: &str) -> Option<String> {
            self.envs.get(name).cloned()
        }
        fn read_file(&self, path: &str) -> Option<String> {
            self.files.get(path).cloned()
        }
    }

    fn manjaro_runner() -> FakeRunner {
        FakeRunner::default()
            .stub(
                "/usr/bin/systemctl --version",
                "systemd 257 (257.8-1-MANJARO)\n",
            )
            .stub("/usr/bin/busctl status org.freedesktop.systemd1", "ok\n")
            .stub("/usr/bin/busctl --user status org.a11y.Bus", "ok\n")
            .stub(
                "/usr/bin/busctl status org.freedesktop.NetworkManager",
                "ok\n",
            )
            .stub("/usr/bin/pacman -Q at-spi2-core", "at-spi2-core 2.60.6-1\n")
            .stub(
                "/usr/bin/pacman.conf",
                "", // not read via run(); files below
            )
            .with_file("/etc/os-release", "ID=manjaro\nNAME=\"Manjaro Linux\"\n")
            .with_file("/proc/sys/kernel/osrelease", "7.1.13-2-MANJARO\n")
            .with_file("/etc/pacman.conf", "[options]\n[core]\n[extra]\n")
            .with_file(
                "/etc/pacman.d/mirrorlist",
                "Server = https://mirror/manjaro/stable\n",
            )
            .with_file("/usr/bin/pacman", "ELF")
    }

    impl FakeRunner {
        fn with_file(mut self, path: &str, body: &str) -> Self {
            self.files.insert(String::from(path), String::from(body));
            self
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn manjaro_report_carries_full_t1_candidates_and_facts() {
        let probe = Probe::system(std::sync::Arc::new(manjaro_runner()));
        let report = probe
            .detect()
            .await
            .expect("detect never fails the session");
        assert_eq!(report.distro.family, DistroFamily::Manjaro);
        assert_eq!(report.systemd_version.as_deref(), Some("257"));
        assert_eq!(report.atspi_version.as_deref(), Some("2.60.6-1"));
        assert!(report.floors.iter().all(|f| f.met), "floors met on Manjaro");
        let manjaro = report.manjaro.expect("Manjaro facts present");
        assert_eq!(manjaro.repos, vec!["core", "extra"]);
        assert!(manjaro.delayed_repos, "delayed repos detected, not silent");
        assert!(
            report.adapters_selected.iter().all(|a| [
                "atspi",
                "journald",
                "systemd1",
                "alpm",
                "xdg-portals",
                "fs-xdg",
                "kb-archwiki",
                "nm"
            ]
            .contains(&a.as_str())),
            "candidates come from the normative set"
        );
        assert!(report.adapters_selected.contains(&String::from("nm")));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn absent_backend_degrades_honestly() {
        let probe = Probe::system(std::sync::Arc::new(FakeRunner::default()));
        let report = probe.detect().await.expect("detect still yields a report");
        assert_eq!(report.distro.family, DistroFamily::Other);
        let systemd_floor = report
            .floors
            .iter()
            .find(|f| f.component == "systemd")
            .expect("systemd floor row");
        assert!(!systemd_floor.met, "absent systemd is never claimed as met");
        assert!(
            !report.adapters_selected.iter().any(|a| a == "nm"),
            "NM hidden when the bus name is not alive"
        );
        assert!(
            !report.adapters_selected.iter().any(|a| a == "kb-archwiki"),
            "ArchWiki is promised only on the Arch/Manjaro rows of the Knowledge column"
        );
        // Floor constants stay in contracts (no magic numbers here).
        assert_eq!(omnibus_contracts::floors::SYSTEMD_FLOOR.major, 252);
        assert_eq!(ATSPI_FLOOR.minor, 50);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn degraded_system_state_keeps_services_present() {
        // Issue #42 regression: a healthy-but-degraded systemd (one failed
        // unit is enough; `is-system-running` exits non-zero there) still owns
        // org.freedesktop.systemd1, so services stay present — hiding them is
        // reserved for a missing provider or the version floor
        // (specs/architecture/platform-matrix.md, Services column).
        let runner = FakeRunner::default()
            .stub(
                "/usr/bin/systemctl --version",
                "systemd 257 (257.8-1-MANJARO)\n",
            )
            .stub("/usr/bin/busctl status org.freedesktop.systemd1", "ok\n");
        let probe = Probe::system(std::sync::Arc::new(runner));
        let report = probe.detect().await.expect("detect never fails");
        let systemd1 = report
            .liveness
            .iter()
            .find(|d| d.name == "systemd1")
            .expect("systemd1 liveness row is always reported");
        assert!(
            systemd1.alive,
            "degraded systemd is alive: svc_* must not hide"
        );
    }
}

//! The adapter registry: exactly one active adapter per subsystem at any moment
//! (specs/architecture/spi-registry.md Invariants), with the one documented
//! exception: the packages subsystem carries the native manager plus the
//! read-only flatpak/snap inventory adapter side by side (v1.5, roadmap 2.9.1
//! — spi-registry.md §9 exception; write semantics stay with the native
//! manager). Slots are filled by the composition root from the probe report; a
//! slot left empty means the subsystem part is absent — tools resolve to
//! `AdapterUnavailable`/hidden per the ladder.

use std::sync::Arc;

use omnibus_contracts::spi::a11y::DesktopA11y;
use omnibus_contracts::spi::app_launcher::AppLauncher;
use omnibus_contracts::spi::audit_sink::AuditSink;
use omnibus_contracts::spi::files::FileScope;
use omnibus_contracts::spi::journal::Journal;
use omnibus_contracts::spi::kb::KnowledgeBase;
use omnibus_contracts::spi::net::NetworkStatus;
use omnibus_contracts::spi::packages::PackageManager;
use omnibus_contracts::spi::portals::{Notify, ScreenCapture};
use omnibus_contracts::spi::probe::PlatformProbe;
use omnibus_contracts::spi::process_telemetry::ProcessTelemetry;
use omnibus_contracts::spi::services::ServiceManager;

/// One typed slot per subsystem (None = not activated).
#[derive(Default, Clone)]
pub struct Registry {
    pub a11y: Option<Arc<dyn DesktopA11y>>,
    pub journal: Option<Arc<dyn Journal>>,
    pub audit_sink: Option<Arc<dyn AuditSink>>,
    pub services: Option<Arc<dyn ServiceManager>>,
    pub packages: Option<Arc<dyn PackageManager>>,
    /// The read-only flatpak/snap inventory (v1.5, roadmap 2.9.1): the one
    /// documented exception to the one-active-adapter rule (§9) — active
    /// alongside `packages` (specs/contracts/core-adapters.md). Read-only by
    /// contract: `pkg_list`/`pkg_info` aggregate it, writes never reach it.
    pub packages_inventory: Option<Arc<dyn PackageManager>>,
    pub screen: Option<Arc<dyn ScreenCapture>>,
    pub notify: Option<Arc<dyn Notify>>,
    pub files: Option<Arc<dyn FileScope>>,
    pub kb: Option<Arc<dyn KnowledgeBase>>,
    pub probe: Option<Arc<dyn PlatformProbe>>,
    pub net: Option<Arc<dyn NetworkStatus>>,
    /// App launch (v1.5, subsystem 11): the `.desktop` inventory and the
    /// transient-unit launch/open writes (specs/domains/app-launch.md).
    pub app_launcher: Option<Arc<dyn AppLauncher>>,
    /// Process telemetry (v1.5, subsystem 12): `/proc` reads, system
    /// statistics, UPower power status and the granted `ps.signal` write
    /// (specs/domains/process-telemetry.md).
    pub process_telemetry: Option<Arc<dyn ProcessTelemetry>>,
    /// Test-only extra report (caps assembly tests; P6 keeps fakes near the core).
    #[cfg(test)]
    pub report_fixture: Option<omnibus_contracts::dto::probe::AdapterReport>,
}

impl Registry {
    /// Aggregate adapter reports of every activated slot — the raw material of the
    /// capabilities document (capability-model.md).
    pub fn adapter_reports(&self) -> Vec<omnibus_contracts::dto::probe::AdapterReport> {
        let mut reports = Vec::new();
        if let Some(a) = &self.a11y {
            reports.push(a.report());
        }
        if let Some(a) = &self.journal {
            reports.push(a.report());
        }
        if let Some(a) = &self.audit_sink {
            reports.push(omnibus_contracts::dto::probe::AdapterReport {
                adapter: String::from(a.kind()),
                subsystem: String::from("audit"),
                contract_version: omnibus_contracts::version::CONTRACT_VERSION.to_string(),
                capabilities: vec![],
                notes: vec![],
            });
        }
        if let Some(a) = &self.services {
            reports.push(a.report());
        }
        if let Some(a) = &self.packages {
            reports.push(a.report());
        }
        if let Some(a) = &self.packages_inventory {
            reports.push(a.report());
        }
        if let Some(a) = &self.screen {
            reports.push(a.report());
        }
        if let Some(a) = &self.notify {
            reports.push(a.report());
        }
        if let Some(a) = &self.files {
            reports.push(a.report());
        }
        if let Some(a) = &self.kb {
            reports.push(a.report());
        }
        if let Some(a) = &self.probe {
            reports.push(a.report());
        }
        if let Some(a) = &self.net {
            reports.push(a.report());
        }
        if let Some(a) = &self.app_launcher {
            reports.push(a.report());
        }
        if let Some(a) = &self.process_telemetry {
            reports.push(a.report());
        }
        #[cfg(test)]
        if let Some(report) = &self.report_fixture {
            reports.push(report.clone());
        }
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal fake for slot tests (P6: fakes live next to the core, not in
    /// adapters).
    struct FakeJournal;

    #[async_trait::async_trait]
    impl Journal for FakeJournal {
        fn report(&self) -> omnibus_contracts::dto::probe::AdapterReport {
            omnibus_contracts::dto::probe::AdapterReport {
                adapter: String::from("fake-journal"),
                subsystem: String::from("journal"),
                contract_version: String::from(omnibus_contracts::version::CONTRACT_VERSION),
                capabilities: vec![],
                notes: vec![],
            }
        }
        async fn query(
            &self,
            _filter: omnibus_contracts::dto::journal::JournalQuery,
        ) -> Result<
            omnibus_contracts::dto::journal::JournalPage,
            omnibus_contracts::errors::ContractError,
        > {
            Err(omnibus_contracts::errors::ContractError::AdapterUnavailable)
        }
        async fn tail(
            &self,
            _request: omnibus_contracts::dto::journal::TailRequest,
        ) -> Result<
            Vec<omnibus_contracts::dto::journal::JournalRecord>,
            omnibus_contracts::errors::ContractError,
        > {
            Err(omnibus_contracts::errors::ContractError::AdapterUnavailable)
        }
    }

    #[test]
    fn empty_registry_reports_nothing() {
        let registry = Registry::default();
        assert!(registry.adapter_reports().is_empty());
        assert!(registry.journal.is_none());
    }

    #[test]
    fn filled_slot_reports_its_adapter() {
        let registry = Registry {
            journal: Some(Arc::new(FakeJournal)),
            ..Registry::default()
        };
        let reports = registry.adapter_reports();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].adapter, "fake-journal");
        assert_eq!(reports[0].subsystem, "journal");
    }
}

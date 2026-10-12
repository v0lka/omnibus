//! The capabilities document assembly (specs/architecture/capability-model.md):
//! aggregate adapter reports into the machine-readable truth for the agent. Every
//! entry names its provider adapter, the contract version, and limitations. Grant
//! entries never appear here — the tool list joins the document with the grant
//! store at build time (config-and-grants.md).

use omnibus_contracts::capabilities::CapabilityStatus;
use omnibus_contracts::dto::caps::{CapabilitiesDocument, CapabilityEntry};
use omnibus_contracts::dto::probe::{FloorStatus, PlatformReport};

/// The assembled machine truth of one session (fresh probe per session).
#[derive(Debug, Clone)]
pub struct CapabilitiesState {
    pub document: CapabilitiesDocument,
    pub floors: Vec<FloorStatus>,
}

impl CapabilitiesState {
    /// Assembles from the fresh platform report and the activated adapters'
    /// reports. Assembly never talks to backends — reports are session facts.
    pub fn assemble(registry: &crate::registry::Registry, platform: &PlatformReport) -> Self {
        let mut entries: Vec<CapabilityEntry> = Vec::new();
        for report in registry.adapter_reports() {
            for fact in report.capabilities {
                entries.push(CapabilityEntry {
                    name: fact.name,
                    ver: fact.ver,
                    status: fact.status,
                    value: fact.value,
                    adapter: Some(report.adapter.clone()),
                    note: fact.note,
                    remedy: fact.remedy,
                });
            }
        }
        // One provider slot per subsystem is the invariant (§9); portals serve
        // two slots from one adapter, so identical (capability, adapter) facts
        // dedup to a single document entry. Sort first — dedup removes runs.
        entries.sort_by(|a, b| {
            (&a.name, &a.adapter, &a.ver, &a.status).cmp(&(&b.name, &b.adapter, &b.ver, &b.status))
        });
        entries.dedup_by(|a, b| {
            a.name == b.name && a.adapter == b.adapter && a.ver == b.ver && a.status == b.status
        });
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            document: CapabilitiesDocument {
                contract_version: omnibus_contracts::version::CONTRACT_VERSION.to_string(),
                distro: platform.distro.clone(),
                capabilities: entries,
            },
            floors: platform.floors.clone(),
        }
    }

    /// Looks one capability up (absence of entry = absence of capability).
    pub fn entry(&self, name: &str) -> Option<&CapabilityEntry> {
        self.document.capabilities.iter().find(|e| e.name == name)
    }

    /// Floor outcome for a subsystem component (`systemd`, `at-spi2`); `None` =
    /// no floor declared for it (trivially met).
    pub fn floor_met(&self, component: &str) -> bool {
        self.floors
            .iter()
            .find(|f| f.component == component)
            .map(|f| f.met)
            .unwrap_or(true)
    }

    /// The reason string below a floor (`systemd 247 < 252` shape).
    pub fn floor_reason(&self, component: &str) -> Option<String> {
        self.floors
            .iter()
            .find(|f| f.component == component)
            .and_then(|f| f.reason.clone())
    }

    /// Ladder resolution for one capability (capability-model.md ladder):
    /// floor first, then the unissued opt-in grant, then absence, then degradation.
    pub fn ladder(
        &self,
        capability: &str,
        component: &str,
        grant_issued: bool,
    ) -> crate::ladder::LadderState {
        let status = self
            .entry(capability)
            .and_then(|e| match e.status.as_str() {
                "present" => Some(CapabilityStatus::Present),
                "degraded" => Some(CapabilityStatus::Degraded),
                "opt-in-required" => Some(CapabilityStatus::OptInRequired),
                _ => None,
            });
        crate::ladder::resolve(status, self.floor_met(component), grant_issued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::dto::probe::{
        AdapterReport, CapabilityFact, DaemonLiveness, DistroFamily, DistroInfo, ManjaroFacts,
    };

    fn platform(floors: Vec<FloorStatus>) -> PlatformReport {
        PlatformReport {
            distro: DistroInfo {
                id: String::from("manjaro"),
                name: String::from("Manjaro Linux"),
                release: String::from("rolling"),
                family: DistroFamily::Manjaro,
            },
            kernel: String::from("test"),
            desktop: Some(String::from("KDE")),
            session_kind: Some(String::from("wayland")),
            systemd_version: Some(String::from("261")),
            atspi_version: Some(String::from("2.60")),
            liveness: vec![DaemonLiveness {
                name: String::from("systemd1"),
                alive: true,
                detail: None,
            }],
            backends_present: vec![],
            floors,
            adapters_selected: vec![String::from("journald")],
            manjaro: Some(ManjaroFacts {
                repos: vec![String::from("core"), String::from("extra")],
                delayed_repos: true,
                mhwd_present: true,
                pamac_present: true,
            }),
        }
    }

    fn registry_with(facts: Vec<CapabilityFact>) -> crate::registry::Registry {
        // Only the report path is exercised here; the slot stays empty.
        crate::registry::Registry {
            report_fixture: Some(AdapterReport {
                adapter: String::from("fake"),
                subsystem: String::from("fake"),
                contract_version: String::from(omnibus_contracts::version::CONTRACT_VERSION),
                capabilities: facts,
                notes: vec![],
            }),
            ..crate::registry::Registry::default()
        }
    }

    fn met_floor(component: &str, actual: &str) -> FloorStatus {
        FloorStatus {
            component: component.to_string(),
            actual: Some(actual.to_string()),
            floor: String::from("floor"),
            met: true,
            reason: None,
        }
    }

    #[test]
    fn document_lists_entries_with_provider_and_version() {
        let facts = vec![CapabilityFact {
            name: omnibus_contracts::capabilities::PKG_UNDO.to_string(),
            ver: String::from("1.0.0"),
            status: String::from("present"),
            value: Some(serde_json::json!("cache-downgrade")),
            note: None,
            remedy: None,
        }];
        let state = CapabilitiesState::assemble(
            &registry_with(facts),
            &platform(vec![met_floor("systemd", "261")]),
        );
        let entry = state
            .entry(omnibus_contracts::capabilities::PKG_UNDO)
            .unwrap_or_else(|| panic!("entry must exist"));
        assert_eq!(entry.adapter.as_deref(), Some("fake"));
        assert_eq!(entry.status, "present");
        assert_eq!(state.document.distro.family, DistroFamily::Manjaro);
    }

    #[test]
    fn ladder_rungs_resolve_through_the_document() {
        use omnibus_contracts::capabilities::{PKG_UNDO, PKG_WRITE};
        let facts = vec![
            CapabilityFact {
                name: PKG_UNDO.to_string(),
                ver: String::from("1.0.0"),
                status: String::from("present"),
                value: None,
                note: None,
                remedy: None,
            },
            CapabilityFact {
                name: PKG_WRITE.to_string(),
                ver: String::from("1.0.0"),
                status: String::from("opt-in-required"),
                value: None,
                note: None,
                remedy: Some(String::from("omnibusd grant pkg.write")),
            },
        ];
        let state = CapabilitiesState::assemble(
            &registry_with(facts),
            &platform(vec![met_floor("systemd", "261")]),
        );
        assert_eq!(
            state.ladder(PKG_UNDO, "systemd", false),
            crate::ladder::LadderState::Available
        );
        assert_eq!(
            state.ladder(PKG_WRITE, "systemd", false),
            crate::ladder::LadderState::OptInRequired
        );
        assert_eq!(
            state.ladder(PKG_WRITE, "systemd", true),
            crate::ladder::LadderState::Available
        );
        assert_eq!(
            state.ladder(
                omnibus_contracts::capabilities::DESKTOP_TREE,
                "systemd",
                false
            ),
            crate::ladder::LadderState::Hidden
        );
    }

    #[test]
    fn below_floor_overrides_everything() {
        let floors = vec![FloorStatus {
            component: String::from("systemd"),
            actual: Some(String::from("247")),
            floor: String::from("252"),
            met: false,
            reason: Some(String::from("systemd 247 < 252")),
        }];
        let state = CapabilitiesState::assemble(&registry_with(vec![]), &platform(floors));
        assert!(!state.floor_met("systemd"));
        assert_eq!(
            state.floor_reason("systemd").as_deref(),
            Some("systemd 247 < 252")
        );
        assert_eq!(
            state.ladder(
                omnibus_contracts::capabilities::SERVICE_READ,
                "systemd",
                false
            ),
            crate::ladder::LadderState::FloorUnsupported
        );
    }
}

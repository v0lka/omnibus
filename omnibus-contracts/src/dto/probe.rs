//! Platform probe DTOs (roadmap 1.4.1, task 2.1.1): distro detection, component
//! versions, daemon liveness, floor enforcement, adapter selection — plus the
//! per-adapter capability report the core aggregates into the capabilities()
//! document (capability-model.md, field names are normative).

crate::dto_enum!(
    pub enum DistroFamily {
        Arch,
        Manjaro,
        Debian,
        Ubuntu,
        Mint,
        Fedora,
        Other,
    }
);

crate::dto!(
    pub struct DistroInfo {
        pub id: String,
        pub name: String,
        pub release: String,
        pub family: DistroFamily,
    }
);

crate::dto!(
    pub struct DaemonLiveness {
        pub name: String,
        pub alive: bool,
        pub detail: Option<String>,
    }
);

crate::dto!(
    pub struct FloorStatus {
        pub component: String,
        pub actual: Option<String>,
        pub floor: String,
        pub met: bool,
        pub reason: Option<String>,
    }
);

crate::dto!(
    /// Manjaro-specific facts the probe must surface explicitly, never silently
    /// (roadmap task 2.4.1 AC: delayed repos detected and reflected in the report).
    pub struct ManjaroFacts {
        pub repos: Vec<String>,
        pub delayed_repos: bool,
        pub mhwd_present: bool,
        pub pamac_present: bool,
    }
);

crate::dto!(
    pub struct PlatformReport {
        pub distro: DistroInfo,
        pub kernel: String,
        pub desktop: Option<String>,
        pub session_kind: Option<String>,
        pub systemd_version: Option<String>,
        pub atspi_version: Option<String>,
        pub liveness: Vec<DaemonLiveness>,
        pub backends_present: Vec<String>,
        pub floors: Vec<FloorStatus>,
        pub adapters_selected: Vec<String>,
        pub manjaro: Option<ManjaroFacts>,
    }
);

crate::dto!(
    /// One capability as reported by its provider adapter; `name` always comes from the
    /// registry constants (CoN rule) and never from an adapter-side literal.
    pub struct CapabilityFact {
        pub name: String,
        pub ver: String,
        pub status: String,
        pub value: Option<serde_json::Value>,
        pub note: Option<String>,
        pub remedy: Option<String>,
    }
);

crate::dto!(
    pub struct AdapterReport {
        pub adapter: String,
        pub subsystem: String,
        pub contract_version: String,
        pub capabilities: Vec<CapabilityFact>,
        pub notes: Vec<String>,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn probe_dto_roundtrips_with_schema() {
        for node in [
            &DistroInfo::schema() as &crate::schema::SchemaNode,
            &DaemonLiveness::schema() as &crate::schema::SchemaNode,
            &FloorStatus::schema() as &crate::schema::SchemaNode,
            &ManjaroFacts::schema() as &crate::schema::SchemaNode,
            &PlatformReport::schema() as &crate::schema::SchemaNode,
            &CapabilityFact::schema() as &crate::schema::SchemaNode,
            &AdapterReport::schema() as &crate::schema::SchemaNode,
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}

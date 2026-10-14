//! The assembled `capabilities()` document, session info, and doctor report DTOs
//! (specs/architecture/capability-model.md, domains/doctor.md). Field names are
//! normative: `ver`, `status`, `value`, `adapter`, `note`, `remedy`; the
//! document-level `floors` array explains below-floor rungs (ladder state 3).

use crate::dto::probe::DistroInfo;

crate::dto!(
    /// One entry of the machine-readable capability document. Grant entries never
    /// appear in the document; the document describes the machine
    /// (config-and-grants.md).
    pub struct CapabilityEntry {
        pub name: String,
        pub ver: String,
        pub status: String,
        pub value: Option<serde_json::Value>,
        pub adapter: Option<String>,
        pub note: Option<String>,
        pub remedy: Option<String>,
    }
);

crate::dto!(
    /// Why one version floor is not met, in the assembled document (ladder
    /// rung 3: "capabilities() explains the reason", e.g. `systemd 247 < 252`).
    /// Floors that are met need no explanation and never appear here.
    pub struct FloorExplanation {
        pub component: String,
        pub reason: String,
    }
);

crate::dto!(
    pub struct CapabilitiesDocument {
        pub contract_version: String,
        pub distro: DistroInfo,
        pub capabilities: Vec<CapabilityEntry>,
        /// Additive explanation field (roadmap 2.1.2): the probe floor facts
        /// that are NOT met, one entry per failing component.
        pub floors: Vec<FloorExplanation>,
    }
);

crate::dto!(
    pub struct LimitsDocument {
        pub tool_call_ms: i64,
        pub tree_ms: i64,
        pub dbus_ms: i64,
        pub read_retries: u8,
    }
);

crate::dto!(
    pub struct SessionInfo {
        pub session_id: String,
        pub started_epoch_ms: i64,
        pub pid: u32,
        pub client_name: Option<String>,
        pub limits: LimitsDocument,
    }
);

crate::dto!(
    pub struct DoctorFinding {
        pub component: String,
        pub severity: String,
        pub detail: String,
        pub remedy: Option<String>,
    }
);

crate::dto!(
    pub struct DoctorCommand {
        pub name: String,
        pub command: String,
        pub executed_by: String,
    }
);

crate::dto!(
    pub struct DoctorReport {
        pub findings: Vec<DoctorFinding>,
        pub ready_commands: Vec<DoctorCommand>,
        pub opt_ins: Vec<String>,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn caps_document_roundtrips_with_schema() {
        for node in [
            &CapabilityEntry::schema() as &crate::schema::SchemaNode,
            &CapabilitiesDocument::schema() as &crate::schema::SchemaNode,
            &SessionInfo::schema() as &crate::schema::SchemaNode,
            &DoctorReport::schema() as &crate::schema::SchemaNode,
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}

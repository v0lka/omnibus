//! Service management DTOs (specs/domains/services.md). Read methods are passive;
//! write actions carry polkit outcomes as honest contract errors (roadmap 1.6.2).

crate::dto!(
    pub struct ServiceUnit {
        pub name: String,
        pub unit_type: String,
        pub description: String,
        pub active_state: String,
        pub sub_state: String,
        pub enabled: Option<bool>,
    }
);

crate::dto!(
    pub struct ServiceFilter {
        pub active_only: Option<bool>,
        pub name_pattern: Option<String>,
        pub limit: Option<u32>,
    }
);

crate::dto!(
    pub struct ServiceStatus {
        pub unit: ServiceUnit,
        pub main_pid: Option<u32>,
        pub since_epoch_ms: Option<i64>,
        pub docs: Vec<String>,
    }
);

crate::dto_enum!(
    pub enum ServiceAction {
        Start,
        Stop,
        Restart,
        Enable,
        Disable,
    }
);

crate::dto!(
    /// `svc_status` / `svc_start` / `svc_stop` argument: one unit name.
    pub struct ServiceNameRequest {
        pub unit: String,
    }
);

crate::dto!(
    pub struct ServiceActionRequest {
        pub unit: String,
        pub action: ServiceAction,
    }
);

crate::dto!(
    pub struct ServiceActionResult {
        pub unit: String,
        pub action: ServiceAction,
        pub active_state: String,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn service_dto_roundtrips_with_schema() {
        for node in [
            &ServiceUnit::schema() as &crate::schema::SchemaNode,
            &ServiceFilter::schema(),
            &ServiceStatus::schema(),
            &ServiceActionRequest::schema(),
            &ServiceActionResult::schema(),
            &ServiceAction::schema(),
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}

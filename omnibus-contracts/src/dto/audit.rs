//! Audit event DTO (specs/domains/audit.md). The schema is fixed:
//! ts, session, actor, tool, adapter, op, params_hash, result, scope_ref — owned by
//! the core, delivered through the AuditSink trait. Raw parameters never travel:
//! only their hash.

crate::dto!(
    pub struct AuditActor {
        pub uid: u32,
        pub pid: u32,
        pub client_name: Option<String>,
    }
);

crate::dto!(
    pub struct AuditEvent {
        pub ts_epoch_ms: i64,
        pub session: String,
        pub actor: AuditActor,
        pub tool: String,
        pub adapter: String,
        pub op: String,
        pub params_hash: String,
        pub result: String,
        pub scope_ref: Option<String>,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn audit_event_roundtrips_with_schema() {
        let node = AuditEvent::schema();
        let value = sample(&node, 0);
        let parsed: AuditEvent = serde_json::from_value(value.clone()).unwrap_or(AuditEvent {
            ts_epoch_ms: 0,
            session: String::new(),
            actor: crate::dto::audit::AuditActor {
                uid: 0,
                pid: 0,
                client_name: None,
            },
            tool: String::new(),
            adapter: String::new(),
            op: String::new(),
            params_hash: String::new(),
            result: String::new(),
            scope_ref: None,
        });
        let back = serde_json::to_value(&parsed).unwrap_or_default();
        assert_eq!(back, value);
    }
}

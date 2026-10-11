//! Journal DTOs (specs/domains/journal.md). `journal.persistent` (storage setting)
//! and `journal.prev_boot` (fact of past boots) are independent capabilities —
//! neither is ever derived from the other (capability-model.md).

use crate::dto::Severity;

crate::dto_enum!(
    /// Which boot the query targets; past boots require the prev_boot capability.
    pub enum BootSelector {
        Current,
        Previous,
    }
);

crate::dto!(
    pub struct JournalQuery {
        pub unit: Option<String>,
        pub boot: Option<BootSelector>,
        pub severity_at_least: Option<Severity>,
        pub since_epoch_ms: Option<i64>,
        pub until_epoch_ms: Option<i64>,
        pub limit: Option<u32>,
    }
);

crate::dto!(
    pub struct JournalRecord {
        pub ts_epoch_ms: i64,
        pub boot_id: Option<String>,
        pub unit: Option<String>,
        pub severity: Option<Severity>,
        pub message: String,
    }
);

crate::dto!(
    pub struct JournalPage {
        pub records: Vec<JournalRecord>,
        pub truncated: bool,
    }
);

crate::dto!(
    pub struct TailRequest {
        pub limit: u32,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn journal_dto_roundtrips_with_schema() {
        for node in [
            &JournalQuery::schema() as &crate::schema::SchemaNode,
            &JournalRecord::schema(),
            &JournalPage::schema(),
            &TailRequest::schema(),
            &BootSelector::schema(),
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}

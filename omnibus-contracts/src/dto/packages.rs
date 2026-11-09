//! Package management DTOs (specs/domains/packages.md). `pkg.undo` is polymorphic:
//! `none` / `cache-downgrade` / `transactional` — the adapter reports its value in
//! the capability document, the schema never promises an undo the backend lacks (R7).

crate::dto!(
    pub struct PackageBrief {
        pub name: String,
        pub version: String,
        pub description: Option<String>,
        pub repo: Option<String>,
        /// Which store the package comes from (v1.5 aggregation, roadmap
        /// 2.9.1): the native manager, the flatpak inventory or the snap
        /// inventory. Every package in `pkg_list` names its source
        /// explicitly — the invariant is never implied by absence.
        pub source: PackageSource,
    }
);

crate::dto_enum!(
    /// The store of a package in the aggregated `pkg_list`/`pkg_info` output
    /// (specs/domains/packages.md v1.5): the native family manager, or the
    /// read-only flatpak/snap inventory.
    pub enum PackageSource {
        Native,
        Flatpak,
        Snap,
    }
);

crate::dto!(
    pub struct PackageInfo {
        pub name: String,
        pub version: String,
        pub description: Option<String>,
        pub repo: Option<String>,
        pub size_bytes: Option<u64>,
        pub install_reason: Option<String>,
        pub depends: Vec<String>,
        pub required_by: Vec<String>,
        pub version_available: Option<String>,
    }
);

crate::dto!(
    pub struct PackageSearch {
        pub query: String,
        pub limit: Option<u32>,
    }
);

crate::dto!(
    pub struct PackageListRequest {
        pub limit: Option<u32>,
    }
);

crate::dto_enum!(
    pub enum PackageAction {
        Install,
        Upgrade,
        Downgrade,
        Remove,
        Reinstall,
    }
);

crate::dto!(
    pub struct PackageHistoryEntry {
        pub ts_epoch_ms: i64,
        pub action: PackageAction,
        pub name: String,
        pub from_version: Option<String>,
        pub to_version: Option<String>,
    }
);

crate::dto!(
    pub struct PackageHistoryQuery {
        pub name: Option<String>,
        pub limit: Option<u32>,
    }
);

crate::dto!(
    pub struct PackageNameRequest {
        pub name: String,
    }
);

crate::dto!(
    pub struct TransactionResult {
        pub name: String,
        /// Advisory version as of the lookup: a concurrent native transaction
        /// can make the applied version differ (audit-facing, best-effort).
        pub version_applied: Option<String>,
    }
);

crate::dto_enum!(
    pub enum UndoSource {
        Cache,
        Transaction,
    }
);

crate::dto!(
    pub struct UndoResult {
        pub name: String,
        pub from_version: String,
        pub to_version: String,
        pub source: UndoSource,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn package_dto_roundtrips_with_schema() {
        for node in [
            &PackageBrief::schema() as &crate::schema::SchemaNode,
            &PackageSource::schema(),
            &PackageInfo::schema(),
            &PackageSearch::schema(),
            &PackageHistoryEntry::schema(),
            &TransactionResult::schema(),
            &UndoResult::schema(),
            &PackageAction::schema(),
            &UndoSource::schema(),
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}

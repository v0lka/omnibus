//! Application-launch DTOs (specs/domains/app-launch.md). Arguments always
//! travel as a typed list inside the DTO and reach the target process
//! verbatim — shell metacharacters are data, never syntax (no `sh -c`).

crate::dto!(
    /// One installed application (the `app.list` inventory entry): the
    /// `.desktop` facts the inventory serves. `app_id` is the stable launch
    /// key (the desktop-file id), not a display string.
    pub struct AppEntry {
        pub app_id: String,
        pub name: String,
        pub exec: String,
        pub icon: Option<String>,
        pub no_display: bool,
    }
);

crate::dto!(
    /// `app.launch` argument: which application to start and its arguments.
    /// `args` is a typed list — the adapter executes the target directly, and
    /// the arguments arrive verbatim (specs/domains/app-launch.md Invariants).
    pub struct AppSpec {
        pub app_id: String,
        pub args: Vec<String>,
    }
);

crate::dto!(
    /// `app.launch` outcome: the transient user unit the process landed in —
    /// the join key for `ps.list` / `journal_query` observation (the launch →
    /// observe side of the closed loop).
    pub struct LaunchOutcome {
        pub unit_name: String,
    }
);

crate::dto!(
    /// `app.open` argument: one target — a URI or an absolute filesystem path
    /// — opened with the associated application, passed verbatim.
    pub struct OpenRequest {
        pub target: String,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn app_launcher_dto_roundtrips_with_schema() {
        for node in [
            &AppEntry::schema() as &crate::schema::SchemaNode,
            &AppSpec::schema(),
            &LaunchOutcome::schema(),
            &OpenRequest::schema(),
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}

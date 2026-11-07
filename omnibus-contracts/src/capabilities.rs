//! The capability registry — the single source of capability names.
//!
//! Every capability name exists exactly once, as a constant below
//! (specs/architecture/capability-model.md, roadmap §7.1). Name literals outside this
//! file are forbidden and rejected by the `omnibus-lints` CoN rule (roadmap 1.1.3):
//! this is the CoN connascence weakened to constants in one place
//! (docs/development/omnibus-architecture.md §8.5).
//!
//! Statuses and semver per capability arrive with roadmap task 1.2.3; the constants
//! below are the closed §7.1 name list.

/// Capability name format: `{domain}.{feature}[.{qualifier}]`. One canonical
/// alphabet for every name: dot-separated segments, each starting with a lowercase
/// ASCII letter and continuing with `[a-z0-9_-]` — no uppercase, no other
/// characters. The R4-CON predicate in `omnibus-lints` matches exactly this
/// alphabet.
pub const CAPABILITY_NAME_FORMAT: &str = "{domain}.{feature}[.{qualifier}]";

pub const DESKTOP_TREE: &str = "desktop.tree";
pub const DESKTOP_WRITE_TYPED: &str = "desktop.write.typed";
pub const JOURNAL_PERSISTENT: &str = "journal.persistent";
pub const JOURNAL_PREV_BOOT: &str = "journal.prev_boot";
pub const JOURNAL_QUERY: &str = "journal.query";
pub const SERVICE_READ: &str = "service.read";
pub const SERVICE_WRITE: &str = "service.write";
pub const PKG_READ: &str = "pkg.read";
pub const PKG_WRITE: &str = "pkg.write";
pub const PKG_UNDO: &str = "pkg.undo";
pub const PORTAL_CLIPBOARD: &str = "portal.clipboard";
pub const PORTAL_SCREENSHOT: &str = "portal.screenshot";
pub const PORTAL_NOTIFY: &str = "portal.notify";
pub const FILES_SCOPE_READ: &str = "files.scope.read";
pub const FILES_SCOPE_WRITE: &str = "files.scope.write";
pub const FILES_SCOPE_TCC: &str = "files.scope.tcc";
pub const KB_PRESENT: &str = "kb.present";
pub const NET_READ: &str = "net.read";

/// The closed §7.1 registry, in roadmap order. The registry is closed over the
/// roadmap task list; new capabilities enter only additively (minor contract version).
pub const ALL: [&str; 18] = [
    DESKTOP_TREE,
    DESKTOP_WRITE_TYPED,
    JOURNAL_PERSISTENT,
    JOURNAL_PREV_BOOT,
    JOURNAL_QUERY,
    SERVICE_READ,
    SERVICE_WRITE,
    PKG_READ,
    PKG_WRITE,
    PKG_UNDO,
    PORTAL_CLIPBOARD,
    PORTAL_SCREENSHOT,
    PORTAL_NOTIFY,
    FILES_SCOPE_READ,
    FILES_SCOPE_WRITE,
    FILES_SCOPE_TCC,
    KB_PRESENT,
    NET_READ,
];

/// Capability status as reported by the registry
/// (specs/architecture/capability-model.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityStatus {
    /// Available on this machine now.
    Present,
    /// Available with a documented limitation.
    Degraded,
    /// Requires an out-of-band grant before first use.
    OptInRequired,
}

/// The typed VALUE of the polyform `pkg.undo` capability (roadmap 1.2.3 AC2):
/// a closed set of undo semantics, serialized as kebab-case strings inside the
/// capability fact's `value` (`"none"`, `"cache-downgrade"`, `"transactional"`).
/// The packages suite parses every reported `pkg.undo` value through this enum,
/// so an off-registry wire value is a suite failure, and the apt-family
/// "no undo" is a typed variant — never a string convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PkgUndoValue {
    /// Honest no-undo: the backend has no rollback (the apt families).
    None,
    /// alpm semantics: revert to the previous version from the package cache.
    CacheDowngrade,
    /// dnf semantics: invert one recorded transaction (`dnf history undo`).
    Transactional,
}

impl PkgUndoValue {
    /// The canonical wire string (the JSON form of the capability value).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CacheDowngrade => "cache-downgrade",
            Self::Transactional => "transactional",
        }
    }

    /// The JSON value adapters put into `CapabilityFact::value` for `pkg.undo`.
    pub fn to_json(self) -> serde_json::Value {
        serde_json::Value::String(String::from(self.as_str()))
    }

    /// Parses a fact value into the typed semantics; `None` for anything
    /// outside the closed set (the core treats an unparseable value as absence
    /// of a usable value, mirroring the unrecognized-status rule).
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(value.clone()).ok()
    }
}

/// Checks the §7.1 name format against the canonical alphabet: `{domain}.`
/// `{feature}[.{qualifier}]` — at least two dot-separated segments, each starting
/// with a lowercase ASCII letter and continuing with `[a-z0-9_-]`. This is the
/// same alphabet the R4-CON predicate in `omnibus-lints` matches; the predicate
/// additionally fixes the first segment to a registry domain.
pub fn is_valid_capability_name(name: &str) -> bool {
    let segments: Vec<&str> = name.split('.').collect();
    segments.len() >= 2
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_closed_and_deduplicated() {
        let mut sorted = ALL;
        sorted.sort_unstable();
        for pair in sorted.windows(2) {
            assert_ne!(pair[0], pair[1], "duplicate capability constant");
        }
        assert_eq!(ALL.len(), 18, "registry is closed over the §7.1 list");
    }

    #[test]
    fn every_constant_matches_the_name_format() {
        for name in ALL {
            assert!(is_valid_capability_name(name), "bad name format: {name}");
        }
    }

    #[test]
    fn name_format_rejects_single_segment_and_uppercase() {
        assert!(!is_valid_capability_name("desktop"));
        assert!(!is_valid_capability_name("Desktop.tree"));
        assert!(!is_valid_capability_name(""));
        assert!(is_valid_capability_name("desktop.write.typed"));
    }

    /// The canonical alphabet: a segment must START with a lowercase letter
    /// and may continue with `[a-z0-9_-]` — the exact alphabet the R4-CON
    /// predicate matches (a digit-leading name must not slip past here while
    /// the lint's alphabet rejects it, or the drift guard has a seam).
    #[test]
    fn name_format_matches_the_documented_alphabet_exactly() {
        assert!(is_valid_capability_name("net.wifi-scan"));
        assert!(is_valid_capability_name("journal.prev_boot"));
        assert!(!is_valid_capability_name("net.9p_mount"));
        assert!(!is_valid_capability_name("net.-wifi"));
        assert!(!is_valid_capability_name("net.wifi scan"));
    }

    /// The typed `pkg.undo` value is a closed kebab-case set on the wire: the
    /// exact strings every package adapter reports and the suite parses back.
    #[test]
    fn pkg_undo_wire_values_are_the_closed_kebab_set() {
        assert_eq!(PkgUndoValue::None.as_str(), "none");
        assert_eq!(PkgUndoValue::CacheDowngrade.as_str(), "cache-downgrade");
        assert_eq!(PkgUndoValue::Transactional.as_str(), "transactional");
        for value in [
            PkgUndoValue::None,
            PkgUndoValue::CacheDowngrade,
            PkgUndoValue::Transactional,
        ] {
            let back = PkgUndoValue::from_json(&value.to_json());
            assert_eq!(back, Some(value), "typed value roundtrips: {value:?}");
        }
    }

    /// Anything outside the closed set — including an unknown string — parses
    /// to None (the suite must fail an off-registry value, not pass it on).
    #[test]
    fn pkg_undo_parse_rejects_unknown_values() {
        assert_eq!(
            PkgUndoValue::from_json(&serde_json::json!("rollback")),
            None
        );
        assert_eq!(PkgUndoValue::from_json(&serde_json::json!(7)), None);
        assert_eq!(PkgUndoValue::from_json(&serde_json::Value::Null), None);
    }
}

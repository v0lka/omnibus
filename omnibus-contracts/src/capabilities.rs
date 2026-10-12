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
}

//! Contract semver — independent of code versions (roadmap task 1.2.1 AC).
//!
//! The contract version describes the seam (trait set + DTO shapes + capability
//! registry), not the daemon build. `omnibusd --version` shows the code version;
//! `capabilities()` shows this one per entry. Removal or reshaping of the eight
//! error codes or a DTO field is major; additive traits/capabilities are minor.

/// Semver of the seam contract as declared by the current workspace state.
/// 1.4.0: `PackageBrief.source` enters additively (the aggregated
/// `pkg_list`/`pkg_info` output names its store: native/flatpak/snap,
/// roadmap task 2.9.1) — additive DTO fields are minor, like additive traits.
pub const CONTRACT_VERSION: &str = "1.4.0";

/// Parse guard: the constant must stay a valid `MAJOR.MINOR.PATCH` string.
pub fn is_valid_semver(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_version_is_valid_semver() {
        assert!(is_valid_semver(CONTRACT_VERSION));
    }

    #[test]
    fn contract_version_is_independent_of_the_code_version() {
        // The code version is 0.x while the seam ships its first stable contract;
        // coupling the two would break the "semver independent of code" invariant.
        assert_ne!(CONTRACT_VERSION, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn semver_guard_rejects_malformed_versions() {
        assert!(!is_valid_semver("1"));
        assert!(!is_valid_semver("1.0"));
        assert!(!is_valid_semver("1.0.0-pre"));
        assert!(!is_valid_semver(""));
    }
}

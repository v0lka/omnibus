//! Version floors (specs/architecture/platform-matrix.md "Version Floors").
//!
//! The floor constants always live in `omnibus-contracts` — never magic numbers in
//! the probe (platform-matrix.md Invariants). The AT-SPI2 floor value comes from the
//! M0 measurements (roadmap task 1.1.1); until the full 5-DE VM sweep runs, the
//! constant carries the local/proxy value and `m0-findings.md` records that status.

/// Semantic version of a platform component, compared per the floor rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlatformVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl PlatformVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses `261.2` / `2.60.6` / `255` / `247.3-1` style outputs; garbage yields
    /// `None` so the probe can degrade honestly instead of guessing. Components
    /// after the first take their leading digit run (`3-1` → `3`).
    pub fn parse(s: &str) -> Option<Self> {
        let leading = |part: &str| -> Option<u32> {
            let digits: String = part
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if digits.is_empty() {
                None
            } else {
                digits.parse().ok()
            }
        };
        let mut parts = s.split('.');
        let major = leading(parts.next()?)?;
        let minor = parts.next().and_then(leading).unwrap_or(0);
        let patch = parts.next().and_then(leading).unwrap_or(0);
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

/// systemd floor for the T2 contract: bookworm ships 252 exactly on the floor;
/// bullseye (247) stays below it and deactivates the service subsystem (T3).
pub const SYSTEMD_FLOOR: PlatformVersion = PlatformVersion::new(252, 0, 0);

/// AT-SPI2 floor fixed by the M0 measurements (roadmap 1.1.1). Proxy value from the
/// local Manjaro check (2.60 present); the 5-DE VM sweep is tracked in
/// `docs/verification/m0-findings.md` as the required external run.
pub const ATSPI_FLOOR: PlatformVersion = PlatformVersion::new(2, 50, 0);

/// True when `actual` satisfies `floor`.
pub fn meets(floor: PlatformVersion, actual: PlatformVersion) -> bool {
    actual >= floor
}

/// The exact machine-readable reason string `capabilities()` renders below a floor,
/// e.g. `systemd 247 < 252` (capability-model.md ladder rung 3).
pub fn floor_reason(component: &str, floor: PlatformVersion, actual: PlatformVersion) -> String {
    format!(
        "{component} {}.{} < {}.{}",
        actual.major, actual.minor, floor.major, floor.minor
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_constants_match_the_platform_matrix() {
        assert_eq!(SYSTEMD_FLOOR, PlatformVersion::new(252, 0, 0));
        assert_eq!(ATSPI_FLOOR, PlatformVersion::new(2, 50, 0));
    }

    #[test]
    fn version_parsing_covers_probe_inputs() {
        assert_eq!(
            PlatformVersion::parse("261"),
            Some(PlatformVersion::new(261, 0, 0))
        );
        assert_eq!(
            PlatformVersion::parse("2.60.6"),
            Some(PlatformVersion::new(2, 60, 6))
        );
        assert_eq!(
            PlatformVersion::parse(" 247.3-1"),
            Some(PlatformVersion::new(247, 3, 0))
        );
        assert_eq!(PlatformVersion::parse("n/a"), None);
        assert_eq!(PlatformVersion::parse(""), None);
    }

    #[test]
    fn floor_comparison_and_reason_rendering() {
        let systemd = PlatformVersion::parse("247.3").unwrap_or(SYSTEMD_FLOOR);
        assert!(!meets(SYSTEMD_FLOOR, systemd));
        assert_eq!(
            floor_reason("systemd", SYSTEMD_FLOOR, systemd),
            "systemd 247.3 < 252.0"
        );
        assert!(meets(
            SYSTEMD_FLOOR,
            PlatformVersion::parse("261.2").unwrap_or(SYSTEMD_FLOOR)
        ));
        assert!(
            meets(SYSTEMD_FLOOR, SYSTEMD_FLOOR),
            "floor value itself is on the floor, not below"
        );
    }
}

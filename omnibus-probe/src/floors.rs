//! Floor enforcement (roadmap 1.4.2): below the floor the adapter is not
//! activated and `capabilities()` explains the exact reason. Floor constants
//! come from `omnibus-contracts` — never local magic numbers.

use omnibus_contracts::dto::probe::FloorStatus;
use omnibus_contracts::floors::{ATSPI_FLOOR, PlatformVersion, SYSTEMD_FLOOR, floor_reason, meets};

/// Evaluates the systemd floor from the detected version string (`None` =
/// systemd absent or unparsable: the floor cannot be claimed as met).
pub fn systemd_floor(version: Option<&str>) -> FloorStatus {
    component_floor("systemd", SYSTEMD_FLOOR, version)
}

/// Evaluates the AT-SPI2 floor (value fixed by the M0 measurements, 1.1.1).
pub fn atspi_floor(version: Option<&str>) -> FloorStatus {
    component_floor("at-spi2", ATSPI_FLOOR, version)
}

fn component_floor(component: &str, floor: PlatformVersion, version: Option<&str>) -> FloorStatus {
    let parsed = version.and_then(PlatformVersion::parse);
    match parsed {
        Some(actual) if meets(floor, actual) => FloorStatus {
            component: String::from(component),
            actual: version.map(String::from),
            floor: format!("{}.{}", floor.major, floor.minor),
            met: true,
            reason: None,
        },
        Some(actual) => FloorStatus {
            component: String::from(component),
            actual: version.map(String::from),
            floor: format!("{}.{}", floor.major, floor.minor),
            met: false,
            reason: Some(floor_reason(component, floor, actual)),
        },
        None => FloorStatus {
            component: String::from(component),
            actual: None,
            floor: format!("{}.{}", floor.major, floor.minor),
            met: false,
            reason: Some(format!(
                "{component} version not detected (floor {}.{}); component likely absent",
                floor.major, floor.minor
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_systemd_is_above_the_floor() {
        let status = systemd_floor(Some("257.8-1"));
        assert!(status.met);
        assert!(status.reason.is_none());
    }

    #[test]
    fn below_floor_carries_the_exact_reason() {
        // Simulated environment: stub report "systemd 247" (roadmap 1.4.2 AC).
        let status = systemd_floor(Some("247.3-1"));
        assert!(!status.met);
        assert_eq!(
            status.reason.as_deref(),
            Some("systemd 247.3 < 252.0"),
            "capabilities() must explain the exact reason"
        );
    }

    #[test]
    fn atspi_floor_uses_the_m0_constant() {
        assert!(atspi_floor(Some("2.60.6-1")).met);
        let low = atspi_floor(Some("2.48.0"));
        assert!(!low.met);
        assert_eq!(low.reason.as_deref(), Some("at-spi2 2.48 < 2.50"));
    }

    #[test]
    fn absent_version_is_not_met_and_honest() {
        let status = systemd_floor(None);
        assert!(!status.met);
        assert!(status.reason.unwrap_or_default().contains("not detected"));
    }

    #[test]
    fn on_the_floor_is_met() {
        // bookworm ships systemd 252 exactly on the floor (task 2.1.2).
        assert!(systemd_floor(Some("252")).met);
    }
}

//! The power side of `power.status` (roadmap 2.8.2, specs/domains/process-telemetry.md
//! Degradation): battery and supply facts through UPower over the system D-Bus,
//! "when present" — probe auto-detect, sysfs cross-check.
//!
//! Presence rule (R3/R7, platform-matrix.md): `power.status` needs UPower
//! *and* a battery. A machine without either hides the tool — the adapter
//! emits no `power.read` capability fact, and a direct trait call answers
//! [`ContractError::CapabilityRequired`] — never a hollow report, and never
//! an error in the log (absence is the honest fact, not a failure).
//!
//! Native types (zbus proxies, object paths) never leave this crate; the
//! mapping to the contract DTO happens here, under one wall-clock budget so
//! a wedged UPower degrades honestly instead of stalling the call.

use std::path::PathBuf;

use omnibus_contracts::dto::process_telemetry::PowerStatus;
use omnibus_contracts::dto::{Percent, Seconds};
use omnibus_contracts::errors::ContractError;
use zbus::Connection;

/// UPower's Device state enum: 2 = discharging (on battery, drawing from it).
const DEVICE_STATE_DISCHARGING: u32 = 2;
/// The bus budget: connect + two property rounds must answer well inside the
/// core's read budget; a wedged bus answers `TimeoutError` (the adapter
/// stopped waiting — the read observes no native state).
pub const POWER_BUDGET: std::time::Duration = std::time::Duration::from_secs(4);

// The proxy declarations rely on `assume_defaults = true`: the destination is
// the interface's well-known name (`org.freedesktop.UPower`) and the default
// path is its own object path — exactly the UPower layout on the system bus.

#[zbus::proxy(interface = "org.freedesktop.UPower", assume_defaults = true)]
trait UPower {
    /// The aggregate display device: the battery the desktop UI reports.
    fn get_display_device(&self) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
    /// True when the system is drawing from a battery.
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower.Device",
    default_path = "/",
    assume_defaults = true
)]
trait Device {
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    /// Seconds until empty; UPower answers 0 when unknown → the DTO's `None`.
    #[zbus(property)]
    fn time_to_empty(&self) -> zbus::Result<i64>;
    /// Seconds until full; UPower answers 0 when unknown → the DTO's `None`.
    #[zbus(property)]
    fn time_to_full(&self) -> zbus::Result<i64>;
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
}

/// The sysfs half of the presence rule: at least one power-supply object of
/// type `Battery` under `<sys>/class/power_supply` (the kernel-side battery
/// fact, independent of UPower's own view). Checks the `type` attribute —
/// supply node names alone are vendor-specific.
pub(crate) fn battery_present(root: &std::path::Path) -> bool {
    let supplies = root.join("class").join("power_supply");
    let Ok(read) = std::fs::read_dir(&supplies) else {
        return false;
    };
    read.flatten().any(|entry| {
        let type_file = entry.path().join("type");
        std::fs::metadata(&type_file)
            .map(|m| m.is_file())
            .unwrap_or(false)
            && std::fs::read_to_string(&type_file)
                .map(|text| text.trim().eq_ignore_ascii_case("Battery"))
                .unwrap_or(false)
    })
}

/// The power provider: probe-detected UPower presence plus the sysfs root
/// for the battery cross-check.
#[derive(Debug, Clone)]
pub(crate) struct PowerProvider {
    /// UPower liveness per the probe (bus-name ownership on the system bus).
    pub upower: bool,
    /// The sysfs root (live `/sys`; the fixture suite's synthetic stand).
    pub sys_root: PathBuf,
}

impl PowerProvider {
    /// The DTO answer, or [`ContractError::CapabilityRequired`] when the
    /// machine lacks UPower or a battery (the R3 hiding rung — the tool is
    /// hidden above the seam; a direct trait call refuses typed, with the
    /// reason living in the adapter report's `ps.read` note).
    pub async fn status(&self) -> Result<PowerStatus, ContractError> {
        if !self.upower || !battery_present(&self.sys_root) {
            return Err(ContractError::CapabilityRequired);
        }
        tokio::time::timeout(POWER_BUDGET, self.read_upower())
            .await
            .map_err(|_| ContractError::TimeoutError)?
    }

    /// The UPower read: connect to the system bus, take the display device,
    /// map its properties onto the DTO. Any bus failure maps to
    /// [`ContractError::AdapterUnavailable`] (the provider went away between
    /// probe and call — honestly absent for this call, not a native crash).
    async fn read_upower(&self) -> Result<PowerStatus, ContractError> {
        async {
            let connection = Connection::system()
                .await
                .map_err(|_| ContractError::AdapterUnavailable)?;
            let upower = UPowerProxy::new(&connection).await.map_err(bus)?;
            let display = upower.get_display_device().await.map_err(bus)?;
            let device = DeviceProxy::builder(&connection)
                .path(display)
                .map_err(bus)?
                .build()
                .await
                .map_err(bus)?;
            let on_battery = upower.on_battery().await.map_err(bus)?;
            let is_present = device.is_present().await.map_err(bus)?;
            let percent = device.percentage().await.map_err(bus)?;
            let to_empty = device.time_to_empty().await.map_err(bus)?;
            let to_full = device.time_to_full().await.map_err(bus)?;
            let state = device.state().await.map_err(bus)?;
            Ok(PowerStatus {
                on_battery: on_battery || state == DEVICE_STATE_DISCHARGING,
                battery_percent: is_present
                    .then(|| Percent(percent.round().clamp(0.0, 100.0) as u32)),
                seconds_until_empty: positive_seconds(to_empty),
                seconds_until_full: positive_seconds(to_full),
            })
        }
        .await
    }
}

/// UPower encodes "unknown" as 0 (or a negative sentinel on old versions);
/// the DTO honestly carries `None` for those.
fn positive_seconds(value: i64) -> Option<Seconds> {
    u64::try_from(value).ok().filter(|s| *s > 0).map(Seconds)
}

fn bus(e: zbus::Error) -> ContractError {
    match e {
        zbus::Error::MethodError(name, _, _) if name.contains("ServiceUnknown") => {
            ContractError::AdapterUnavailable
        }
        _ => ContractError::NativeFailure {
            code: 4,
            payload: format!("dbus: UPower conversation failed: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sysfs battery cross-check reads the `type` attribute, not the
    /// node name: a `C:`-style vendor name must not be the deciding fact.
    #[test]
    fn battery_presence_reads_the_type_attribute() {
        let dir = tempfile::tempdir().expect("tempdir");
        let supplies = dir.path().join("class").join("power_supply");
        std::fs::create_dir_all(supplies.join("BAT0")).expect("bat dir");
        std::fs::write(supplies.join("BAT0").join("type"), "Battery\n").expect("type");
        std::fs::create_dir_all(supplies.join("AC")).expect("ac dir");
        std::fs::write(supplies.join("AC").join("type"), "Mains\n").expect("type");
        assert!(battery_present(dir.path()));
        // A battery-named node with a non-battery type is not a battery.
        let dir2 = tempfile::tempdir().expect("tempdir");
        let supplies2 = dir2.path().join("class").join("power_supply");
        std::fs::create_dir_all(supplies2.join("BATT")).expect("dir");
        std::fs::write(supplies2.join("BATT").join("type"), "UPS\n").expect("type");
        assert!(!battery_present(dir2.path()));
        // No power_supply tree at all: honestly absent.
        assert!(!battery_present(&dir2.path().join("elsewhere")));
    }

    #[test]
    fn unknown_durations_are_none() {
        assert_eq!(positive_seconds(0), None);
        assert_eq!(positive_seconds(-5), None);
        assert_eq!(positive_seconds(1800), Some(Seconds(1_800)));
    }

    #[tokio::test]
    async fn without_upower_the_answer_is_capability_required() {
        let provider = PowerProvider {
            upower: false,
            sys_root: PathBuf::from("/nonexistent-omnibus-suite"),
        };
        let answer = provider.status().await;
        assert!(matches!(answer, Err(ContractError::CapabilityRequired)));
    }

    #[tokio::test]
    async fn upower_without_a_battery_hides_the_tool_too() {
        // The R3/R7 rule: UPower alone is not enough — a desktop with the
        // power daemon but no battery hides power.status as well. The sysfs
        // stand here has no battery, so the answer is the hiding rung even
        // with `upower: true`.
        let provider = PowerProvider {
            upower: true,
            sys_root: PathBuf::from("/nonexistent-omnibus-suite"),
        };
        let answer = provider.status().await;
        assert!(matches!(answer, Err(ContractError::CapabilityRequired)));
    }
}

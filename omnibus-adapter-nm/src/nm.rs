//! zbus access to `org.freedesktop.NetworkManager` and mapping of NM native
//! values onto contract DTOs. All native types (object paths, raw enum codes)
//! stay inside this module — only contract DTOs are returned (DTO discipline).
//!
//! The module is strictly read-only: every operation below reads properties or
//! calls passive getters. `GetAccessPoints` is a passive read of the AP list NM
//! already holds — it triggers no scan and mutates no state.

use omnibus_contracts::dto::net::{NetDevice, NetStatusReport, WifiNetwork};
use omnibus_contracts::errors::ContractError;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, Proxy};

/// Well-known NM identity on the system bus.
pub const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
/// The single NM root object.
pub const NM_ROOT: &str = "/org/freedesktop/NetworkManager";

/// NM `Connectivity` property values (NetworkManager 1.x D-Bus API).
const CONNECTIVITY_UNKNOWN: u32 = 0;
const CONNECTIVITY_NONE: u32 = 1;
const CONNECTIVITY_PORTAL: u32 = 2;
const CONNECTIVITY_LIMITED: u32 = 3;
const CONNECTIVITY_FULL: u32 = 4;

/// NM `Device.DeviceType` values that name a human-readable kind.
const DEVICE_TYPE_ETHERNET: u32 = 1;
const DEVICE_TYPE_WIFI: u32 = 2;
const DEVICE_TYPE_BLUETOOTH: u32 = 5;
const DEVICE_TYPE_WIMAX: u32 = 8;
const DEVICE_TYPE_GSM_MODEM: u32 = 9;
const DEVICE_TYPE_CDMA_MODEM: u32 = 10;
const DEVICE_TYPE_INFINIBAND: u32 = 12;
const DEVICE_TYPE_BOND: u32 = 13;
const DEVICE_TYPE_VLAN: u32 = 14;
const DEVICE_TYPE_ADSL: u32 = 15;
const DEVICE_TYPE_BRIDGE: u32 = 16;
const DEVICE_TYPE_GENERIC: u32 = 17;
const DEVICE_TYPE_TEAM: u32 = 18;
const DEVICE_TYPE_TUN: u32 = 19;
const DEVICE_TYPE_IP_TUNNEL: u32 = 20;
const DEVICE_TYPE_MACVLAN: u32 = 21;
const DEVICE_TYPE_VXLAN: u32 = 22;
const DEVICE_TYPE_VETH: u32 = 23;
const DEVICE_TYPE_MACSEC: u32 = 24;
const DEVICE_TYPE_DUMMY: u32 = 25;
const DEVICE_TYPE_PPP: u32 = 26;
const DEVICE_TYPE_OVS_INTERFACE: u32 = 27;
const DEVICE_TYPE_OVS_BRIDGE: u32 = 28;
const DEVICE_TYPE_OVS_PORT: u32 = 29;
const DEVICE_TYPE_WIREGUARD: u32 = 32;

/// NM `Device.State` values (the closed NM device state machine).
const DEVICE_STATE_UNKNOWN: u32 = 0;
const DEVICE_STATE_UNMANAGED: u32 = 10;
const DEVICE_STATE_UNAVAILABLE: u32 = 20;
const DEVICE_STATE_DISCONNECTED: u32 = 30;
const DEVICE_STATE_PREPARE: u32 = 40;
const DEVICE_STATE_CONFIG: u32 = 50;
const DEVICE_STATE_NEED_AUTH: u32 = 60;
const DEVICE_STATE_IP_CONFIG: u32 = 70;
const DEVICE_STATE_IP_CHECK: u32 = 80;
const DEVICE_STATE_SECONDARIES: u32 = 90;
const DEVICE_STATE_ACTIVATED: u32 = 100;
const DEVICE_STATE_DEACTIVATING: u32 = 110;
const DEVICE_STATE_FAILED: u32 = 120;

/// The 802-11 AP flag meaning "privacy" (WEP or better).
const AP_FLAG_PRIVACY: u32 = 0x1;

/// The D-Bus error name suffix when the destination service has no owner.
const DBUS_ERROR_SERVICE_UNKNOWN_SUFFIX: &str = "ServiceUnknown";

/// Maps the NM connectivity enum onto the contract's string vocabulary.
pub fn connectivity_name(code: u32) -> String {
    match code {
        CONNECTIVITY_NONE => String::from("none"),
        CONNECTIVITY_PORTAL => String::from("portal"),
        CONNECTIVITY_LIMITED => String::from("limited"),
        CONNECTIVITY_FULL => String::from("full"),
        CONNECTIVITY_UNKNOWN => String::from("unknown"),
        _ => String::from("unknown"),
    }
}

/// Maps the NM device type code onto a stable lowercase kind name.
pub fn device_kind_name(code: u32) -> String {
    match code {
        DEVICE_TYPE_ETHERNET => String::from("ethernet"),
        DEVICE_TYPE_WIFI => String::from("wifi"),
        DEVICE_TYPE_BLUETOOTH => String::from("bluetooth"),
        DEVICE_TYPE_WIMAX => String::from("wimax"),
        DEVICE_TYPE_GSM_MODEM => String::from("gsm-modem"),
        DEVICE_TYPE_CDMA_MODEM => String::from("cdma-modem"),
        DEVICE_TYPE_INFINIBAND => String::from("infiniband"),
        DEVICE_TYPE_BOND => String::from("bond"),
        DEVICE_TYPE_VLAN => String::from("vlan"),
        DEVICE_TYPE_ADSL => String::from("adsl"),
        DEVICE_TYPE_BRIDGE => String::from("bridge"),
        DEVICE_TYPE_GENERIC => String::from("generic"),
        DEVICE_TYPE_TEAM => String::from("team"),
        DEVICE_TYPE_TUN => String::from("tun"),
        DEVICE_TYPE_IP_TUNNEL => String::from("ip-tunnel"),
        DEVICE_TYPE_MACVLAN => String::from("macvlan"),
        DEVICE_TYPE_VXLAN => String::from("vxlan"),
        DEVICE_TYPE_VETH => String::from("veth"),
        DEVICE_TYPE_MACSEC => String::from("macsec"),
        DEVICE_TYPE_DUMMY => String::from("dummy"),
        DEVICE_TYPE_PPP => String::from("ppp"),
        DEVICE_TYPE_OVS_INTERFACE => String::from("ovs-interface"),
        DEVICE_TYPE_OVS_BRIDGE => String::from("ovs-bridge"),
        DEVICE_TYPE_OVS_PORT => String::from("ovs-port"),
        DEVICE_TYPE_WIREGUARD => String::from("wireguard"),
        other => format!("nm-device-type-{other}"),
    }
}

/// Maps the NM device state code onto a stable lowercase state name.
pub fn device_state_name(code: u32) -> String {
    match code {
        DEVICE_STATE_UNKNOWN => String::from("unknown"),
        DEVICE_STATE_UNMANAGED => String::from("unmanaged"),
        DEVICE_STATE_UNAVAILABLE => String::from("unavailable"),
        DEVICE_STATE_DISCONNECTED => String::from("disconnected"),
        DEVICE_STATE_PREPARE => String::from("preparing"),
        DEVICE_STATE_CONFIG => String::from("configuring"),
        DEVICE_STATE_NEED_AUTH => String::from("need-auth"),
        DEVICE_STATE_IP_CONFIG => String::from("ip-config"),
        DEVICE_STATE_IP_CHECK => String::from("ip-check"),
        DEVICE_STATE_SECONDARIES => String::from("secondaries"),
        DEVICE_STATE_ACTIVATED => String::from("activated"),
        DEVICE_STATE_DEACTIVATING => String::from("deactivating"),
        DEVICE_STATE_FAILED => String::from("failed"),
        other => format!("nm-state-{other}"),
    }
}

/// Maps a zbus failure onto the contract taxonomy. A vanished service means the
/// adapter is effectively absent; a timed-out D-Bus round trip is a contract
/// timeout; everything else preserves the original message as native failure.
pub fn map_dbus_error(err: zbus::Error) -> ContractError {
    if is_nm_absence(&err) {
        return ContractError::AdapterUnavailable;
    }
    if let zbus::Error::InputOutput(io) = &err
        && io.kind() == std::io::ErrorKind::TimedOut
    {
        return ContractError::TimeoutError;
    }
    ContractError::NativeFailure {
        code: 1,
        payload: err.to_string(),
    }
}

/// True when the failure means NM is not reachable on the system bus.
fn is_nm_absence(err: &zbus::Error) -> bool {
    match err {
        zbus::Error::MethodError(name, _, _) => {
            name.as_str().ends_with(DBUS_ERROR_SERVICE_UNKNOWN_SUFFIX)
        }
        _ => false,
    }
}

/// True when NM owns its well-known name on the system bus.
pub async fn nm_is_present(connection: &Connection) -> bool {
    let Ok(service) = zbus::names::BusName::try_from(NM_SERVICE) else {
        return false;
    };
    match zbus::fdo::DBusProxy::new(connection).await {
        Ok(dbus) => matches!(dbus.name_has_owner(service).await, Ok(true)),
        Err(_) => false,
    }
}

/// The NM root facts `status()` and the adapter report share.
pub struct NmOverview {
    pub version: String,
    pub connectivity: String,
    pub networking_enabled: bool,
    pub primary_connection_id: Option<String>,
}

/// Reads one string property from an NM object; missing/unexpected shapes
/// collapse to `None` — optional fields degrade honestly, never panic.
async fn string_property(proxy: &Proxy<'_>, name: &str) -> Option<String> {
    proxy.get_property::<String>(name).await.ok()
}

/// Reads the NM root object facts: version, connectivity state, the
/// networking-enabled flag, and the primary connection id.
pub async fn read_nm_overview(connection: &Connection) -> Result<NmOverview, ContractError> {
    let nm = Proxy::new(connection, NM_SERVICE, NM_ROOT, NM_SERVICE)
        .await
        .map_err(map_dbus_error)?;
    let version = nm
        .get_property::<String>("Version")
        .await
        .map_err(map_dbus_error)?;
    let connectivity_code = nm
        .get_property::<u32>("Connectivity")
        .await
        .map_err(map_dbus_error)?;
    let networking_enabled = nm
        .get_property::<bool>("NetworkingEnabled")
        .await
        .map_err(map_dbus_error)?;
    let primary_connection_id = read_primary_connection_id(&nm).await;
    Ok(NmOverview {
        version,
        connectivity: connectivity_name(connectivity_code),
        networking_enabled,
        primary_connection_id,
    })
}

/// Resolves `PrimaryConnection` (an object path) into the active connection id;
/// the root path (`/`) means "no primary connection".
async fn read_primary_connection_id(nm: &Proxy<'_>) -> Option<String> {
    let path: OwnedObjectPath = nm.get_property("PrimaryConnection").await.ok()?;
    let path = path.as_str();
    if path == "/" || path.is_empty() {
        return None;
    }
    let active = Proxy::new(
        nm.connection(),
        NM_SERVICE,
        path,
        "org.freedesktop.NetworkManager.Connection.Active",
    )
    .await
    .ok()?;
    string_property(&active, "Id").await
}

/// Gathers the full read-only network status: devices, per-device addresses,
/// and the access points visible to wireless devices.
pub async fn read_status(connection: &Connection) -> Result<NetStatusReport, ContractError> {
    let overview = read_nm_overview(connection).await?;
    let devices = read_devices(connection).await?;
    let wifi_networks = read_wifi_networks(connection).await;
    Ok(NetStatusReport {
        connectivity: overview.connectivity,
        primary_connection: overview.primary_connection_id,
        devices,
        wifi_networks,
    })
}

/// Enumerates devices via `GetDevices` and maps each one to a contract DTO.
async fn read_devices(connection: &Connection) -> Result<Vec<NetDevice>, ContractError> {
    let nm = Proxy::new(connection, NM_SERVICE, NM_ROOT, NM_SERVICE)
        .await
        .map_err(map_dbus_error)?;
    let paths: Vec<OwnedObjectPath> = nm.call("GetDevices", &()).await.map_err(map_dbus_error)?;
    let mut devices = Vec::with_capacity(paths.len());
    for path in paths {
        if let Some(device) = read_device(connection, path.as_str()).await {
            devices.push(device);
        }
    }
    Ok(devices)
}

/// Reads one device; a device that vanished mid-read (a property call fails)
/// is skipped instead of failing the whole report.
async fn read_device(connection: &Connection, path: &str) -> Option<NetDevice> {
    let device = Proxy::new(
        connection,
        NM_SERVICE,
        path,
        "org.freedesktop.NetworkManager.Device",
    )
    .await
    .ok()?;
    let name = string_property(&device, "Interface").await?;
    let kind_code = device.get_property::<u32>("DeviceType").await.ok()?;
    let state_code = device.get_property::<u32>("State").await.ok()?;
    let ip4 = read_ip4_address(connection, &device).await;
    let ip6 = read_ip6_address(connection, &device).await;
    let connection_id = read_device_connection_id(connection, &device).await;
    Some(NetDevice {
        name,
        kind: device_kind_name(kind_code),
        state: device_state_name(state_code),
        ip4,
        ip6,
        connection: connection_id,
    })
}

/// First IPv4 address of the device's IP4 config (NM `Addresses` is `aau`:
/// each entry is `[address in network byte order, prefix, gateway]`).
async fn read_ip4_address(connection: &Connection, device: &Proxy<'_>) -> Option<String> {
    let config_path: OwnedObjectPath = device.get_property("Ip4Config").await.ok()?;
    let config_path = config_path.as_str();
    if config_path == "/" || config_path.is_empty() {
        return None;
    }
    let config = Proxy::new(
        connection,
        NM_SERVICE,
        config_path,
        "org.freedesktop.NetworkManager.IP4Config",
    )
    .await
    .ok()?;
    let addresses: Vec<Vec<u32>> = config.get_property("Addresses").await.ok()?;
    addresses
        .first()
        .and_then(|entry| entry.first())
        .map(|raw| ip4_to_string(u32::from_be(*raw)))
}

/// First IPv6 address of the device's IP6 config (NM `Addresses` is `a(ayuay)`).
async fn read_ip6_address(connection: &Connection, device: &Proxy<'_>) -> Option<String> {
    let config_path: OwnedObjectPath = device.get_property("Ip6Config").await.ok()?;
    let config_path = config_path.as_str();
    if config_path == "/" || config_path.is_empty() {
        return None;
    }
    let config = Proxy::new(
        connection,
        NM_SERVICE,
        config_path,
        "org.freedesktop.NetworkManager.IP6Config",
    )
    .await
    .ok()?;
    let addresses: Vec<(Vec<u8>, u32, Vec<u8>)> = config.get_property("Addresses").await.ok()?;
    addresses.first().and_then(|(bytes, _, _)| {
        let segments: [u8; 16] = bytes.as_slice().try_into().ok()?;
        Some(std::net::Ipv6Addr::from(segments).to_string())
    })
}

/// Formats a host-order IPv4 address as a dotted quad.
fn ip4_to_string(value: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        (value >> 24) & 0xff,
        (value >> 16) & 0xff,
        (value >> 8) & 0xff,
        value & 0xff
    )
}

/// Id of the active connection currently bound to the device, if any.
async fn read_device_connection_id(connection: &Connection, device: &Proxy<'_>) -> Option<String> {
    let active_path: OwnedObjectPath = device.get_property("ActiveConnection").await.ok()?;
    let active_path = active_path.as_str();
    if active_path == "/" || active_path.is_empty() {
        return None;
    }
    let active = Proxy::new(
        connection,
        NM_SERVICE,
        active_path,
        "org.freedesktop.NetworkManager.Connection.Active",
    )
    .await
    .ok()?;
    string_property(&active, "Id").await
}

/// Collects the access points visible to every wireless device. The AP list is
/// read passively per wireless device; non-wireless devices simply yield no AP
/// interface and are skipped.
async fn read_wifi_networks(connection: &Connection) -> Vec<WifiNetwork> {
    let nm = match Proxy::new(connection, NM_SERVICE, NM_ROOT, NM_SERVICE).await {
        Ok(nm) => nm,
        Err(_) => return Vec::new(),
    };
    let device_paths: Vec<OwnedObjectPath> = match nm.call("GetDevices", &()).await {
        Ok(paths) => paths,
        Err(_) => return Vec::new(),
    };
    let mut networks = Vec::new();
    for device_path in device_paths {
        let wireless = Proxy::new(
            connection,
            NM_SERVICE,
            device_path.as_str(),
            "org.freedesktop.NetworkManager.Device.Wireless",
        )
        .await;
        let wireless = match wireless {
            Ok(w) => w,
            Err(_) => continue,
        };
        let ap_paths: Vec<OwnedObjectPath> = match wireless.call("GetAccessPoints", &()).await {
            Ok(paths) => paths,
            Err(_) => continue,
        };
        for ap_path in ap_paths {
            if let Some(network) = read_wireless_ap(connection, ap_path.as_str()).await {
                networks.push(network);
            }
        }
    }
    dedupe_by_ssid(networks)
}

/// Reads one access point object; hidden APs (empty SSID) are skipped.
async fn read_wireless_ap(connection: &Connection, path: &str) -> Option<WifiNetwork> {
    let ap = Proxy::new(
        connection,
        NM_SERVICE,
        path,
        "org.freedesktop.NetworkManager.AccessPoint",
    )
    .await
    .ok()?;
    let ssid_bytes: Vec<u8> = ap.get_property("Ssid").await.ok()?;
    if ssid_bytes.is_empty() {
        return None;
    }
    let ssid = String::from_utf8_lossy(&ssid_bytes).into_owned();
    let strength = ap.get_property::<u8>("Strength").await.unwrap_or(0);
    let wpa_flags: u32 = ap.get_property("WpaFlags").await.unwrap_or(0);
    let rsn_flags: u32 = ap.get_property("RsnFlags").await.unwrap_or(0);
    let ap_flags: u32 = ap.get_property("Flags").await.unwrap_or(0);
    let secured = wpa_flags != 0 || rsn_flags != 0 || ap_flags & AP_FLAG_PRIVACY != 0;
    Some(WifiNetwork {
        ssid,
        strength,
        secured,
    })
}

/// Collapses the same SSID seen via several devices/radios, keeping the
/// strongest signal.
fn dedupe_by_ssid(networks: Vec<WifiNetwork>) -> Vec<WifiNetwork> {
    let mut unique: Vec<WifiNetwork> = Vec::with_capacity(networks.len());
    for network in networks {
        match unique
            .iter_mut()
            .find(|existing| existing.ssid == network.ssid)
        {
            Some(existing) => {
                if network.strength > existing.strength {
                    *existing = network;
                }
            }
            None => unique.push(network),
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connectivity_codes_map_to_contract_names() {
        assert_eq!(connectivity_name(CONNECTIVITY_FULL), "full");
        assert_eq!(connectivity_name(CONNECTIVITY_LIMITED), "limited");
        assert_eq!(connectivity_name(CONNECTIVITY_PORTAL), "portal");
        assert_eq!(connectivity_name(CONNECTIVITY_NONE), "none");
        assert_eq!(connectivity_name(CONNECTIVITY_UNKNOWN), "unknown");
        assert_eq!(connectivity_name(999), "unknown");
    }

    #[test]
    fn device_kind_codes_map_to_stable_names() {
        assert_eq!(device_kind_name(DEVICE_TYPE_ETHERNET), "ethernet");
        assert_eq!(device_kind_name(DEVICE_TYPE_WIFI), "wifi");
        assert_eq!(device_kind_name(DEVICE_TYPE_WIREGUARD), "wireguard");
        assert!(device_kind_name(4242).starts_with("nm-device-type-"));
    }

    #[test]
    fn device_state_codes_map_to_stable_names() {
        assert_eq!(device_state_name(DEVICE_STATE_ACTIVATED), "activated");
        assert_eq!(device_state_name(DEVICE_STATE_DISCONNECTED), "disconnected");
        assert!(device_state_name(55).starts_with("nm-state-"));
    }

    #[test]
    fn ip4_roundtrip_uses_network_byte_order() {
        // NM hands out the raw u32 in network byte order (verified on a live
        // NM: 127.0.0.1 arrives as 16777343 = 0x0100007F); the adapter swaps
        // once before the dotted-quad formatting.
        let raw_192_168_1_10 = 0x0A01A8C0_u32;
        assert_eq!(
            ip4_to_string(u32::from_be(raw_192_168_1_10)),
            "192.168.1.10"
        );
        let raw_127_0_0_1 = 16777343_u32;
        assert_eq!(ip4_to_string(u32::from_be(raw_127_0_0_1)), "127.0.0.1");
    }

    #[test]
    fn wifi_dedupe_keeps_the_strongest_per_ssid() {
        let deduped = dedupe_by_ssid(vec![
            WifiNetwork {
                ssid: String::from("home"),
                strength: 40,
                secured: true,
            },
            WifiNetwork {
                ssid: String::from("home"),
                strength: 80,
                secured: true,
            },
            WifiNetwork {
                ssid: String::from("cafe"),
                strength: 30,
                secured: false,
            },
        ]);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].strength, 80);
        assert_eq!(deduped[1].ssid, "cafe");
    }
}

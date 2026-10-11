//! NetworkManager adapter: read-only network status over the NM D-Bus API
//! (roadmap 2.2.3; ADR-003: NM is its own D-Bus service and subsystem §4.10).
//!
//! The [`NetworkStatus`] trait is declared just-in-time together with this, its
//! first implementation. Behind netplan/networkd (no NM on the system bus) the
//! adapter is not activated and the net status tool stays hidden — absence is
//! honest, not an error. There are deliberately no write methods: the network
//! domain is read-only (never-list §7.4).
//!
//! Native types (zbus values, NM device paths) never leave this crate — mapping
//! to contract DTOs happens inside the adapter ([`nm`] module).

mod nm;

use async_trait::async_trait;
use omnibus_contracts::capabilities::NET_READ;
use omnibus_contracts::dto::net::NetStatusReport;
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::net::NetworkStatus;
use omnibus_contracts::version::CONTRACT_VERSION;
use zbus::Connection;

/// Live NetworkManager status provider: devices, connectivity, visible wifi
/// networks. Cloning is cheap (the bus connection is shared).
#[derive(Debug, Clone)]
pub struct NmAdapter {
    connection: Connection,
    version: String,
    networking_enabled: bool,
}

impl NmAdapter {
    /// Connects to the system bus and verifies NM is live there. Any failure
    /// along the way is reported as adapter-unavailable: the composition root
    /// simply leaves the slot empty and the net tool stays hidden.
    pub async fn connect() -> Result<Self, ContractError> {
        let connection = Connection::system()
            .await
            .map_err(|_| ContractError::AdapterUnavailable)?;
        if !nm::nm_is_present(&connection).await {
            return Err(ContractError::AdapterUnavailable);
        }
        let overview = match nm::read_nm_overview(&connection).await {
            Ok(overview) => overview,
            // NM answered the bus-level presence check but not the property
            // read: treat the backend as unavailable rather than half-alive.
            Err(_) => return Err(ContractError::AdapterUnavailable),
        };
        Ok(Self {
            connection,
            version: overview.version,
            networking_enabled: overview.networking_enabled,
        })
    }
}

#[async_trait]
impl NetworkStatus for NmAdapter {
    /// The adapter report: identity plus the NET_READ capability fact, present
    /// whenever this adapter was constructed against a live NM.
    fn report(&self) -> AdapterReport {
        AdapterReport {
            adapter: String::from("nm"),
            subsystem: String::from("net"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![CapabilityFact {
                name: String::from(NET_READ),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("present"),
                value: None,
                note: Some(format!(
                    "NetworkManager {} on the system bus; networking {} at activation; \
                     the network domain is read-only",
                    self.version,
                    if self.networking_enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                )),
                remedy: None,
            }],
            notes: Vec::new(),
        }
    }

    /// Devices, connectivity, and visible wifi networks; read-only by contract.
    async fn status(&self) -> Result<NetStatusReport, ContractError> {
        // NM may have left the bus since connect(): absence stays honest.
        if !nm::nm_is_present(&self.connection).await {
            return Err(ContractError::AdapterUnavailable);
        }
        nm::read_status(&self.connection).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_fact_uses_the_registry_constant() {
        let fact = CapabilityFact {
            name: String::from(NET_READ),
            ver: String::from(CONTRACT_VERSION),
            status: String::from("present"),
            value: None,
            note: None,
            remedy: None,
        };
        assert_eq!(fact.name, NET_READ);
        assert!(omnibus_contracts::capabilities::ALL.contains(&fact.name.as_str()));
    }

    // The suite binding against the live NM lives in tests/live_nm.rs; it
    // skips gracefully when NM is absent on this machine.
}

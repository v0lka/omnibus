//! `NetworkStatus` — read-only network facts via NetworkManager (subsystem 10),
//! declared just-in-time together with `omnibus-adapter-nm` (roadmap task 2.2.3,
//! ADR-003). The contract declares no write operation: switching the network
//! backend and any network write action are on the never-list (§7.4).

use async_trait::async_trait;

use crate::dto::net::NetStatusReport;
use crate::errors::ContractError;

/// `net_status` behind the `net.read` capability. Absent NM (netplan/networkd
/// stands) means the adapter is not activated and the tool is hidden — the rest of
/// the system is unaffected (roadmap 2.2.3 AC).
#[async_trait]
pub trait NetworkStatus: Send + Sync {
    /// The adapter report (NM presence, version).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// Devices, connectivity, and visible wifi networks; read-only by contract.
    async fn status(&self) -> Result<NetStatusReport, ContractError>;
}

//! Network status DTOs (specs/domains/network.md). The domain is read-only — the
//! contract declares no write operation, and the adapter crate carries no write
//! methods by construction (roadmap task 2.2.3, never-list §7.4).

crate::dto!(
    pub struct NetDevice {
        pub name: String,
        pub kind: String,
        pub state: String,
        pub ip4: Option<String>,
        pub ip6: Option<String>,
        pub connection: Option<String>,
    }
);

crate::dto!(
    pub struct WifiNetwork {
        pub ssid: String,
        pub strength: u8,
        pub secured: bool,
    }
);

crate::dto!(
    pub struct NetStatusReport {
        pub connectivity: String,
        pub primary_connection: Option<String>,
        pub devices: Vec<NetDevice>,
        pub wifi_networks: Vec<WifiNetwork>,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn net_dto_roundtrips_with_schema() {
        for node in [
            &NetDevice::schema() as &crate::schema::SchemaNode,
            &WifiNetwork::schema() as &crate::schema::SchemaNode,
            &NetStatusReport::schema() as &crate::schema::SchemaNode,
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}

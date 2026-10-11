//! Adapter-owned `node_ref` addressing: `<bus-destination>|<object-path>`.
//!
//! The core treats the string as opaque and mints its own element tokens over it
//! (DTO discipline, specs/domains/desktop-a11y.md); only this crate packs and
//! unpacks the pair. Addressing carries bus destinations and object paths —
//! never pixels or coordinates (P2/P3).

/// Packs a D-Bus destination and an object path into one opaque node reference.
pub fn encode_node_ref(dest: &str, path: &str) -> String {
    format!("{dest}|{path}")
}

/// Unpacks a node reference; `None` when the string is malformed.
///
/// A malformed reference can never resolve, so callers map `None` to
/// [`ContractError::ElementStale`] (the reference is as good as dead).
pub fn decode_node_ref(node_ref: &str) -> Option<(String, String)> {
    let (dest, path) = node_ref.split_once('|')?;
    if dest.is_empty() || dest.contains('|') || !path.starts_with('/') || path.contains('|') {
        return None;
    }
    Some((dest.to_string(), path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_dest_and_path() {
        let raw = encode_node_ref(":1.40", "/org/a11y/atspi/accessible/3");
        assert_eq!(raw, ":1.40|/org/a11y/atspi/accessible/3");
        let (dest, path) = decode_node_ref(&raw).unwrap_or_default();
        assert_eq!(dest, ":1.40");
        assert_eq!(path, "/org/a11y/atspi/accessible/3");
    }

    #[test]
    fn well_known_destinations_roundtrip() {
        let raw = encode_node_ref("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root");
        let (dest, path) = decode_node_ref(&raw).unwrap_or_default();
        assert_eq!(dest, "org.a11y.atspi.Registry");
        assert_eq!(path, "/org/a11y/atspi/accessible/root");
    }

    #[test]
    fn malformed_references_never_decode() {
        assert!(decode_node_ref("").is_none());
        assert!(decode_node_ref("|/org/a11y/atspi/accessible/1").is_none());
        assert!(decode_node_ref(":1.1|").is_none());
        assert!(decode_node_ref(":1.1|relative/path").is_none());
        assert!(decode_node_ref(":1.1|/a|/b").is_none());
    }
}

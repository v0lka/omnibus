//! Pure portal wire helpers: request-path composition, handle tokens, file
//! URI decoding, and portal response parsing. Unit-tested without D-Bus.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use zbus::zvariant::OwnedValue;

/// The sender part of a portal request path: the unique connection name with
/// the leading colon stripped and dots replaced (`:1.749` -> `1_749`).
pub fn sender_part(unique_name: &str) -> String {
    unique_name
        .strip_prefix(':')
        .unwrap_or(unique_name)
        .replace('.', "_")
}

/// Composes the predictable request object path for a portal call:
/// `/org/freedesktop/portal/desktop/request/<sender>/<token>`.
pub fn request_path(sender: &str, token: &str) -> String {
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

/// Fresh alphanumeric handle token (the portal spec allows `[A-Za-z0-9_]+`).
pub fn new_handle_token() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("omnibus_{millis}")
}

/// Percent-decodes a URI path segment (`%20` and `+` as space).
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = |pair: [u8; 2]| -> Option<u8> {
                    let hi = (pair[0] as char).to_digit(16)?;
                    let lo = (pair[1] as char).to_digit(16)?;
                    Some((hi * 16 + lo) as u8)
                };
                if let Some(byte) = hex([bytes[index + 1], bytes[index + 2]]) {
                    out.push(byte);
                    index += 3;
                } else {
                    out.push(b'%');
                    index += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Converts a `file://` URI into a local filesystem path.
pub fn file_uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///path` (empty host) or `file://host/path`; the path starts at the
    // first `/` after the host section, slash included.
    let path = if rest.starts_with('/') {
        rest
    } else {
        let slash = rest.find('/')?;
        &rest[slash..]
    };
    let decoded = percent_decode(path);
    if decoded.starts_with('/') {
        Some(decoded)
    } else {
        None
    }
}

/// Extracts the returned screenshot URI from the portal response results.
pub fn uri_from_results(results: &HashMap<String, OwnedValue>) -> Option<String> {
    results
        .get("uri")
        .and_then(|value| value.downcast_ref::<&str>().ok().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn sender_part_encodes_the_unique_name() {
        assert_eq!(sender_part(":1.749"), "1_749");
        assert_eq!(sender_part(":1.0"), "1_0");
    }

    #[test]
    fn request_path_follows_the_portal_layout() {
        let path = request_path(&sender_part(":1.749"), "omnibus_42");
        assert_eq!(
            path,
            "/org/freedesktop/portal/desktop/request/1_749/omnibus_42"
        );
    }

    #[test]
    fn handle_tokens_are_alphanumeric_with_underscores() {
        let token = new_handle_token();
        assert!(token.starts_with("omnibus_"));
        assert!(
            token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "token {token} must stay alphanumeric"
        );
    }

    #[test]
    fn percent_decode_handles_spaces_and_bare_percent() {
        assert_eq!(percent_decode("/home/x%20y.png"), "/home/x y.png");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("/plain"), "/plain");
    }

    #[test]
    fn file_uris_convert_with_host_and_percent_forms() {
        assert_eq!(
            file_uri_to_path("file:///home/x%20y.png"),
            Some(String::from("/home/x y.png"))
        );
        assert_eq!(
            file_uri_to_path("file://localhost/home/x.png"),
            Some(String::from("/home/x.png"))
        );
        assert_eq!(file_uri_to_path("http://x/y.png"), None);
        assert_eq!(file_uri_to_path("file:relative.png"), None);
    }

    #[test]
    fn uri_extraction_reads_the_results_dictionary() {
        let mut results = HashMap::new();
        let uri = OwnedValue::try_from(Value::from("file:///tmp/s.png"))
            .unwrap_or_else(|_| OwnedValue::from(false));
        results.insert(String::from("uri"), uri);
        assert_eq!(
            uri_from_results(&results),
            Some(String::from("file:///tmp/s.png"))
        );
        let empty = HashMap::new();
        assert_eq!(uri_from_results(&empty), None);
    }
}

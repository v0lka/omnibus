//! Opaque addressing tokens: `session_id` and `element_id` are minted only by the
//! core (specs/architecture/spi-registry.md "DTO Discipline", roadmap 1.2.2).
//!
//! Enforcement model (Rust has no cross-crate private constructors, so the usual
//! `pub(crate)` trick cannot distinguish core from adapters):
//!
//! - The token types are opaque newtypes with **no** public constructor taking raw
//!   values; the compile-time negative fixture (omnibus-lints red fixture) proves
//!   `ElementId::new/from/...` does not exist.
//! - Minting requires a [`MintKey`], a per-process secret the core generates at
//!   startup from fresh entropy and never shares: adapters and tools never receive
//!   one, so only the core can mint valid tokens (runtime enforcement of the same
//!   invariant; documents stay honest about which half is compile-time).
//!
//! Tokens serialize as strings on the MCP surface but their internal structure is
//! never part of any contract.

use std::fmt;

/// 32 bytes of per-process entropy the core supplies to the minting functions.
///
/// The core creates exactly one per session from a CSPRNG and keeps it private;
/// `contracts` deliberately does not depend on an entropy source (allowlist).
#[derive(Clone, PartialEq, Eq)]
pub struct MintKey {
    bytes: [u8; 32],
}

impl std::fmt::Debug for MintKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never render key material.
        write!(f, "MintKey(<{} bytes withheld>)", self.bytes.len())
    }
}

impl MintKey {
    /// Builds the key from entropy the caller (the core) generated securely.
    /// All-zero input is rejected: a skipped entropy setup must be loud.
    pub fn from_entropy(bytes: [u8; 32]) -> Self {
        assert_ne!(bytes, [0u8; 32], "MintKey entropy must not be all zeros");
        Self { bytes }
    }
}

/// FNV-1a over the key bytes plus context, mixed per token; enough to make token
/// values unpredictable to processes that never saw the key (the trust anchor is
/// the local user, topology-and-lifecycle.md).
fn mix(key: &[u8; 32], context: &[u8], seq: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for chunk in key.iter().chain(context.iter()) {
        hash ^= u64::from(*chunk);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        hash ^= hash >> 29;
    }
    for shift in (0..8).step_by(8) {
        hash ^= u64::from((seq >> shift) as u8);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

macro_rules! opaque_token {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Mints a fresh token from the core's key plus a context string and a
            /// monotonically increasing sequence number. Only the core calls this.
            pub fn mint(key: &MintKey, context: &str, seq: u64) -> Self {
                Self(format!("{}-{:016x}", $prefix, mix(&key.bytes, context.as_bytes(), seq)))
            }
            /// The opaque string form (safe to show on the MCP surface).
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

opaque_token!(
    /// Session identity: one MCP connection, minted by the core at session start,
    /// used for audit correlation (topology-and-lifecycle.md "Session = Connection").
    SessionId,
    "sess"
);

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({})", self.0)
    }
}

impl fmt::Debug for ElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ElementId({})", self.0)
    }
}

opaque_token!(
    /// A11y element identity: minted by the core when a tree snapshot is delivered;
    /// resolving a foreign token is a typed `ElementStale` (spi-registry.md).
    ElementId,
    "elem"
);

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> MintKey {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = i as u8 + 1;
        }
        MintKey::from_entropy(bytes)
    }

    #[test]
    fn tokens_are_opaque_prefixed_and_stable() {
        let k = key();
        let s = SessionId::mint(&k, "test-session", 1);
        assert!(s.as_str().starts_with("sess-"));
        assert_eq!(s.as_str().len(), 5 + 16);
        // Same inputs -> same token (deterministic for the audit trail).
        assert_eq!(SessionId::mint(&k, "test-session", 1), s);
        // Different sequence -> different token.
        assert_ne!(SessionId::mint(&k, "test-session", 2), s);
        assert_ne!(ElementId::mint(&k, "test-session", 1).as_str(), s.as_str());
    }

    #[test]
    fn zero_entropy_key_is_rejected() {
        let result = std::panic::catch_unwind(|| MintKey::from_entropy([0u8; 32]));
        assert!(result.is_err(), "all-zero entropy must be a loud failure");
    }

    #[test]
    fn tokens_roundtrip_through_serde_as_plain_strings() {
        let k = key();
        let e = ElementId::mint(&k, "sess-abc", 7);
        let json = serde_json::to_string(&e).unwrap_or_default();
        assert_eq!(json, format!("\"{}\"", e.as_str()));
        let back: ElementId = serde_json::from_str(&json).unwrap_or_else(|_| e.clone());
        assert_eq!(back, e);
    }

    #[test]
    fn key_debug_withholds_material() {
        let rendered = format!("{:?}", key());
        assert!(!rendered.contains("0102"), "key bytes leaked: {rendered}");
    }
}

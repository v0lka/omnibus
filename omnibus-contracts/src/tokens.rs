//! Opaque addressing tokens: `session_id` and `element_id` are minted only by the
//! core (specs/architecture/spi-registry.md "DTO Discipline", roadmap 1.2.2).
//!
//! Enforcement model (Rust has no cross-crate private constructors, so the usual
//! `pub(crate)` trick cannot distinguish core from adapters):
//!
//! - The token types are opaque newtypes with no public constructor taking raw
//!   values — by review discipline only: no R4 fixture (or any other compile-time
//!   check) pins constructor absence, and both halves of the enforcement below
//!   are runtime.
//! - The derived `serde::Deserialize` accepts any string, so a foreign or stale
//!   token is not a constructor escape either: it mints nothing — it simply fails
//!   to resolve in the core, which answers with a typed `ElementStale`
//!   (spi-registry.md).
//! - Minting requires a [`MintKey`], a per-process secret the core generates at
//!   startup from fresh entropy and never shares: adapters and tools never receive
//!   one, so only the core can mint valid tokens — the runtime enforcement half.
//!   Documents stay honest about what is and is not compile-time here.
//!
//! Tokens serialize as strings on the MCP surface but their internal structure is
//! never part of any contract.

use std::fmt;

use crate::errors::ContractError;

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
    ///
    /// Precondition: `bytes` is not all zeros. This constructor is infallible and
    /// the seam is panic-free, so it cannot enforce the precondition itself:
    /// callers that cannot guarantee it must use [`MintKey::try_from_entropy`].
    /// All-zero input yields a predictable degenerate key — never install one
    /// silently; the `_unchecked` suffix marks the transferred obligation at
    /// every call site.
    pub fn from_entropy_unchecked(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    /// Checked constructor: rejects all-zero entropy with a contract error
    /// instead of accepting a predictable degenerate key.
    pub fn try_from_entropy(bytes: [u8; 32]) -> Result<Self, ContractError> {
        if bytes == [0u8; 32] {
            return Err(ContractError::Unsupported {
                reason: String::from("MintKey entropy must not be all zeros"),
            });
        }
        Ok(Self { bytes })
    }
}

/// Deterministic token derivation: FNV-1a over the key bytes plus context, mixed
/// per token with every byte of the sequence number.
///
/// What this derivation delivers is uniqueness across `(context, seq)` and
/// determinism for audit correlation — **not** unpredictability. FNV-1a is not a
/// PRF: the mix is invertible, so anyone who knows the key, the context and the
/// seq can compute every token of that context. Token values are also not secret
/// on this machine: they travel in the audit journal, which the local user can
/// already read (the trust anchor is the local user, topology-and-lifecycle.md).
/// The key's security role is minting authority — only the core ever holds one,
/// so only the core can mint tokens that resolve (specs promise tokens opacity
/// and per-instance minting, nothing more).
fn mix(key: &[u8; 32], context: &[u8], seq: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for chunk in key.iter().chain(context.iter()) {
        hash ^= u64::from(*chunk);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        hash ^= hash >> 29;
    }
    for byte in seq.to_le_bytes() {
        hash ^= u64::from(byte);
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
    use crate::errors::ContractError;

    fn key() -> MintKey {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = i as u8 + 1;
        }
        MintKey::from_entropy_unchecked(bytes)
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
    fn mint_mixes_every_byte_of_the_sequence() {
        let k = key();
        // Historical bug: only seq's lowest byte entered the mix, so 1 and 257
        // minted byte-identical tokens.
        assert_ne!(
            ElementId::mint(&k, "test-session", 1),
            ElementId::mint(&k, "test-session", 257)
        );
        // Every sequence number in a realistic session must mint a distinct token.
        let unique: std::collections::HashSet<String> = (0u64..600)
            .map(|seq| ElementId::mint(&k, "test-session", seq).as_str().to_owned())
            .collect();
        assert_eq!(
            unique.len(),
            600,
            "distinct sequence numbers must mint distinct tokens"
        );
    }

    #[test]
    fn zero_entropy_key_is_rejected() {
        let checked = MintKey::try_from_entropy([0u8; 32]);
        assert!(
            matches!(checked, Err(ContractError::Unsupported { .. })),
            "all-zero entropy must be a typed contract error, not a silent key"
        );
        assert!(MintKey::try_from_entropy([7u8; 32]).is_ok());
    }

    #[test]
    fn tokens_roundtrip_through_serde_as_plain_strings() {
        let k = key();
        let e = ElementId::mint(&k, "sess-abc", 7);
        let json = serde_json::to_string(&e).expect("token serializes");
        assert_eq!(json, format!("\"{}\"", e.as_str()));
        let back: ElementId = serde_json::from_str(&json).expect("token deserializes");
        assert_eq!(back, e);
    }

    #[test]
    fn key_debug_withholds_material() {
        let rendered = format!("{:?}", key());
        assert!(!rendered.contains("0102"), "key bytes leaked: {rendered}");
    }
}

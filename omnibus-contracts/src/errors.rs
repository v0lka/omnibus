//! The error taxonomy — the closed set of eight contract codes
//! (specs/architecture/error-taxonomy.md).
//!
//! Every SPI trait method returns a contract error; `anyhow`/`Box<dyn Error>` never
//! appear at the boundary. OS authorization failures surface as [`ContractError::NativeFailure`]
//! with the original message preserved in the payload.

use core::fmt;

/// The closed eight-code taxonomy. Removal or reshaping is a major contract version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    /// The tool exists but its capability is absent on this machine (hidden-tool direct call).
    CapabilityRequired,
    /// Capability present, write grant missing or the path is outside granted scopes.
    ScopeDenied,
    /// The platform is below the version floor.
    FloorUnsupported,
    /// The subsystem has no active adapter.
    AdapterUnavailable,
    /// The OS backend failed: code plus preserved payload of the original message.
    NativeFailure { code: i32, payload: String },
    /// The referenced UI element no longer exists; the agent re-reads the tree.
    ElementStale,
    /// Budget exhausted (reads retried once; writes are never retried).
    TimeoutError,
    /// Operation is structurally impossible on this backend; the reason names alternatives.
    Unsupported { reason: String },
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapabilityRequired => write!(f, "capability required"),
            Self::ScopeDenied => write!(
                f,
                "scope denied: grant missing or path outside granted scopes"
            ),
            Self::FloorUnsupported => write!(f, "platform below version floor"),
            Self::AdapterUnavailable => write!(f, "no active adapter for subsystem"),
            Self::NativeFailure { code, payload } => write!(f, "native failure {code}: {payload}"),
            Self::ElementStale => write!(f, "element stale: re-read the tree"),
            Self::TimeoutError => write!(f, "timeout"),
            Self::Unsupported { reason } => write!(f, "unsupported: {reason}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_failure_preserves_the_original_payload() {
        let err = ContractError::NativeFailure {
            code: 7,
            payload: String::from("original OS message"),
        };
        assert_eq!(
            err,
            ContractError::NativeFailure {
                code: 7,
                payload: String::from("original OS message")
            }
        );
        assert!(err.to_string().contains("original OS message"));
    }

    #[test]
    fn taxonomy_is_closed_over_eight_codes() {
        // Exhaustive match proves the set is closed: adding a variant breaks this test.
        let codes = [
            ContractError::CapabilityRequired,
            ContractError::ScopeDenied,
            ContractError::FloorUnsupported,
            ContractError::AdapterUnavailable,
            ContractError::NativeFailure {
                code: 0,
                payload: String::new(),
            },
            ContractError::ElementStale,
            ContractError::TimeoutError,
            ContractError::Unsupported {
                reason: String::new(),
            },
        ];
        let named: Vec<&str> = codes
            .iter()
            .map(|c| match c {
                ContractError::CapabilityRequired => "CapabilityRequired",
                ContractError::ScopeDenied => "ScopeDenied",
                ContractError::FloorUnsupported => "FloorUnsupported",
                ContractError::AdapterUnavailable => "AdapterUnavailable",
                ContractError::NativeFailure { .. } => "NativeFailure",
                ContractError::ElementStale => "ElementStale",
                ContractError::TimeoutError => "TimeoutError",
                ContractError::Unsupported { .. } => "Unsupported",
            })
            .collect();
        assert_eq!(named.len(), 8);
    }
}

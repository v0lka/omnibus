//! Out-of-band grant registry — the R5/R6 nucleus
//! (specs/architecture/security-model.md).
//!
//! Hard rules encoded here:
//! - Consent exists only out of the agent's reach: there is deliberately NO consent
//!   parameter on [`authorize`] — no MCP method accepts a consent token or field, so a
//!   "user confirmed" flag sent by an agent cannot unlock anything (R6).
//! - Every grant always carries provenance `config|cli|doctor`; the constructor makes
//!   a provenance-less grant unrepresentable (R5 audit provenance).

use omnibus_contracts::errors::ContractError;

/// Where a grant came from — the audit provenance vocabulary (R5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantProvenance {
    /// Shipped in configuration files.
    Config,
    /// Issued by a human via the CLI.
    Cli,
    /// Issued through the doctor walkthrough, confirmed by a human.
    Doctor,
}

/// A human-issued, out-of-band write grant for one capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub capability: String,
    pub provenance: GrantProvenance,
}

impl Grant {
    /// Grants are always created with provenance; there is no way to omit it.
    pub fn new(capability: impl Into<String>, provenance: GrantProvenance) -> Self {
        Self {
            capability: capability.into(),
            provenance,
        }
    }
}

/// The registry of issued grants, owned by the core.
#[derive(Debug, Clone, Default)]
pub struct GrantRegistry {
    grants: Vec<Grant>,
}

impl GrantRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a grant issued out of band (config, CLI, or doctor).
    pub fn issue(&mut self, grant: Grant) {
        self.grants.push(grant);
    }

    /// True when an out-of-band grant for the capability exists.
    pub fn has_grant(&self, capability: &str) -> bool {
        self.grants.iter().any(|g| g.capability == capability)
    }
}

/// Authorizes a write action for a capability.
///
/// Note the signature: it accepts the registry and the capability — nothing else.
/// There is no parameter an agent could set to claim consent; consent is only ever an
/// out-of-band event recorded in the registry (security-model.md "No In-Band Consent").
pub fn authorize(registry: &GrantRegistry, capability: &str) -> Result<(), ContractError> {
    if registry.has_grant(capability) {
        Ok(())
    } else {
        Err(ContractError::ScopeDenied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::capabilities::{JOURNAL_PERSISTENT, PKG_WRITE};

    #[test]
    fn grantless_write_is_denied_with_scope_denied() {
        let registry = GrantRegistry::new();
        assert_eq!(
            authorize(&registry, PKG_WRITE),
            Err(ContractError::ScopeDenied)
        );
    }

    #[test]
    fn out_of_band_grant_unlocks_the_write() {
        let mut registry = GrantRegistry::new();
        registry.issue(Grant::new(PKG_WRITE, GrantProvenance::Doctor));
        assert_eq!(authorize(&registry, PKG_WRITE), Ok(()));
    }

    #[test]
    fn every_grant_carries_provenance() {
        let mut registry = GrantRegistry::new();
        registry.issue(Grant::new(JOURNAL_PERSISTENT, GrantProvenance::Cli));
        // Provenance is part of the grant struct — R5 vocabulary config|cli|doctor.
        let grant = &registry.grants[0];
        assert_ne!(grant.provenance, GrantProvenance::Config);
    }
}

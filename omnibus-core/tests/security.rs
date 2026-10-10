//! Integration tests: security negatives from quality-gates.md (R5/R6) that the
//! skeleton can already enforce mechanically.
//!
//! The full MCP-level suite (no MCP method accepts consent; doctor never mutates)
//! grows here as the facade lands (roadmap 1.3.1, 1.7.1).

use omnibus_contracts::capabilities::PKG_WRITE;
use omnibus_contracts::errors::ContractError;
use omnibus_core::grants::{Grant, GrantProvenance, GrantRegistry, authorize};

/// R6: an agent claiming in-band confirmation stays denied — the runtime has no
/// consent API at all, so there is no field to set; the grantless call is denied
/// regardless of anything the agent sends.
#[test]
fn claimed_in_band_consent_does_not_unlock_a_write() {
    let registry = GrantRegistry::new();
    // The signature accepts no "confirmed" token: the only inputs are the registry
    // and the capability. Whatever the agent claims, grant-less is denied.
    let denial = authorize(&registry, PKG_WRITE);
    assert_eq!(denial, Err(ContractError::ScopeDenied));
}

/// The doctor path: a human-issued grant via doctor provenance unlocks the write;
/// the agent still cannot mint one itself (only humans issue out of band). The
/// provenance-presence assertion (R5) lives in the grants unit tests.
#[test]
fn doctor_issued_grant_is_out_of_band_and_authorizes() {
    let mut registry = GrantRegistry::new();
    registry.issue(Grant::new(PKG_WRITE, GrantProvenance::Doctor));
    assert_eq!(authorize(&registry, PKG_WRITE), Ok(()));
}

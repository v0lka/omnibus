//! Integration tests: security negatives from quality-gates.md (R5/R6) that the
//! skeleton can already enforce mechanically.
//!
//! The full MCP-level suite (no MCP method accepts consent; doctor never mutates)
//! grows here as the facade lands (roadmap 1.3.1, 1.7.1).

use omnibus_contracts::capabilities::PKG_WRITE;
use omnibus_contracts::errors::ContractError;
use omnibus_core::grants::{Grant, GrantProvenance, GrantRegistry, authorize};

mod common;

use common::{args, build_core};
use omnibus_contracts::capabilities as cap;
use omnibus_core::registry::Registry;
use omnibus_core::scopes::ScopeManifests;
use omnibus_core::tools::{self, GateDecision};

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

/// quality-gates.md "Security Tests": doctor over MCP never mutates the system
/// regardless of the agent's answers. The MCP doctor arm parses no arguments —
/// whatever consent-shaped fields the agent sends, the answer is the diagnosis
/// document; no opt-in grant is issued and the gated tools stay gated.
#[tokio::test]
async fn doctor_over_mcp_never_mutates_regardless_of_consent_arguments() {
    let dir = tempfile::tempdir().expect("tempdir");
    let core = build_core(
        Registry::default(),
        GrantRegistry::new(),
        ScopeManifests::default(),
        common::pipeline_for(&common::session(1), &dir.path().join("audit.jsonl")),
        common::platform(vec![]),
        1,
    );
    let args = args(&[
        ("confirm", serde_json::Value::Bool(true)),
        ("user_consented", serde_json::Value::Bool(true)),
        (
            "approved_mutations",
            serde_json::json!(["enable journal persistence"]),
        ),
    ]);
    let outcome = tools::dispatch(&core, "doctor", args)
        .await
        .expect("doctor always answers");
    // The answer is the diagnosis document (findings + ready commands), never
    // a mutation result.
    let text = outcome.value.to_string();
    assert!(
        text.contains("findings"),
        "doctor returns the diagnosis document: {text}"
    );
    // No mutation side effect: the opt-in grant was not self-issued.
    assert!(authorize(&core.grants, cap::JOURNAL_PERSISTENT).is_err());
    // The gated tool stays gated. With no capability facts the ladder answers
    // Hidden regardless of grants — this documents the hidden rung; the
    // mutation guard above (no self-issued grant) is the security assertion.
    assert!(!matches!(
        tools::gate(&core, "journal_query"),
        GateDecision::Run
    ));
    // R6, list-driven: doctor proposes nothing from the never-list. Every
    // ready command is executed by a human (no in-band confirmation surface),
    // and no proposal names a forbidden mechanism (input emulation,
    // programmatic TCC issuance, network-backend switch, systemd backports).
    const NEVER_LIST_MARKERS: [&str; 6] = [
        "ydotool",
        "uinput",
        "tccutil",
        "defaults write com.apple.TCC",
        "systemctl edit",
        "networkmanager conf",
    ];
    let report: omnibus_contracts::dto::caps::DoctorReport =
        serde_json::from_value(outcome.value).expect("doctor returns the report DTO");
    for command in &report.ready_commands {
        assert_eq!(
            command.executed_by, "human",
            "every doctor proposal is human-executed: {command:?}"
        );
        for marker in NEVER_LIST_MARKERS {
            assert!(
                !command.command.contains(marker) && !command.name.contains(marker),
                "doctor proposed a never-list mechanism ({marker}): {command:?}"
            );
        }
    }
    for finding in &report.findings {
        let text = format!(
            "{} {}",
            finding.detail,
            finding.remedy.clone().unwrap_or_default()
        );
        for marker in NEVER_LIST_MARKERS {
            assert!(
                !text.contains(marker),
                "doctor finding names a never-list mechanism ({marker}): {text}"
            );
        }
    }
}

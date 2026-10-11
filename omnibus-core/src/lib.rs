//! `omnibus-core` — the OS-agnostic core of Omnibus
//! (specs/architecture/layers-and-crates.md): MCP facade over rmcp, sessions,
//! grant checks, the audit pipeline, the capabilities document, and doctor.
//!
//! The core depends on `omnibus-contracts` only; adapters arrive through the
//! registry the composition root assembles from the probe report (P6: adapters
//! never conspire, the core never links adapter crates).

pub mod audit;
pub mod caps;
pub mod doctor;
pub mod error_map;
pub mod facade;
pub mod grants;
pub mod ladder;
pub mod limits;
pub mod registry;
pub mod scopes;
pub mod session;
pub mod tools;
pub mod write_gate;

/// Everything the facade needs, wired once per session by the composition root.
pub struct Core {
    pub session: session::SessionContext,
    pub registry: registry::Registry,
    pub grants: grants::GrantRegistry,
    pub scopes: scopes::ScopeManifests,
    pub caps: caps::CapabilitiesState,
    pub audit: audit::AuditPipeline,
    pub write_gate: write_gate::WriteGate,
    /// The fresh probe report this session was built from (a session fact:
    /// a new instance re-probes; §7.2 runtime detection wins).
    pub platform: omnibus_contracts::dto::probe::PlatformReport,
}

impl Core {
    /// Builds the core for one fresh session (probe → register → serve; the
    /// adapter FSM itself is owned by the registry slots). The platform report
    /// is a session fact: a new instance re-probes and rebuilds the document.
    pub fn new(
        session: session::SessionContext,
        registry: registry::Registry,
        grants: grants::GrantRegistry,
        scopes: scopes::ScopeManifests,
        audit: audit::AuditPipeline,
        write_gate: write_gate::WriteGate,
        platform: omnibus_contracts::dto::probe::PlatformReport,
    ) -> Self {
        let caps = caps::CapabilitiesState::assemble(&registry, &platform.clone());
        Self {
            session,
            registry,
            grants,
            scopes,
            caps,
            audit,
            write_gate,
            platform,
        }
    }
}

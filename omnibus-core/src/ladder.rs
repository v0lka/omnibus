//! The degradation ladder — the R3 nucleus
//! (specs/architecture/capability-model.md "Degradation Ladder").
//!
//! Rungs, by increasing severity:
//! 1. Degraded — tool present, schema marks the limitation.
//! 2. Missing capability — tool hidden; a direct call returns `CapabilityRequired`.
//! 3. Below version floor — the adapter is not activated at all (`FloorUnsupported`).
//! 4. Grant not issued — status opt-in-required; until issued, behavior equals rung 2.
//!
//! The ladder integration test proper arrives with roadmap task 1.3.5; this module
//! fixes the state mapping so the taxonomy and the ladder cannot drift apart.

use omnibus_contracts::capabilities::CapabilityStatus;

/// The tool-visible outcome of ladder resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderState {
    /// No rung applies: the tool is present and usable.
    Available,
    /// Rung 1: tool present, limitation visible in the schema.
    Degraded,
    /// Rung 2: capability absent — tool hidden, direct call yields `CapabilityRequired`.
    Hidden,
    /// Rung 3: platform below floor — adapter not activated.
    FloorUnsupported,
    /// Rung 4: opt-in capability without an issued grant; behaves like rung 2
    /// (`CapabilityRequired` on direct call) until the human grants it.
    OptInRequired,
}

/// Resolves the ladder for one capability: `None` means the capability has no entry
/// (absence of entry = absence of capability, capability-model.md).
///
/// Precedence follows the spec ordering by severity: floor first (rung 3), then the
/// unissued opt-in grant (rung 4), then absence (rung 2), then degradation (rung 1).
pub fn resolve(
    status: Option<CapabilityStatus>,
    floor_met: bool,
    opt_in_granted: bool,
) -> LadderState {
    if !floor_met {
        LadderState::FloorUnsupported
    } else {
        match status {
            None => LadderState::Hidden,
            Some(CapabilityStatus::OptInRequired) if !opt_in_granted => LadderState::OptInRequired,
            Some(CapabilityStatus::OptInRequired) | Some(CapabilityStatus::Present) => {
                LadderState::Available
            }
            Some(CapabilityStatus::Degraded) => LadderState::Degraded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::capabilities::CapabilityStatus::*;

    #[test]
    fn present_and_granted_is_available() {
        assert_eq!(resolve(Some(Present), true, false), LadderState::Available);
        assert_eq!(
            resolve(Some(OptInRequired), true, true),
            LadderState::Available
        );
    }

    #[test]
    fn degraded_capability_stays_present_but_marked() {
        assert_eq!(resolve(Some(Degraded), true, false), LadderState::Degraded);
    }

    #[test]
    fn missing_capability_entry_hides_the_tool() {
        assert_eq!(resolve(None, true, false), LadderState::Hidden);
    }

    #[test]
    fn below_floor_deactivates_the_adapter_first() {
        // Rung 3 wins even when the capability would otherwise be present.
        assert_eq!(
            resolve(Some(Present), false, false),
            LadderState::FloorUnsupported
        );
        assert_eq!(resolve(None, false, false), LadderState::FloorUnsupported);
    }

    #[test]
    fn unissued_opt_in_grant_behaves_like_hidden_on_direct_call() {
        assert_eq!(
            resolve(Some(OptInRequired), true, false),
            LadderState::OptInRequired
        );
    }
}

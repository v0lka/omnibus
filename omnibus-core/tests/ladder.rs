//! Integration tests: the R3 degradation ladder state mapping
//! (specs/architecture/capability-model.md; quality-gates.md R3).
//!
//! The environment-simulating ladder suite of roadmap task 1.3.5 grows here.

use omnibus_contracts::capabilities::CapabilityStatus::{Degraded, OptInRequired, Present};
use omnibus_core::ladder::{LadderState, resolve};

#[test]
fn ladder_walks_the_four_rungs() {
    // Rung 1: degraded.
    assert_eq!(resolve(Some(Degraded), true, false), LadderState::Degraded);
    // Rung 2: missing entry hides the tool.
    assert_eq!(resolve(None, true, false), LadderState::Hidden);
    // Rung 3: below floor deactivates first.
    assert_eq!(
        resolve(Some(Present), false, false),
        LadderState::FloorUnsupported
    );
    // Rung 4: opt-in without grant behaves as hidden until issued.
    assert_eq!(
        resolve(Some(OptInRequired), true, false),
        LadderState::OptInRequired
    );
}

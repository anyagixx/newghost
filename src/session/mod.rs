// FILE: src/session/mod.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Define the session core surface and re-export the pure state machine types used by later session orchestration layers.
//   SCOPE: Session module wiring, stable session identifiers, pure state-transition exports, and typed effect exports.
//   DEPENDS: std, src/session/effects.rs, src/session/state.rs
//   LINKS: M-SESSION, V-M-SESSION, DF-SESSION-EFFECTS, VF-006
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   SessionId - stable session identifier used by state and effect contracts
//   effects - typed session effects and component-targeted commands
//   effect_handler - stable top-level dispatcher over registry, timer, and metric targets
//   state - pure state machine transitions and close reasons
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added the Phase 3 pure session state machine surface with typed effect generation exports.
// END_CHANGE_SUMMARY

pub mod effect_handler;
pub mod effects;
pub mod state;

pub type SessionId = u64;

pub use effect_handler::{
    EffectHandler, MetricEffectTarget, RegistryEffectTarget, TimerEffectTarget,
};
pub use effects::{MetricEvent, RegistryCommand, SessionEffect, TimerCommand};
pub use state::{CloseReason, SessionEvent, SessionState};

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

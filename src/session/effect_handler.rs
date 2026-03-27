// FILE: src/session/effect_handler.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Route top-level SessionEffect values to exactly one target component without embedding business logic.
//   SCOPE: EffectHandler construction, target traits, single-effect apply, and ordered batch application.
//   DEPENDS: async-trait, src/session/effects.rs
//   LINKS: M-SESSION, V-M-SESSION, DF-SESSION-EFFECTS
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   RegistryEffectTarget - target contract for registry commands
//   TimerEffectTarget - target contract for timer commands
//   MetricEffectTarget - target contract for metric events
//   EffectHandler - stable top-level effect router
//   apply - route one SessionEffect to its owning target
//   apply_all - preserve effect order across a batch
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added a stable top-level effect handler with one-call routing per target component.
// END_CHANGE_SUMMARY

use async_trait::async_trait;

use crate::session::effects::{MetricEvent, RegistryCommand, SessionEffect, TimerCommand};

#[cfg(test)]
#[path = "effect_handler.test.rs"]
mod tests;

#[async_trait]
pub trait RegistryEffectTarget: Send + Sync {
    async fn execute(&self, command: RegistryCommand);
}

#[async_trait]
pub trait TimerEffectTarget: Send + Sync {
    async fn execute(&self, command: TimerCommand);
}

pub trait MetricEffectTarget: Send + Sync {
    fn emit(&self, event: MetricEvent);
}

pub struct EffectHandler<R, T, M> {
    registry: R,
    timers: T,
    metrics: M,
}

impl<R, T, M> EffectHandler<R, T, M> {
    pub fn new(registry: R, timers: T, metrics: M) -> Self {
        Self {
            registry,
            timers,
            metrics,
        }
    }
}

impl<R, T, M> EffectHandler<R, T, M>
where
    R: RegistryEffectTarget,
    T: TimerEffectTarget,
    M: MetricEffectTarget,
{
    // START_CONTRACT: apply
    //   PURPOSE: Route one top-level SessionEffect to exactly one target component.
    //   INPUTS: { effect: SessionEffect - typed effect emitted by SessionState::transition }
    //   OUTPUTS: { () - effect forwarded to its owning target }
    //   SIDE_EFFECTS: [invokes one target component method]
    //   LINKS: [M-SESSION, V-M-SESSION]
    // END_CONTRACT: apply
    pub async fn apply(&self, effect: SessionEffect) {
        match effect {
            SessionEffect::Registry(command) => self.registry.execute(command).await,
            SessionEffect::Timer(command) => self.timers.execute(command).await,
            SessionEffect::Metric(event) => self.metrics.emit(event),
        }
    }

    // START_CONTRACT: apply_all
    //   PURPOSE: Preserve effect order while forwarding a batch of top-level SessionEffect values.
    //   INPUTS: { effects: Vec<SessionEffect> - ordered effect list from the pure state machine }
    //   OUTPUTS: { () - all effects forwarded in order }
    //   SIDE_EFFECTS: [invokes one target component per effect]
    //   LINKS: [M-SESSION, V-M-SESSION]
    // END_CONTRACT: apply_all
    pub async fn apply_all(&self, effects: Vec<SessionEffect>) {
        for effect in effects {
            self.apply(effect).await;
        }
    }
}

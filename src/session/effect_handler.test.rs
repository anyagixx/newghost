use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::{EffectHandler, MetricEffectTarget, RegistryEffectTarget, TimerEffectTarget};
use crate::session::effects::{MetricEvent, RegistryCommand, SessionEffect, TimerCommand};

#[derive(Clone, Default)]
struct RegistrySpy {
    calls: Arc<Mutex<Vec<RegistryCommand>>>,
}

#[async_trait]
impl RegistryEffectTarget for RegistrySpy {
    async fn execute(&self, command: RegistryCommand) {
        self.calls
            .lock()
            .expect("registry lock poisoned")
            .push(command);
    }
}

#[derive(Clone, Default)]
struct TimerSpy {
    calls: Arc<Mutex<Vec<TimerCommand>>>,
}

#[async_trait]
impl TimerEffectTarget for TimerSpy {
    async fn execute(&self, command: TimerCommand) {
        self.calls
            .lock()
            .expect("timer lock poisoned")
            .push(command);
    }
}

#[derive(Clone, Default)]
struct MetricSpy {
    calls: Arc<Mutex<Vec<MetricEvent>>>,
}

impl MetricEffectTarget for MetricSpy {
    fn emit(&self, event: MetricEvent) {
        self.calls.lock().expect("metric lock poisoned").push(event);
    }
}

#[tokio::test]
async fn registry_effect_hits_only_registry_target() {
    let registry = RegistrySpy::default();
    let timers = TimerSpy::default();
    let metrics = MetricSpy::default();
    let handler = EffectHandler::new(registry.clone(), timers.clone(), metrics.clone());

    handler
        .apply(SessionEffect::Registry(RegistryCommand::Remove {
            session_id: 1,
        }))
        .await;

    assert_eq!(
        registry
            .calls
            .lock()
            .expect("registry lock poisoned")
            .as_slice(),
        &[RegistryCommand::Remove { session_id: 1 }]
    );
    assert!(timers.calls.lock().expect("timer lock poisoned").is_empty());
    assert!(metrics
        .calls
        .lock()
        .expect("metric lock poisoned")
        .is_empty());
}

#[tokio::test]
async fn apply_all_preserves_effect_order() {
    let registry = RegistrySpy::default();
    let timers = TimerSpy::default();
    let metrics = MetricSpy::default();
    let handler = EffectHandler::new(registry.clone(), timers.clone(), metrics.clone());

    handler
        .apply_all(vec![
            SessionEffect::Timer(TimerCommand::CancelIdle { session_id: 2 }),
            SessionEffect::Metric(MetricEvent::SessionClosing {
                session_id: 2,
                reason: "drain_shutdown",
            }),
            SessionEffect::Registry(RegistryCommand::MarkNoNewStreams { session_id: 2 }),
        ])
        .await;

    assert_eq!(
        timers.calls.lock().expect("timer lock poisoned").as_slice(),
        &[TimerCommand::CancelIdle { session_id: 2 }]
    );
    assert_eq!(
        metrics
            .calls
            .lock()
            .expect("metric lock poisoned")
            .as_slice(),
        &[MetricEvent::SessionClosing {
            session_id: 2,
            reason: "drain_shutdown",
        }]
    );
    assert_eq!(
        registry
            .calls
            .lock()
            .expect("registry lock poisoned")
            .as_slice(),
        &[RegistryCommand::MarkNoNewStreams { session_id: 2 }]
    );
}

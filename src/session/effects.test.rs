use super::{MetricEvent, RegistryCommand, SessionEffect, TimerCommand};

#[test]
fn session_effect_top_level_variant_count_stays_bounded() {
    let known_variants = [
        SessionEffect::Registry(RegistryCommand::Remove { session_id: 1 }),
        SessionEffect::Timer(TimerCommand::CancelIdle { session_id: 1 }),
        SessionEffect::Metric(MetricEvent::SessionClosed {
            session_id: 1,
            reason: "closed",
        }),
    ];

    assert!(
        known_variants.len() <= 4,
        "SessionEffect top-level routing grew beyond component targets"
    );
}

#[test]
fn registry_command_variant_count_stays_local_and_small() {
    let known_variants = [
        RegistryCommand::MarkNoNewStreams { session_id: 1 },
        RegistryCommand::CloseStreams {
            session_id: 1,
            graceful: true,
        },
        RegistryCommand::Remove { session_id: 1 },
    ];

    assert!(
        known_variants.len() <= 6,
        "RegistryCommand grew too broad for a single local component"
    );
}

#[test]
fn timer_command_variant_count_stays_local_and_small() {
    let known_variants = [
        TimerCommand::ScheduleIdle {
            session_id: 1,
            timeout: std::time::Duration::from_secs(5),
        },
        TimerCommand::CancelIdle { session_id: 1 },
    ];

    assert!(
        known_variants.len() <= 4,
        "TimerCommand grew too broad for a single local component"
    );
}

#[test]
fn metric_event_is_stable_debug_surface() {
    let event = MetricEvent::SessionClosing {
        session_id: 7,
        reason: "drain_shutdown",
    };

    assert_eq!(
        format!("{event:?}"),
        "SessionClosing { session_id: 7, reason: \"drain_shutdown\" }"
    );
}

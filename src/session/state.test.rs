// FILE: src/session/state.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify the pure session state machine, including ordered effects and one-way close semantics.
//   SCOPE: Stream open and close transitions, drain and transport-loss closing paths, deadline closure, and terminal-state behavior.
//   DEPENDS: src/session/state.rs, src/session/effects.rs
//   LINKS: V-M-SESSION, VF-010
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   stream_opened_increments_count_and_cancels_idle_timer - proves stream-open transitions update count and cancel idle timers
//   final_stream_close_schedules_idle_timer - proves final stream close schedules an idle deadline
//   drain_requested_moves_to_closing_and_stops_new_streams - proves drain requests close the session in the expected order
//   transport_lost_moves_to_closing_with_graceful_close - proves transport loss enters closing with graceful stream shutdown
//   deadline_reached_closes_and_deregisters - proves deadline expiry forces close and deregistration effects
//   closed_state_is_terminal - proves closed sessions ignore further events
//   closing_state_is_one_way_and_does_not_reopen_streams - proves closing sessions never reopen stream admission
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added GRACE markup so pure session-state verification remains explicit for autonomous debugging.
// END_CHANGE_SUMMARY

use std::time::{Duration, Instant};

use super::{CloseReason, SessionEvent, SessionState};
use crate::session::effects::{MetricEvent, RegistryCommand, SessionEffect, TimerCommand};

fn active_state(stream_count: u32) -> SessionState {
    SessionState::Active {
        since: Instant::now(),
        stream_count,
    }
}

#[test]
fn stream_opened_increments_count_and_cancels_idle_timer() {
    let (next_state, effects) =
        active_state(1).transition(11, SessionEvent::StreamOpened, Duration::from_secs(30));

    assert!(matches!(
        next_state,
        SessionState::Active {
            stream_count: 2,
            ..
        }
    ));
    assert_eq!(
        effects,
        vec![
            SessionEffect::Timer(TimerCommand::CancelIdle { session_id: 11 }),
            SessionEffect::Metric(MetricEvent::StreamOpened {
                session_id: 11,
                stream_count: 2,
            }),
        ]
    );
}

#[test]
fn final_stream_close_removes_session_immediately() {
    let idle_timeout = Duration::from_secs(45);
    let (next_state, effects) =
        active_state(1).transition(22, SessionEvent::StreamClosed, idle_timeout);

    assert_eq!(next_state, SessionState::Closed);
    assert_eq!(
        effects,
        vec![
            SessionEffect::Metric(MetricEvent::StreamClosed {
                session_id: 22,
                stream_count: 0,
            }),
            SessionEffect::Timer(TimerCommand::CancelIdle { session_id: 22 }),
            SessionEffect::Registry(RegistryCommand::Remove { session_id: 22 }),
            SessionEffect::Metric(MetricEvent::SessionClosed {
                session_id: 22,
                reason: "client_disconnect",
            }),
        ]
    );
}

#[test]
fn drain_requested_moves_to_closing_and_stops_new_streams() {
    let deadline = Instant::now() + Duration::from_secs(10);
    let (next_state, effects) = active_state(2).transition(
        33,
        SessionEvent::DrainRequested { deadline },
        Duration::from_secs(30),
    );

    assert_eq!(
        next_state,
        SessionState::Closing {
            reason: CloseReason::DrainShutdown,
            deadline,
        }
    );
    assert_eq!(
        effects,
        vec![
            SessionEffect::Registry(RegistryCommand::MarkNoNewStreams { session_id: 33 }),
            SessionEffect::Registry(RegistryCommand::CloseStreams {
                session_id: 33,
                graceful: true,
            }),
            SessionEffect::Timer(TimerCommand::CancelIdle { session_id: 33 }),
            SessionEffect::Metric(MetricEvent::SessionClosing {
                session_id: 33,
                reason: "drain_shutdown",
            }),
        ]
    );
}

#[test]
fn transport_lost_moves_to_closing_with_graceful_close() {
    let deadline = Instant::now() + Duration::from_secs(5);
    let (next_state, effects) = active_state(1).transition(
        44,
        SessionEvent::TransportLost { deadline },
        Duration::from_secs(30),
    );

    assert_eq!(
        next_state,
        SessionState::Closing {
            reason: CloseReason::TransportLost,
            deadline,
        }
    );
    assert_eq!(
        effects,
        vec![
            SessionEffect::Registry(RegistryCommand::CloseStreams {
                session_id: 44,
                graceful: true,
            }),
            SessionEffect::Metric(MetricEvent::SessionClosing {
                session_id: 44,
                reason: "transport_lost",
            }),
        ]
    );
}

#[test]
fn deadline_reached_closes_and_deregisters() {
    let deadline = Instant::now() + Duration::from_secs(2);
    let closing = SessionState::Closing {
        reason: CloseReason::DrainShutdown,
        deadline,
    };

    let (next_state, effects) =
        closing.transition(55, SessionEvent::DeadlineReached, Duration::from_secs(30));

    assert_eq!(next_state, SessionState::Closed);
    assert_eq!(
        effects,
        vec![
            SessionEffect::Registry(RegistryCommand::CloseStreams {
                session_id: 55,
                graceful: false,
            }),
            SessionEffect::Registry(RegistryCommand::Remove { session_id: 55 }),
            SessionEffect::Timer(TimerCommand::CancelIdle { session_id: 55 }),
            SessionEffect::Metric(MetricEvent::SessionClosed {
                session_id: 55,
                reason: "drain_shutdown",
            }),
        ]
    );
}

#[test]
fn closed_state_is_terminal() {
    let (next_state, effects) =
        SessionState::Closed.transition(66, SessionEvent::StreamOpened, Duration::from_secs(30));

    assert_eq!(next_state, SessionState::Closed);
    assert!(effects.is_empty());
}

#[test]
fn closing_state_is_one_way_and_does_not_reopen_streams() {
    let deadline = Instant::now() + Duration::from_secs(2);
    let closing = SessionState::Closing {
        reason: CloseReason::DrainShutdown,
        deadline,
    };

    let (next_state, effects) =
        closing.transition(77, SessionEvent::StreamOpened, Duration::from_secs(30));

    assert_eq!(
        next_state,
        SessionState::Closing {
            reason: CloseReason::DrainShutdown,
            deadline,
        }
    );
    assert!(effects.is_empty());
}

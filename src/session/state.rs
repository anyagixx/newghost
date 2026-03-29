// FILE: src/session/state.rs
// VERSION: 0.1.1
// START_MODULE_CONTRACT
//   PURPOSE: Provide the pure per-session state machine used by later session orchestration layers.
//   SCOPE: Session states, state-transition events, close reasons, and deterministic typed effect generation with no IO.
//   DEPENDS: std, src/session/effects.rs, src/session/mod.rs
//   LINKS: M-SESSION, V-M-SESSION, DF-SESSION-EFFECTS, VF-006
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   SessionState - active, closing, or closed session lifecycle state
//   SessionEvent - pure transition input emitted by the session orchestrator
//   CloseReason - stable close reasons for shutdown, idle timeout, and transport loss
//   transition - pure state machine transition with ordered typed effect generation
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Final stream close now retires the session immediately so bursty CONNECT traffic cannot leak capacity until idle timeout.
// END_CHANGE_SUMMARY

use std::time::{Duration, Instant};

use crate::session::effects::{MetricEvent, RegistryCommand, SessionEffect, TimerCommand};
use crate::session::SessionId;

#[cfg(test)]
#[path = "state.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Active {
        since: Instant,
        stream_count: u32,
    },
    Closing {
        reason: CloseReason,
        deadline: Instant,
    },
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    StreamOpened,
    StreamClosed,
    TransportLost { deadline: Instant },
    DrainRequested { deadline: Instant },
    DeadlineReached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    ClientDisconnect,
    AuthRevoked,
    DrainShutdown,
    IdleTimeout,
    TransportLost,
}

impl CloseReason {
    fn as_metric_reason(&self) -> &'static str {
        match self {
            Self::ClientDisconnect => "client_disconnect",
            Self::AuthRevoked => "auth_revoked",
            Self::DrainShutdown => "drain_shutdown",
            Self::IdleTimeout => "idle_timeout",
            Self::TransportLost => "transport_lost",
        }
    }
}

impl SessionState {
    // START_CONTRACT: transition
    //   PURPOSE: Apply one pure session event and emit deterministic typed effects for later handlers.
    //   INPUTS: { self: SessionState - current lifecycle state, session_id: SessionId - stable session identifier, event: SessionEvent - requested state transition, idle_timeout: Duration - idle scheduling hint }
    //   OUTPUTS: { (SessionState, Vec<SessionEffect>) - next lifecycle state and ordered effect list }
    //   SIDE_EFFECTS: [none]
    //   LINKS: [M-SESSION, V-M-SESSION]
    // END_CONTRACT: transition
    pub fn transition(
        self,
        session_id: SessionId,
        event: SessionEvent,
        _idle_timeout: Duration,
    ) -> (SessionState, Vec<SessionEffect>) {
        // START_BLOCK_TRANSITION_SESSION_STATE
        match (self, event) {
            (
                SessionState::Active {
                    since,
                    stream_count,
                },
                SessionEvent::StreamOpened,
            ) => {
                let next_count = stream_count.saturating_add(1);
                (
                    SessionState::Active {
                        since,
                        stream_count: next_count,
                    },
                    vec![
                        SessionEffect::Timer(TimerCommand::CancelIdle { session_id }),
                        SessionEffect::Metric(MetricEvent::StreamOpened {
                            session_id,
                            stream_count: next_count,
                        }),
                    ],
                )
            }
            (
                SessionState::Active {
                    since,
                    stream_count,
                },
                SessionEvent::StreamClosed,
            ) => {
                let next_count = stream_count.saturating_sub(1);
                let mut effects = vec![SessionEffect::Metric(MetricEvent::StreamClosed {
                    session_id,
                    stream_count: next_count,
                })];

                if next_count == 0 {
                    effects.push(SessionEffect::Timer(TimerCommand::CancelIdle { session_id }));
                    effects.push(SessionEffect::Registry(RegistryCommand::Remove { session_id }));
                    effects.push(SessionEffect::Metric(MetricEvent::SessionClosed {
                        session_id,
                        reason: CloseReason::ClientDisconnect.as_metric_reason(),
                    }));
                    (SessionState::Closed, effects)
                } else {
                    (
                        SessionState::Active {
                            since,
                            stream_count: next_count,
                        },
                        effects,
                    )
                }
            }
            (SessionState::Active { .. }, SessionEvent::TransportLost { deadline }) => (
                SessionState::Closing {
                    reason: CloseReason::TransportLost,
                    deadline,
                },
                vec![
                    SessionEffect::Registry(RegistryCommand::CloseStreams {
                        session_id,
                        graceful: true,
                    }),
                    SessionEffect::Metric(MetricEvent::SessionClosing {
                        session_id,
                        reason: CloseReason::TransportLost.as_metric_reason(),
                    }),
                ],
            ),
            (SessionState::Active { .. }, SessionEvent::DrainRequested { deadline }) => (
                SessionState::Closing {
                    reason: CloseReason::DrainShutdown,
                    deadline,
                },
                vec![
                    SessionEffect::Registry(RegistryCommand::MarkNoNewStreams { session_id }),
                    SessionEffect::Registry(RegistryCommand::CloseStreams {
                        session_id,
                        graceful: true,
                    }),
                    SessionEffect::Timer(TimerCommand::CancelIdle { session_id }),
                    SessionEffect::Metric(MetricEvent::SessionClosing {
                        session_id,
                        reason: CloseReason::DrainShutdown.as_metric_reason(),
                    }),
                ],
            ),
            (SessionState::Closing { reason, .. }, SessionEvent::DeadlineReached) => (
                SessionState::Closed,
                vec![
                    SessionEffect::Registry(RegistryCommand::CloseStreams {
                        session_id,
                        graceful: false,
                    }),
                    SessionEffect::Registry(RegistryCommand::Remove { session_id }),
                    SessionEffect::Timer(TimerCommand::CancelIdle { session_id }),
                    SessionEffect::Metric(MetricEvent::SessionClosed {
                        session_id,
                        reason: reason.as_metric_reason(),
                    }),
                ],
            ),
            (SessionState::Closed, _) => (SessionState::Closed, Vec::new()),
            (state, _) => (state, Vec::new()),
        }
        // END_BLOCK_TRANSITION_SESSION_STATE
    }
}

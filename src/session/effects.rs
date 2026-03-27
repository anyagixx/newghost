// FILE: src/session/effects.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Represent top-level typed session effects that route to exactly one target component.
//   SCOPE: SessionEffect enum, component-scoped command enums, and local enum-hygiene limits.
//   DEPENDS: std, src/session/mod.rs
//   LINKS: M-SESSION, V-M-SESSION, DF-SESSION-EFFECTS
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   SessionEffect - top-level typed effect routed by target component
//   RegistryCommand - session-registry commands emitted by pure transitions
//   TimerCommand - timer-wheel commands emitted by pure transitions
//   MetricEvent - observability events emitted by pure transitions
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added typed top-level session effects with bounded command enums for later handler routing.
// END_CHANGE_SUMMARY

use std::time::Duration;

use crate::session::SessionId;

#[cfg(test)]
#[path = "effects.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffect {
    Registry(RegistryCommand),
    Timer(TimerCommand),
    Metric(MetricEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryCommand {
    MarkNoNewStreams {
        session_id: SessionId,
    },
    CloseStreams {
        session_id: SessionId,
        graceful: bool,
    },
    Remove {
        session_id: SessionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimerCommand {
    ScheduleIdle {
        session_id: SessionId,
        timeout: Duration,
    },
    CancelIdle {
        session_id: SessionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricEvent {
    StreamOpened {
        session_id: SessionId,
        stream_count: u32,
    },
    StreamClosed {
        session_id: SessionId,
        stream_count: u32,
    },
    SessionClosing {
        session_id: SessionId,
        reason: &'static str,
    },
    SessionClosed {
        session_id: SessionId,
        reason: &'static str,
    },
}

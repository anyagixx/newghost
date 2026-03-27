use std::time::{Duration, Instant};

use super::{SessionEvent, SessionId, SessionState};

#[test]
fn session_module_reexports_state_contract() {
    let session_id: SessionId = 77;
    let (next_state, effects) = SessionState::Active {
        since: Instant::now(),
        stream_count: 0,
    }
    .transition(
        session_id,
        SessionEvent::StreamOpened,
        Duration::from_secs(15),
    );

    assert!(matches!(
        next_state,
        SessionState::Active {
            stream_count: 1,
            ..
        }
    ));
    assert_eq!(effects.len(), 2);
}

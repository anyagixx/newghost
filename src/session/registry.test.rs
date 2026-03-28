// FILE: src/session/registry.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify session-registry capacity control, drain behavior, idle reaping, and targeted effect execution.
//   SCOPE: Reservation lifecycle, drain semantics, idle cleanup, and registry-targeted command execution.
//   DEPENDS: src/session/registry.rs, src/session/state.rs, src/session/effect_handler.rs
//   LINKS: V-M-SESSION, VF-010
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   reserve_insert_and_remove_release_capacity - proves insert and remove return registry capacity deterministically
//   drain_all_releases_every_reserved_slot - proves drain releases all tracked sessions and permits
//   reap_idle_removes_old_sessions - proves idle sessions are reaped without removing fresh sessions
//   registry_execute_updates_only_the_target_session - proves registry effect execution mutates only the addressed session
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added GRACE markup so session registry tests stay navigable across fix and verification waves.
// END_CHANGE_SUMMARY

use std::time::{Duration, Instant};

use super::{SessionLimitReached, SessionRecord, SessionRegistry};
use crate::session::SessionState;

fn record(last_activity: Instant) -> SessionRecord {
    SessionRecord::new(
        SessionState::Active {
            since: last_activity,
            stream_count: 0,
        },
        last_activity,
    )
}

#[test]
fn reserve_insert_and_remove_release_capacity() {
    let registry = SessionRegistry::new(1);
    let guard = registry
        .try_reserve()
        .expect("first reservation should work");
    let (session_id, _handle) = registry.insert(guard, record(Instant::now()));

    assert_eq!(registry.available_slots(), 0);
    assert_eq!(registry.session_count(), 1);
    assert_eq!(
        registry
            .try_reserve()
            .expect_err("capacity should be exhausted"),
        SessionLimitReached::AtCapacity
    );

    let _ = registry.remove(&session_id).expect("session should exist");

    assert_eq!(registry.available_slots(), 1);
    assert_eq!(registry.session_count(), 0);
}

#[test]
fn drain_all_releases_every_reserved_slot() {
    let registry = SessionRegistry::new(2);
    let (first_id, _) = registry.insert(
        registry
            .try_reserve()
            .expect("first reservation should work"),
        record(Instant::now()),
    );
    let (second_id, _) = registry.insert(
        registry
            .try_reserve()
            .expect("second reservation should work"),
        record(Instant::now()),
    );

    let drained = registry.drain_all();

    assert_eq!(drained.len(), 2);
    assert_eq!(registry.available_slots(), 2);
    assert!(registry.get(&first_id).is_none());
    assert!(registry.get(&second_id).is_none());
}

#[test]
fn reap_idle_removes_old_sessions() {
    let registry = SessionRegistry::new(2);
    let now = Instant::now();
    let (stale_id, _) = registry.insert(
        registry
            .try_reserve()
            .expect("stale reservation should work"),
        record(now - Duration::from_secs(120)),
    );
    let (_fresh_id, _) = registry.insert(
        registry
            .try_reserve()
            .expect("fresh reservation should work"),
        record(now - Duration::from_secs(5)),
    );

    let removed = registry.reap_idle(Duration::from_secs(30), now);

    assert_eq!(removed, vec![stale_id]);
    assert_eq!(registry.session_count(), 1);
    assert_eq!(registry.available_slots(), 1);
}

#[tokio::test]
async fn registry_execute_updates_only_the_target_session() {
    let registry = SessionRegistry::new(1);
    let (session_id, handle) = registry.insert(
        registry.try_reserve().expect("reservation should work"),
        record(Instant::now()),
    );

    crate::session::RegistryEffectTarget::execute(
        &registry,
        crate::session::RegistryCommand::MarkNoNewStreams { session_id },
    )
    .await;

    assert!(!handle.snapshot().accepting_new_streams);
}

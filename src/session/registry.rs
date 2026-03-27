// FILE: src/session/registry.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Store session handles, enforce max-session capacity, and execute registry-targeted session effects.
//   SCOPE: Capacity reservation, insert/get/remove, idle reap, drain-all, and registry command execution.
//   DEPENDS: async-trait, std, tokio, src/session/effect_handler.rs, src/session/effects.rs, src/session/state.rs
//   LINKS: M-SESSION, V-M-SESSION, DF-SESSION-EFFECTS
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   SessionRecord - mutable per-session data owned by the registry
//   SessionHandle - cloneable handle for reading or mutating one session record
//   SessionRegistry - capacity-aware storage and registry-command executor
//   ReservationGuard - reserved capacity permit required before insertion
//   try_reserve - reserve one session slot without waiting
//   insert - register a session handle in the registry and consume the reservation
//   remove - deregister a session and release capacity deterministically
//   drain_all - clear all tracked sessions and release capacity deterministically
//   reap_idle - remove sessions whose last activity exceeds the configured idle bound
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added capacity-aware session storage with deterministic removal and drain semantics.
// END_CHANGE_SUMMARY

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::session::effect_handler::RegistryEffectTarget;
use crate::session::effects::RegistryCommand;
use crate::session::{CloseReason, SessionId, SessionState};

#[cfg(test)]
#[path = "registry.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub state: SessionState,
    pub accepting_new_streams: bool,
    pub last_activity: Instant,
}

impl SessionRecord {
    pub fn new(state: SessionState, last_activity: Instant) -> Self {
        Self {
            state,
            accepting_new_streams: true,
            last_activity,
        }
    }
}

#[derive(Clone)]
pub struct SessionHandle {
    inner: Arc<Mutex<SessionRecord>>,
}

impl SessionHandle {
    fn new(record: SessionRecord) -> Self {
        Self {
            inner: Arc::new(Mutex::new(record)),
        }
    }

    pub fn snapshot(&self) -> SessionRecord {
        self.inner.lock().expect("session lock poisoned").clone()
    }

    pub fn with_record<R>(&self, update: impl FnOnce(&mut SessionRecord) -> R) -> R {
        let mut guard = self.inner.lock().expect("session lock poisoned");
        update(&mut guard)
    }
}

#[derive(Debug)]
pub struct ReservationGuard {
    permit: OwnedSemaphorePermit,
}

struct StoredSession {
    handle: SessionHandle,
    permit: OwnedSemaphorePermit,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SessionLimitReached {
    #[error("session registry is at capacity")]
    AtCapacity,
}

pub struct SessionRegistry {
    sessions: Mutex<HashMap<SessionId, StoredSession>>,
    semaphore: Arc<Semaphore>,
    next_session_id: AtomicU64,
}

impl SessionRegistry {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(max_sessions)),
            next_session_id: AtomicU64::new(1),
        }
    }

    pub fn try_reserve(&self) -> Result<ReservationGuard, SessionLimitReached> {
        let permit = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| SessionLimitReached::AtCapacity)?;

        Ok(ReservationGuard { permit })
    }

    pub fn insert(
        &self,
        guard: ReservationGuard,
        record: SessionRecord,
    ) -> (SessionId, SessionHandle) {
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let handle = SessionHandle::new(record);
        let stored = StoredSession {
            handle: handle.clone(),
            permit: guard.permit,
        };

        self.sessions
            .lock()
            .expect("session registry lock poisoned")
            .insert(session_id, stored);

        (session_id, handle)
    }

    pub fn get(&self, session_id: &SessionId) -> Option<SessionHandle> {
        self.sessions
            .lock()
            .expect("session registry lock poisoned")
            .get(session_id)
            .map(|stored| stored.handle.clone())
    }

    pub fn remove(&self, session_id: &SessionId) -> Option<SessionHandle> {
        self.sessions
            .lock()
            .expect("session registry lock poisoned")
            .remove(session_id)
            .map(|stored| {
                drop(stored.permit);
                stored.handle
            })
    }

    pub fn drain_all(&self) -> Vec<SessionHandle> {
        let drained = self
            .sessions
            .lock()
            .expect("session registry lock poisoned")
            .drain()
            .map(|(_, stored)| stored)
            .collect::<Vec<_>>();

        drained
            .into_iter()
            .map(|stored| {
                drop(stored.permit);
                stored.handle
            })
            .collect()
    }

    pub fn reap_idle(&self, max_idle: Duration, now: Instant) -> Vec<SessionId> {
        let mut sessions = self
            .sessions
            .lock()
            .expect("session registry lock poisoned");
        let mut removed = Vec::new();

        sessions.retain(|session_id, stored| {
            let idle_for = now.saturating_duration_since(stored.handle.snapshot().last_activity);
            let keep = idle_for <= max_idle;
            if !keep {
                removed.push(*session_id);
            }
            keep
        });

        removed
    }

    pub fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .expect("session registry lock poisoned")
            .len()
    }

    pub fn available_slots(&self) -> usize {
        self.semaphore.available_permits()
    }

    pub fn mark_no_new_streams(&self, session_id: &SessionId) {
        if let Some(handle) = self.get(session_id) {
            handle.with_record(|record| record.accepting_new_streams = false);
        }
    }

    pub fn close_streams(&self, session_id: &SessionId, graceful: bool) {
        if let Some(handle) = self.get(session_id) {
            handle.with_record(|record| {
                record.accepting_new_streams = false;
                record.state = if graceful {
                    SessionState::Closing {
                        reason: CloseReason::DrainShutdown,
                        deadline: Instant::now(),
                    }
                } else {
                    SessionState::Closed
                };
            });
        }
    }
}

#[async_trait]
impl RegistryEffectTarget for SessionRegistry {
    async fn execute(&self, command: RegistryCommand) {
        match command {
            RegistryCommand::MarkNoNewStreams { session_id } => {
                self.mark_no_new_streams(&session_id);
            }
            RegistryCommand::CloseStreams {
                session_id,
                graceful,
            } => {
                self.close_streams(&session_id, graceful);
            }
            RegistryCommand::Remove { session_id } => {
                let _ = self.remove(&session_id);
            }
        }
    }
}

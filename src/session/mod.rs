// FILE: src/session/mod.rs
// VERSION: 0.1.3
// START_MODULE_CONTRACT
//   PURPOSE: Define the session core surface and orchestrate registry, transport selection, pure state transitions, and typed effect routing.
//   SCOPE: Session module wiring, session manager orchestration, stable session identifiers, pure state-transition exports, typed effect exports, and shutdown coordination.
//   DEPENDS: std, thiserror, tracing, src/config/mod.rs, src/session/effects.rs, src/session/state.rs, src/session/registry.rs, src/session/transport_selector.rs, src/session/effect_handler.rs
//   LINKS: M-SESSION, V-M-SESSION, DF-SESSION-EFFECTS, VF-006
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   SessionId - stable session identifier used by state and effect contracts
//   SessionManagerConfig - idle and shutdown timing derived from validated app config
//   SessionRequest - deterministic session registration input
//   SessionManager - thin orchestrator over registry, selector, state machine, and effect handler
//   SessionControl - bridge-facing contract over registration, resolution, and lifecycle events
//   register_session - reserve capacity and register a new session
//   resolve_stream - delegate transport opening then apply the state-machine stream-open event
//   handle_event - apply one pure state-machine transition and dispatch typed effects
//   shutdown - drain all sessions and close them deterministically
//   registry - session storage, capacity, drain, and registry-command execution
//   transport_selector - sequential iroh then WSS transport resolution strategy
//   effects - typed session effects and component-targeted commands
//   effect_handler - stable top-level dispatcher over registry, timer, and metric targets
//   state - pure state machine transitions and close reasons
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.3 - Added structured transport-resolution failure evidence so live verification can isolate the first divergent transport block quickly.
// END_CHANGE_SUMMARY

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::transport::adapter_contract::TransportRequest;
use crate::transport::stream::ResolvedStream;

pub mod effect_handler;
pub mod effects;
pub mod registry;
pub mod state;
pub mod transport_selector;

pub type SessionId = u64;

pub use effect_handler::{
    EffectHandler, MetricEffectTarget, RegistryEffectTarget, TimerEffectTarget,
};
pub use effects::{MetricEvent, RegistryCommand, SessionEffect, TimerCommand};
pub use registry::{
    ReservationGuard, SessionHandle, SessionLimitReached, SessionRecord, SessionRegistry,
};
pub use state::{CloseReason, SessionEvent, SessionState};
pub use transport_selector::{TransportSelectError, TransportSelector, TransportSelectorConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionManagerConfig {
    pub idle_timeout: Duration,
    pub graceful_shutdown_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRequest {
    pub started_at: Instant,
    pub peer_label: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SessionManagerError {
    #[error("session limit reached")]
    SessionLimitReached,
    #[error("session not found: {0}")]
    SessionNotFound(SessionId),
    #[error("session not accepting new streams: {0}")]
    SessionNotAcceptingNewStreams(SessionId),
    #[error("transport resolution failed: {0}")]
    TransportResolutionFailed(TransportSelectError),
}

pub struct SessionManager<I, W, T, M> {
    registry: Arc<SessionRegistry>,
    selector: TransportSelector<I, W>,
    effect_handler: EffectHandler<Arc<SessionRegistry>, T, M>,
    config: SessionManagerConfig,
}

#[async_trait]
pub trait SessionControl: Send + Sync {
    fn register_session(
        &self,
        request: &SessionRequest,
    ) -> Result<(SessionId, SessionHandle), SessionManagerError>;

    async fn resolve_stream(
        &self,
        session_id: SessionId,
        request: &TransportRequest,
        cancel: CancellationToken,
    ) -> Result<ResolvedStream, SessionManagerError>;

    async fn handle_event(
        &self,
        session_id: SessionId,
        event: SessionEvent,
    ) -> Result<(), SessionManagerError>;
}

impl SessionManagerConfig {
    pub fn from_app_config(config: &AppConfig) -> Self {
        Self {
            idle_timeout: config.timeouts.socks5_total_timeout,
            graceful_shutdown_timeout: config.timeouts.graceful_timeout,
        }
    }
}

impl<I, W, T, M> SessionManager<I, W, T, M> {
    pub fn new(
        registry: Arc<SessionRegistry>,
        selector: TransportSelector<I, W>,
        effect_handler: EffectHandler<Arc<SessionRegistry>, T, M>,
        config: SessionManagerConfig,
    ) -> Self {
        Self {
            registry,
            selector,
            effect_handler,
            config,
        }
    }
}

impl<I, W, T, M> SessionManager<I, W, T, M>
where
    I: crate::transport::adapter_contract::TransportAdapter,
    W: crate::transport::adapter_contract::TransportAdapter,
    T: TimerEffectTarget,
    M: MetricEffectTarget,
{
    // START_CONTRACT: register_session
    //   PURPOSE: Reserve session capacity and register a new Active session in the registry.
    //   INPUTS: { request: &SessionRequest - deterministic session registration input }
    //   OUTPUTS: { Result<(SessionId, SessionHandle), SessionManagerError> - registered session identifier and handle }
    //   SIDE_EFFECTS: [allocates registry capacity]
    //   LINKS: [M-SESSION, V-M-SESSION]
    // END_CONTRACT: register_session
    pub fn register_session(
        &self,
        request: &SessionRequest,
    ) -> Result<(SessionId, SessionHandle), SessionManagerError> {
        let reservation = self
            .registry
            .try_reserve()
            .map_err(|_| SessionManagerError::SessionLimitReached)?;

        Ok(self.registry.insert(
            reservation,
            SessionRecord::new(
                SessionState::Active {
                    since: request.started_at,
                    stream_count: 0,
                },
                request.started_at,
            ),
        ))
    }

    // START_CONTRACT: resolve_stream
    //   PURPOSE: Delegate transport selection and record the successful stream-open transition for one session.
    //   INPUTS: { session_id: SessionId - registered session identifier, request: &TransportRequest - stable peer label for adapter diagnostics, cancel: CancellationToken - caller cancellation boundary }
    //   OUTPUTS: { Result<ResolvedStream, SessionManagerError> - resolved transport stream or deterministic session or transport error }
    //   SIDE_EFFECTS: [emits structured transport-selection log and applies state-machine effects]
    //   LINKS: [M-SESSION, V-M-SESSION]
    // END_CONTRACT: resolve_stream
    pub async fn resolve_stream(
        &self,
        session_id: SessionId,
        request: &TransportRequest,
        cancel: CancellationToken,
    ) -> Result<ResolvedStream, SessionManagerError> {
        // START_BLOCK_SELECT_TRANSPORT
        let handle = self
            .registry
            .get(&session_id)
            .ok_or(SessionManagerError::SessionNotFound(session_id))?;

        let accepting_new_streams = handle.snapshot().accepting_new_streams;
        if !accepting_new_streams {
            return Err(SessionManagerError::SessionNotAcceptingNewStreams(
                session_id,
            ));
        }

        let resolved = match self.selector.open_stream(request, cancel).await {
            Ok(stream) => stream,
            Err(err) => {
                warn!(
                    session_id,
                    peer = %request.peer_label,
                    target_host = %request.target_host,
                    target_port = request.target_port,
                    error = %err,
                    "[SessionManager][resolveStream][BLOCK_SELECT_TRANSPORT] transport resolution failed"
                );
                return Err(SessionManagerError::TransportResolutionFailed(err));
            }
        };

        handle.with_record(|record| record.last_activity = Instant::now());
        self.handle_event(session_id, SessionEvent::StreamOpened)
            .await?;

        info!(
            session_id,
            transport_kind = ?resolved.transport_kind,
            peer = %request.peer_label,
            "[SessionManager][resolveStream][BLOCK_SELECT_TRANSPORT] resolved transport stream"
        );

        Ok(resolved)
        // END_BLOCK_SELECT_TRANSPORT
    }

    // START_CONTRACT: handle_event
    //   PURPOSE: Apply one pure state transition and forward the resulting typed effects through the effect handler.
    //   INPUTS: { session_id: SessionId - registered session identifier, event: SessionEvent - lifecycle event to apply }
    //   OUTPUTS: { Result<(), SessionManagerError> - ok when effects were applied or deterministic session-not-found error }
    //   SIDE_EFFECTS: [updates one registry-owned session record and forwards typed effects]
    //   LINKS: [M-SESSION, V-M-SESSION]
    // END_CONTRACT: handle_event
    pub async fn handle_event(
        &self,
        session_id: SessionId,
        event: SessionEvent,
    ) -> Result<(), SessionManagerError> {
        // START_BLOCK_APPLY_SESSION_EFFECTS
        let handle = self
            .registry
            .get(&session_id)
            .ok_or(SessionManagerError::SessionNotFound(session_id))?;

        let effects = handle.with_record(|record| {
            let current_state = record.state.clone();
            let (next_state, effects) =
                current_state.transition(session_id, event, self.config.idle_timeout);
            record.state = next_state;
            record.last_activity = Instant::now();
            effects
        });

        self.effect_handler.apply_all(effects).await;

        info!(
            session_id,
            "[SessionManager][handleEvent][BLOCK_APPLY_SESSION_EFFECTS] applied session effects"
        );

        Ok(())
        // END_BLOCK_APPLY_SESSION_EFFECTS
    }

    // START_CONTRACT: shutdown
    //   PURPOSE: Drain all registered sessions and force them into a closed state during shutdown.
    //   INPUTS: { none }
    //   OUTPUTS: { usize - number of drained sessions }
    //   SIDE_EFFECTS: [clears registry ownership and updates drained handles to Closed]
    //   LINKS: [M-SESSION, M-CLI]
    // END_CONTRACT: shutdown
    pub async fn shutdown(&self) -> usize {
        let drained = self.registry.drain_all();
        let drained_count = drained.len();

        for handle in drained {
            handle.with_record(|record| {
                record.accepting_new_streams = false;
                record.state = SessionState::Closed;
                record.last_activity = Instant::now();
            });
        }

        drained_count
    }
}

#[async_trait]
impl<I, W, T, M> SessionControl for SessionManager<I, W, T, M>
where
    I: crate::transport::adapter_contract::TransportAdapter + Send + Sync,
    W: crate::transport::adapter_contract::TransportAdapter + Send + Sync,
    T: TimerEffectTarget + Send + Sync,
    M: MetricEffectTarget + Send + Sync,
{
    fn register_session(
        &self,
        request: &SessionRequest,
    ) -> Result<(SessionId, SessionHandle), SessionManagerError> {
        SessionManager::register_session(self, request)
    }

    async fn resolve_stream(
        &self,
        session_id: SessionId,
        request: &TransportRequest,
        cancel: CancellationToken,
    ) -> Result<ResolvedStream, SessionManagerError> {
        SessionManager::resolve_stream(self, session_id, request, cancel).await
    }

    async fn handle_event(
        &self,
        session_id: SessionId,
        event: SessionEvent,
    ) -> Result<(), SessionManagerError> {
        SessionManager::handle_event(self, session_id, event).await
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

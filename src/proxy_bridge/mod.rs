// FILE: src/proxy_bridge/mod.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Consume ProxyIntent work items, resolve generic transport streams through SessionManager, and relay bytes bidirectionally with bounded request lifetime and drain behavior.
//   SCOPE: Worker loop, per-intent orchestration, bidirectional stream pumping, active-task drain, and session lifecycle notification.
//   DEPENDS: std, thiserror, tokio, tokio-util, tracing, src/socks5/mod.rs, src/session/mod.rs, src/transport/stream.rs
//   LINKS: M-PROXY-BRIDGE, V-M-PROXY-BRIDGE, DF-SOCKS5-REQUEST, DF-SHUTDOWN
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   ProxyBridgeConfig - pump buffer and total timeout settings
//   ProxyBridge - queue-driven bridge worker and drain coordinator
//   ProxyResult - success or failure outcome for one proxied request
//   run_worker - consume ProxyIntent items until drain is requested
//   pump_bidirectional - relay bytes between local proxy socket and resolved generic stream
//   drain_all - stop new bridge work and wait for active pumps to complete
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added the Phase 5 bridge worker over ProxyIntent and generic transport streams.
// END_CHANGE_SUMMARY

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{info, warn};

use crate::session::{SessionControl, SessionEvent, SessionManagerError, SessionRequest};
use crate::socks5::{ProxyError, ProxyIntent, Socks5Proxy, Socks5Reply};
use crate::transport::adapter_contract::TransportRequest;
use crate::transport::stream::ResolvedStream;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyBridgeConfig {
    pub pump_buffer_bytes: usize,
    pub total_request_timeout: Duration,
}

#[derive(Debug, Error)]
pub enum ProxyResult {
    #[error("proxy request completed")]
    Completed,
    #[error("proxy request failed: {0}")]
    Failed(#[from] ProxyError),
}

pub struct ProxyBridge<S> {
    config: ProxyBridgeConfig,
    session: Arc<S>,
    drain_token: CancellationToken,
    task_tracker: Arc<TaskTracker>,
}

impl<S> Clone for ProxyBridge<S> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            session: self.session.clone(),
            drain_token: self.drain_token.clone(),
            task_tracker: self.task_tracker.clone(),
        }
    }
}

impl<S> ProxyBridge<S> {
    pub fn new(config: ProxyBridgeConfig, session: Arc<S>) -> Self {
        Self {
            config,
            session,
            drain_token: CancellationToken::new(),
            task_tracker: Arc::new(TaskTracker::new()),
        }
    }

    pub fn stop_accept(&self) {
        self.drain_token.cancel();
    }
}

impl<S> ProxyBridge<S>
where
    S: SessionControl + 'static,
{
    pub async fn run_worker(&self, mut intent_rx: mpsc::Receiver<ProxyIntent>) {
        loop {
            tokio::select! {
                _ = self.drain_token.cancelled() => break,
                maybe_intent = intent_rx.recv() => {
                    let Some(intent) = maybe_intent else {
                        break;
                    };
                    let bridge = self.clone();
                    let task_tracker = self.task_tracker.clone();
                    task_tracker.spawn(async move {
                        let _ = bridge.process_intent(intent).await;
                    });
                }
            }
        }
    }

    async fn process_intent(&self, mut intent: ProxyIntent) -> ProxyResult {
        let session_id = match self.session.register_session(&SessionRequest {
            started_at: std::time::Instant::now(),
            peer_label: format!("request-{}", intent.request_id),
        }) {
            Ok((session_id, _)) => session_id,
            Err(SessionManagerError::SessionLimitReached) => {
                let _ = Socks5Proxy::send_reply_and_close(
                    &mut intent.client_stream,
                    Socks5Reply::GeneralFailure,
                )
                .await;
                return ProxyResult::Failed(ProxyError::SessionLimitReached);
            }
            Err(err) => {
                let _ = Socks5Proxy::send_reply_and_close(
                    &mut intent.client_stream,
                    Socks5Reply::GeneralFailure,
                )
                .await;
                return ProxyResult::Failed(ProxyError::TransportFailed(err.to_string()));
            }
        };

        let resolve_request = TransportRequest {
            peer_label: format!("request-{}", intent.request_id),
        };
        let cancel = self.drain_token.child_token();
        let resolved = match tokio::time::timeout(
            self.config.total_request_timeout,
            self.session
                .resolve_stream(session_id, &resolve_request, cancel),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => {
                let proxy_err = map_session_error(err);
                if let Some(reply) = Socks5Proxy::map_reply(&proxy_err) {
                    let _ =
                        Socks5Proxy::send_reply_and_close(&mut intent.client_stream, reply).await;
                }
                let _ = self
                    .session
                    .handle_event(session_id, SessionEvent::DeadlineReached)
                    .await;
                return ProxyResult::Failed(proxy_err);
            }
            Err(_) => {
                let proxy_err = ProxyError::TransportFailed("request timeout exceeded".to_string());
                if let Some(reply) = Socks5Proxy::map_reply(&proxy_err) {
                    let _ =
                        Socks5Proxy::send_reply_and_close(&mut intent.client_stream, reply).await;
                }
                let _ = self
                    .session
                    .handle_event(session_id, SessionEvent::DeadlineReached)
                    .await;
                return ProxyResult::Failed(proxy_err);
            }
        };

        let _ = Socks5Proxy::send_reply(&mut intent.client_stream, Socks5Reply::Succeeded).await;

        match self
            .pump_bidirectional(session_id, intent.client_stream, resolved)
            .await
        {
            Ok(()) => ProxyResult::Completed,
            Err(err) => ProxyResult::Failed(err),
        }
    }

    // START_CONTRACT: pump_bidirectional
    //   PURPOSE: Relay bytes between the local proxy socket and a resolved generic stream until either side closes.
    //   INPUTS: { session_id: u64 - registered session identifier, client_stream: TcpStream - local proxy socket, resolved: ResolvedStream - generic remote stream }
    //   OUTPUTS: { Result<(), ProxyError> - completion or post-reply failure }
    //   SIDE_EFFECTS: [moves bytes in both directions, shuts streams down, and notifies session lifecycle completion]
    //   LINKS: [M-PROXY-BRIDGE, V-M-PROXY-BRIDGE]
    // END_CONTRACT: pump_bidirectional
    pub async fn pump_bidirectional(
        &self,
        session_id: u64,
        mut client_stream: tokio::net::TcpStream,
        resolved: ResolvedStream,
    ) -> Result<(), ProxyError> {
        // START_BLOCK_PUMP_BIDIRECTIONAL
        let peer_label = resolved.stream.peer_label().to_string();
        let (mut transport_reader, mut transport_writer) = resolved.stream.split();
        let mut client_buffer = vec![0_u8; self.config.pump_buffer_bytes];
        let mut transport_buffer = vec![0_u8; self.config.pump_buffer_bytes];

        loop {
            tokio::select! {
                read_client = client_stream.read(&mut client_buffer) => {
                    match read_client {
                        Ok(0) => break,
                        Ok(bytes_read) => {
                            transport_writer.write_all(&client_buffer[..bytes_read]).await
                                .map_err(ProxyError::PumpFailed)?;
                        }
                        Err(err) => return Err(ProxyError::PumpFailed(err)),
                    }
                }
                read_transport = transport_reader.read(&mut transport_buffer) => {
                    match read_transport {
                        Ok(0) => break,
                        Ok(bytes_read) => {
                            client_stream.write_all(&transport_buffer[..bytes_read]).await
                                .map_err(ProxyError::PumpFailed)?;
                        }
                        Err(err) => return Err(ProxyError::PumpFailed(err)),
                    }
                }
                _ = self.drain_token.cancelled() => {
                    warn!(session_id, peer = %peer_label, "[ProxyBridge][pumpBidirectional][BLOCK_PUMP_BIDIRECTIONAL] bridge drain requested");
                    break;
                }
            }
        }

        let _ = self
            .session
            .handle_event(session_id, SessionEvent::StreamClosed)
            .await;
        info!(
            session_id,
            peer = %peer_label,
            "[ProxyBridge][pumpBidirectional][BLOCK_PUMP_BIDIRECTIONAL] bridge pump completed"
        );
        Ok(())
        // END_BLOCK_PUMP_BIDIRECTIONAL
    }

    pub async fn drain_all(&self) {
        self.drain_token.cancel();
        self.task_tracker.close();
        self.task_tracker.wait().await;
    }
}

fn map_session_error(error: SessionManagerError) -> ProxyError {
    match error {
        SessionManagerError::SessionLimitReached => ProxyError::SessionLimitReached,
        SessionManagerError::SessionNotFound(_) => {
            ProxyError::TransportFailed("session not found".to_string())
        }
        SessionManagerError::TransportResolutionFailed(err) => {
            ProxyError::TransportFailed(err.to_string())
        }
    }
}

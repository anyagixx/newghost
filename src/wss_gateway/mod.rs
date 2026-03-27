// FILE: src/wss_gateway/mod.rs
// VERSION: 0.1.2
// START_MODULE_CONTRACT
//   PURPOSE: Create WSS-backed transport streams by composing TCP, TLS, websocket upgrade, auth validation, and remote target relay under the shared adapter contract without owning transport selection logic.
//   SCOPE: Outbound WSS open-stream behavior, inbound WSS server loop, target-connect relay, adapter-scoped task tracking, and cleanup-sensitive shutdown paths.
//   DEPENDS: std, async-trait, futures-util, http, thiserror, tokio, tokio-tungstenite, tokio-util, tracing, src/tls/mod.rs, src/auth/mod.rs, src/obs/mod.rs, src/transport/*
//   LINKS: M-WSS-GATEWAY, M-TLS, M-AUTH, M-OBS, V-M-WSS-GATEWAY, DF-WSS-HANDSHAKE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   GatewayConfig - typed WSS gateway configuration for client and server roles
//   WssGateway - WSS transport adapter and server boundary
//   WssError - deterministic open-stream and server-loop errors
//   run_server - start the remote WSS listener and validate auth handshakes
//   open_stream - establish an outbound WSS-backed resolved stream
//   task_tracker - expose adapter-scoped task tracking
//   stop_accept - stop the accept loop during shutdown
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.2 - Replaced the placeholder echo server path with a target-aware TCP relay so WSS streams can proxy real outbound traffic.
// END_CHANGE_SUMMARY

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use http::Uri;
use thiserror::Error;
use tokio::io::{duplex, split, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tokio_tungstenite::{accept_async, client_async, WebSocketStream};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::auth::{AuthDecision, AuthPolicy, HandshakeMetadata};
use crate::obs::ProxyMetricsHandle;
use crate::tls::TlsContextHandle;
use crate::transport::adapter_contract::{TransportAdapter, TransportRequest};
use crate::transport::stream::{
    BoxedRead, BoxedWrite, ResolvedStream, ShutdownError, TransportKind, TransportStream,
};
use crate::transport::task_tracker::AdapterTaskTracker;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

#[derive(Clone)]
pub struct GatewayConfig {
    pub server_addr: std::net::SocketAddr,
    pub server_name: String,
    pub websocket_uri: Uri,
    pub auth_token: String,
    pub tls_context: TlsContextHandle,
    pub auth_policy: AuthPolicy,
    pub metrics: ProxyMetricsHandle,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WssError {
    #[error("operation cancelled")]
    Cancelled,
    #[error("tcp connect failed: {0}")]
    TcpConnectFailed(String),
    #[error("websocket upgrade failed: {0}")]
    UpgradeFailed(String),
    #[error("handshake failed: {0}")]
    HandshakeFailed(String),
    #[error("auth rejected: {0}")]
    AuthRejected(String),
    #[error("invalid websocket uri: {0}")]
    InvalidWebsocketUri(String),
    #[error("target connect failed: {0}")]
    TargetConnectFailed(String),
    #[error("invalid target request: {0}")]
    InvalidTargetRequest(String),
}

#[derive(Clone)]
pub struct WssGateway {
    config: GatewayConfig,
    task_tracker: Arc<AdapterTaskTracker>,
    accept_token: CancellationToken,
}

struct BridgeLifecycle {
    active_tasks: AtomicUsize,
    notify: Notify,
}

struct WssTransportStream {
    stream: DuplexStream,
    peer_label: String,
    shutdown: CancellationToken,
    lifecycle: Arc<BridgeLifecycle>,
}

impl WssGateway {
    pub fn new(config: GatewayConfig) -> Self {
        Self {
            config,
            task_tracker: Arc::new(AdapterTaskTracker::new("wss")),
            accept_token: CancellationToken::new(),
        }
    }

    pub fn task_tracker(&self) -> &AdapterTaskTracker {
        self.task_tracker.as_ref()
    }

    pub fn stop_accept(&self) {
        self.accept_token.cancel();
    }

    // START_CONTRACT: run_server
    //   PURPOSE: Start the remote WSS listener and wrap accepted sessions into transport streams.
    //   INPUTS: { listener: TcpListener - bound TCP listener used for TLS and websocket accept }
    //   OUTPUTS: { Result<(), WssError> - server loop termination status }
    //   SIDE_EFFECTS: [accepts TCP connections, performs TLS and websocket handshakes, validates auth, connects to the requested target, and relays bytes]
    //   LINKS: [M-WSS-GATEWAY, M-TLS, M-AUTH]
    // END_CONTRACT: run_server
    pub async fn run_server(&self, listener: TcpListener) -> Result<(), WssError> {
        let gateway = self.clone();
        let accept_token = self.accept_token.clone();
        let server_task = self.task_tracker.spawn(async move {
            loop {
                tokio::select! {
                    _ = accept_token.cancelled() => break Ok(()),
                    accepted = listener.accept() => {
                        let (stream, peer_addr) = accepted
                            .map_err(|err| WssError::TcpConnectFailed(err.to_string()))?;
                        let connection_gateway = gateway.clone();
                        let task_tracker = connection_gateway.task_tracker.clone();
                        task_tracker.spawn(async move {
                            if let Err(err) = connection_gateway
                                .handle_accepted(stream, peer_addr.to_string())
                                .await
                            {
                                warn!(peer = %peer_addr, error = %err, "wss server connection ended with error");
                            }
                        });
                    }
                }
            }
        });

        server_task
            .await
            .map_err(|err| WssError::HandshakeFailed(err.to_string()))?
    }

    async fn handle_accepted(&self, stream: TcpStream, peer_label: String) -> Result<(), WssError> {
        let tls_stream = self
            .config
            .tls_context
            .accept(stream)
            .await
            .map_err(|err| WssError::HandshakeFailed(err.to_string()))?;
        let mut websocket = accept_async(tls_stream)
            .await
            .map_err(|err| WssError::UpgradeFailed(err.to_string()))?;

        let auth_message = websocket
            .next()
            .await
            .ok_or_else(|| WssError::HandshakeFailed("missing auth message".to_string()))?
            .map_err(|err| WssError::HandshakeFailed(err.to_string()))?;

        let credentials = match auth_message {
            Message::Binary(bytes) => bytes.to_vec(),
            Message::Text(text) => text.to_string().into_bytes(),
            Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {
                return Err(WssError::HandshakeFailed(
                    "unsupported auth message type".to_string(),
                ));
            }
        };

        match self
            .config
            .auth_policy
            .validate_handshake(&HandshakeMetadata {
                credentials,
                peer_label: peer_label.clone(),
            }) {
            AuthDecision::Allow(_) => {
                websocket
                    .send(Message::Text("ok".into()))
                    .await
                    .map_err(|err| WssError::HandshakeFailed(err.to_string()))?;
                info!(
                    peer = %peer_label,
                    "[WssGateway][openStream][BLOCK_ADAPTER_CLEANUP_CONTRACT] accepted WSS handshake"
                );
                self.server_proxy_loop(websocket).await
            }
            AuthDecision::Reject(rejection) => {
                let _ = websocket.close(None).await;
                Err(WssError::AuthRejected(rejection.redacted_detail))
            }
        }
    }

    async fn server_proxy_loop<S>(&self, mut websocket: WebSocketStream<S>) -> Result<(), WssError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let target_message = websocket
            .next()
            .await
            .ok_or_else(|| WssError::InvalidTargetRequest("missing target request".to_string()))?
            .map_err(|err| WssError::UpgradeFailed(err.to_string()))?;
        let (target_host, target_port) = parse_target_request(target_message)?;
        let target_stream = TcpStream::connect((target_host.as_str(), target_port))
            .await
            .map_err(|err| WssError::TargetConnectFailed(err.to_string()))?;

        websocket
            .send(Message::Text("connected".into()))
            .await
            .map_err(|err| WssError::UpgradeFailed(err.to_string()))?;

        let (mut ws_sink, mut ws_stream) = websocket.split();
        let (mut target_reader, mut target_writer) = target_stream.into_split();

        let websocket_to_target = async {
            while let Some(message) = ws_stream.next().await {
                match message.map_err(|err| WssError::UpgradeFailed(err.to_string()))? {
                    Message::Binary(bytes) => {
                        target_writer
                            .write_all(&bytes)
                            .await
                            .map_err(|err| WssError::TargetConnectFailed(err.to_string()))?;
                    }
                    Message::Text(text) => {
                        target_writer
                            .write_all(text.as_bytes())
                            .await
                            .map_err(|err| WssError::TargetConnectFailed(err.to_string()))?;
                    }
                    Message::Close(_) => {
                        let _ = target_writer.shutdown().await;
                        break;
                    }
                    Message::Ping(_) => {}
                    Message::Pong(_) | Message::Frame(_) => {}
                }
            }

            Ok::<(), WssError>(())
        };

        let target_to_websocket = async {
            let mut buffer = [0_u8; 8192];
            loop {
                let bytes_read = target_reader
                    .read(&mut buffer)
                    .await
                    .map_err(|err| WssError::TargetConnectFailed(err.to_string()))?;
                if bytes_read == 0 {
                    let _ = ws_sink.send(Message::Close(None)).await;
                    break;
                }

                ws_sink
                    .send(Message::Binary(buffer[..bytes_read].to_vec().into()))
                    .await
                    .map_err(|err| WssError::UpgradeFailed(err.to_string()))?;
            }

            Ok::<(), WssError>(())
        };

        tokio::try_join!(websocket_to_target, target_to_websocket)?;
        Ok(())
    }

    async fn connect_websocket(
        &self,
    ) -> Result<WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>, WssError> {
        let tcp_stream = TcpStream::connect(self.config.server_addr)
            .await
            .map_err(|err| WssError::TcpConnectFailed(err.to_string()))?;
        let tls_stream = self
            .config
            .tls_context
            .connect(tcp_stream, &self.config.server_name)
            .await
            .map_err(|err| WssError::HandshakeFailed(err.to_string()))?;

        let request = self
            .config
            .websocket_uri
            .to_string()
            .into_client_request()
            .map_err(|err| WssError::InvalidWebsocketUri(err.to_string()))?;

        let (websocket, _) = client_async(request, tls_stream)
            .await
            .map_err(|err| WssError::UpgradeFailed(err.to_string()))?;

        Ok(websocket)
    }

    fn spawn_bridge_tasks<S>(
        &self,
        websocket: WebSocketStream<S>,
        bridge_stream: DuplexStream,
        shutdown: CancellationToken,
        lifecycle: Arc<BridgeLifecycle>,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut ws_sink, mut ws_stream) = websocket.split();
        let (mut bridge_reader, mut bridge_writer) = split(bridge_stream);
        let write_shutdown = shutdown.clone();
        let write_lifecycle = lifecycle.clone();

        self.task_tracker.spawn(async move {
            let mut buffer = [0_u8; 8192];
            loop {
                tokio::select! {
                    _ = write_shutdown.cancelled() => {
                        let _ = ws_sink.send(Message::Close(None)).await;
                        break;
                    }
                    read_result = bridge_reader.read(&mut buffer) => {
                        match read_result {
                            Ok(0) => {
                                let _ = ws_sink.send(Message::Close(None)).await;
                                break;
                            }
                            Ok(bytes_read) => {
                                if ws_sink.send(Message::Binary(buffer[..bytes_read].to_vec().into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
            finish_bridge_task(&write_lifecycle);
        });

        let read_shutdown = shutdown;
        self.task_tracker.spawn(async move {
            loop {
                tokio::select! {
                    _ = read_shutdown.cancelled() => {
                        let _ = bridge_writer.shutdown().await;
                        break;
                    }
                    frame = ws_stream.next() => {
                        match frame {
                            Some(Ok(Message::Binary(bytes))) => {
                                if bridge_writer.write_all(&bytes).await.is_err() {
                                    break;
                                }
                            }
                            Some(Ok(Message::Text(text))) => {
                                if bridge_writer.write_all(text.as_str().as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                            Some(Ok(Message::Close(_))) | None => {
                                let _ = bridge_writer.shutdown().await;
                                break;
                            }
                            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                            Some(Ok(Message::Frame(_))) => {}
                            Some(Err(_)) => {
                                let _ = bridge_writer.shutdown().await;
                                break;
                            }
                        }
                    }
                }
            }
            finish_bridge_task(&lifecycle);
        });
    }
}

#[async_trait]
impl TransportAdapter for WssGateway {
    type Error = WssError;

    // START_CONTRACT: open_stream
    //   PURPOSE: Establish an outbound WSS-backed transport stream.
    //   INPUTS: { request: &TransportRequest - stable peer label request metadata, cancel: CancellationToken - cancellation boundary for open_stream }
    //   OUTPUTS: { Result<ResolvedStream, WssError> - WSS-backed resolved stream or deterministic error }
    //   SIDE_EFFECTS: [performs TCP, TLS, websocket handshake, auth exchange, and tracked bridge task spawn]
    //   LINKS: [M-WSS-GATEWAY, V-M-WSS-GATEWAY]
    // END_CONTRACT: open_stream
    async fn open_stream(
        &self,
        request: &TransportRequest,
        cancel: CancellationToken,
    ) -> Result<ResolvedStream, Self::Error> {
        // START_BLOCK_ADAPTER_CLEANUP_CONTRACT
        if cancel.is_cancelled() {
            warn!(
                peer = %request.peer_label,
                "[WssGateway][openStream][BLOCK_ADAPTER_CLEANUP_CONTRACT] cancelled before connect"
            );
            return Err(WssError::Cancelled);
        }

        let mut websocket = tokio::select! {
            _ = cancel.cancelled() => Err(WssError::Cancelled),
            websocket = self.connect_websocket() => websocket,
        }?;

        websocket
            .send(Message::Binary(
                self.config.auth_token.as_bytes().to_vec().into(),
            ))
            .await
            .map_err(|err| WssError::HandshakeFailed(err.to_string()))?;

        let auth_ack = tokio::select! {
            _ = cancel.cancelled() => Err(WssError::Cancelled),
            next = websocket.next() => {
                next.ok_or_else(|| WssError::HandshakeFailed("missing auth ack".to_string()))
                    .and_then(|frame| frame.map_err(|err| WssError::HandshakeFailed(err.to_string())))
            }
        }?;

        match auth_ack {
            Message::Text(text) if text == "ok" => {}
            Message::Binary(bytes) if bytes.as_ref() == b"ok" => {}
            other @ Message::Text(_)
            | other @ Message::Binary(_)
            | other @ Message::Ping(_)
            | other @ Message::Pong(_)
            | other @ Message::Close(_)
            | other @ Message::Frame(_) => {
                return Err(WssError::AuthRejected(format!(
                    "unexpected auth ack: {other:?}"
                )))
            }
        }

        websocket
            .send(Message::Text(
                format!("CONNECT {} {}", request.target_host, request.target_port).into(),
            ))
            .await
            .map_err(|err| WssError::HandshakeFailed(err.to_string()))?;

        let target_ack = tokio::select! {
            _ = cancel.cancelled() => Err(WssError::Cancelled),
            next = websocket.next() => {
                next.ok_or_else(|| WssError::HandshakeFailed("missing target ack".to_string()))
                    .and_then(|frame| frame.map_err(|err| WssError::HandshakeFailed(err.to_string())))
            }
        }?;

        match target_ack {
            Message::Text(text) if text == "connected" => {}
            Message::Binary(bytes) if bytes.as_ref() == b"connected" => {}
            Message::Text(text) => return Err(WssError::TargetConnectFailed(text.to_string())),
            Message::Binary(bytes) => {
                return Err(WssError::TargetConnectFailed(
                    String::from_utf8_lossy(&bytes).to_string(),
                ))
            }
            Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {
                return Err(WssError::HandshakeFailed(
                    "unexpected target ack message type".to_string(),
                ))
            }
        }

        let (local_stream, bridge_stream) = duplex(32 * 1024);
        let lifecycle = Arc::new(BridgeLifecycle {
            active_tasks: AtomicUsize::new(2),
            notify: Notify::new(),
        });
        let shutdown = cancel.child_token();
        self.spawn_bridge_tasks(
            websocket,
            bridge_stream,
            shutdown.clone(),
            lifecycle.clone(),
        );
        self.config.metrics.increment_intents_enqueued();

        info!(
            peer = %request.peer_label,
            "[WssGateway][openStream][BLOCK_ADAPTER_CLEANUP_CONTRACT] established WSS transport stream"
        );

        Ok(ResolvedStream {
            stream: Box::new(WssTransportStream {
                stream: local_stream,
                peer_label: request.peer_label.clone(),
                shutdown,
                lifecycle,
            }),
            transport_kind: TransportKind::Wss,
        })
        // END_BLOCK_ADAPTER_CLEANUP_CONTRACT
    }

    fn task_tracker(&self) -> &AdapterTaskTracker {
        self.task_tracker()
    }
}

fn parse_target_request(message: Message) -> Result<(String, u16), WssError> {
    let text = match message {
        Message::Text(text) => text.to_string(),
        Message::Binary(bytes) => {
            String::from_utf8(bytes.to_vec()).map_err(|_| {
                WssError::InvalidTargetRequest("target request must be valid UTF-8".to_string())
            })?
        }
        Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {
            return Err(WssError::InvalidTargetRequest(
                "unsupported target request message type".to_string(),
            ))
        }
    };

    let mut parts = text.splitn(3, ' ');
    let method = parts.next().unwrap_or_default();
    let host = parts.next().unwrap_or_default();
    let port = parts.next().unwrap_or_default();

    if method != "CONNECT" || host.trim().is_empty() || port.trim().is_empty() {
        return Err(WssError::InvalidTargetRequest(text));
    }

    let port = port
        .parse::<u16>()
        .map_err(|_| WssError::InvalidTargetRequest(text.clone()))?;

    Ok((host.to_string(), port))
}

fn finish_bridge_task(lifecycle: &Arc<BridgeLifecycle>) {
    if lifecycle.active_tasks.fetch_sub(1, Ordering::SeqCst) == 1 {
        lifecycle.notify.notify_waiters();
    }
}

#[async_trait]
impl TransportStream for WssTransportStream {
    fn split(self: Box<Self>) -> (BoxedRead, BoxedWrite) {
        let stream = self.stream;
        let (read_half, write_half) = split(stream);
        (Box::pin(read_half), Box::pin(write_half))
    }

    fn peer_label(&self) -> &str {
        &self.peer_label
    }

    async fn shutdown(mut self: Box<Self>, timeout: Duration) -> Result<(), ShutdownError> {
        self.shutdown.cancel();
        let _ = self.stream.shutdown().await;
        if self.lifecycle.active_tasks.load(Ordering::SeqCst) == 0 {
            return Ok(());
        }

        tokio::time::timeout(timeout, async {
            while self.lifecycle.active_tasks.load(Ordering::SeqCst) > 0 {
                self.lifecycle.notify.notified().await;
            }
        })
        .await
        .map_err(|_| ShutdownError::Timeout)
    }
}

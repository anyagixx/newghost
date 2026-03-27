// FILE: src/iroh_adapter/mod.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Open iroh direct or relay-backed transport streams under the shared adapter contract and clean up partial state on cancel or error.
//   SCOPE: Endpoint lifecycle, outbound iroh stream establishment, tracked bridge tasks, and deterministic release behavior.
//   DEPENDS: std, async-trait, iroh, thiserror, tokio, tokio-util, tracing, src/obs/mod.rs, src/transport/*
//   LINKS: M-IROH-ADAPTER, M-OBS, V-M-IROH-ADAPTER, DF-TRANSPORT-FALLBACK
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   IrohAdapterConfig - typed endpoint, address, and ALPN input
//   IrohAdapter - iroh transport adapter over the shared TransportAdapter contract
//   IrohError - deterministic connect, stream-open, and release failures
//   open_stream - establish an iroh-backed resolved stream
//   task_tracker - expose adapter-scoped task tracking
//   release - close the owned endpoint and wait for tracked tasks
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Created Phase 2 iroh adapter with tracked stream bridging, release handling, and tests.
// END_CHANGE_SUMMARY

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::{Endpoint, EndpointAddr};
use thiserror::Error;
use tokio::io::{duplex, split, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::obs::ProxyMetricsHandle;
use crate::transport::adapter_contract::{TransportAdapter, TransportRequest};
use crate::transport::stream::{
    BoxedRead, BoxedWrite, ResolvedStream, ShutdownError, TransportKind, TransportStream,
};
use crate::transport::task_tracker::AdapterTaskTracker;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

pub(crate) const IROH_ALPN_MARKER: u8 = 0x7f;

#[derive(Clone)]
pub struct IrohAdapterConfig {
    pub endpoint: Endpoint,
    pub remote_addr: EndpointAddr,
    pub alpn: Vec<u8>,
    pub metrics: ProxyMetricsHandle,
}

#[derive(Clone)]
pub struct IrohAdapter {
    config: IrohAdapterConfig,
    task_tracker: Arc<AdapterTaskTracker>,
}

struct BridgeLifecycle {
    active_tasks: AtomicUsize,
    notify: Notify,
}

struct IrohTransportStream {
    stream: DuplexStream,
    peer_label: String,
    shutdown: CancellationToken,
    lifecycle: Arc<BridgeLifecycle>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IrohError {
    #[error("operation cancelled")]
    Cancelled,
    #[error("transport unavailable: {0}")]
    TransportUnavailable(String),
    #[error("stream open failed: {0}")]
    StreamOpenFailed(String),
    #[error("contract violation: {0}")]
    ContractViolation(String),
    #[error("release failed")]
    ReleaseFailed,
}

impl IrohAdapter {
    pub fn new(config: IrohAdapterConfig) -> Self {
        Self {
            config,
            task_tracker: Arc::new(AdapterTaskTracker::new("iroh")),
        }
    }

    pub fn task_tracker(&self) -> &AdapterTaskTracker {
        self.task_tracker.as_ref()
    }

    pub async fn release(&self) -> Result<(), IrohError> {
        self.config.endpoint.close().await;
        self.task_tracker
            .close_and_wait(Duration::from_secs(2))
            .await
            .map_err(|_| IrohError::ReleaseFailed)
    }

    fn spawn_bridge_tasks(
        &self,
        send_stream: iroh::endpoint::SendStream,
        recv_stream: iroh::endpoint::RecvStream,
        bridge_stream: DuplexStream,
        shutdown: CancellationToken,
        lifecycle: Arc<BridgeLifecycle>,
    ) {
        let (mut bridge_reader, mut bridge_writer) = split(bridge_stream);
        let write_shutdown = shutdown.clone();
        let write_lifecycle = lifecycle.clone();

        self.task_tracker.spawn(async move {
            let mut send_stream = send_stream;
            let mut buffer = [0_u8; 8192];
            loop {
                tokio::select! {
                    _ = write_shutdown.cancelled() => {
                        let _ = send_stream.finish();
                        break;
                    }
                    read_result = bridge_reader.read(&mut buffer) => {
                        match read_result {
                            Ok(0) => {
                                let _ = send_stream.finish();
                                break;
                            }
                            Ok(bytes_read) => {
                                if send_stream.write_all(&buffer[..bytes_read]).await.is_err() {
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
            let mut recv_stream = recv_stream;
            let mut marker_buf = [0_u8; 1];
            match recv_stream.read_exact(&mut marker_buf).await {
                Ok(_) if marker_buf[0] == IROH_ALPN_MARKER => {}
                Ok(_) | Err(_) => {
                    let _ = bridge_writer.shutdown().await;
                    finish_bridge_task(&lifecycle);
                    return;
                }
            }

            let mut buffer = [0_u8; 8192];
            loop {
                tokio::select! {
                    _ = read_shutdown.cancelled() => {
                        let _ = bridge_writer.shutdown().await;
                        break;
                    }
                    read_result = recv_stream.read(&mut buffer) => {
                        match read_result {
                            Ok(Some(0)) => {
                                let _ = bridge_writer.shutdown().await;
                                break;
                            }
                            Ok(Some(bytes_read)) => {
                                if bridge_writer.write_all(&buffer[..bytes_read]).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {
                                let _ = bridge_writer.shutdown().await;
                                break;
                            }
                            Err(_) => {
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
impl TransportAdapter for IrohAdapter {
    type Error = IrohError;

    // START_CONTRACT: open_stream
    //   PURPOSE: Open a direct or relay-backed transport stream to the remote peer.
    //   INPUTS: { request: &TransportRequest - stable peer label request metadata, cancel: CancellationToken - cancellation boundary }
    //   OUTPUTS: { Result<ResolvedStream, IrohError> - iroh-backed resolved stream or deterministic error }
    //   SIDE_EFFECTS: [opens an iroh connection and tracked bridge tasks]
    //   LINKS: [M-IROH-ADAPTER, V-M-IROH-ADAPTER]
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
                "[IrohAdapter][openStream][BLOCK_ADAPTER_CLEANUP_CONTRACT] cancelled before connect"
            );
            return Err(IrohError::Cancelled);
        }

        let connection = tokio::select! {
            _ = cancel.cancelled() => Err(IrohError::Cancelled),
            connection = self.config.endpoint.connect(self.config.remote_addr.clone(), &self.config.alpn) => {
                connection.map_err(|err| IrohError::TransportUnavailable(err.to_string()))
            }
        }?;

        let (mut send_stream, recv_stream) = tokio::select! {
            _ = cancel.cancelled() => Err(IrohError::Cancelled),
            opened = connection.open_bi() => {
                opened.map_err(|err| IrohError::StreamOpenFailed(err.to_string()))
            }
        }?;

        send_stream
            .write_all(&[IROH_ALPN_MARKER])
            .await
            .map_err(|err| IrohError::ContractViolation(err.to_string()))?;

        let (local_stream, bridge_stream) = duplex(32 * 1024);
        let lifecycle = Arc::new(BridgeLifecycle {
            active_tasks: AtomicUsize::new(2),
            notify: Notify::new(),
        });
        let shutdown = cancel.child_token();
        self.spawn_bridge_tasks(
            send_stream,
            recv_stream,
            bridge_stream,
            shutdown.clone(),
            lifecycle.clone(),
        );

        self.config.metrics.increment_intents_enqueued();
        info!(
            peer = %request.peer_label,
            "[IrohAdapter][openStream][BLOCK_ADAPTER_CLEANUP_CONTRACT] established iroh transport stream"
        );

        Ok(ResolvedStream {
            stream: Box::new(IrohTransportStream {
                stream: local_stream,
                peer_label: request.peer_label.clone(),
                shutdown,
                lifecycle,
            }),
            transport_kind: TransportKind::IrohDirect,
        })
        // END_BLOCK_ADAPTER_CLEANUP_CONTRACT
    }

    fn task_tracker(&self) -> &AdapterTaskTracker {
        self.task_tracker()
    }
}

fn finish_bridge_task(lifecycle: &Arc<BridgeLifecycle>) {
    if lifecycle.active_tasks.fetch_sub(1, Ordering::SeqCst) == 1 {
        lifecycle.notify.notify_waiters();
    }
}

#[async_trait]
impl TransportStream for IrohTransportStream {
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

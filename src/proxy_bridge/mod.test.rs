use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::io::{duplex, split, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ProxyBridge, ProxyBridgeConfig, ProxyResult};
use crate::session::{
    EffectHandler, MetricEffectTarget, MetricEvent, SessionManager, SessionManagerConfig,
    SessionRegistry, TimerCommand, TimerEffectTarget, TransportSelector, TransportSelectorConfig,
};
use crate::socks5::{ProxyIntent, ProxyProtocol, TargetAddr};
use crate::transport::adapter_contract::{TransportAdapter, TransportRequest};
use crate::transport::stream::{
    BoxedRead, BoxedWrite, ResolvedStream, ShutdownError, TransportKind, TransportStream,
};
use crate::transport::task_tracker::AdapterTaskTracker;

type BoxFutureResult =
    Pin<Box<dyn Future<Output = Result<ResolvedStream, MockAdapterError>> + Send + 'static>>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
enum MockAdapterError {
    #[error("{0}")]
    Message(String),
}

#[derive(Clone)]
struct MockAdapter {
    task_tracker: Arc<AdapterTaskTracker>,
    behavior: Arc<dyn Fn(TransportRequest, CancellationToken) -> BoxFutureResult + Send + Sync>,
}

impl MockAdapter {
    fn new(
        behavior: impl Fn(TransportRequest, CancellationToken) -> BoxFutureResult
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            task_tracker: Arc::new(AdapterTaskTracker::new("proxy-bridge-test")),
            behavior: Arc::new(behavior),
        }
    }
}

#[async_trait]
impl TransportAdapter for MockAdapter {
    type Error = MockAdapterError;

    async fn open_stream(
        &self,
        request: &TransportRequest,
        cancel: CancellationToken,
    ) -> Result<ResolvedStream, Self::Error> {
        (self.behavior)(request.clone(), cancel).await
    }

    fn task_tracker(&self) -> &AdapterTaskTracker {
        self.task_tracker.as_ref()
    }
}

struct MockStream {
    stream: DuplexStream,
    peer_label: String,
}

#[async_trait]
impl TransportStream for MockStream {
    fn split(self: Box<Self>) -> (BoxedRead, BoxedWrite) {
        let (read_half, write_half) = split(self.stream);
        (Box::pin(read_half), Box::pin(write_half))
    }

    fn peer_label(&self) -> &str {
        &self.peer_label
    }

    async fn shutdown(mut self: Box<Self>, _timeout: Duration) -> Result<(), ShutdownError> {
        let _ = self.stream.shutdown().await;
        Ok(())
    }
}

#[derive(Clone, Default)]
struct TimerSpy;

#[async_trait]
impl TimerEffectTarget for TimerSpy {
    async fn execute(&self, _command: TimerCommand) {}
}

#[derive(Clone, Default)]
struct MetricSpy {
    calls: Arc<Mutex<Vec<MetricEvent>>>,
}

impl MetricEffectTarget for MetricSpy {
    fn emit(&self, event: MetricEvent) {
        self.calls.lock().expect("metric lock").push(event);
    }
}

async fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let client = TcpStream::connect(addr).await.expect("client");
    let (server, _) = listener.accept().await.expect("accept");
    (client, server)
}

fn build_manager(
    iroh: MockAdapter,
    wss: MockAdapter,
    metrics: MetricSpy,
) -> Arc<SessionManager<MockAdapter, MockAdapter, TimerSpy, MetricSpy>> {
    let registry = Arc::new(SessionRegistry::new(4));
    let selector = TransportSelector::new(
        iroh,
        wss,
        TransportSelectorConfig {
            iroh_timeout: Duration::from_millis(50),
            wss_timeout: Duration::from_millis(50),
            safety_timeout: Duration::from_millis(250),
        },
    );
    let effect_handler = EffectHandler::new(registry.clone(), TimerSpy, metrics);

    Arc::new(SessionManager::new(
        registry,
        selector,
        effect_handler,
        SessionManagerConfig {
            idle_timeout: Duration::from_secs(15),
            graceful_shutdown_timeout: Duration::from_secs(30),
        },
    ))
}

#[tokio::test]
async fn valid_proxy_intent_is_tunneled_over_resolved_stream() {
    let (remote_local, mut remote_peer) = duplex(1024);
    let holder = Arc::new(std::sync::Mutex::new(Some(remote_local)));
    let iroh = MockAdapter::new({
        let holder = holder.clone();
        move |request, _cancel| {
            let peer = request.peer_label;
            let stream = holder
                .lock()
                .expect("stream holder")
                .take()
                .expect("single use");
            Box::pin(async move {
                Ok(ResolvedStream {
                    stream: Box::new(MockStream {
                        stream,
                        peer_label: peer,
                    }),
                    transport_kind: TransportKind::IrohDirect,
                })
            })
        }
    });
    let wss = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Err(MockAdapterError::Message("unused".to_string())) })
    });
    let manager = build_manager(iroh, wss, MetricSpy::default());
    let bridge = ProxyBridge::new(
        ProxyBridgeConfig {
            pump_buffer_bytes: 1024,
            total_request_timeout: Duration::from_secs(2),
        },
        manager,
    );
    let (mut client_side, bridge_side) = tcp_pair().await;
    let intent = ProxyIntent {
        target: TargetAddr::Domain("example.com".to_string(), 443),
        client_stream: bridge_side,
        protocol_kind: ProxyProtocol::Socks5,
        request_id: 1,
    };

    let process_task = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.process_intent(intent).await }
    });

    let mut success_reply = [0_u8; 10];
    client_side
        .read_exact(&mut success_reply)
        .await
        .expect("reply");
    assert_eq!(success_reply[1], 0x00);

    client_side.write_all(b"ping").await.expect("client write");
    let mut forwarded = [0_u8; 4];
    remote_peer
        .read_exact(&mut forwarded)
        .await
        .expect("remote read");
    assert_eq!(&forwarded, b"ping");

    remote_peer.write_all(b"pong").await.expect("remote write");
    let mut returned = [0_u8; 4];
    client_side
        .read_exact(&mut returned)
        .await
        .expect("client read");
    assert_eq!(&returned, b"pong");

    drop(client_side);
    drop(remote_peer);
    let result = process_task.await.expect("join");
    assert!(matches!(result, ProxyResult::Completed));
}

#[tokio::test]
async fn transport_failure_returns_client_visible_reply() {
    let iroh = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Err(MockAdapterError::Message("iroh down".to_string())) })
    });
    let wss = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Err(MockAdapterError::Message("wss down".to_string())) })
    });
    let manager = build_manager(iroh, wss, MetricSpy::default());
    let bridge = ProxyBridge::new(
        ProxyBridgeConfig {
            pump_buffer_bytes: 1024,
            total_request_timeout: Duration::from_secs(2),
        },
        manager,
    );
    let (mut client_side, bridge_side) = tcp_pair().await;
    let intent = ProxyIntent {
        target: TargetAddr::Domain("example.com".to_string(), 443),
        client_stream: bridge_side,
        protocol_kind: ProxyProtocol::Socks5,
        request_id: 2,
    };

    let process_task = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.process_intent(intent).await }
    });

    let mut failure_reply = [0_u8; 10];
    client_side
        .read_exact(&mut failure_reply)
        .await
        .expect("reply");
    assert_eq!(failure_reply[1], 0x01);

    let result = process_task.await.expect("join");
    assert!(matches!(result, ProxyResult::Failed(_)));
}

#[tokio::test]
async fn drain_all_stops_worker_and_waits_for_tasks() {
    let iroh = MockAdapter::new(|request, _cancel| {
        let (stream, _peer) = duplex(128);
        let peer = request.peer_label;
        Box::pin(async move {
            Ok(ResolvedStream {
                stream: Box::new(MockStream {
                    stream,
                    peer_label: peer,
                }),
                transport_kind: TransportKind::IrohDirect,
            })
        })
    });
    let wss = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Err(MockAdapterError::Message("unused".to_string())) })
    });
    let manager = build_manager(iroh, wss, MetricSpy::default());
    let bridge = ProxyBridge::new(
        ProxyBridgeConfig {
            pump_buffer_bytes: 1024,
            total_request_timeout: Duration::from_secs(2),
        },
        manager,
    );
    let (tx, rx) = mpsc::channel(4);
    let worker = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.run_worker(rx).await }
    });

    bridge.stop_accept();
    bridge.drain_all().await;
    drop(tx);
    worker.await.expect("worker join");
}

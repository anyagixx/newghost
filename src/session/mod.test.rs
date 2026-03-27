use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use thiserror::Error;
use tokio::io::{duplex, split, AsyncWriteExt, DuplexStream};
use tokio_util::sync::CancellationToken;

use super::{
    EffectHandler, MetricEffectTarget, MetricEvent, SessionEvent, SessionId, SessionManager,
    SessionManagerConfig, SessionRegistry, SessionRequest, SessionState, TimerCommand,
    TimerEffectTarget, TransportSelectError, TransportSelector, TransportSelectorConfig,
};
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
    calls: Arc<Mutex<Vec<String>>>,
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
            task_tracker: Arc::new(AdapterTaskTracker::new("session-test")),
            calls: Arc::new(Mutex::new(Vec::new())),
            behavior: Arc::new(behavior),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().expect("calls lock poisoned").len()
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
        self.calls
            .lock()
            .expect("calls lock poisoned")
            .push(request.peer_label.clone());
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

fn resolved_stream(peer_label: &str, kind: TransportKind) -> ResolvedStream {
    let (local, _remote) = duplex(64);
    ResolvedStream {
        stream: Box::new(MockStream {
            stream: local,
            peer_label: peer_label.to_string(),
        }),
        transport_kind: kind,
    }
}

#[derive(Clone, Default)]
struct TimerSpy {
    calls: Arc<Mutex<Vec<TimerCommand>>>,
}

#[async_trait]
impl TimerEffectTarget for TimerSpy {
    async fn execute(&self, command: TimerCommand) {
        self.calls
            .lock()
            .expect("timer lock poisoned")
            .push(command);
    }
}

#[derive(Clone, Default)]
struct MetricSpy {
    calls: Arc<Mutex<Vec<MetricEvent>>>,
}

impl MetricEffectTarget for MetricSpy {
    fn emit(&self, event: MetricEvent) {
        self.calls.lock().expect("metric lock poisoned").push(event);
    }
}

fn build_manager(
    iroh: MockAdapter,
    wss: MockAdapter,
    timer: TimerSpy,
    metrics: MetricSpy,
) -> SessionManager<MockAdapter, MockAdapter, TimerSpy, MetricSpy> {
    let registry = Arc::new(SessionRegistry::new(2));
    let selector = TransportSelector::new(
        iroh,
        wss,
        TransportSelectorConfig {
            iroh_timeout: Duration::from_millis(50),
            wss_timeout: Duration::from_millis(50),
            safety_timeout: Duration::from_millis(250),
        },
    );
    let effect_handler = EffectHandler::new(registry.clone(), timer, metrics);

    SessionManager::new(
        registry,
        selector,
        effect_handler,
        SessionManagerConfig {
            idle_timeout: Duration::from_secs(15),
            graceful_shutdown_timeout: Duration::from_secs(30),
        },
    )
}

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

#[test]
fn register_session_reserves_capacity() {
    let iroh = MockAdapter::new(|request, _cancel| {
        Box::pin(async move {
            Ok(resolved_stream(
                &request.peer_label,
                TransportKind::IrohDirect,
            ))
        })
    });
    let wss = MockAdapter::new(|request, _cancel| {
        Box::pin(async move { Ok(resolved_stream(&request.peer_label, TransportKind::Wss)) })
    });
    let manager = build_manager(iroh, wss, TimerSpy::default(), MetricSpy::default());

    let (session_id, handle) = manager
        .register_session(&SessionRequest {
            started_at: Instant::now(),
            peer_label: "peer-register".to_string(),
        })
        .expect("registration should succeed");

    assert_eq!(session_id, 1);
    assert!(matches!(
        handle.snapshot().state,
        SessionState::Active {
            stream_count: 0,
            ..
        }
    ));
}

#[tokio::test]
async fn resolve_stream_updates_state_after_successful_transport_selection() {
    let iroh = MockAdapter::new(|request, _cancel| {
        Box::pin(async move {
            Ok(resolved_stream(
                &request.peer_label,
                TransportKind::IrohDirect,
            ))
        })
    });
    let wss = MockAdapter::new(|request, _cancel| {
        Box::pin(async move { Ok(resolved_stream(&request.peer_label, TransportKind::Wss)) })
    });
    let timer = TimerSpy::default();
    let metrics = MetricSpy::default();
    let manager = build_manager(iroh.clone(), wss.clone(), timer.clone(), metrics.clone());

    let (session_id, handle) = manager
        .register_session(&SessionRequest {
            started_at: Instant::now(),
            peer_label: "peer-resolve".to_string(),
        })
        .expect("registration should succeed");

    let resolved = manager
        .resolve_stream(
            session_id,
            &TransportRequest {
                peer_label: "peer-resolve".to_string(),
            },
            CancellationToken::new(),
        )
        .await
        .expect("resolve should succeed");

    assert_eq!(resolved.transport_kind, TransportKind::IrohDirect);
    assert_eq!(iroh.call_count(), 1);
    assert_eq!(wss.call_count(), 0);
    assert!(matches!(
        handle.snapshot().state,
        SessionState::Active {
            stream_count: 1,
            ..
        }
    ));
    assert_eq!(
        metrics
            .calls
            .lock()
            .expect("metric lock poisoned")
            .as_slice(),
        &[MetricEvent::StreamOpened {
            session_id,
            stream_count: 1,
        }]
    );
    assert_eq!(
        timer.calls.lock().expect("timer lock poisoned").as_slice(),
        &[TimerCommand::CancelIdle { session_id }]
    );
}

#[tokio::test]
async fn handle_event_dispatches_effects_for_stream_close() {
    let iroh = MockAdapter::new(|request, _cancel| {
        Box::pin(async move {
            Ok(resolved_stream(
                &request.peer_label,
                TransportKind::IrohDirect,
            ))
        })
    });
    let wss = MockAdapter::new(|request, _cancel| {
        Box::pin(async move { Ok(resolved_stream(&request.peer_label, TransportKind::Wss)) })
    });
    let timer = TimerSpy::default();
    let metrics = MetricSpy::default();
    let manager = build_manager(iroh, wss, timer.clone(), metrics.clone());

    let (session_id, handle) = manager
        .register_session(&SessionRequest {
            started_at: Instant::now(),
            peer_label: "peer-close".to_string(),
        })
        .expect("registration should succeed");
    handle.with_record(|record| {
        record.state = SessionState::Active {
            since: Instant::now(),
            stream_count: 1,
        };
    });

    manager
        .handle_event(session_id, SessionEvent::StreamClosed)
        .await
        .expect("handle_event should succeed");

    assert!(matches!(
        handle.snapshot().state,
        SessionState::Active {
            stream_count: 0,
            ..
        }
    ));
    assert_eq!(
        metrics
            .calls
            .lock()
            .expect("metric lock poisoned")
            .as_slice(),
        &[MetricEvent::StreamClosed {
            session_id,
            stream_count: 0,
        }]
    );
    assert_eq!(
        timer.calls.lock().expect("timer lock poisoned").as_slice(),
        &[TimerCommand::ScheduleIdle {
            session_id,
            timeout: Duration::from_secs(15),
        }]
    );
}

#[tokio::test]
async fn resolve_stream_surfaces_transport_failure_deterministically() {
    let iroh = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Err(MockAdapterError::Message("iroh down".to_string())) })
    });
    let wss = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Err(MockAdapterError::Message("wss down".to_string())) })
    });
    let manager = build_manager(iroh, wss, TimerSpy::default(), MetricSpy::default());

    let (session_id, _) = manager
        .register_session(&SessionRequest {
            started_at: Instant::now(),
            peer_label: "peer-fail".to_string(),
        })
        .expect("registration should succeed");

    let err = match manager
        .resolve_stream(
            session_id,
            &TransportRequest {
                peer_label: "peer-fail".to_string(),
            },
            CancellationToken::new(),
        )
        .await
    {
        Ok(_) => panic!("resolve should fail"),
        Err(err) => err,
    };

    assert_eq!(
        err,
        super::SessionManagerError::TransportResolutionFailed(TransportSelectError::AllFailed {
            iroh_err: Some("iroh down".to_string()),
            wss_err: Some("wss down".to_string()),
        })
    );
}

#[tokio::test]
async fn shutdown_drains_sessions() {
    let iroh = MockAdapter::new(|request, _cancel| {
        Box::pin(async move {
            Ok(resolved_stream(
                &request.peer_label,
                TransportKind::IrohDirect,
            ))
        })
    });
    let wss = MockAdapter::new(|request, _cancel| {
        Box::pin(async move { Ok(resolved_stream(&request.peer_label, TransportKind::Wss)) })
    });
    let manager = build_manager(iroh, wss, TimerSpy::default(), MetricSpy::default());

    let (_first_id, first_handle) = manager
        .register_session(&SessionRequest {
            started_at: Instant::now(),
            peer_label: "peer-a".to_string(),
        })
        .expect("first registration should succeed");
    let (_second_id, second_handle) = manager
        .register_session(&SessionRequest {
            started_at: Instant::now(),
            peer_label: "peer-b".to_string(),
        })
        .expect("second registration should succeed");

    let drained = manager.shutdown().await;

    assert_eq!(drained, 2);
    assert_eq!(first_handle.snapshot().state, SessionState::Closed);
    assert_eq!(second_handle.snapshot().state, SessionState::Closed);
}

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::io::{duplex, split, AsyncWriteExt, DuplexStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{TransportSelectError, TransportSelector, TransportSelectorConfig};
use crate::transport::adapter_contract::{TransportAdapter, TransportRequest};
use crate::transport::stream::{
    BoxedRead, BoxedWrite, ResolvedStream, ShutdownError, TransportKind, TransportStream,
};
use crate::transport::task_tracker::AdapterTaskTracker;

type BoxFutureResult =
    Pin<Box<dyn Future<Output = Result<ResolvedStream, MockAdapterError>> + Send + 'static>>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
enum MockAdapterError {
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Message(String),
}

#[derive(Clone)]
struct MockAdapter {
    task_tracker: Arc<AdapterTaskTracker>,
    calls: Arc<AtomicUsize>,
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
            task_tracker: Arc::new(AdapterTaskTracker::new("mock")),
            calls: Arc::new(AtomicUsize::new(0)),
            behavior: Arc::new(behavior),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
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
        self.calls.fetch_add(1, Ordering::SeqCst);
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

fn resolved_stream(peer_label: &str, transport_kind: TransportKind) -> ResolvedStream {
    let (local, _remote) = duplex(64);
    ResolvedStream {
        stream: Box::new(MockStream {
            stream: local,
            peer_label: peer_label.to_string(),
        }),
        transport_kind,
    }
}

#[tokio::test]
async fn iroh_success_skips_wss_fallback() {
    let iroh = MockAdapter::new(|request, _cancel| {
        Box::pin(async move {
            Ok(resolved_stream(
                &request.peer_label,
                TransportKind::IrohDirect,
            ))
        })
    });
    let wss = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Ok(resolved_stream("unexpected", TransportKind::Wss)) })
    });
    let selector = TransportSelector::new(
        iroh.clone(),
        wss.clone(),
        TransportSelectorConfig {
            iroh_timeout: Duration::from_millis(50),
            wss_timeout: Duration::from_millis(50),
            safety_timeout: Duration::from_millis(200),
        },
    );

    let resolved = selector
        .open_stream(
            &TransportRequest {
                peer_label: "peer-a".to_string(),
            },
            CancellationToken::new(),
        )
        .await
        .expect("iroh should resolve");

    assert_eq!(resolved.transport_kind, TransportKind::IrohDirect);
    assert_eq!(iroh.calls(), 1);
    assert_eq!(wss.calls(), 0);
}

#[tokio::test]
async fn iroh_timeout_falls_back_to_wss() {
    let iroh = MockAdapter::new(|_request, cancel| {
        Box::pin(async move {
            cancel.cancelled().await;
            Err(MockAdapterError::Cancelled)
        })
    });
    let wss = MockAdapter::new(|request, _cancel| {
        Box::pin(async move { Ok(resolved_stream(&request.peer_label, TransportKind::Wss)) })
    });
    let selector = TransportSelector::new(
        iroh.clone(),
        wss.clone(),
        TransportSelectorConfig {
            iroh_timeout: Duration::from_millis(30),
            wss_timeout: Duration::from_millis(50),
            safety_timeout: Duration::from_millis(200),
        },
    );

    let resolved = selector
        .open_stream(
            &TransportRequest {
                peer_label: "peer-b".to_string(),
            },
            CancellationToken::new(),
        )
        .await
        .expect("wss fallback should resolve");

    assert_eq!(resolved.transport_kind, TransportKind::Wss);
    assert_eq!(iroh.calls(), 1);
    assert_eq!(wss.calls(), 1);
}

#[tokio::test]
async fn both_attempts_fail_with_combined_diagnostics() {
    let iroh = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Err(MockAdapterError::Message("iroh down".to_string())) })
    });
    let wss = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Err(MockAdapterError::Message("wss refused".to_string())) })
    });
    let selector = TransportSelector::new(
        iroh,
        wss,
        TransportSelectorConfig {
            iroh_timeout: Duration::from_millis(50),
            wss_timeout: Duration::from_millis(50),
            safety_timeout: Duration::from_millis(200),
        },
    );

    let err = match selector
        .open_stream(
            &TransportRequest {
                peer_label: "peer-c".to_string(),
            },
            CancellationToken::new(),
        )
        .await
    {
        Ok(_) => panic!("both attempts should fail"),
        Err(err) => err,
    };

    assert_eq!(
        err,
        TransportSelectError::AllFailed {
            iroh_err: Some("iroh down".to_string()),
            wss_err: Some("wss refused".to_string()),
        }
    );
}

#[tokio::test]
async fn explicit_cancel_stops_selection() {
    let barrier = Arc::new(Mutex::new(()));
    let iroh = MockAdapter::new({
        let barrier = barrier.clone();
        move |_request, cancel| {
            let barrier = barrier.clone();
            Box::pin(async move {
                let _guard = barrier.lock().await;
                cancel.cancelled().await;
                Err(MockAdapterError::Cancelled)
            })
        }
    });
    let wss = MockAdapter::new(|_request, _cancel| {
        Box::pin(async { Ok(resolved_stream("unexpected", TransportKind::Wss)) })
    });
    let selector = TransportSelector::new(
        iroh.clone(),
        wss.clone(),
        TransportSelectorConfig {
            iroh_timeout: Duration::from_secs(1),
            wss_timeout: Duration::from_millis(50),
            safety_timeout: Duration::from_secs(2),
        },
    );

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_clone.cancel();
    });

    let err = match selector
        .open_stream(
            &TransportRequest {
                peer_label: "peer-d".to_string(),
            },
            cancel,
        )
        .await
    {
        Ok(_) => panic!("cancel should stop selection"),
        Err(err) => err,
    };

    assert_eq!(err, TransportSelectError::Cancelled);
    assert_eq!(iroh.calls(), 1);
    assert_eq!(wss.calls(), 0);
}

#[tokio::test]
async fn safety_timeout_surfaces_contract_violation() {
    let iroh = MockAdapter::new(|_request, _cancel| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            Err(MockAdapterError::Message("too late".to_string()))
        })
    });
    let wss = MockAdapter::new(|_request, _cancel| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            Err(MockAdapterError::Message("too late".to_string()))
        })
    });
    let selector = TransportSelector::new(
        iroh,
        wss,
        TransportSelectorConfig {
            iroh_timeout: Duration::from_millis(200),
            wss_timeout: Duration::from_millis(200),
            safety_timeout: Duration::from_millis(50),
        },
    );

    let err = match selector
        .open_stream(
            &TransportRequest {
                peer_label: "peer-safety".to_string(),
            },
            CancellationToken::new(),
        )
        .await
    {
        Ok(_) => panic!("outer safety timeout should trip first"),
        Err(err) => err,
    };

    assert_eq!(err, TransportSelectError::ContractViolation);
}

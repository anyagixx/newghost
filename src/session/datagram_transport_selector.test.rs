// FILE: src/session/datagram_transport_selector.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify the bounded datagram transport selector stays explicitly WSS-only for the initial UDP phase.
//   SCOPE: WSS success, WSS failure, explicit cancellation, and bounded timeout behavior.
//   DEPENDS: src/session/datagram_transport_selector.rs
//   LINKS: V-M-DATAGRAM-TRANSPORT-SELECTOR
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   wss_success_resolves_bounded_datagram_transport - proves the approved WSS carrier resolves deterministically
//   wss_failure_is_explicit_and_bounded - proves carrier failure stays explicitly WSS-scoped
//   explicit_cancel_stops_datagram_selection - proves cancellation stops the selector before carrier resolution
//   wss_timeout_is_reported_without_implying_other_carriers - proves timeout remains bounded and WSS-specific
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added deterministic datagram transport selector tests so the initial WSS-only UDP scope cannot silently widen.
// END_CHANGE_SUMMARY

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{
    DatagramTransportKind, DatagramTransportSelectError, DatagramTransportSelector,
    DatagramTransportSelectorConfig, WssDatagramPath,
};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
enum MockWssError {
    #[error("{0}")]
    Message(String),
}

#[derive(Clone)]
struct MockWssPath {
    delay: Option<Duration>,
    result: Result<(), MockWssError>,
    calls: Arc<Mutex<Vec<u64>>>,
}

#[async_trait]
impl WssDatagramPath for MockWssPath {
    type Error = MockWssError;

    async fn open_path(
        &self,
        association_id: u64,
        cancel: CancellationToken,
    ) -> Result<(), Self::Error> {
        self.calls.lock().await.push(association_id);
        if let Some(delay) = self.delay {
            tokio::select! {
                _ = cancel.cancelled() => Err(MockWssError::Message("cancelled".to_string())),
                _ = tokio::time::sleep(delay) => self.result.clone(),
            }
        } else {
            self.result.clone()
        }
    }
}

#[tokio::test]
async fn wss_success_resolves_bounded_datagram_transport() {
    let selector = DatagramTransportSelector::new(
        MockWssPath {
            delay: None,
            result: Ok(()),
            calls: Arc::new(Mutex::new(Vec::new())),
        },
        DatagramTransportSelectorConfig {
            wss_timeout: Duration::from_millis(50),
        },
    );

    let resolved = selector
        .select_transport(11, CancellationToken::new())
        .await
        .expect("wss datagram should resolve");

    assert_eq!(resolved.association_id, 11);
    assert_eq!(resolved.transport_kind, DatagramTransportKind::WssDatagram);
}

#[tokio::test]
async fn wss_failure_is_explicit_and_bounded() {
    let selector = DatagramTransportSelector::new(
        MockWssPath {
            delay: None,
            result: Err(MockWssError::Message("wss datagram down".to_string())),
            calls: Arc::new(Mutex::new(Vec::new())),
        },
        DatagramTransportSelectorConfig {
            wss_timeout: Duration::from_millis(50),
        },
    );

    let error = selector
        .select_transport(12, CancellationToken::new())
        .await
        .expect_err("wss datagram failure should surface");

    assert_eq!(
        error,
        DatagramTransportSelectError::WssFailed("wss datagram down".to_string())
    );
}

#[tokio::test]
async fn explicit_cancel_stops_datagram_selection() {
    let selector = DatagramTransportSelector::new(
        MockWssPath {
            delay: Some(Duration::from_millis(100)),
            result: Ok(()),
            calls: Arc::new(Mutex::new(Vec::new())),
        },
        DatagramTransportSelectorConfig {
            wss_timeout: Duration::from_millis(200),
        },
    );
    let cancel = CancellationToken::new();
    cancel.cancel();

    let error = selector
        .select_transport(13, cancel)
        .await
        .expect_err("cancel should stop selection");

    assert_eq!(error, DatagramTransportSelectError::Cancelled);
}

#[tokio::test]
async fn wss_timeout_is_reported_without_implying_other_carriers() {
    let selector = DatagramTransportSelector::new(
        MockWssPath {
            delay: Some(Duration::from_millis(100)),
            result: Ok(()),
            calls: Arc::new(Mutex::new(Vec::new())),
        },
        DatagramTransportSelectorConfig {
            wss_timeout: Duration::from_millis(25),
        },
    );

    let error = selector
        .select_transport(14, CancellationToken::new())
        .await
        .expect_err("timeout should surface");

    assert_eq!(error, DatagramTransportSelectError::WssTimeout(25));
}

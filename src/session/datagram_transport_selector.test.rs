// FILE: src/session/datagram_transport_selector.test.rs
// VERSION: 0.1.1
// START_MODULE_CONTRACT
//   PURPOSE: Verify the bounded datagram transport selector stays explicitly WSS-only for the initial UDP phase and can emit one outbound datagram through that carrier.
//   SCOPE: WSS success, WSS failure, explicit cancellation, bounded timeout behavior, and outbound datagram emission.
//   DEPENDS: src/session/datagram_transport_selector.rs
//   LINKS: V-M-DATAGRAM-TRANSPORT-SELECTOR
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   wss_success_resolves_bounded_datagram_transport - proves the approved WSS carrier resolves deterministically
//   wss_failure_is_explicit_and_bounded - proves carrier failure stays explicitly WSS-scoped
//   explicit_cancel_stops_datagram_selection - proves cancellation stops the selector before carrier resolution
//   wss_timeout_is_reported_without_implying_other_carriers - proves timeout remains bounded and WSS-specific
//   outbound_datagram_emission_uses_the_selected_wss_carrier - proves the bounded WSS carrier receives the normalized outbound envelope
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added deterministic outbound-emission coverage so repair waves can prove carrier-side datagram handoff separately from local dispatch.
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
use crate::transport::datagram_contract::{DatagramEnvelope, DatagramTarget};

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
    emitted: Arc<Mutex<Vec<DatagramEnvelope>>>,
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

    async fn emit_datagram(
        &self,
        envelope: &DatagramEnvelope,
        cancel: CancellationToken,
    ) -> Result<(), Self::Error> {
        self.emitted.lock().await.push(envelope.clone());
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
            emitted: Arc::new(Mutex::new(Vec::new())),
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
            emitted: Arc::new(Mutex::new(Vec::new())),
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
            emitted: Arc::new(Mutex::new(Vec::new())),
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
            emitted: Arc::new(Mutex::new(Vec::new())),
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

#[tokio::test]
async fn outbound_datagram_emission_uses_the_selected_wss_carrier() {
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let selector = DatagramTransportSelector::new(
        MockWssPath {
            delay: None,
            result: Ok(()),
            calls: Arc::new(Mutex::new(Vec::new())),
            emitted: emitted.clone(),
        },
        DatagramTransportSelectorConfig {
            wss_timeout: Duration::from_millis(50),
        },
    );
    let envelope = DatagramEnvelope {
        association_id: 21,
        relay_client_addr: "127.0.0.1:50000".parse().expect("relay client addr"),
        target: DatagramTarget::Ip("127.0.0.1:55123".parse().expect("target addr")),
        payload: b"phase24-probe".to_vec(),
    };

    let resolved = selector
        .emit_outbound_datagram(&envelope, CancellationToken::new())
        .await
        .expect("outbound emit should succeed");

    assert_eq!(resolved.association_id, 21);
    assert_eq!(resolved.transport_kind, DatagramTransportKind::WssDatagram);
    assert_eq!(emitted.lock().await.as_slice(), &[envelope]);
}

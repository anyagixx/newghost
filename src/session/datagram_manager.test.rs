// FILE: src/session/datagram_manager.test.rs
// VERSION: 0.1.4
// START_MODULE_CONTRACT
//   PURPOSE: Verify datagram association open, outbound dispatch, runtime bridge dispatch, inbound dispatch, and cleanup trajectories over the governed UDP registry.
//   SCOPE: Open success, explicit close, outbound dispatch, selector-backed runtime-bridge dispatch, inbound dispatch, explicit dispatch failure, and missing-association failure behavior.
//   DEPENDS: src/session/datagram_manager.rs, src/session/udp_registry.rs, src/obs/mod.rs, src/transport/datagram_contract.rs
//   LINKS: V-M-DATAGRAM-SESSION-MANAGER
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   open_association_records_owned_udp_state - proves opening an association allocates deterministic owned state
//   outbound_dispatch_refreshes_activity_and_hits_outbound_target - proves outbound datagrams stay associated with the correct session identity
//   accept_outbound_datagram_opens_and_dispatches_owned_association - proves the local handoff can open an owned association and forward one outbound datagram through the manager
//   accept_outbound_datagram_reuses_existing_owned_association - proves repeated local handoffs for the same relay and client pair reuse the same association id
//   runtime_bridge_forwards_governed_datagram_into_selector_emit - proves the SOCKS5 runtime handoff opens an association and reaches selector-backed WSS emission
//   runtime_bridge_reuses_owned_association_across_packets - proves repeated runtime handoffs reuse the same governed association id
//   inbound_dispatch_refreshes_activity_and_hits_inbound_target - proves inbound datagrams stay associated with the correct session identity
//   close_association_releases_owned_state - proves explicit close frees registry-owned state
//   outbound_dispatch_failure_is_explicit - proves manager-side dispatch failures stay deterministic after activity refresh
//   missing_association_is_explicit_for_dispatch - proves dispatch on unknown association ids fails deterministically
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.4 - Added direct manager log-anchor assertions so association-open, outbound-dispatch, inbound-dispatch, and local handoff trajectory evidence no longer depends on manual log review.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use super::{
    DatagramDispatchTarget, DatagramRuntimeBridge, DatagramSessionError, DatagramSessionManager,
};
use crate::obs::test_tracing_dispatch;
use crate::session::UdpAssociationRegistry;
use crate::session::{
    DatagramTransportSelector, DatagramTransportSelectorConfig, WssDatagramPath,
};
use crate::socks5::udp_associate::UdpRelayRuntimeTarget;
use crate::transport::datagram_contract::{DatagramEnvelope, DatagramTarget};

#[derive(Default)]
struct RecordingTarget {
    seen: Mutex<Vec<DatagramEnvelope>>,
}

#[derive(Debug, thiserror::Error)]
#[error("recording target failed")]
struct RecordingTargetError;

#[async_trait]
impl DatagramDispatchTarget for Arc<RecordingTarget> {
    type Error = RecordingTargetError;

    async fn dispatch(&self, envelope: &DatagramEnvelope) -> Result<(), Self::Error> {
        self.seen
            .lock()
            .expect("recording target lock poisoned")
            .push(envelope.clone());
        Ok(())
    }
}

#[derive(Default)]
struct FailingTarget;

#[derive(Debug, thiserror::Error)]
#[error("failing target")]
struct FailingTargetError;

#[async_trait]
impl DatagramDispatchTarget for Arc<FailingTarget> {
    type Error = FailingTargetError;

    async fn dispatch(&self, _envelope: &DatagramEnvelope) -> Result<(), Self::Error> {
        Err(FailingTargetError)
    }
}

#[derive(Clone)]
struct MockWssPath {
    emitted: Arc<AsyncMutex<Vec<DatagramEnvelope>>>,
}

#[derive(Debug, thiserror::Error)]
#[error("mock wss path failed")]
struct MockWssError;

#[async_trait]
impl WssDatagramPath for MockWssPath {
    type Error = MockWssError;

    async fn open_path(
        &self,
        _association_id: u64,
        _cancel: CancellationToken,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn emit_datagram(
        &self,
        envelope: &DatagramEnvelope,
        _cancel: CancellationToken,
    ) -> Result<(), Self::Error> {
        self.emitted.lock().await.push(envelope.clone());
        Ok(())
    }
}

fn target_addr(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

fn sample_envelope(association_id: u64) -> DatagramEnvelope {
    DatagramEnvelope {
        association_id,
        relay_client_addr: target_addr(50000),
        target: DatagramTarget::Ip(target_addr(443)),
        payload: vec![0xde, 0xad],
    }
}

#[test]
fn open_association_records_owned_udp_state() {
    let (dispatch, capture) = test_tracing_dispatch();
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let registry = Arc::new(UdpAssociationRegistry::new(1));
    let manager = DatagramSessionManager::new(
        registry.clone(),
        Arc::new(RecordingTarget::default()),
        Arc::new(RecordingTarget::default()),
    );

    let (association_id, record) = manager
        .open_association(target_addr(40000), target_addr(50000), Instant::now())
        .expect("open association");

    assert_eq!(association_id, 1);
    assert_eq!(record.relay_addr, target_addr(40000));
    assert_eq!(registry.association_count(), 1);
    assert!(capture.lines().iter().any(|line| line.contains(
        "[DatagramSessionManager][openAssociation][BLOCK_OPEN_DATAGRAM_ASSOCIATION]"
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn outbound_dispatch_refreshes_activity_and_hits_outbound_target() {
    let (dispatch, capture) = test_tracing_dispatch();
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let registry = Arc::new(UdpAssociationRegistry::new(1));
    let outbound = Arc::new(RecordingTarget::default());
    let manager = DatagramSessionManager::new(
        registry.clone(),
        outbound.clone(),
        Arc::new(RecordingTarget::default()),
    );
    let now = Instant::now();
    let later = now + Duration::from_secs(5);
    let (association_id, _) = manager
        .open_association(target_addr(40000), target_addr(50000), now)
        .expect("open association");

    manager
        .forward_outbound_datagram(sample_envelope(association_id), later)
        .await
        .expect("dispatch outbound");

    assert_eq!(outbound.seen.lock().expect("seen").len(), 1);
    assert_eq!(
        registry.get(association_id).expect("registry record").last_activity,
        later
    );
    let lines = capture.lines();
    assert!(lines.iter().any(|line| line.contains(
        "[DatagramSessionManager][forwardOutboundDatagram][BLOCK_FORWARD_OUTBOUND_DATAGRAM] dispatching outbound datagram"
    )));
    assert!(lines.iter().any(|line| line.contains(
        "[DatagramSessionManager][forwardOutboundDatagram][BLOCK_FORWARD_OUTBOUND_DATAGRAM] outbound datagram reached manager dispatch target"
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn inbound_dispatch_refreshes_activity_and_hits_inbound_target() {
    let (dispatch, capture) = test_tracing_dispatch();
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let registry = Arc::new(UdpAssociationRegistry::new(1));
    let inbound = Arc::new(RecordingTarget::default());
    let manager = DatagramSessionManager::new(
        registry.clone(),
        Arc::new(RecordingTarget::default()),
        inbound.clone(),
    );
    let now = Instant::now();
    let later = now + Duration::from_secs(5);
    let (association_id, _) = manager
        .open_association(target_addr(40000), target_addr(50000), now)
        .expect("open association");

    manager
        .forward_inbound_datagram(sample_envelope(association_id), later)
        .await
        .expect("dispatch inbound");

    assert_eq!(inbound.seen.lock().expect("seen").len(), 1);
    assert_eq!(
        registry.get(association_id).expect("registry record").last_activity,
        later
    );
    let lines = capture.lines();
    assert!(lines.iter().any(|line| line.contains(
        "[DatagramSessionManager][forwardInboundDatagram][BLOCK_FORWARD_INBOUND_DATAGRAM] dispatching inbound datagram"
    )));
    assert!(lines.iter().any(|line| line.contains(
        "[DatagramSessionManager][forwardInboundDatagram][BLOCK_FORWARD_INBOUND_DATAGRAM] inbound datagram reached manager dispatch target"
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn accept_outbound_datagram_opens_and_dispatches_owned_association() {
    let (dispatch, capture) = test_tracing_dispatch();
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let registry = Arc::new(UdpAssociationRegistry::new(1));
    let outbound = Arc::new(RecordingTarget::default());
    let manager = DatagramSessionManager::new(
        registry.clone(),
        outbound.clone(),
        Arc::new(RecordingTarget::default()),
    );
    let now = Instant::now();

    let envelope = manager
        .accept_outbound_datagram(
            target_addr(40000),
            target_addr(50000),
            DatagramTarget::Ip(target_addr(443)),
            vec![0xaa, 0xbb],
            now,
        )
        .await
        .expect("accept outbound datagram");

    assert_eq!(envelope.association_id, 1);
    assert_eq!(registry.association_count(), 1);
    assert_eq!(outbound.seen.lock().expect("seen").len(), 1);
    let lines = capture.lines();
    let open_idx = lines
        .iter()
        .position(|line| {
            line.contains("[DatagramSessionManager][openAssociation][BLOCK_OPEN_DATAGRAM_ASSOCIATION]")
        })
        .expect("open association marker should be present");
    let accept_idx = lines
        .iter()
        .position(|line| {
            line.contains(
                "[DatagramSessionManager][acceptOutboundDatagram][BLOCK_ACCEPT_OUTBOUND_DATAGRAM]",
            )
        })
        .expect("accept outbound marker should be present");
    let dispatch_idx = lines
        .iter()
        .position(|line| {
            line.contains(
                "[DatagramSessionManager][forwardOutboundDatagram][BLOCK_FORWARD_OUTBOUND_DATAGRAM] dispatching outbound datagram",
            )
        })
        .expect("outbound dispatch marker should be present");
    assert!(open_idx < accept_idx);
    assert!(accept_idx < dispatch_idx);
}

#[tokio::test]
async fn accept_outbound_datagram_reuses_existing_owned_association() {
    let registry = Arc::new(UdpAssociationRegistry::new(2));
    let outbound = Arc::new(RecordingTarget::default());
    let manager = DatagramSessionManager::new(
        registry.clone(),
        outbound.clone(),
        Arc::new(RecordingTarget::default()),
    );
    let now = Instant::now();

    let first = manager
        .accept_outbound_datagram(
            target_addr(40000),
            target_addr(50000),
            DatagramTarget::Ip(target_addr(443)),
            vec![0xaa],
            now,
        )
        .await
        .expect("first datagram");
    let second = manager
        .accept_outbound_datagram(
            target_addr(40000),
            target_addr(50000),
            DatagramTarget::Ip(target_addr(444)),
            vec![0xbb],
            now + Duration::from_secs(1),
        )
        .await
        .expect("second datagram");

    assert_eq!(first.association_id, second.association_id);
    assert_eq!(registry.association_count(), 1);
    assert_eq!(outbound.seen.lock().expect("seen").len(), 2);
}

#[tokio::test]
async fn runtime_bridge_forwards_governed_datagram_into_selector_emit() {
    let registry = Arc::new(UdpAssociationRegistry::new(1));
    let emitted = Arc::new(AsyncMutex::new(Vec::new()));
    let bridge = DatagramRuntimeBridge::new(
        registry.clone(),
        DatagramTransportSelector::new(
            MockWssPath {
                emitted: emitted.clone(),
            },
            DatagramTransportSelectorConfig {
                wss_timeout: Duration::from_millis(50),
            },
        ),
        Arc::new(RecordingTarget::default()),
    );

    bridge
        .forward_runtime_datagram(
            target_addr(40000),
            target_addr(50000),
            DatagramTarget::Ip(target_addr(443)),
            vec![0xaa, 0xbb],
        )
        .await
        .expect("runtime bridge should forward datagram");

    let emitted = emitted.lock().await;
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].association_id, 1);
    assert_eq!(registry.association_count(), 1);
}

#[tokio::test]
async fn runtime_bridge_reuses_owned_association_across_packets() {
    let registry = Arc::new(UdpAssociationRegistry::new(2));
    let emitted = Arc::new(AsyncMutex::new(Vec::new()));
    let bridge = DatagramRuntimeBridge::new(
        registry.clone(),
        DatagramTransportSelector::new(
            MockWssPath {
                emitted: emitted.clone(),
            },
            DatagramTransportSelectorConfig {
                wss_timeout: Duration::from_millis(50),
            },
        ),
        Arc::new(RecordingTarget::default()),
    );

    bridge
        .forward_runtime_datagram(
            target_addr(40000),
            target_addr(50000),
            DatagramTarget::Ip(target_addr(443)),
            vec![0xaa],
        )
        .await
        .expect("first runtime datagram");
    bridge
        .forward_runtime_datagram(
            target_addr(40000),
            target_addr(50000),
            DatagramTarget::Ip(target_addr(444)),
            vec![0xbb],
        )
        .await
        .expect("second runtime datagram");

    let emitted = emitted.lock().await;
    assert_eq!(emitted.len(), 2);
    assert_eq!(emitted[0].association_id, emitted[1].association_id);
    assert_eq!(registry.association_count(), 1);
}

#[test]
fn close_association_releases_owned_state() {
    let registry = Arc::new(UdpAssociationRegistry::new(1));
    let manager = DatagramSessionManager::new(
        registry.clone(),
        Arc::new(RecordingTarget::default()),
        Arc::new(RecordingTarget::default()),
    );
    let (association_id, _) = manager
        .open_association(target_addr(40000), target_addr(50000), Instant::now())
        .expect("open association");

    let closed = manager
        .close_association(association_id)
        .expect("close association");

    assert_eq!(closed.expected_client_addr, target_addr(50000));
    assert!(registry.get(association_id).is_none());
}

#[tokio::test]
async fn outbound_dispatch_failure_is_explicit() {
    let registry = Arc::new(UdpAssociationRegistry::new(1));
    let manager = DatagramSessionManager::new(
        registry.clone(),
        Arc::new(FailingTarget),
        Arc::new(RecordingTarget::default()),
    );
    let now = Instant::now();
    let (association_id, _) = manager
        .open_association(target_addr(40000), target_addr(50000), now)
        .expect("open association");

    let later = now + Duration::from_secs(1);
    let error = manager
        .forward_outbound_datagram(sample_envelope(association_id), later)
        .await
        .expect_err("dispatch failure should surface");

    assert_eq!(
        error,
        DatagramSessionError::DispatchFailed("failing target".to_string())
    );
    assert_eq!(
        registry.get(association_id).expect("registry record").last_activity,
        later
    );
}

#[tokio::test]
async fn missing_association_is_explicit_for_dispatch() {
    let manager = DatagramSessionManager::new(
        Arc::new(UdpAssociationRegistry::new(1)),
        Arc::new(RecordingTarget::default()),
        Arc::new(RecordingTarget::default()),
    );

    let error = manager
        .forward_outbound_datagram(sample_envelope(99), Instant::now())
        .await
        .expect_err("dispatch should fail");

    assert_eq!(error, DatagramSessionError::AssociationNotFound(99));
}

// FILE: src/session/datagram_manager.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify datagram association open, outbound dispatch, inbound dispatch, and cleanup trajectories over the governed UDP registry.
//   SCOPE: Open success, explicit close, outbound dispatch, inbound dispatch, and missing-association failure behavior.
//   DEPENDS: src/session/datagram_manager.rs, src/session/udp_registry.rs, src/transport/datagram_contract.rs
//   LINKS: V-M-DATAGRAM-SESSION-MANAGER
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   open_association_records_owned_udp_state - proves opening an association allocates deterministic owned state
//   outbound_dispatch_refreshes_activity_and_hits_outbound_target - proves outbound datagrams stay associated with the correct session identity
//   inbound_dispatch_refreshes_activity_and_hits_inbound_target - proves inbound datagrams stay associated with the correct session identity
//   close_association_releases_owned_state - proves explicit close frees registry-owned state
//   missing_association_is_explicit_for_dispatch - proves dispatch on unknown association ids fails deterministically
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added deterministic datagram manager tests so UDP lifecycle and dispatch trajectories stay separately reviewable.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use super::{DatagramDispatchTarget, DatagramSessionError, DatagramSessionManager};
use crate::session::UdpAssociationRegistry;
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
}

#[tokio::test]
async fn outbound_dispatch_refreshes_activity_and_hits_outbound_target() {
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
}

#[tokio::test]
async fn inbound_dispatch_refreshes_activity_and_hits_inbound_target() {
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

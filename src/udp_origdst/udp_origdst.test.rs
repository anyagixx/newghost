// FILE: src/udp_origdst/udp_origdst.test.rs
// VERSION: 0.1.1
// START_MODULE_CONTRACT
//   PURPOSE: Verify the repo-local original-destination helper contract and runtime surfaces leave stable tuple-level evidence and deterministic forwarding behavior.
//   SCOPE: Helper contract tuple-evidence labeling and runtime forwarding checks.
//   DEPENDS: async-trait, tokio, src/udp_origdst/mod.rs, src/transport/datagram_contract.rs
//   LINKS: V-M-UDP-ORIGDST-CONTRACT, V-M-UDP-ORIGDST-RUNTIME
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   tuple_evidence_label_includes_tuple_boundaries - proves tuple evidence labels keep source, listener, target, and payload length visible
//   runtime_forwards_recovered_tuple_into_governed_handoff - proves the runtime forwards a validated recovered tuple into the governed handoff target
//   runtime_rejects_payload_length_mismatch - proves the runtime rejects a tuple whose declared payload length disagrees with the actual payload
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added deterministic runtime checks for recovered-tuple forwarding and payload-length mismatch handling.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use crate::transport::datagram_contract::DatagramTarget;

use super::{
    tuple_evidence_label, RecoveredUdpTuple, UdpOrigDstError, UdpOrigDstGovernedHandoff,
    UdpOrigDstRuntime,
};

#[test]
fn tuple_evidence_label_includes_tuple_boundaries() {
    let tuple = RecoveredUdpTuple {
        client_source_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 40000),
        helper_listener_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12000),
        original_target: DatagramTarget::Ip(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(91, 99, 128, 146)),
            55123,
        )),
        payload_len: 27,
    };

    let label = tuple_evidence_label(&tuple);

    assert!(label.contains("127.0.0.1:40000"));
    assert!(label.contains("127.0.0.1:12000"));
    assert!(label.contains("91.99.128.146:55123"));
    assert!(label.ends_with("|27"));
}

#[derive(Clone, Default)]
struct RecordingHandoff {
    calls: Arc<Mutex<Vec<(RecoveredUdpTuple, Vec<u8>)>>>,
}

#[async_trait]
impl UdpOrigDstGovernedHandoff for RecordingHandoff {
    async fn forward_recovered_tuple(
        &self,
        tuple: RecoveredUdpTuple,
        payload: Vec<u8>,
    ) -> Result<(), UdpOrigDstError> {
        self.calls
            .lock()
            .expect("recording handoff lock poisoned")
            .push((tuple, payload));
        Ok(())
    }
}

#[tokio::test]
async fn runtime_forwards_recovered_tuple_into_governed_handoff() {
    let handoff = RecordingHandoff::default();
    let runtime = UdpOrigDstRuntime::new(handoff.clone());
    let tuple = RecoveredUdpTuple {
        client_source_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 40100),
        helper_listener_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12001),
        original_target: DatagramTarget::Ip(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(149, 154, 167, 51)),
            1500,
        )),
        payload_len: 4,
    };

    runtime
        .forward_recovered_datagram(tuple.clone(), b"ping".to_vec())
        .await
        .expect("forward recovered tuple");

    let calls = handoff.calls.lock().expect("recorded calls lock poisoned");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, tuple);
    assert_eq!(calls[0].1, b"ping".to_vec());
}

#[tokio::test]
async fn runtime_rejects_payload_length_mismatch() {
    let runtime = UdpOrigDstRuntime::new(RecordingHandoff::default());
    let tuple = RecoveredUdpTuple {
        client_source_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 40101),
        helper_listener_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12002),
        original_target: DatagramTarget::Ip(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(149, 154, 167, 52)),
            1501,
        )),
        payload_len: 5,
    };

    let error = runtime
        .forward_recovered_datagram(tuple, b"ping".to_vec())
        .await
        .expect_err("payload mismatch must fail");

    assert_eq!(
        error,
        UdpOrigDstError::PayloadLengthMismatch {
            expected: 5,
            actual: 4,
        }
    );
}

// FILE: src/udp_origdst/udp_origdst.test.rs
// VERSION: 0.1.3
// START_MODULE_CONTRACT
//   PURPOSE: Verify the repo-local original-destination helper contract and runtime surfaces leave stable tuple-level evidence, deterministic forwarding behavior, and one cancellable live listener loop.
//   SCOPE: Helper contract tuple-evidence labeling, runtime forwarding, and Linux-listener loop checks.
//   DEPENDS: async-trait, tokio, tokio-util, src/udp_origdst/mod.rs, src/transport/datagram_contract.rs
//   LINKS: V-M-UDP-ORIGDST-CONTRACT, V-M-UDP-ORIGDST-RUNTIME
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   tuple_evidence_label_includes_tuple_boundaries - proves tuple evidence labels keep source, listener, target, and payload length visible
//   runtime_forwards_recovered_tuple_into_governed_handoff - proves the runtime forwards a validated recovered tuple into the governed handoff target
//   runtime_rejects_payload_length_mismatch - proves the runtime rejects a tuple whose declared payload length disagrees with the actual payload
//   runtime_runs_linux_ipv4_listener_until_cancelled - proves the live repo-local helper loop can recover and forward one packet before bounded cancellation
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.3 - Kept the live-listener runtime check while making transparent-socket enablement an explicit opt-in surface for the privileged TPROXY branch.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

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

#[tokio::test]
async fn runtime_runs_linux_ipv4_listener_until_cancelled() {
    let handoff = RecordingHandoff::default();
    let runtime = UdpOrigDstRuntime::new(handoff.clone());
    let helper_socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind helper listener");
    let helper_addr = helper_socket.local_addr().expect("helper listener addr");
    let sender = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind sender");
    let sender_addr = sender.local_addr().expect("sender addr");
    let cancel = CancellationToken::new();

    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move {
            runtime
                .run_linux_ipv4_listener_until_cancelled(helper_socket, 64, false, cancel)
                .await
        }
    });

    sleep(Duration::from_millis(25)).await;
    sender
        .send_to(b"loop", helper_addr)
        .expect("send loop packet");

    for _ in 0..20 {
        if handoff
            .calls
            .lock()
            .expect("recording handoff lock poisoned")
            .len()
            == 1
        {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    let calls = handoff.calls.lock().expect("recorded calls lock poisoned");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.client_source_addr, sender_addr);
    assert_eq!(calls[0].0.helper_listener_addr, helper_addr);
    assert_eq!(calls[0].0.original_target, DatagramTarget::Ip(helper_addr));
    assert_eq!(calls[0].1, b"loop".to_vec());
    drop(calls);

    cancel.cancel();
    task.await
        .expect("listener task join")
        .expect("listener loop should stop cleanly");
}

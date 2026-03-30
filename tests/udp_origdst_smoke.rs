// FILE: tests/udp_origdst_smoke.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Prove the repo-local original-destination helper smoke packet recovers more than one tuple and forwards each into governed handoff while preserving one separate baseline probe.
//   SCOPE: Two-tuple Linux original-destination recovery, governed-handoff recording, tuple-evidence labeling, and a separate loopback baseline-preserve probe.
//   DEPENDS: async-trait, tokio, n0wss::transport::datagram_contract, n0wss::udp_origdst
//   LINKS: V-M-UDP-ORIGDST-SMOKE, LV-032, DF-UDP-ORIGDST-SMOKE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   udp_origdst_smoke_recovers_two_distinct_tuples_and_preserves_baseline_probe - proves two distinct recovered tuples each reach governed handoff while a separate preserved baseline probe stays reachable
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added the Phase-41 smoke packet for two-tuple original-destination recovery and separate baseline-preserve proof.
// END_CHANGE_SUMMARY

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;

use n0wss::transport::datagram_contract::DatagramTarget;
use n0wss::udp_origdst::linux::{enable_ipv4_recv_original_dst, recv_recovered_ipv4_datagram};
use n0wss::udp_origdst::{
    tuple_evidence_label, RecoveredUdpTuple, UdpOrigDstError, UdpOrigDstGovernedHandoff,
    UdpOrigDstHelperConfig, UdpOrigDstRuntime,
};

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
async fn udp_origdst_smoke_recovers_two_distinct_tuples_and_preserves_baseline_probe() {
    // START_BLOCK_UDP_ORIGDST_SMOKE
    let baseline_listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind baseline listener");
    baseline_listener
        .set_nonblocking(false)
        .expect("baseline listener blocking mode");
    let baseline_addr = baseline_listener
        .local_addr()
        .expect("baseline listener addr");
    let baseline_thread = thread::spawn(move || {
        let (mut stream, _) = baseline_listener.accept().expect("accept baseline probe");
        stream.write_all(b"ok").expect("write baseline probe response");
    });

    let helper_config = UdpOrigDstHelperConfig {
        listener_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 10073),
        preserve_baseline_proxy_addr: baseline_addr,
    };

    let handoff = RecordingHandoff::default();
    let runtime = UdpOrigDstRuntime::new(handoff.clone());

    let receiver_one = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind receiver one");
    let receiver_two = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind receiver two");
    enable_ipv4_recv_original_dst(&receiver_one).expect("enable origdst receiver one");
    enable_ipv4_recv_original_dst(&receiver_two).expect("enable origdst receiver two");

    let sender_one = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind sender one");
    let sender_two = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind sender two");

    let receiver_one_addr = receiver_one.local_addr().expect("receiver one addr");
    let receiver_two_addr = receiver_two.local_addr().expect("receiver two addr");
    let sender_one_addr = sender_one.local_addr().expect("sender one addr");
    let sender_two_addr = sender_two.local_addr().expect("sender two addr");

    sender_one
        .send_to(b"alpha", receiver_one_addr)
        .expect("send alpha tuple");
    sender_two
        .send_to(b"beta!", receiver_two_addr)
        .expect("send beta tuple");

    let recovered_one = recv_recovered_ipv4_datagram(&receiver_one, receiver_one_addr, 64)
        .expect("recover tuple one");
    let recovered_two = recv_recovered_ipv4_datagram(&receiver_two, receiver_two_addr, 64)
        .expect("recover tuple two");

    let tuple_one_label = tuple_evidence_label(&recovered_one.tuple);
    let tuple_two_label = tuple_evidence_label(&recovered_two.tuple);
    assert_ne!(tuple_one_label, tuple_two_label);
    assert!(tuple_one_label.contains(&sender_one_addr.to_string()));
    assert!(tuple_one_label.contains(&receiver_one_addr.to_string()));
    assert!(tuple_two_label.contains(&sender_two_addr.to_string()));
    assert!(tuple_two_label.contains(&receiver_two_addr.to_string()));

    runtime
        .forward_recovered_datagram(recovered_one.tuple.clone(), recovered_one.payload.clone())
        .await
        .expect("forward tuple one");
    runtime
        .forward_recovered_datagram(recovered_two.tuple.clone(), recovered_two.payload.clone())
        .await
        .expect("forward tuple two");

    let calls = handoff.calls.lock().expect("recorded calls lock poisoned");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0.client_source_addr, sender_one_addr);
    assert_eq!(calls[0].0.helper_listener_addr, receiver_one_addr);
    assert_eq!(calls[0].0.original_target, DatagramTarget::Ip(receiver_one_addr));
    assert_eq!(calls[0].1, b"alpha".to_vec());
    assert_eq!(calls[1].0.client_source_addr, sender_two_addr);
    assert_eq!(calls[1].0.helper_listener_addr, receiver_two_addr);
    assert_eq!(calls[1].0.original_target, DatagramTarget::Ip(receiver_two_addr));
    assert_eq!(calls[1].1, b"beta!".to_vec());
    drop(calls);

    let baseline_probe = TcpStream::connect_timeout(&helper_config.preserve_baseline_proxy_addr, Duration::from_secs(1))
        .expect("baseline probe remains reachable");
    baseline_probe
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("set baseline probe read timeout");
    drop(baseline_probe);
    baseline_thread.join().expect("baseline thread join");
    // END_BLOCK_UDP_ORIGDST_SMOKE
}

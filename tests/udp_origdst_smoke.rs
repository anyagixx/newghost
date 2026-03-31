// FILE: tests/udp_origdst_smoke.rs
// VERSION: 0.2.0
// START_MODULE_CONTRACT
//   PURPOSE: Prove the repo-local original-destination helper smoke packets recover bounded tuples for both the Phase-41 recovery branch and the Phase-45 non-OUTPUT branch while preserving a separate baseline probe.
//   SCOPE: Two-tuple Linux original-destination recovery, non-helper tuple recovery under the bounded non-OUTPUT plan markers, governed-handoff recording, tuple-evidence labeling, and a separate loopback baseline-preserve probe.
//   DEPENDS: async-trait, tokio, n0wss::transport::datagram_contract, n0wss::udp_origdst
//   LINKS: V-M-UDP-ORIGDST-SMOKE, V-M-TPROXY-NONOUTPUT-SMOKE, LV-032, LV-036, DF-UDP-ORIGDST-SMOKE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   udp_origdst_smoke_recovers_two_distinct_tuples_and_preserves_baseline_probe - proves two distinct recovered tuples each reach governed handoff while a separate preserved baseline probe stays reachable
//   udp_origdst_nonoutput_smoke_recovers_non_helper_tuple_and_preserves_baseline_probe - proves the bounded non-OUTPUT smoke packet keeps route-mark and local-delivery plan proof separate while forwarding one recovered non-helper tuple into governed handoff
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.2.0 - Added the Phase-45 non-OUTPUT smoke packet for one recovered non-helper tuple plus separate route-mark, local-delivery, and baseline-preserve proof.
// END_CHANGE_SUMMARY

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;

use n0wss::transport::datagram_contract::DatagramTarget;
use n0wss::udp_origdst::linux::{
    enable_ipv4_recv_original_dst, plan_linux_nonoutput_tproxy, recv_recovered_ipv4_datagram,
};
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

#[tokio::test]
async fn udp_origdst_nonoutput_smoke_recovers_non_helper_tuple_and_preserves_baseline_probe() {
    // START_BLOCK_TPROXY_NONOUTPUT_SMOKE
    let baseline_listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind non-output baseline listener");
    baseline_listener
        .set_nonblocking(false)
        .expect("non-output baseline listener blocking mode");
    let baseline_addr = baseline_listener
        .local_addr()
        .expect("non-output baseline listener addr");
    let baseline_thread = thread::spawn(move || {
        let (mut stream, _) = baseline_listener
            .accept()
            .expect("accept non-output baseline probe");
        stream
            .write_all(b"ok")
            .expect("write non-output baseline probe response");
    });

    let helper_listener_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 10073);
    let nonoutput_plan = plan_linux_nonoutput_tproxy(helper_listener_addr);
    assert_eq!(nonoutput_plan.listener_addr, helper_listener_addr);
    assert_eq!(nonoutput_plan.host_output_marker, "output-owner-mark-only");
    assert_eq!(nonoutput_plan.route_marker, "policy-routing-fwmark");
    assert_eq!(nonoutput_plan.ingress_marker, "veth-netns-ingress");
    assert_eq!(nonoutput_plan.interception_chain_marker, "prerouting-tproxy");
    assert_eq!(nonoutput_plan.route_localnet_marker, "route-localnet");
    assert_eq!(nonoutput_plan.rp_filter_marker, "rp-filter-relaxed");
    assert!(nonoutput_plan.requires_transparent_socket);

    let helper_config = UdpOrigDstHelperConfig {
        listener_addr: helper_listener_addr,
        preserve_baseline_proxy_addr: baseline_addr,
    };

    let handoff = RecordingHandoff::default();
    let runtime = UdpOrigDstRuntime::new(handoff.clone());

    let receiver = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind non-output receiver");
    enable_ipv4_recv_original_dst(&receiver).expect("enable non-output origdst receiver");
    let receiver_addr = receiver.local_addr().expect("non-output receiver addr");

    let sender = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind non-output sender");
    let sender_addr = sender.local_addr().expect("non-output sender addr");
    sender
        .send_to(b"phase45-nonoutput", receiver_addr)
        .expect("send non-output tuple");

    let recovered = recv_recovered_ipv4_datagram(&receiver, helper_listener_addr, 128)
        .expect("recover non-output tuple");
    let tuple_label = tuple_evidence_label(&recovered.tuple);
    assert!(tuple_label.contains(&sender_addr.to_string()));
    assert!(tuple_label.contains(&helper_listener_addr.to_string()));
    assert!(tuple_label.contains(&receiver_addr.to_string()));
    assert_ne!(
        recovered.tuple.original_target,
        DatagramTarget::Ip(helper_listener_addr)
    );
    assert_eq!(
        recovered.tuple.original_target,
        DatagramTarget::Ip(receiver_addr)
    );

    runtime
        .forward_recovered_datagram(recovered.tuple.clone(), recovered.payload.clone())
        .await
        .expect("forward non-output tuple");

    let calls = handoff.calls.lock().expect("recorded non-output calls lock poisoned");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.client_source_addr, sender_addr);
    assert_eq!(calls[0].0.helper_listener_addr, helper_listener_addr);
    assert_eq!(calls[0].0.original_target, DatagramTarget::Ip(receiver_addr));
    assert_eq!(calls[0].1, b"phase45-nonoutput".to_vec());
    drop(calls);

    let baseline_probe =
        TcpStream::connect_timeout(&helper_config.preserve_baseline_proxy_addr, Duration::from_secs(1))
            .expect("non-output baseline probe remains reachable");
    baseline_probe
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("set non-output baseline probe read timeout");
    drop(baseline_probe);
    baseline_thread.join().expect("non-output baseline thread join");
    // END_BLOCK_TPROXY_NONOUTPUT_SMOKE
}

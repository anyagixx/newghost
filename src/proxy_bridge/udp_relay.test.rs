// FILE: src/proxy_bridge/udp_relay.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify server-side UDP relay outbound delivery, inbound association mapping, and foreign-source rejection.
//   SCOPE: Outbound datagram relay, inbound datagram receive, and unexpected-source failure behavior.
//   DEPENDS: src/proxy_bridge/udp_relay.rs, src/transport/datagram_contract.rs
//   LINKS: V-M-UDP-EGRESS-RELAY
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   outbound_datagram_reaches_remote_udp_target - proves outbound relay reaches the resolved UDP peer
//   inbound_datagram_returns_to_owning_association - proves inbound packets are mapped back to the owning association
//   foreign_inbound_source_is_rejected - proves unexpected inbound UDP sources are rejected deterministically
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added deterministic UDP relay tests so outbound or inbound relay semantics stay association-scoped and reviewable.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use tokio::net::UdpSocket;

use super::{relay_inbound_datagram, relay_outbound_datagram, UdpRelayError};
use crate::transport::datagram_contract::{DatagramEnvelope, DatagramTarget};

fn sample_envelope(target: SocketAddr) -> DatagramEnvelope {
    DatagramEnvelope {
        association_id: 21,
        relay_client_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 55000),
        target: DatagramTarget::Ip(target),
        payload: b"hello-udp".to_vec(),
    }
}

#[tokio::test]
async fn outbound_datagram_reaches_remote_udp_target() {
    let remote = UdpSocket::bind("127.0.0.1:0").await.expect("remote bind");
    let target = remote.local_addr().expect("remote addr");
    let envelope = sample_envelope(target);

    let relay = relay_outbound_datagram(&envelope)
        .await
        .expect("relay outbound");

    let mut buffer = [0_u8; 64];
    let (bytes_read, source) = remote.recv_from(&mut buffer).await.expect("recv");
    assert_eq!(&buffer[..bytes_read], b"hello-udp");
    let relay_addr = relay.relay_socket.local_addr().expect("relay addr");
    assert_eq!(source.port(), relay_addr.port());
    assert!(source.ip().is_loopback());
}

#[tokio::test]
async fn inbound_datagram_returns_to_owning_association() {
    let remote = UdpSocket::bind("127.0.0.1:0").await.expect("remote bind");
    let target = remote.local_addr().expect("remote addr");
    let envelope = sample_envelope(target);

    let relay = relay_outbound_datagram(&envelope)
        .await
        .expect("relay outbound");
    let mut buffer = [0_u8; 64];
    let (_bytes_read, source) = remote.recv_from(&mut buffer).await.expect("recv outbound");
    remote
        .send_to(b"world-udp", source)
        .await
        .expect("send inbound");

    let inbound = relay_inbound_datagram(&relay).await.expect("relay inbound");
    assert_eq!(inbound.association_id, 21);
    assert_eq!(inbound.target, DatagramTarget::Ip(target));
    assert_eq!(inbound.payload, b"world-udp".to_vec());
}

#[tokio::test]
async fn foreign_inbound_source_is_rejected() {
    let remote = UdpSocket::bind("127.0.0.1:0").await.expect("remote bind");
    let target = remote.local_addr().expect("remote addr");
    let foreign = UdpSocket::bind("127.0.0.1:0").await.expect("foreign bind");
    let envelope = sample_envelope(target);

    let relay = relay_outbound_datagram(&envelope)
        .await
        .expect("relay outbound");
    let mut buffer = [0_u8; 64];
    let (_bytes_read, source) = remote.recv_from(&mut buffer).await.expect("recv outbound");
    let relay_addr = source;
    foreign
        .send_to(b"foreign", relay_addr)
        .await
        .expect("send foreign");

    let error = relay_inbound_datagram(&relay)
        .await
        .expect_err("unexpected source must fail");
    assert_eq!(
        error,
        UdpRelayError::UnexpectedSource {
            expected: target,
            actual: foreign.local_addr().expect("foreign addr"),
        }
    );
}

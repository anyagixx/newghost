// FILE: src/socks5/udp_associate.test.rs
// VERSION: 0.1.2
// START_MODULE_CONTRACT
//   PURPOSE: Verify governed SOCKS5 UDP ASSOCIATE relay binds, deterministic UDP relay packet normalization, and deterministic packet encoding for inbound relay delivery.
//   SCOPE: Success replies for UDP ASSOCIATE, domain-target packet parsing, outbound-to-inbound packet encoding, fragmentation rejection, and foreign-source rejection.
//   DEPENDS: src/socks5/udp_associate.rs, src/transport/datagram_contract.rs
//   LINKS: V-M-SOCKS5-UDP-ASSOCIATE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   udp_associate_returns_governed_relay_bind - proves UDP ASSOCIATE allocates a relay bind that follows the control socket local address and replies with it
//   parses_udp_datagram_into_transport_contract - proves a SOCKS5 UDP packet becomes a bounded datagram envelope
//   encodes_transport_contract_back_into_udp_packet - proves one inbound datagram envelope can be encoded back into the SOCKS5 UDP relay packet shape
//   fragmented_udp_packets_are_rejected - proves unsupported SOCKS5 UDP fragmentation is rejected deterministically
//   foreign_udp_source_is_rejected - proves association-owned relay parsing rejects packets from a foreign sender
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.2 - Tightened UDP ASSOCIATE expectations so the returned relay bind follows the control-socket local address instead of assuming localhost forever.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

use super::{encode_udp_datagram, handle_udp_associate, parse_udp_datagram, UdpAssociateError};
use crate::transport::datagram_contract::{DatagramEnvelope, DatagramTarget};

async fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let client = TcpStream::connect(addr).await.expect("client connect");
    let (server, _) = listener.accept().await.expect("accept");
    (client, server)
}

#[tokio::test]
async fn udp_associate_returns_governed_relay_bind() {
    let (mut client, mut server) = tcp_pair().await;

    let task = tokio::spawn(async move { handle_udp_associate(&mut server).await });
    let mut reply = [0_u8; 10];
    client.read_exact(&mut reply).await.expect("read udp reply");

    let record = task.await.expect("join").expect("udp associate");
    let expected_ip = client.local_addr().expect("control local addr").ip();
    assert_eq!(reply[0], 0x05);
    assert_eq!(reply[1], 0x00);
    assert_eq!(reply[3], 0x01);
    assert_eq!(
        IpAddr::V4(Ipv4Addr::new(reply[4], reply[5], reply[6], reply[7])),
        expected_ip
    );
    assert_eq!(record.relay_addr.ip(), expected_ip);
    assert_eq!(record.relay_socket.local_addr().expect("udp local addr"), record.relay_addr);
}

#[test]
fn parses_udp_datagram_into_transport_contract() {
    let source = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53000);
    let packet = [
        0x00, 0x00, 0x00, 0x03, 0x0b, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'c',
        b'o', b'm', 0x01, 0xbb, 0xde, 0xad, 0xbe, 0xef,
    ];

    let envelope = parse_udp_datagram(source, source, &packet, 41).expect("parse udp packet");
    assert_eq!(envelope.association_id, 41);
    assert_eq!(envelope.relay_client_addr, source);
    assert_eq!(
        envelope.target,
        DatagramTarget::Domain("example.com".to_string(), 443)
    );
    assert_eq!(envelope.payload, vec![0xde, 0xad, 0xbe, 0xef]);
}

#[test]
fn encodes_transport_contract_back_into_udp_packet() {
    let source = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53000);
    let envelope = DatagramEnvelope {
        association_id: 41,
        relay_client_addr: source,
        target: DatagramTarget::Domain("example.com".to_string(), 443),
        payload: vec![0xde, 0xad, 0xbe, 0xef],
    };

    let packet = encode_udp_datagram(&envelope).expect("encode udp packet");
    let reparsed =
        parse_udp_datagram(source, source, packet.as_slice(), envelope.association_id)
            .expect("roundtrip parse");
    assert_eq!(reparsed.target, envelope.target);
    assert_eq!(reparsed.payload, envelope.payload);
}

#[test]
fn fragmented_udp_packets_are_rejected() {
    let source = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53000);
    let packet = [0x00, 0x00, 0x01, 0x01, 127, 0, 0, 1, 0x01, 0xbb, 0xaa];

    let error = parse_udp_datagram(source, source, &packet, 41).expect_err("reject fragment");
    assert_eq!(error, UdpAssociateError::UnsupportedFragmentation(0x01));
}

#[test]
fn foreign_udp_source_is_rejected() {
    let expected = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53000);
    let actual = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53001);
    let packet = [0x00, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0x01, 0xbb, 0xaa];

    let error = parse_udp_datagram(actual, expected, &packet, 41).expect_err("reject foreign");
    assert_eq!(
        error,
        UdpAssociateError::ForeignSource {
            expected,
            actual,
        }
    );
}

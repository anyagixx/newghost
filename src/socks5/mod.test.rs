// FILE: src/socks5/mod.test.rs
// VERSION: 0.1.3
// START_MODULE_CONTRACT
//   PURPOSE: Verify SOCKS5 request parsing, bounded queue failure behavior, UDP ASSOCIATE control handling, live UDP runtime-loop forwarding, and success-reply socket semantics.
//   SCOPE: Valid CONNECT parsing, UDP ASSOCIATE control-path acceptance, bounded live UDP relay forwarding, queue saturation failure replies, closed-queue behavior, and success replies that keep the client socket open.
//   DEPENDS: src/socks5/mod.rs
//   LINKS: V-M-SOCKS5, VF-002, VF-005
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   parses_valid_connect_request_into_proxy_intent - proves a valid CONNECT request becomes a normalized ProxyIntent
//   udp_associate_returns_reply_without_queueing_connect_intent - proves the live listener accepts UDP ASSOCIATE without leaking into the CONNECT work queue
//   udp_associate_runtime_loop_forwards_governed_datagram - proves the live UDP relay loop forwards the first governed packet into the bounded runtime handoff target
//   queue_saturation_returns_failure_reply_quickly - proves saturated queues fail fast with a client-visible reply
//   closed_queue_returns_failure_reply_and_closes_stream - proves closed queue handling returns failure and closes the client stream
//   success_reply_does_not_close_client_socket - proves a success reply leaves the client socket open for payload pumping
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.3 - Tightened UDP ASSOCIATE reply expectations so tests follow the control-socket local address rather than hardcoded localhost bytes.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::sync::Mutex;

use super::udp_associate::UdpAssociateError;
use crate::transport::datagram_contract::DatagramTarget;
use super::{ProxyIntent, ProxyProtocol, Socks5Proxy, Socks5ProxyConfig, TargetAddr};

#[derive(Default)]
struct RecordingUdpRuntimeTarget {
    forwarded: Mutex<Vec<(SocketAddr, SocketAddr, DatagramTarget, Vec<u8>)>>,
}

#[async_trait]
impl super::udp_associate::UdpRelayRuntimeTarget for RecordingUdpRuntimeTarget {
    async fn forward_runtime_datagram(
        &self,
        relay_addr: SocketAddr,
        expected_client_addr: SocketAddr,
        target: DatagramTarget,
        payload: Vec<u8>,
    ) -> Result<(), UdpAssociateError> {
        self.forwarded
            .lock()
            .await
            .push((relay_addr, expected_client_addr, target, payload));
        Ok(())
    }
}

async fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let client = TcpStream::connect(addr).await.expect("client connect");
    let (server, _) = listener.accept().await.expect("accept");
    (client, server)
}

async fn send_no_auth_connect_domain(stream: &mut TcpStream, domain: &str, port: u16) -> [u8; 2] {
    let mut request = vec![0x05, 0x01, 0x00, 0x05, 0x01, 0x00, 0x03, domain.len() as u8];
    request.extend_from_slice(domain.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await.expect("write request");

    let mut auth_reply = [0_u8; 2];
    stream
        .read_exact(&mut auth_reply)
        .await
        .expect("read auth reply");
    auth_reply
}

async fn send_no_auth_udp_associate_ipv4(stream: &mut TcpStream, port: u16) -> [u8; 2] {
    let request = [0x05, 0x01, 0x00, 0x05, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, (port >> 8) as u8, (port & 0xff) as u8];
    stream.write_all(&request).await.expect("write request");

    let mut auth_reply = [0_u8; 2];
    stream
        .read_exact(&mut auth_reply)
        .await
        .expect("read auth reply");
    auth_reply
}

fn governed_udp_packet(target: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x00, 0x00, 0x00, 0x01];
    packet.extend_from_slice(&match target.ip() {
        IpAddr::V4(ip) => ip.octets(),
        IpAddr::V6(_) => panic!("test only supports ipv4"),
    });
    packet.extend_from_slice(&target.port().to_be_bytes());
    packet.extend_from_slice(payload);
    packet
}

#[tokio::test]
async fn parses_valid_connect_request_into_proxy_intent() {
    let (mut client, server) = tcp_pair().await;
    let parse_task = tokio::spawn(async move { Socks5Proxy::parse_request(server).await });

    let auth_reply = send_no_auth_connect_domain(&mut client, "example.com", 443).await;
    assert_eq!(auth_reply, [0x05, 0x00]);

    let intent = parse_task
        .await
        .expect("join parse task")
        .expect("parse request");

    assert_eq!(intent.protocol_kind, ProxyProtocol::Socks5);
    assert_eq!(
        intent.target,
        TargetAddr::Domain("example.com".to_string(), 443)
    );
    assert!(intent.request_id > 0);
}

#[tokio::test]
async fn udp_associate_returns_reply_without_queueing_connect_intent() {
    let (tx, mut rx) = mpsc::channel::<ProxyIntent>(1);
    let proxy = Socks5Proxy::new(
        Socks5ProxyConfig {
            listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            total_timeout: Duration::from_secs(2),
        },
        tx,
    );

    let (mut client, server) = tcp_pair().await;
    let handler_task = tokio::spawn({
        let proxy = proxy.clone();
        async move { proxy.handle_connection_inner(server).await }
    });

    let auth_reply = send_no_auth_udp_associate_ipv4(&mut client, 0).await;
    assert_eq!(auth_reply, [0x05, 0x00]);

    let mut reply = [0_u8; 10];
    client.read_exact(&mut reply).await.expect("read udp reply");
    let expected_ip = client.local_addr().expect("control local addr").ip();
    assert_eq!(reply[0], 0x05);
    assert_eq!(reply[1], 0x00);
    assert_eq!(reply[3], 0x01);
    assert_eq!(
        IpAddr::V4(Ipv4Addr::new(reply[4], reply[5], reply[6], reply[7])),
        expected_ip
    );

    client.shutdown().await.expect("close control stream");
    handler_task
        .await
        .expect("join udp associate handler")
        .expect("udp associate should succeed");
    assert!(rx.try_recv().is_err(), "udp associate must not enqueue CONNECT work");
}

#[tokio::test]
async fn udp_associate_runtime_loop_forwards_governed_datagram() {
    let (tx, _rx) = mpsc::channel::<ProxyIntent>(1);
    let runtime_target = Arc::new(RecordingUdpRuntimeTarget::default());
    let proxy = Socks5Proxy::new(
        Socks5ProxyConfig {
            listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            total_timeout: Duration::from_secs(2),
        },
        tx,
    )
    .with_udp_runtime_target(runtime_target.clone());

    let (mut client, server) = tcp_pair().await;
    let handler_task = tokio::spawn({
        let proxy = proxy.clone();
        async move { proxy.handle_connection_inner(server).await }
    });

    let auth_reply = send_no_auth_udp_associate_ipv4(&mut client, 0).await;
    assert_eq!(auth_reply, [0x05, 0x00]);

    let mut reply = [0_u8; 10];
    client.read_exact(&mut reply).await.expect("read udp reply");
    let relay_addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(reply[4], reply[5], reply[6], reply[7])),
        u16::from_be_bytes([reply[8], reply[9]]),
    );

    let client_udp = UdpSocket::bind("127.0.0.1:0").await.expect("bind client udp");
    let client_udp_addr = client_udp.local_addr().expect("client udp addr");
    let target = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53);
    let payload = b"phase25".to_vec();
    let packet = governed_udp_packet(target, &payload);
    client_udp
        .send_to(&packet, relay_addr)
        .await
        .expect("send governed udp packet");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if runtime_target.forwarded.lock().await.len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime target receives forwarded datagram");

    let forwarded = runtime_target.forwarded.lock().await;
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded[0].0, relay_addr);
    assert_eq!(forwarded[0].1, client_udp_addr);
    assert_eq!(forwarded[0].2, DatagramTarget::Ip(target));
    assert_eq!(forwarded[0].3, payload);
    drop(forwarded);

    client.shutdown().await.expect("close control stream");
    handler_task
        .await
        .expect("join udp associate handler")
        .expect("udp associate runtime loop should succeed");
}

#[tokio::test]
async fn queue_saturation_returns_failure_reply_quickly() {
    let (tx, mut rx) = mpsc::channel::<ProxyIntent>(1);
    let proxy = Socks5Proxy::new(
        Socks5ProxyConfig {
            listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            total_timeout: Duration::from_secs(2),
        },
        tx,
    );

    let (mut first_client, first_server) = tcp_pair().await;
    let first_task = tokio::spawn(async move { Socks5Proxy::parse_request(first_server).await });
    let _ = send_no_auth_connect_domain(&mut first_client, "example.com", 443).await;
    let first_intent = first_task
        .await
        .expect("join parse task")
        .expect("parse request");
    proxy
        .intent_tx
        .try_send(first_intent)
        .expect("queue first intent");

    let (mut second_client, second_server) = tcp_pair().await;
    let start = Instant::now();
    let handler_task = tokio::spawn({
        let proxy = proxy.clone();
        async move { proxy.handle_connection_inner(second_server).await }
    });

    let _ = send_no_auth_connect_domain(&mut second_client, "example.com", 443).await;
    let mut reply = [0_u8; 10];
    second_client
        .read_exact(&mut reply)
        .await
        .expect("read failure reply");

    assert!(start.elapsed() < Duration::from_millis(50));
    assert_eq!(reply[1], 0x01);
    handler_task
        .await
        .expect("join handler")
        .expect("handler result");
    assert!(rx.recv().await.is_some());
}

#[tokio::test]
async fn closed_queue_returns_failure_reply_and_closes_stream() {
    let (tx, rx) = mpsc::channel::<ProxyIntent>(1);
    let proxy = Socks5Proxy::new(
        Socks5ProxyConfig {
            listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            total_timeout: Duration::from_secs(2),
        },
        tx,
    );
    drop(rx);

    let (mut client, server) = tcp_pair().await;
    let handler_task = tokio::spawn({
        let proxy = proxy.clone();
        async move { proxy.handle_connection_inner(server).await }
    });

    let _ = send_no_auth_connect_domain(&mut client, "example.com", 443).await;
    let mut reply = [0_u8; 10];
    client
        .read_exact(&mut reply)
        .await
        .expect("read failure reply");

    assert_eq!(reply[1], 0x01);
    let mut eof = [0_u8; 1];
    let read = client.read(&mut eof).await.expect("read eof after close");
    assert_eq!(read, 0);

    handler_task
        .await
        .expect("join handler")
        .expect("handler result");
}

#[tokio::test]
async fn success_reply_does_not_close_client_socket() {
    let (mut client, mut server) = tcp_pair().await;

    let send_task = tokio::spawn(async move {
        Socks5Proxy::send_reply(&mut server, super::Socks5Reply::Succeeded)
            .await
            .expect("send reply");

        let mut trailing = [0_u8; 1];
        server
            .read_exact(&mut trailing)
            .await
            .expect("read trailing byte");
        trailing[0]
    });

    let mut reply = [0_u8; 10];
    client.read_exact(&mut reply).await.expect("read success reply");
    assert_eq!(reply[1], 0x00);

    client.write_all(&[0x7f]).await.expect("write trailing byte");

    let trailing = send_task.await.expect("join send task");
    assert_eq!(trailing, 0x7f);
}

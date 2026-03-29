// FILE: src/wss_gateway/mod.test.rs
// VERSION: 0.1.2
// START_MODULE_CONTRACT
//   PURPOSE: Verify TLS-backed WSS stream establishment, production datagram-carrier handshake, server-side datagram ingress, cancellation handling, and adapter cleanup guarantees.
//   SCOPE: Successful WSS handshake and byte relay, production datagram-path open and emit behavior, server-side relay delivery, pre-open cancellation, mid-open cancellation, and failed-open cleanup.
//   DEPENDS: src/wss_gateway/mod.rs, src/auth/mod.rs, src/obs/mod.rs, src/tls/mod.rs, src/transport/adapter_contract.rs, src/transport/stream.rs, src/proxy_bridge/udp_relay.rs
//   LINKS: V-M-WSS-GATEWAY, VF-003, VF-008
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   valid_tls_plus_wss_handshake_returns_resolved_stream - proves TLS plus WSS handshake yields a usable resolved stream
//   datagram_open_path_uses_runtime_handshake - proves the production datagram carrier performs auth plus datagram-ready handshake over the real websocket runtime
//   datagram_emit_sends_binary_frame_after_runtime_handshake - proves the production datagram carrier emits a governed binary datagram frame after the runtime handshake
//   server_runtime_relays_datagram_to_udp_target - proves the real server runtime demuxes DATAGRAM OPEN and relays the emitted datagram to a real UDP target
//   cancel_before_open_returns_err_and_spawns_no_tasks - proves pre-open cancellation leaves no tracked tasks behind
//   cancel_during_open_returns_err_and_socket_closes - proves mid-open cancellation closes the socket and surfaces cancellation
//   failed_open_does_not_leak_tracked_tasks - proves failed opens do not leak adapter tasks
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.2 - Added a real run_server datagram-ingress test so runtime glue now proves server-side relay invocation instead of only mock or handshake-only coverage.
// END_CHANGE_SUMMARY

use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::time::sleep;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthPolicy, AuthPolicyConfig};
use crate::obs::{init_observability, ObservabilityConfig};
use crate::session::WssDatagramPath;
use crate::tls::{TlsConfig, TlsContextHandle};
use crate::transport::adapter_contract::{TransportAdapter, TransportRequest};
use crate::transport::datagram_contract::{DatagramEnvelope, DatagramTarget};
use crate::transport::stream::TransportKind;
use crate::wss_gateway::datagram::decode_message;

use super::{GatewayConfig, WssError, WssGateway};

fn write_tls_fixture() -> (tempfile::TempDir, TlsContextHandle) {
    let dir = tempdir().expect("tempdir should build");
    let key_pair = KeyPair::generate().expect("key pair should build");
    let mut params = CertificateParams::new(vec!["localhost".to_string()])
        .expect("certificate params should build");
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, "localhost");
    params.distinguished_name = distinguished_name;
    let cert = params
        .self_signed(&key_pair)
        .expect("certificate should build");

    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    fs::write(&cert_path, cert.pem()).expect("cert should write");
    fs::write(&key_path, key_pair.serialize_pem()).expect("key should write");

    let tls = TlsContextHandle::from_config(&TlsConfig {
        cert_path: cert_path.clone(),
        key_path,
        trust_anchor_path: cert_path,
    })
    .expect("tls should load");

    (dir, tls)
}

fn build_gateway(
    server_addr: std::net::SocketAddr,
    tls: TlsContextHandle,
    token: &str,
) -> WssGateway {
    let observability = init_observability(ObservabilityConfig {
        service_name: "n0wss".to_string(),
        mode_label: "test".to_string(),
        burst_detection: crate::config::BurstDetectionConfig {
            alert_threshold: 10,
            alert_window: Duration::from_secs(1),
            min_log_interval: Duration::from_secs(5),
            ring_capacity: 128,
        },
        peak_reset_interval: Duration::from_secs(60),
    })
    .expect("observability should initialize");

    WssGateway::new(GatewayConfig {
        server_addr,
        server_name: "localhost".to_string(),
        websocket_uri: "wss://localhost/stream".parse().expect("uri should parse"),
        auth_token: token.to_string(),
        tls_context: tls,
        auth_policy: AuthPolicy::from_config(AuthPolicyConfig {
            expected_token: token.to_string(),
        })
        .expect("policy should build"),
        metrics: observability.metrics,
    })
}

async fn accept_datagram_runtime_handshake(
    listener: TcpListener,
    tls: TlsContextHandle,
) -> (u64, Option<DatagramEnvelope>) {
    let (stream, _) = listener.accept().await.expect("accept should work");
    let tls_stream = tls.accept(stream).await.expect("tls accept");
    let mut websocket = accept_async(tls_stream).await.expect("accept websocket");

    let auth_message = websocket.next().await.expect("auth frame").expect("auth ok");
    assert!(matches!(auth_message, Message::Binary(_)));
    websocket
        .send(Message::Text("ok".into()))
        .await
        .expect("send auth ack");

    let open_message = websocket.next().await.expect("open frame").expect("open ok");
    let open_text = match open_message {
        Message::Text(text) => text.to_string(),
        other => panic!("unexpected datagram open message: {other:?}"),
    };
    let association_id = open_text
        .strip_prefix("DATAGRAM OPEN ")
        .expect("datagram open prefix")
        .parse::<u64>()
        .expect("association id");
    websocket
        .send(Message::Text("datagram-ready".into()))
        .await
        .expect("send datagram ready");

    let maybe_datagram = match websocket.next().await {
        Some(Ok(Message::Binary(bytes))) => Some(decode_message(Message::Binary(bytes)).expect("decode datagram")),
        Some(Ok(Message::Close(_))) | None => None,
        Some(Ok(other)) => panic!("unexpected frame after datagram ready: {other:?}"),
        Some(Err(err)) => panic!("unexpected websocket error: {err}"),
    };

    (association_id, maybe_datagram)
}

#[tokio::test]
async fn valid_tls_plus_wss_handshake_returns_resolved_stream() {
    let (_dir, tls) = write_tls_fixture();
    let upstream_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("upstream listener should bind");
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("upstream addr should resolve");
    let upstream_task = tokio::spawn(async move {
        let (mut upstream_stream, _) = upstream_listener.accept().await.expect("accept upstream");
        let mut buffer = [0_u8; 9];
        upstream_stream
            .read_exact(&mut buffer)
            .await
            .expect("upstream should read bytes");
        assert_eq!(&buffer, b"hello-wss");
        upstream_stream
            .write_all(b"hello-wss")
            .await
            .expect("upstream should write bytes");
    });
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let server_addr = listener.local_addr().expect("listener addr should resolve");
    let server_gateway = build_gateway(server_addr, tls.clone(), "token-12345");
    let server_task = tokio::spawn({
        let gateway = server_gateway.clone();
        async move { gateway.run_server(listener).await }
    });

    let client_gateway = build_gateway(server_addr, tls, "token-12345");
    let resolved = client_gateway
        .open_stream(
            &TransportRequest {
                peer_label: "client-01".to_string(),
                target_host: upstream_addr.ip().to_string(),
                target_port: upstream_addr.port(),
            },
            CancellationToken::new(),
        )
        .await
        .expect("open_stream should succeed");

    assert_eq!(resolved.transport_kind, TransportKind::Wss);
    assert_eq!(resolved.stream.peer_label(), "client-01");

    let (mut reader, mut writer) = resolved.stream.split();
    writer
        .write_all(b"hello-wss")
        .await
        .expect("write should succeed");
    let mut buffer = [0_u8; 9];
    reader
        .read_exact(&mut buffer)
        .await
        .expect("read should succeed");
    assert_eq!(&buffer, b"hello-wss");

    client_gateway.stop_accept();
    server_gateway.stop_accept();
    upstream_task.await.expect("upstream task should join");
    assert!(server_task.await.expect("server task should join").is_ok());
}

#[tokio::test]
async fn datagram_open_path_uses_runtime_handshake() {
    let (_dir, tls) = write_tls_fixture();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let server_addr = listener.local_addr().expect("listener addr should resolve");
    let server_tls = tls.clone();
    let handshake_task = tokio::spawn(async move {
        accept_datagram_runtime_handshake(listener, server_tls).await
    });

    let gateway = build_gateway(server_addr, tls, "token-12345");
    gateway
        .open_path(41, CancellationToken::new())
        .await
        .expect("open_path should succeed");

    let (association_id, maybe_datagram) = handshake_task.await.expect("join handshake task");
    assert_eq!(association_id, 41);
    assert!(maybe_datagram.is_none(), "open_path should stop after runtime handshake");
}

#[tokio::test]
async fn datagram_emit_sends_binary_frame_after_runtime_handshake() {
    let (_dir, tls) = write_tls_fixture();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let server_addr = listener.local_addr().expect("listener addr should resolve");
    let server_tls = tls.clone();
    let handshake_task = tokio::spawn(async move {
        accept_datagram_runtime_handshake(listener, server_tls).await
    });

    let gateway = build_gateway(server_addr, tls, "token-12345");
    let envelope = DatagramEnvelope {
        association_id: 52,
        relay_client_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 19000),
        target: DatagramTarget::Ip(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53)),
        payload: b"phase25-dgram".to_vec(),
    };

    gateway
        .emit_datagram(&envelope, CancellationToken::new())
        .await
        .expect("emit_datagram should succeed");

    let (association_id, maybe_datagram) = handshake_task.await.expect("join handshake task");
    assert_eq!(association_id, 52);
    assert_eq!(maybe_datagram.expect("datagram frame"), envelope);
}

#[tokio::test]
async fn server_runtime_relays_datagram_to_udp_target() {
    let (_dir, tls) = write_tls_fixture();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let server_addr = listener.local_addr().expect("listener addr should resolve");
    let server_gateway = build_gateway(server_addr, tls.clone(), "token-12345");
    let server_task = tokio::spawn({
        let gateway = server_gateway.clone();
        async move { gateway.run_server(listener).await }
    });

    let remote = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("remote udp target should bind");
    let remote_addr = remote.local_addr().expect("remote addr should resolve");

    let client_gateway = build_gateway(server_addr, tls, "token-12345");
    let envelope = DatagramEnvelope {
        association_id: 77,
        relay_client_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 19100),
        target: DatagramTarget::Ip(remote_addr),
        payload: b"phase25-runtime-server".to_vec(),
    };

    client_gateway
        .emit_datagram(&envelope, CancellationToken::new())
        .await
        .expect("emit_datagram should succeed");

    let mut buffer = [0_u8; 128];
    let (bytes_read, _source) = tokio::time::timeout(Duration::from_secs(2), remote.recv_from(&mut buffer))
        .await
        .expect("server relay should reach target in time")
        .expect("remote recv should succeed");
    assert_eq!(&buffer[..bytes_read], envelope.payload.as_slice());

    client_gateway.stop_accept();
    server_gateway.stop_accept();
    assert!(server_task.await.expect("server task should join").is_ok());
}

#[tokio::test]
async fn cancel_before_open_returns_err_and_spawns_no_tasks() {
    let (_dir, tls) = write_tls_fixture();
    let gateway = build_gateway("127.0.0.1:65535".parse().expect("addr"), tls, "token-12345");
    let cancel = CancellationToken::new();
    cancel.cancel();

    let err = gateway
        .open_stream(
            &TransportRequest {
                peer_label: "client-01".to_string(),
                target_host: "127.0.0.1".to_string(),
                target_port: 65535,
            },
            cancel,
        )
        .await
        .err()
        .expect("cancelled open_stream must fail");

    assert_eq!(err, WssError::Cancelled);
    assert_eq!(gateway.task_tracker().alive_count(), 0);
}

#[tokio::test]
async fn cancel_during_open_returns_err_and_socket_closes() {
    let (_dir, tls) = write_tls_fixture();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let server_addr = listener.local_addr().expect("listener addr should resolve");
    let gateway = build_gateway(server_addr, tls, "token-12345");
    let cancel = CancellationToken::new();

    let accept_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept should work");
        sleep(Duration::from_millis(300)).await;
        let mut probe = [0_u8; 32];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            let read = tokio::time::timeout_at(deadline, stream.read(&mut probe)).await;
            match read {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break 0,
                Ok(Ok(_)) => continue,
            }
        }
    });

    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        sleep(Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });

    let err = gateway
        .open_stream(
            &TransportRequest {
                peer_label: "client-01".to_string(),
                target_host: "127.0.0.1".to_string(),
                target_port: server_addr.port(),
            },
            cancel,
        )
        .await
        .err()
        .expect("cancelled open_stream must fail");

    assert_eq!(err, WssError::Cancelled);
    assert_eq!(gateway.task_tracker().alive_count(), 0);
    assert_eq!(accept_task.await.expect("accept task should join"), 0);
}

#[tokio::test]
async fn failed_open_does_not_leak_tracked_tasks() {
    let (_dir, tls) = write_tls_fixture();
    let gateway = build_gateway("127.0.0.1:9".parse().expect("addr"), tls, "token-12345");

    let err = gateway
        .open_stream(
            &TransportRequest {
                peer_label: "client-01".to_string(),
                target_host: "127.0.0.1".to_string(),
                target_port: 9,
            },
            CancellationToken::new(),
        )
        .await
        .err()
        .expect("connection refused must fail");

    assert!(matches!(err, WssError::TcpConnectFailed(_)));
    assert_eq!(gateway.task_tracker().alive_count(), 0);
}

use std::fs;
use std::time::Duration;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthPolicy, AuthPolicyConfig};
use crate::obs::{init_observability, ObservabilityConfig};
use crate::tls::{TlsConfig, TlsContextHandle};
use crate::transport::adapter_contract::{TransportAdapter, TransportRequest};
use crate::transport::stream::TransportKind;

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

#[tokio::test]
async fn valid_tls_plus_wss_handshake_returns_resolved_stream() {
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

    let client_gateway = build_gateway(server_addr, tls, "token-12345");
    let resolved = client_gateway
        .open_stream(
            &TransportRequest {
                peer_label: "client-01".to_string(),
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
            },
            CancellationToken::new(),
        )
        .await
        .err()
        .expect("connection refused must fail");

    assert!(matches!(err, WssError::TcpConnectFailed(_)));
    assert_eq!(gateway.task_tracker().alive_count(), 0);
}

use std::time::Duration;

use iroh::Endpoint;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::obs::{init_observability, ObservabilityConfig};
use crate::transport::adapter_contract::{TransportAdapter, TransportRequest};
use crate::transport::stream::TransportKind;

use super::{IrohAdapter, IrohAdapterConfig, IrohError, IROH_ALPN_MARKER};

const ALPN: &[u8] = b"n0wss/iroh-test";

async fn make_endpoint() -> Endpoint {
    Endpoint::empty_builder()
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .expect("endpoint should bind")
}

fn metrics() -> crate::obs::ProxyMetricsHandle {
    init_observability(ObservabilityConfig {
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
    .expect("observability should initialize")
    .metrics
}

async fn spawn_echo_server(endpoint: Endpoint) {
    let incoming = endpoint
        .accept()
        .await
        .expect("incoming connection should exist");
    let connection = incoming.await.expect("connecting should complete");
    let (mut send_stream, mut recv_stream) = connection.accept_bi().await.expect("accept_bi");
    let mut marker = [0_u8; 1];
    recv_stream.read_exact(&mut marker).await.expect("marker");
    assert_eq!(marker[0], IROH_ALPN_MARKER);

    send_stream
        .write_all(&[IROH_ALPN_MARKER])
        .await
        .expect("initial echo marker");

    let mut buffer = [0_u8; 8192];
    loop {
        match recv_stream.read(&mut buffer).await {
            Ok(Some(0)) | Ok(None) => {
                let _ = send_stream.finish();
                break;
            }
            Ok(Some(bytes_read)) => {
                send_stream
                    .write_all(&buffer[..bytes_read])
                    .await
                    .expect("echo payload");
            }
            Err(_) => {
                let _ = send_stream.finish();
                break;
            }
        }
    }
}

#[tokio::test]
async fn opens_direct_stream_with_stable_diagnostics() {
    let server = make_endpoint().await;
    let client = make_endpoint().await;
    let server_addr = server.addr();

    let server_task = tokio::spawn(spawn_echo_server(server.clone()));

    let adapter = IrohAdapter::new(IrohAdapterConfig {
        endpoint: client.clone(),
        remote_addr: server_addr,
        alpn: ALPN.to_vec(),
        metrics: metrics(),
    });

    let resolved = adapter
        .open_stream(
            &TransportRequest {
                peer_label: "iroh-peer".to_string(),
                target_host: "example.com".to_string(),
                target_port: 443,
            },
            CancellationToken::new(),
        )
        .await
        .expect("open_stream should succeed");

    assert_eq!(resolved.transport_kind, TransportKind::IrohDirect);
    assert_eq!(resolved.stream.peer_label(), "iroh-peer");

    let (mut reader, mut writer) = resolved.stream.split();
    writer.write_all(b"hello-iroh").await.expect("write");
    let mut buffer = [0_u8; 10];
    reader.read_exact(&mut buffer).await.expect("read");
    assert_eq!(&buffer, b"hello-iroh");

    drop(writer);
    drop(reader);
    adapter.release().await.expect("release should succeed");
    server_task.await.expect("server task should join");
    client.close().await;
}

#[tokio::test]
async fn cancel_before_open_returns_err_and_no_tracked_tasks() {
    let server = make_endpoint().await;
    let client = make_endpoint().await;
    let adapter = IrohAdapter::new(IrohAdapterConfig {
        endpoint: client.clone(),
        remote_addr: server.addr(),
        alpn: ALPN.to_vec(),
        metrics: metrics(),
    });

    let cancel = CancellationToken::new();
    cancel.cancel();

    let err = adapter
        .open_stream(
            &TransportRequest {
                peer_label: "iroh-peer".to_string(),
                target_host: "example.com".to_string(),
                target_port: 443,
            },
            cancel,
        )
        .await
        .err()
        .expect("cancelled open_stream must fail");

    assert_eq!(err, IrohError::Cancelled);
    assert_eq!(adapter.task_tracker().alive_count(), 0);

    adapter.release().await.expect("release should succeed");
    server.close().await;
}

#[tokio::test]
async fn failed_open_does_not_leak_tracked_tasks() {
    let endpoint = make_endpoint().await;
    let remote_addr = endpoint.addr();

    let adapter = IrohAdapter::new(IrohAdapterConfig {
        endpoint: endpoint.clone(),
        remote_addr,
        alpn: ALPN.to_vec(),
        metrics: metrics(),
    });

    let err = adapter
        .open_stream(
            &TransportRequest {
                peer_label: "iroh-peer".to_string(),
                target_host: "example.com".to_string(),
                target_port: 443,
            },
            CancellationToken::new(),
        )
        .await
        .err()
        .expect("failed open must return err");

    assert!(matches!(
        err,
        IrohError::TransportUnavailable(_) | IrohError::StreamOpenFailed(_)
    ));
    assert_eq!(adapter.task_tracker().alive_count(), 0);

    adapter.release().await.expect("release should succeed");
    endpoint.close().await;
}

#[tokio::test]
async fn release_closes_endpoint() {
    let endpoint = make_endpoint().await;
    let remote = make_endpoint().await;
    let adapter = IrohAdapter::new(IrohAdapterConfig {
        endpoint: endpoint.clone(),
        remote_addr: remote.addr(),
        alpn: ALPN.to_vec(),
        metrics: metrics(),
    });

    adapter.release().await.expect("release should succeed");
    sleep(Duration::from_millis(50)).await;
    assert!(endpoint.is_closed());
    remote.close().await;
}

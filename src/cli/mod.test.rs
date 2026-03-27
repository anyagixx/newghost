// FILE: src/cli/mod.test.rs
// VERSION: 0.1.2
// START_MODULE_CONTRACT
//   PURPOSE: Verify deterministic CLI bootstrap, runtime launch, and shutdown sequencing for client and server startup paths.
//   SCOPE: Client startup, server startup, optional client TLS bootstrap, runtime listener binding, and shutdown ordering.
//   DEPENDS: src/cli/mod.rs, src/tls/mod.rs, src/wss_gateway/mod.rs, src/socks5/mod.rs
//   LINKS: V-M-CLI, V-M-TLS
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   selects_client_mode_on_valid_startup - proves baseline client bootstrap
//   selects_client_mode_and_builds_tls_when_trust_anchor_is_configured - proves optional client TLS bootstrap
//   selects_server_mode_and_builds_tls_on_valid_startup - proves server TLS bootstrap
//   server_runtime_binds_listener_until_cancelled - proves server runtime stays alive and binds a socket until cancellation
//   client_runtime_binds_socks5_listener_until_cancelled - proves client runtime stays alive and binds a SOCKS5 socket until cancellation
//   shutdown_stops_accepts_before_drain_and_release - proves deterministic shutdown ordering
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.2 - Added runtime launch coverage so server and client modes prove real listener binding before shutdown.
// END_CHANGE_SUMMARY

use std::fs;
use std::net::TcpListener as StdTcpListener;
use std::time::Duration;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use tempfile::tempdir;
use tokio::net::TcpStream;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::{coordinate_shutdown, run_from, run_until_shutdown_from, ApplicationMode, ShutdownState};

fn write_server_tls_fixture() -> (tempfile::TempDir, String, String) {
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

    let cert_path = dir.path().join("server-cert.pem");
    let key_path = dir.path().join("server-key.pem");
    fs::write(&cert_path, cert.pem()).expect("cert should write");
    fs::write(&key_path, key_pair.serialize_pem()).expect("key should write");

    (
        dir,
        cert_path.display().to_string(),
        key_path.display().to_string(),
    )
}

#[test]
fn selects_client_mode_on_valid_startup() {
    let run_result = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "client",
        "--remote-wss-url",
        "wss://example.com/tunnel",
    ])
    .expect("client startup should succeed");

    assert_eq!(run_result.mode, ApplicationMode::Client);
    assert!(run_result.startup.tls_context.is_none());
    assert_eq!(
        run_result.startup.session_config.idle_timeout,
        std::time::Duration::from_secs(10)
    );
    assert!(run_result.shutdown.can_accept_new_work());
}

#[test]
fn selects_client_mode_and_builds_tls_when_trust_anchor_is_configured() {
    let (_dir, cert_path, _key_path) = write_server_tls_fixture();

    let run_result = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "client",
        "--remote-wss-url",
        "wss://example.com/tunnel",
        "--tls-trust-anchor-path",
        cert_path.as_str(),
        "--tls-server-name-override",
        "example.com",
    ])
    .expect("client startup with trust anchor should succeed");

    assert_eq!(run_result.mode, ApplicationMode::Client);
    assert!(run_result.startup.tls_context.is_some());
    assert_eq!(
        run_result.startup.tls_context.expect("tls context").leaf_subject,
        "CN=localhost"
    );
}

#[test]
fn selects_server_mode_and_builds_tls_on_valid_startup() {
    let (_dir, cert_path, key_path) = write_server_tls_fixture();

    let run_result = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "server",
        "--tls-cert-path",
        cert_path.as_str(),
        "--tls-key-path",
        key_path.as_str(),
    ])
    .expect("server startup should succeed");

    assert_eq!(run_result.mode, ApplicationMode::Server);
    assert!(run_result.startup.tls_context.is_some());
    assert_eq!(
        run_result.startup.session_config.graceful_shutdown_timeout,
        std::time::Duration::from_secs(60)
    );
}

fn reserve_local_addr() -> String {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("ephemeral listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    drop(listener);
    addr.to_string()
}

async fn wait_for_listener(addr: &str) {
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return;
        }
        sleep(Duration::from_millis(20)).await;
    }

    panic!("listener {addr} did not become reachable");
}

#[tokio::test]
async fn server_runtime_binds_listener_until_cancelled() {
    let (_dir, cert_path, key_path) = write_server_tls_fixture();
    let listen_addr = reserve_local_addr();
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let cert_path = cert_path.clone();
        let key_path = key_path.clone();
        let listen_addr = listen_addr.clone();
        async move {
            run_until_shutdown_from(
                [
                    "n0wss",
                    "--auth-token",
                    "token-12345",
                    "server",
                    "--listen-addr",
                    listen_addr.as_str(),
                    "--tls-cert-path",
                    cert_path.as_str(),
                    "--tls-key-path",
                    key_path.as_str(),
                ],
                cancel,
            )
            .await
        }
    });

    wait_for_listener(&listen_addr).await;
    cancel.cancel();

    task.await
        .expect("server runtime task should join")
        .expect("server runtime should shut down cleanly");
}

#[tokio::test]
async fn client_runtime_binds_socks5_listener_until_cancelled() {
    let (_dir, cert_path, _key_path) = write_server_tls_fixture();
    let listen_addr = reserve_local_addr();
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let cert_path = cert_path.clone();
        let listen_addr = listen_addr.clone();
        async move {
            run_until_shutdown_from(
                [
                    "n0wss",
                    "--auth-token",
                    "token-12345",
                    "client",
                    "--listen-addr",
                    listen_addr.as_str(),
                    "--remote-wss-url",
                    "wss://127.0.0.1:7443/tunnel",
                    "--tls-trust-anchor-path",
                    cert_path.as_str(),
                ],
                cancel,
            )
            .await
        }
    });

    wait_for_listener(&listen_addr).await;
    cancel.cancel();

    task.await
        .expect("client runtime task should join")
        .expect("client runtime should shut down cleanly");
}

#[test]
fn shutdown_stops_accepts_before_drain_and_release() {
    let run_result = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "client",
        "--remote-wss-url",
        "wss://example.com/tunnel",
    ])
    .expect("client startup should succeed");

    assert!(run_result.shutdown.can_accept_new_work());

    let snapshot = coordinate_shutdown(&run_result.shutdown);

    assert_eq!(snapshot.state, ShutdownState::TransportReleased);
    assert!(snapshot.accepts_stopped);
    assert!(snapshot.drains_requested);
    assert!(snapshot.transports_released);
    assert!(!run_result.shutdown.can_accept_new_work());
}

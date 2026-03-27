use std::fs;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use tempfile::tempdir;

use super::{coordinate_shutdown, run_from, ApplicationMode, ShutdownState};

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
    assert!(run_result.shutdown.can_accept_new_work());
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

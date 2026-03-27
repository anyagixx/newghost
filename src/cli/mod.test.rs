// FILE: src/cli/mod.test.rs
// VERSION: 0.1.1
// START_MODULE_CONTRACT
//   PURPOSE: Verify deterministic CLI bootstrap and shutdown sequencing for client and server startup paths.
//   SCOPE: Client startup, server startup, optional client TLS bootstrap, and shutdown ordering.
//   DEPENDS: src/cli/mod.rs, src/tls/mod.rs
//   LINKS: V-M-CLI, V-M-TLS
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   selects_client_mode_on_valid_startup - proves baseline client bootstrap
//   selects_client_mode_and_builds_tls_when_trust_anchor_is_configured - proves optional client TLS bootstrap
//   selects_server_mode_and_builds_tls_on_valid_startup - proves server TLS bootstrap
//   shutdown_stops_accepts_before_drain_and_release - proves deterministic shutdown ordering
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added client-side TLS bootstrap coverage for trust-anchor-driven live startup.
// END_CHANGE_SUMMARY

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

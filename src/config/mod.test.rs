// FILE: src/config/mod.test.rs
// VERSION: 0.1.2
// START_MODULE_CONTRACT
//   PURPOSE: Verify deterministic configuration parsing and validation for the config module, including the live origdst-helper launch shape.
//   SCOPE: Success and failure cases for client, server, and origdst-live configuration.
//   DEPENDS: src/config/mod.rs
//   LINKS: V-M-CONFIG, V-M-ORIGDST-LIVE-CONFIG-SHAPE, VF-001
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   parses_valid_client_config - validates a complete client configuration
//   parses_valid_server_config - validates a complete server configuration
//   parses_valid_client_tls_config - validates client trust-anchor and endpoint-identity overrides
//   parses_valid_origdst_live_config - validates the explicit live helper launch shape
//   rejects_zero_limits - validates non-zero limits
//   rejects_zero_origdst_live_operator_uid - validates explicit operator-user targeting
//   rejects_empty_auth_token - validates token requirements
//   rejects_non_wss_remote_url - validates remote URL scheme
//   rejects_invalid_shutdown_ordering - validates graceful vs force timeouts
// END_MODULE_MAP

use super::{load_config_from, ConfigError, RuntimeMode};

fn base_args() -> Vec<&'static str> {
    vec!["n0wss", "--auth-token", "secret-token"]
}

#[test]
fn parses_valid_client_config() {
    let mut args = base_args();
    args.extend([
        "client",
        "--listen-addr",
        "127.0.0.1:1081",
        "--remote-wss-url",
        "wss://edge.example.com/tunnel",
    ]);

    let config = load_config_from(args).expect("client config must parse");

    match config.runtime_mode {
        RuntimeMode::Client(client) => {
            assert_eq!(client.listen_addr.to_string(), "127.0.0.1:1081");
            assert_eq!(
                client.remote_wss_url.as_str(),
                "wss://edge.example.com/tunnel"
            );
        }
        RuntimeMode::Server(_) | RuntimeMode::OrigDstLive(_) => panic!("expected client mode"),
    }

    assert_eq!(config.limits.max_pending_intents, 128);
    assert_eq!(config.timeouts.iroh_connect_timeout.as_secs(), 2);
}

#[test]
fn parses_valid_server_config() {
    let mut args = base_args();
    args.extend([
        "server",
        "--listen-addr",
        "0.0.0.0:7443",
        "--tls-cert-path",
        "certs/server.pem",
        "--tls-key-path",
        "certs/server.key",
    ]);

    let config = load_config_from(args).expect("server config must parse");

    match config.runtime_mode {
        RuntimeMode::Server(server) => {
            assert_eq!(server.listen_addr.to_string(), "0.0.0.0:7443");
            assert_eq!(server.tls_cert_path.to_string_lossy(), "certs/server.pem");
            assert_eq!(server.tls_key_path.to_string_lossy(), "certs/server.key");
        }
        RuntimeMode::Client(_) | RuntimeMode::OrigDstLive(_) => panic!("expected server mode"),
    }
}

#[test]
fn parses_valid_client_tls_config() {
    let mut args = base_args();
    args.extend([
        "client",
        "--remote-wss-url",
        "wss://edge.example.com/tunnel",
        "--tls-trust-anchor-path",
        "certs/live-ca.pem",
        "--tls-server-name-override",
        "ghost-srv.example.internal",
    ]);

    let config = load_config_from(args).expect("client tls config must parse");

    match config.runtime_mode {
        RuntimeMode::Client(client) => {
            let tls = client.tls.expect("client tls config should be present");
            assert_eq!(tls.trust_anchor_path.to_string_lossy(), "certs/live-ca.pem");
            assert_eq!(
                tls.server_name_override.as_deref(),
                Some("ghost-srv.example.internal")
            );
        }
        RuntimeMode::Server(_) | RuntimeMode::OrigDstLive(_) => panic!("expected client mode"),
    }
}

#[test]
fn parses_valid_origdst_live_config() {
    let mut args = base_args();
    args.extend([
        "origdst-live",
        "--listener-addr",
        "127.0.0.1:10073",
        "--payload-capacity-bytes",
        "65507",
        "--operator-uid",
        "1000",
        "--preserve-baseline-proxy-addr",
        "127.0.0.1:1080",
    ]);

    let config = load_config_from(args).expect("origdst live config must parse");

    match config.runtime_mode {
        RuntimeMode::OrigDstLive(live) => {
            assert_eq!(live.listener_addr.to_string(), "127.0.0.1:10073");
            assert_eq!(live.payload_capacity_bytes, 65_507);
            assert_eq!(live.operator_uid, 1000);
            assert_eq!(
                live.preserve_baseline_proxy_addr.to_string(),
                "127.0.0.1:1080"
            );
        }
        RuntimeMode::Client(_) | RuntimeMode::Server(_) => panic!("expected origdst-live mode"),
    }
}

#[test]
fn rejects_zero_limits() {
    let mut args = base_args();
    args.extend([
        "--max-pending-intents",
        "0",
        "client",
        "--remote-wss-url",
        "wss://edge.example.com/tunnel",
    ]);

    let err = load_config_from(args).expect_err("zero queue limit must fail");
    assert_eq!(
        err,
        ConfigError::NonPositiveValue {
            field: "max_pending_intents"
        }
    );
}

#[test]
fn rejects_zero_origdst_live_operator_uid() {
    let mut args = base_args();
    args.extend([
        "origdst-live",
        "--operator-uid",
        "0",
    ]);

    let err = load_config_from(args).expect_err("zero operator uid must fail");
    assert_eq!(err, ConfigError::NonPositiveValue { field: "operator_uid" });
}

#[test]
fn rejects_empty_auth_token() {
    let args = vec![
        "n0wss",
        "--auth-token",
        "   ",
        "client",
        "--remote-wss-url",
        "wss://edge.example.com/tunnel",
    ];

    let err = load_config_from(args).expect_err("blank auth token must fail");
    assert_eq!(err, ConfigError::EmptyAuthToken);
}

#[test]
fn rejects_non_wss_remote_url() {
    let mut args = base_args();
    args.extend([
        "client",
        "--remote-wss-url",
        "https://edge.example.com/tunnel",
    ]);

    let err = load_config_from(args).expect_err("non-wss scheme must fail");
    assert_eq!(err, ConfigError::InvalidRemoteWssScheme);
}

#[test]
fn rejects_invalid_shutdown_ordering() {
    let mut args = base_args();
    args.extend([
        "--graceful-timeout-secs",
        "120",
        "--force-kill-after-secs",
        "60",
        "client",
        "--remote-wss-url",
        "wss://edge.example.com/tunnel",
    ]);

    let err = load_config_from(args).expect_err("graceful timeout above force kill must fail");
    assert_eq!(err, ConfigError::InvalidShutdownOrdering);
}

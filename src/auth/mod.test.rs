// FILE: src/auth/mod.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify deterministic auth-policy decisions, config derivation, and secret redaction behavior.
//   SCOPE: Success and failure handshake decisions, empty-policy rejection, config-derived policy wiring, and redaction invariants.
//   DEPENDS: src/auth/mod.rs, src/config/mod.rs, src/obs/mod.rs
//   LINKS: V-M-AUTH, VF-004
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   accepts_valid_credentials - proves valid credentials are accepted with tunnel permissions
//   rejects_invalid_credentials_with_redaction - proves invalid credentials are rejected with redacted detail
//   rejects_missing_credentials - proves empty credential payloads are rejected deterministically
//   rejects_empty_policy_token - proves policy construction rejects an empty expected token
//   derives_policy_from_app_config - proves auth policy can be derived from validated application config
//   auth_redaction_uses_observability_policy - proves secret redaction stays aligned with observability rules
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added GRACE markup so auth verification remains navigable for autonomous verification waves.
// END_CHANGE_SUMMARY

use crate::config::load_config_from;

use super::{
    redact_secret, AuthDecision, AuthPolicy, AuthPolicyConfig, AuthPolicyError, HandshakeMetadata,
    Permissions, RejectReason,
};

fn sample_metadata(token: &str) -> HandshakeMetadata {
    HandshakeMetadata {
        credentials: token.as_bytes().to_vec(),
        peer_label: "client-01".to_string(),
    }
}

#[test]
fn accepts_valid_credentials() {
    let policy = AuthPolicy::from_config(AuthPolicyConfig {
        expected_token: "token-12345".to_string(),
    })
    .expect("policy should initialize");

    let decision = policy.validate_handshake(&sample_metadata("token-12345"));

    match decision {
        AuthDecision::Allow(identity) => {
            assert_eq!(identity.label, "peer:client-01");
            assert_eq!(identity.permissions, Permissions::TunnelProxy);
        }
        other => panic!("expected allow decision, got {other:?}"),
    }
}

#[test]
fn rejects_invalid_credentials_with_redaction() {
    let policy = AuthPolicy::from_config(AuthPolicyConfig {
        expected_token: "token-12345".to_string(),
    })
    .expect("policy should initialize");

    let decision = policy.validate_handshake(&sample_metadata("bad-secret"));

    match decision {
        AuthDecision::Reject(rejection) => {
            assert_eq!(rejection.reason, RejectReason::InvalidCredential);
            assert_eq!(rejection.redacted_detail, "credential=ba***et");
            assert!(!rejection.redacted_detail.contains("bad-secret"));
        }
        other => panic!("expected reject decision, got {other:?}"),
    }
}

#[test]
fn rejects_missing_credentials() {
    let policy = AuthPolicy::from_config(AuthPolicyConfig {
        expected_token: "token-12345".to_string(),
    })
    .expect("policy should initialize");

    let decision = policy.validate_handshake(&HandshakeMetadata {
        credentials: Vec::new(),
        peer_label: "client-01".to_string(),
    });

    match decision {
        AuthDecision::Reject(rejection) => {
            assert_eq!(rejection.reason, RejectReason::MissingCredential);
            assert_eq!(rejection.redacted_detail, "credential missing");
        }
        other => panic!("expected reject decision, got {other:?}"),
    }
}

#[test]
fn rejects_empty_policy_token() {
    let err = AuthPolicy::from_config(AuthPolicyConfig {
        expected_token: String::new(),
    })
    .expect_err("empty token must fail");

    assert_eq!(err, AuthPolicyError::EmptyExpectedToken);
}

#[test]
fn derives_policy_from_app_config() {
    let config = load_config_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "client",
        "--remote-wss-url",
        "wss://example.com/tunnel",
    ])
    .expect("config should parse");

    let policy = AuthPolicy::from_config(AuthPolicyConfig::from_app_config(&config))
        .expect("policy should initialize");

    match policy.validate_handshake(&sample_metadata("token-12345")) {
        AuthDecision::Allow(_) => {}
        other => panic!("expected allow decision, got {other:?}"),
    }
}

#[test]
fn auth_redaction_uses_observability_policy() {
    assert_eq!(redact_secret("abcd"), "***");
    assert_eq!(redact_secret("bad-secret"), "ba***et");
}

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

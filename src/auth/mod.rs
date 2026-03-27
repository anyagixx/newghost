// FILE: src/auth/mod.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Validate tunnel admission policy for static tokens or tickets and redact sensitive values before logs or errors are emitted.
//   SCOPE: Auth policy creation, handshake validation, allow or reject decisions, and redacted failure details.
//   DEPENDS: std, thiserror, tracing, src/config/mod.rs, src/obs/mod.rs
//   LINKS: M-AUTH, M-CONFIG, M-OBS, V-M-AUTH, DF-AUTH-REJECT, VF-004
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   AuthPolicyConfig - typed policy input for token validation
//   HandshakeMetadata - auth boundary input from gateway parsing
//   AuthPolicy - reusable token validation policy
//   AuthDecision - allow or reject result
//   ValidatedIdentity - stable non-secret identity returned on success
//   AuthRejection - redacted rejection details returned on failure
//   validate_handshake - validate incoming credentials against policy
//   redact_secret - re-export redaction behavior for auth errors and logs
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Created Phase 1 auth boundary with redacted token validation and tests.
// END_CHANGE_SUMMARY

use thiserror::Error;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::obs;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPolicyConfig {
    pub expected_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeMetadata {
    pub credentials: Vec<u8>,
    pub peer_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPolicy {
    expected_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDecision {
    Allow(ValidatedIdentity),
    Reject(AuthRejection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedIdentity {
    pub label: String,
    pub permissions: Permissions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permissions {
    TunnelProxy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRejection {
    pub reason: RejectReason,
    pub redacted_detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    MissingCredential,
    InvalidCredential,
    EmptyPeerLabel,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AuthPolicyError {
    #[error("expected token must not be empty")]
    EmptyExpectedToken,
}

impl AuthPolicyConfig {
    pub fn from_app_config(config: &AppConfig) -> Self {
        Self {
            expected_token: config.auth_token.clone(),
        }
    }
}

impl AuthPolicy {
    pub fn from_config(config: AuthPolicyConfig) -> Result<Self, AuthPolicyError> {
        if config.expected_token.trim().is_empty() {
            return Err(AuthPolicyError::EmptyExpectedToken);
        }

        Ok(Self {
            expected_token: config.expected_token,
        })
    }

    // START_CONTRACT: validate_handshake
    //   PURPOSE: Authorize or reject handshake credentials without leaking secret material.
    //   INPUTS: { metadata: &HandshakeMetadata - parsed credential bytes and stable peer label }
    //   OUTPUTS: { AuthDecision - allow with stable identity or reject with redacted detail }
    //   SIDE_EFFECTS: [structured auth logs only]
    //   LINKS: [M-AUTH, M-OBS, V-M-AUTH]
    // END_CONTRACT: validate_handshake
    pub fn validate_handshake(&self, metadata: &HandshakeMetadata) -> AuthDecision {
        // START_BLOCK_VALIDATE_HANDSHAKE
        if metadata.peer_label.trim().is_empty() {
            warn!(
                "[Auth][validateHandshake][BLOCK_VALIDATE_HANDSHAKE] rejected handshake with empty peer label"
            );
            return AuthDecision::Reject(AuthRejection {
                reason: RejectReason::EmptyPeerLabel,
                redacted_detail: "peer label missing".to_string(),
            });
        }

        if metadata.credentials.is_empty() {
            warn!(
                peer = %metadata.peer_label,
                "[Auth][validateHandshake][BLOCK_VALIDATE_HANDSHAKE] rejected handshake with missing credential"
            );
            return AuthDecision::Reject(AuthRejection {
                reason: RejectReason::MissingCredential,
                redacted_detail: "credential missing".to_string(),
            });
        }

        let presented_token = String::from_utf8_lossy(&metadata.credentials);
        let redacted_token = redact_secret(presented_token.as_ref());

        if presented_token != self.expected_token {
            warn!(
                peer = %metadata.peer_label,
                credential = %redacted_token,
                "[Auth][validateHandshake][BLOCK_VALIDATE_HANDSHAKE] rejected handshake with invalid credential"
            );
            return AuthDecision::Reject(AuthRejection {
                reason: RejectReason::InvalidCredential,
                redacted_detail: format!("credential={redacted_token}"),
            });
        }

        let identity = ValidatedIdentity {
            label: format!("peer:{}", metadata.peer_label),
            permissions: Permissions::TunnelProxy,
        };

        info!(
            peer = %metadata.peer_label,
            identity = %identity.label,
            "[Auth][validateHandshake][BLOCK_VALIDATE_HANDSHAKE] accepted handshake"
        );
        AuthDecision::Allow(identity)
        // END_BLOCK_VALIDATE_HANDSHAKE
    }
}

// START_CONTRACT: redact_secret
//   PURPOSE: Normalize sensitive values before they reach auth logs or user-facing errors.
//   INPUTS: { secret: &str - raw secret value }
//   OUTPUTS: { String - redacted representation }
//   SIDE_EFFECTS: [none]
//   LINKS: [M-AUTH, M-OBS]
// END_CONTRACT: redact_secret
pub fn redact_secret(secret: &str) -> String {
    obs::redact_secret(secret)
}

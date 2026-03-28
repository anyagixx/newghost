// FILE: src/obs/mod.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify observability bootstrap, metric snapshots, config-derived mode labeling, and redaction behavior.
//   SCOPE: Successful initialization, invalid config rejection, metric updates, burst observation wiring, and secret redaction.
//   DEPENDS: src/obs/mod.rs, src/config/mod.rs
//   LINKS: V-M-OBS, VF-001, VF-009
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   initializes_observability_handles - proves subscriber and metric handles initialize deterministically
//   rejects_empty_service_name - proves invalid observability config fails fast
//   metrics_reflect_queue_limit_and_reply_code_outcomes - proves metric snapshots reflect queue and reply-code events
//   redacts_sensitive_values - proves observability redaction hides raw secrets
//   derives_mode_label_from_app_config - proves mode labels derive from validated application config
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added GRACE markup so observability verification stays navigable and reusable across later waves.
// END_CHANGE_SUMMARY

use std::time::Duration;

use crate::config::{load_config_from, BurstDetectionConfig, RuntimeMode};

use super::{
    init_observability, record_burst, redact_secret, BurstEvent, ObservabilityConfig,
    ObservabilityError,
};

fn sample_config() -> ObservabilityConfig {
    ObservabilityConfig {
        service_name: "n0wss".to_string(),
        mode_label: "client".to_string(),
        burst_detection: BurstDetectionConfig {
            alert_threshold: 3,
            alert_window: Duration::from_secs(1),
            min_log_interval: Duration::from_secs(5),
            ring_capacity: 128,
        },
        peak_reset_interval: Duration::from_secs(60),
    }
}

#[test]
fn initializes_observability_handles() {
    let handles = init_observability(sample_config()).expect("observability should initialize");

    let metrics = handles.metrics.snapshot();
    assert_eq!(handles.subscriber.service_name, "n0wss");
    assert_eq!(handles.subscriber.mode_label, "client");
    assert_eq!(metrics.intents_rejected_queue_full, 0);
    assert_eq!(metrics.peak_rate_per_sec, 0);
}

#[test]
fn rejects_empty_service_name() {
    let mut config = sample_config();
    config.service_name.clear();

    let err = init_observability(config).expect_err("empty service name must fail");

    assert_eq!(err, ObservabilityError::EmptyServiceName);
}

#[test]
fn metrics_reflect_queue_limit_and_reply_code_outcomes() {
    let handles = init_observability(sample_config()).expect("observability should initialize");

    handles.metrics.increment_intents_enqueued();
    handles.metrics.increment_sessions_rejected_limit();
    handles.metrics.increment_reply_code("0x01");
    handles.metrics.increment_reply_code("0x01");
    handles
        .metrics
        .observe_reply_duration(Duration::from_millis(42));
    handles.metrics.set_active_sessions(7);
    handles.metrics.set_intent_queue_capacity(128);
    handles.metrics.set_intent_queue_len(8);

    let observation = record_burst(
        &handles.burst_detector,
        BurstEvent {
            queue_capacity: 128,
        },
    );
    let snapshot = handles.metrics.snapshot();

    assert_eq!(observation.recent_rejections, 1);
    assert_eq!(snapshot.intents_enqueued, 1);
    assert_eq!(snapshot.sessions_rejected_limit, 1);
    assert_eq!(snapshot.intent_queue_capacity, 128);
    assert_eq!(snapshot.intent_queue_len, 128);
    assert_eq!(snapshot.active_sessions, 7);
    assert_eq!(snapshot.reply_code_counts.get("0x01"), Some(&2));
    assert_eq!(snapshot.reply_duration_ms, vec![42]);
}

#[test]
fn redacts_sensitive_values() {
    assert_eq!(redact_secret("abcd"), "***");
    assert_eq!(redact_secret("supersecret"), "su***et");
}

#[test]
fn derives_mode_label_from_app_config() {
    let config = load_config_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "server",
        "--tls-cert-path",
        "cert.pem",
        "--tls-key-path",
        "key.pem",
    ])
    .expect("config should parse");

    let obs_config = ObservabilityConfig::from_app_config(&config);
    assert_eq!(obs_config.service_name, "n0wss");
    assert_eq!(obs_config.mode_label, "server");
    assert!(matches!(config.runtime_mode, RuntimeMode::Server(_)));
}

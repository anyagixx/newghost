use std::thread;
use std::time::Duration;

use crate::config::BurstDetectionConfig;

use super::{init_observability, record_burst, reset_peak, BurstEvent, ObservabilityConfig};

fn detector_config() -> ObservabilityConfig {
    ObservabilityConfig {
        service_name: "n0wss".to_string(),
        mode_label: "client".to_string(),
        burst_detection: BurstDetectionConfig {
            alert_threshold: 3,
            alert_window: Duration::from_millis(200),
            min_log_interval: Duration::from_secs(1),
            ring_capacity: 64,
        },
        peak_reset_interval: Duration::from_secs(60),
    }
}

#[test]
fn burst_spike_updates_peak_and_emits_rate_limited_log() {
    let handles = init_observability(detector_config()).expect("observability should initialize");

    let first = record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    let second = record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    let third = record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });

    assert!(!first.emitted_log);
    assert!(!second.emitted_log);
    assert!(third.emitted_log);
    assert_eq!(third.recent_rejections, 3);
    assert_eq!(third.peak_rate_per_sec, 3);
    assert_eq!(handles.burst_detector.log_entries().len(), 1);
}

#[test]
fn slow_rejections_do_not_trigger_burst_alert() {
    let handles = init_observability(detector_config()).expect("observability should initialize");

    let first = record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    thread::sleep(Duration::from_millis(250));
    let second = record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    thread::sleep(Duration::from_millis(250));
    let third = record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });

    assert_eq!(first.recent_rejections, 1);
    assert_eq!(second.recent_rejections, 1);
    assert_eq!(third.recent_rejections, 1);
    assert!(handles.burst_detector.log_entries().is_empty());
}

#[test]
fn peak_rate_resets_on_maintenance_interval() {
    let handles = init_observability(detector_config()).expect("observability should initialize");

    record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    assert_eq!(handles.metrics.snapshot().peak_rate_per_sec, 3);

    reset_peak(&handles.burst_detector);

    assert_eq!(handles.metrics.snapshot().peak_rate_per_sec, 0);
}

#[test]
fn sustained_burst_logs_are_rate_limited() {
    let handles = init_observability(detector_config()).expect("observability should initialize");

    for _ in 0..3 {
        record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    }
    assert_eq!(handles.burst_detector.log_entries().len(), 1);

    for _ in 0..3 {
        record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    }
    assert_eq!(
        handles.burst_detector.log_entries().len(),
        1,
        "min_log_interval should suppress repeated burst logs in the same window"
    );

    thread::sleep(Duration::from_secs(1));
    for _ in 0..3 {
        record_burst(&handles.burst_detector, BurstEvent { queue_capacity: 64 });
    }
    assert_eq!(handles.burst_detector.log_entries().len(), 2);
}

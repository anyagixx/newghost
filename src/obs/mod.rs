// FILE: src/obs/mod.rs
// VERSION: 0.1.3
// START_MODULE_CONTRACT
//   PURPOSE: Initialize tracing, stable field propagation, metrics collection, sliding-window burst detection, peak-rate gauges, and redaction-aware logging behavior.
//   SCOPE: Observability bootstrap, in-memory metrics updates, burst detection, peak resets, and secret redaction helpers.
//   DEPENDS: std, thiserror, tracing, tracing-subscriber, src/config/mod.rs
//   LINKS: M-OBS, M-CONFIG, V-M-OBS, DF-BURST-OBSERVABILITY, VF-008
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   ObservabilityConfig - typed observability initialization input
//   ObservabilityHandles - tracing, metrics, and burst detector outputs
//   ProxyMetricsHandle - thread-safe counters, gauges, and histogram sink
//   BurstDetectorHandle - sliding-window burst detector with rate-limited alerts
//   TestTracingCapture - test-only in-memory sink for governed trace assertions
//   TestTracingWriter - test-only writer that clones log bytes into shared capture state
//   init_observability - create handles and emit initialization marker
//   record_burst - record queue saturation into metrics and burst detector
//   reset_peak - clear the peak-rate gauge on maintenance cadence
//   redact_secret - redact sensitive strings before logs
//   test_tracing_dispatch - build a test-only tracing dispatch and capture pair for direct anchor assertions
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.3 - Added a test-only tracing capture helper so governed tests can assert stable log anchors directly instead of inferring trajectory from outcomes alone.
// END_CHANGE_SUMMARY

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::sync::OnceLock;

use thiserror::Error;
use tracing::{info, warn, Dispatch};
use tracing_subscriber::FmtSubscriber;
#[cfg(test)]
use tracing_subscriber::fmt::MakeWriter;

use crate::config::{AppConfig, BurstDetectionConfig, RuntimeMode};

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

#[cfg(test)]
#[path = "burst_detector.test.rs"]
mod burst_detector_tests;

static GLOBAL_TRACING_DISPATCH_INSTALLED: OnceLock<()> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub service_name: String,
    pub mode_label: String,
    pub burst_detection: BurstDetectionConfig,
    pub peak_reset_interval: Duration,
}

#[derive(Debug, Clone)]
pub struct ObservabilityHandles {
    pub subscriber: TracingSubscriberHandle,
    pub metrics: ProxyMetricsHandle,
    pub burst_detector: BurstDetectorHandle,
}

#[derive(Debug, Clone)]
pub struct TracingSubscriberHandle {
    dispatch: Dispatch,
    pub service_name: String,
    pub mode_label: String,
}

#[derive(Debug, Clone)]
pub struct ProxyMetricsHandle {
    inner: Arc<Mutex<ProxyMetricsInner>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProxyMetricsSnapshot {
    pub intents_enqueued: u64,
    pub intents_rejected_queue_full: u64,
    pub sessions_rejected_limit: u64,
    pub intent_queue_len: usize,
    pub intent_queue_capacity: usize,
    pub active_sessions: usize,
    pub peak_rate_per_sec: u64,
    pub reply_code_counts: BTreeMap<String, u64>,
    pub reply_duration_ms: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct BurstDetectorHandle {
    inner: Arc<Mutex<BurstDetectorState>>,
    metrics: ProxyMetricsHandle,
    log_sink: Arc<Mutex<Vec<String>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurstEvent {
    pub queue_capacity: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurstObservation {
    pub recent_rejections: u64,
    pub total_rejected: u64,
    pub peak_rate_per_sec: u64,
    pub emitted_log: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ObservabilityError {
    #[error("service name must not be empty")]
    EmptyServiceName,
    #[error("mode label must not be empty")]
    EmptyModeLabel,
    #[error("peak reset interval must be greater than zero")]
    NonPositivePeakResetInterval,
}

#[derive(Debug, Clone)]
struct ProxyMetricsInner {
    intents_enqueued: u64,
    intents_rejected_queue_full: u64,
    sessions_rejected_limit: u64,
    intent_queue_len: usize,
    intent_queue_capacity: usize,
    active_sessions: usize,
    peak_rate_per_sec: u64,
    reply_code_counts: BTreeMap<String, u64>,
    reply_duration_ms: Vec<u64>,
}

#[derive(Debug)]
struct BurstDetectorState {
    config: BurstDetectionConfig,
    recent_rejections: VecDeque<Instant>,
    last_log_at: Option<Instant>,
    total_rejected: u64,
    peak_rate_per_sec: u64,
}

impl ObservabilityConfig {
    pub fn from_app_config(config: &AppConfig) -> Self {
        Self {
            service_name: "n0wss".to_string(),
            mode_label: match config.runtime_mode {
                RuntimeMode::Client(_) => "client".to_string(),
                RuntimeMode::Server(_) => "server".to_string(),
                RuntimeMode::OrigDstLive(_) => "origdst-live".to_string(),
            },
            burst_detection: config.burst_detection.clone(),
            peak_reset_interval: Duration::from_secs(60),
        }
    }
}

impl TracingSubscriberHandle {
    pub fn dispatch(&self) -> &Dispatch {
        &self.dispatch
    }
}

impl ProxyMetricsHandle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProxyMetricsInner {
                intents_enqueued: 0,
                intents_rejected_queue_full: 0,
                sessions_rejected_limit: 0,
                intent_queue_len: 0,
                intent_queue_capacity: 0,
                active_sessions: 0,
                peak_rate_per_sec: 0,
                reply_code_counts: BTreeMap::new(),
                reply_duration_ms: Vec::new(),
            })),
        }
    }

    pub fn increment_intents_enqueued(&self) {
        let mut inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        inner.intents_enqueued += 1;
    }

    pub fn increment_intents_rejected_queue_full(&self) {
        let mut inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        inner.intents_rejected_queue_full += 1;
    }

    pub fn increment_sessions_rejected_limit(&self) {
        let mut inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        inner.sessions_rejected_limit += 1;
    }

    pub fn set_intent_queue_len(&self, len: usize) {
        let mut inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        inner.intent_queue_len = len;
    }

    pub fn set_intent_queue_capacity(&self, capacity: usize) {
        let mut inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        inner.intent_queue_capacity = capacity;
    }

    pub fn set_active_sessions(&self, active_sessions: usize) {
        let mut inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        inner.active_sessions = active_sessions;
    }

    pub fn observe_reply_duration(&self, duration: Duration) {
        let mut inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        inner.reply_duration_ms.push(duration.as_millis() as u64);
    }

    pub fn increment_reply_code(&self, reply_code: &str) {
        let mut inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        *inner
            .reply_code_counts
            .entry(reply_code.to_string())
            .or_insert(0) += 1;
    }

    pub fn set_peak_rate_per_sec(&self, peak_rate_per_sec: u64) {
        let mut inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        inner.peak_rate_per_sec = peak_rate_per_sec;
    }

    pub fn snapshot(&self) -> ProxyMetricsSnapshot {
        let inner = self.inner.lock().expect("proxy metrics mutex poisoned");
        ProxyMetricsSnapshot {
            intents_enqueued: inner.intents_enqueued,
            intents_rejected_queue_full: inner.intents_rejected_queue_full,
            sessions_rejected_limit: inner.sessions_rejected_limit,
            intent_queue_len: inner.intent_queue_len,
            intent_queue_capacity: inner.intent_queue_capacity,
            active_sessions: inner.active_sessions,
            peak_rate_per_sec: inner.peak_rate_per_sec,
            reply_code_counts: inner.reply_code_counts.clone(),
            reply_duration_ms: inner.reply_duration_ms.clone(),
        }
    }
}

impl Default for ProxyMetricsHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[derive(Clone, Default)]
pub struct TestTracingCapture {
    buffer: Arc<Mutex<Vec<u8>>>,
}

#[cfg(test)]
pub struct TestTracingWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

#[cfg(test)]
impl std::io::Write for TestTracingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer
            .lock()
            .expect("test tracing buffer mutex poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
impl<'a> MakeWriter<'a> for TestTracingCapture {
    type Writer = TestTracingWriter;

    fn make_writer(&'a self) -> Self::Writer {
        TestTracingWriter {
            buffer: self.buffer.clone(),
        }
    }
}

#[cfg(test)]
impl TestTracingCapture {
    pub fn lines(&self) -> Vec<String> {
        String::from_utf8_lossy(
            &self
                .buffer
                .lock()
                .expect("test tracing buffer mutex poisoned"),
        )
        .lines()
        .map(|line| line.to_string())
        .collect()
    }
}

#[cfg(test)]
pub fn test_tracing_dispatch() -> (Dispatch, TestTracingCapture) {
    let capture = TestTracingCapture::default();
    let subscriber = FmtSubscriber::builder()
        .with_target(false)
        .with_ansi(false)
        .with_writer(capture.clone())
        .finish();
    (Dispatch::new(subscriber), capture)
}

impl BurstDetectorHandle {
    pub fn log_entries(&self) -> Vec<String> {
        self.log_sink
            .lock()
            .expect("burst detector log sink mutex poisoned")
            .clone()
    }
}

// START_CONTRACT: init_observability
//   PURPOSE: Create tracing, metrics, and burst-detection handles for runtime startup.
//   INPUTS: { config: ObservabilityConfig - validated observability settings derived from runtime config }
//   OUTPUTS: { Result<ObservabilityHandles, ObservabilityError> - initialized observability handles }
//   SIDE_EFFECTS: [installs a process-wide tracing subscriber on first successful initialization and emits the initialization marker]
//   LINKS: [M-OBS, M-CONFIG, V-M-OBS]
// END_CONTRACT: init_observability
pub fn init_observability(
    config: ObservabilityConfig,
) -> Result<ObservabilityHandles, ObservabilityError> {
    // START_BLOCK_INIT_TRACING
    if config.service_name.trim().is_empty() {
        return Err(ObservabilityError::EmptyServiceName);
    }
    if config.mode_label.trim().is_empty() {
        return Err(ObservabilityError::EmptyModeLabel);
    }
    if config.peak_reset_interval.is_zero() {
        return Err(ObservabilityError::NonPositivePeakResetInterval);
    }

    let subscriber = FmtSubscriber::builder()
        .with_target(false)
        .with_ansi(false)
        .finish();
    let dispatch = Dispatch::new(subscriber);
    let tracing_handle = TracingSubscriberHandle {
        dispatch: dispatch.clone(),
        service_name: config.service_name.clone(),
        mode_label: config.mode_label.clone(),
    };

    let _ = GLOBAL_TRACING_DISPATCH_INSTALLED.get_or_init(|| {
        let _ = tracing::dispatcher::set_global_default(dispatch.clone());
    });

    info!(
        service = %config.service_name,
        mode = %config.mode_label,
        peak_reset_interval_secs = config.peak_reset_interval.as_secs(),
        "[Observability][initObservability][BLOCK_INIT_TRACING] initialized observability"
    );

    let metrics = ProxyMetricsHandle::new();
    let burst_detector = BurstDetectorHandle {
        inner: Arc::new(Mutex::new(BurstDetectorState {
            config: config.burst_detection,
            recent_rejections: VecDeque::new(),
            last_log_at: None,
            total_rejected: 0,
            peak_rate_per_sec: 0,
        })),
        metrics: metrics.clone(),
        log_sink: Arc::new(Mutex::new(Vec::new())),
    };

    Ok(ObservabilityHandles {
        subscriber: tracing_handle,
        metrics,
        burst_detector,
    })
    // END_BLOCK_INIT_TRACING
}

// START_CONTRACT: record_burst
//   PURPOSE: Record a queue-rejection event into metrics and the sliding-window burst detector.
//   INPUTS: { detector: &BurstDetectorHandle - shared detector state, event: BurstEvent - queue-capacity context for the rejection }
//   OUTPUTS: { BurstObservation - current rejection-window and logging outcome }
//   SIDE_EFFECTS: [updates in-memory metrics and may emit a rate-limited warning marker]
//   LINKS: [M-OBS, V-M-OBS, DF-BURST-OBSERVABILITY]
// END_CONTRACT: record_burst
pub fn record_burst(detector: &BurstDetectorHandle, event: BurstEvent) -> BurstObservation {
    // START_BLOCK_AGGREGATE_REJECTIONS
    let now = Instant::now();
    let mut emitted_log = false;

    let (recent_rejections, total_rejected, peak_rate_per_sec, alert_window_secs) = {
        let mut state = detector
            .inner
            .lock()
            .expect("burst detector mutex poisoned");

        state.total_rejected += 1;
        state.recent_rejections.push_back(now);
        detector.metrics.increment_intents_rejected_queue_full();
        detector
            .metrics
            .set_intent_queue_capacity(event.queue_capacity);
        detector.metrics.set_intent_queue_len(event.queue_capacity);

        let cutoff = now.checked_sub(state.config.alert_window).unwrap_or(now);
        while let Some(front) = state.recent_rejections.front() {
            if *front < cutoff {
                state.recent_rejections.pop_front();
            } else {
                break;
            }
        }

        let recent_rejections = state.recent_rejections.len() as u64;
        state.peak_rate_per_sec = state.peak_rate_per_sec.max(recent_rejections);
        detector
            .metrics
            .set_peak_rate_per_sec(state.peak_rate_per_sec);

        let should_log = recent_rejections >= state.config.alert_threshold
            && state
                .last_log_at
                .map(|last_log_at| now.duration_since(last_log_at) >= state.config.min_log_interval)
                .unwrap_or(true);

        if should_log {
            state.last_log_at = Some(now);
            emitted_log = true;
        }

        (
            recent_rejections,
            state.total_rejected,
            state.peak_rate_per_sec,
            state.config.alert_window.as_secs_f32(),
        )
    };

    if emitted_log {
        let log_line = format!(
            "[Observability][recordBurst][BLOCK_AGGREGATE_REJECTIONS] rejection burst detected recent_rejections={recent_rejections} window_secs={alert_window_secs} total_rejected={total_rejected} peak_rate_per_sec={peak_rate_per_sec} queue_capacity={}",
            event.queue_capacity
        );
        detector
            .log_sink
            .lock()
            .expect("burst detector log sink mutex poisoned")
            .push(log_line.clone());
        warn!(
            recent_rejections,
            window_secs = alert_window_secs,
            total_rejected,
            peak_rate_per_sec,
            queue_capacity = event.queue_capacity,
            "[Observability][recordBurst][BLOCK_AGGREGATE_REJECTIONS] rejection burst detected"
        );
    }

    BurstObservation {
        recent_rejections,
        total_rejected,
        peak_rate_per_sec,
        emitted_log,
    }
    // END_BLOCK_AGGREGATE_REJECTIONS
}

// START_CONTRACT: reset_peak
//   PURPOSE: Reset the peak-rate gauge for a fresh maintenance interval.
//   INPUTS: { detector: &BurstDetectorHandle - shared detector state }
//   OUTPUTS: { () - no return value }
//   SIDE_EFFECTS: [resets in-memory burst peak and mirrored metrics gauge]
//   LINKS: [M-OBS, V-M-OBS]
// END_CONTRACT: reset_peak
pub fn reset_peak(detector: &BurstDetectorHandle) {
    let mut state = detector
        .inner
        .lock()
        .expect("burst detector mutex poisoned");
    state.peak_rate_per_sec = 0;
    detector.metrics.set_peak_rate_per_sec(0);
}

// START_CONTRACT: redact_secret
//   PURPOSE: Return a stable redacted form for secrets before logging.
//   INPUTS: { secret: &str - raw secret value }
//   OUTPUTS: { String - redacted representation that preserves only limited edges }
//   SIDE_EFFECTS: [none]
//   LINKS: [M-OBS, M-AUTH]
// END_CONTRACT: redact_secret
pub fn redact_secret(secret: &str) -> String {
    if secret.chars().count() <= 4 {
        return "***".to_string();
    }

    let prefix: String = secret.chars().take(2).collect();
    let suffix: String = secret
        .chars()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}***{suffix}")
}

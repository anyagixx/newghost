// FILE: src/config/mod.rs
// VERSION: 0.1.2
// START_MODULE_CONTRACT
//   PURPOSE: Load and validate runtime configuration for client, server, and live origdst-helper modes.
//   SCOPE: CLI parsing, typed configuration assembly, deterministic validation, explicit live-helper launch shape, and stable log markers.
//   DEPENDS: clap, thiserror, tracing, url
//   LINKS: M-CONFIG, M-ORIGDST-LIVE-CONFIG-SHAPE, V-M-CONFIG, V-M-ORIGDST-LIVE-CONFIG-SHAPE, DF-CLIENT-BOOT, VF-001
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   AppConfig - validated application configuration
//   RuntimeMode - client, server, or live origdst-helper configuration branch
//   ClientTlsConfig - client-side trust-anchor and optional endpoint-identity override
//   OrigDstLiveConfig - explicit live helper listener and preserved-baseline launch shape
//   LimitsConfig - concurrency and queue limits
//   TimeoutConfig - transport and shutdown timing knobs
//   BurstDetectionConfig - observability thresholds for burst detection
//   load_config_from - parse and validate configuration from argv
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.2 - Added explicit origdst-live launch config so Phase-42 can run one governed helper process without hidden shell or desktop state.
// END_CHANGE_SUMMARY

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use thiserror::Error;
use tracing::{error, info};
use url::Url;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub runtime_mode: RuntimeMode,
    pub auth_token: String,
    pub limits: LimitsConfig,
    pub timeouts: TimeoutConfig,
    pub burst_detection: BurstDetectionConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeMode {
    Client(ClientConfig),
    Server(ServerConfig),
    OrigDstLive(OrigDstLiveConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    pub listen_addr: SocketAddr,
    pub remote_wss_url: Url,
    pub tls: Option<ClientTlsConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTlsConfig {
    pub trust_anchor_path: PathBuf,
    pub server_name_override: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub listen_addr: SocketAddr,
    pub tls_cert_path: PathBuf,
    pub tls_key_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrigDstLiveConfig {
    pub listener_addr: SocketAddr,
    pub payload_capacity_bytes: usize,
    pub operator_uid: u32,
    pub preserve_baseline_proxy_addr: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitsConfig {
    pub max_pending_intents: usize,
    pub max_sessions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeoutConfig {
    pub iroh_connect_timeout: Duration,
    pub wss_connect_timeout: Duration,
    pub socks5_total_timeout: Duration,
    pub graceful_timeout: Duration,
    pub force_kill_after: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurstDetectionConfig {
    pub alert_threshold: u64,
    pub alert_window: Duration,
    pub min_log_interval: Duration,
    pub ring_capacity: usize,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConfigError {
    #[error("argument parsing failed: {0}")]
    ArgumentParsing(String),
    #[error("auth token must not be empty")]
    EmptyAuthToken,
    #[error("remote WSS URL must use the wss scheme")]
    InvalidRemoteWssScheme,
    #[error("{field} must be greater than zero")]
    NonPositiveValue { field: &'static str },
    #[error("graceful timeout must not exceed force kill timeout")]
    InvalidShutdownOrdering,
    #[error("server TLS certificate and key paths must not be empty")]
    EmptyTlsPath,
}

#[derive(Debug, Parser)]
#[command(name = "n0wss", version, disable_help_subcommand = true)]
struct CliArgs {
    #[command(subcommand)]
    mode: ModeArgs,

    #[arg(long)]
    auth_token: String,

    #[arg(long, default_value_t = 128)]
    max_pending_intents: usize,

    #[arg(long, default_value_t = 256)]
    max_sessions: usize,

    #[arg(long, default_value_t = 2)]
    iroh_connect_timeout_secs: u64,

    #[arg(long, default_value_t = 5)]
    wss_connect_timeout_secs: u64,

    #[arg(long, default_value_t = 10)]
    socks5_total_timeout_secs: u64,

    #[arg(long, default_value_t = 60)]
    graceful_timeout_secs: u64,

    #[arg(long, default_value_t = 90)]
    force_kill_after_secs: u64,

    #[arg(long, default_value_t = 50)]
    burst_alert_threshold: u64,

    #[arg(long, default_value_t = 1)]
    burst_alert_window_secs: u64,

    #[arg(long, default_value_t = 5)]
    burst_min_log_interval_secs: u64,

    #[arg(long, default_value_t = 1000)]
    burst_ring_capacity: usize,
}

#[derive(Debug, Subcommand)]
enum ModeArgs {
    Client {
        #[arg(long, default_value = "127.0.0.1:1080")]
        listen_addr: SocketAddr,
        #[arg(long)]
        remote_wss_url: Url,
        #[arg(long)]
        tls_trust_anchor_path: Option<PathBuf>,
        #[arg(long)]
        tls_server_name_override: Option<String>,
    },
    Server {
        #[arg(long, default_value = "0.0.0.0:7443")]
        listen_addr: SocketAddr,
        #[arg(long)]
        tls_cert_path: PathBuf,
        #[arg(long)]
        tls_key_path: PathBuf,
    },
    OrigdstLive {
        #[arg(long, default_value = "127.0.0.1:10073")]
        listener_addr: SocketAddr,
        #[arg(long, default_value_t = 65_507)]
        payload_capacity_bytes: usize,
        #[arg(long, default_value_t = 1000)]
        operator_uid: u32,
        #[arg(long, default_value = "127.0.0.1:1080")]
        preserve_baseline_proxy_addr: SocketAddr,
    },
}

// START_CONTRACT: load_config_from
//   PURPOSE: Parse CLI arguments into a validated AppConfig.
//   INPUTS: { args: Iterator<Item = OsString> - command-line arguments including binary name }
//   OUTPUTS: { Result<AppConfig, ConfigError> - validated typed configuration or deterministic error }
//   SIDE_EFFECTS: [structured log emission only]
//   LINKS: [M-CONFIG, V-M-CONFIG, VF-001]
// END_CONTRACT: load_config_from
pub fn load_config_from<I, T>(args: I) -> Result<AppConfig, ConfigError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    // START_BLOCK_VALIDATE_CONFIGURATION
    let parsed = CliArgs::try_parse_from(args)
        .map_err(|err| ConfigError::ArgumentParsing(err.to_string()))?;

    let config = AppConfig::try_from(parsed)?;

    info!(
        mode = match &config.runtime_mode {
            RuntimeMode::Client(_) => "client",
            RuntimeMode::Server(_) => "server",
            RuntimeMode::OrigDstLive(_) => "origdst-live",
        },
        max_pending_intents = config.limits.max_pending_intents,
        max_sessions = config.limits.max_sessions,
        "[Config][loadConfig][BLOCK_VALIDATE_CONFIGURATION] validated configuration"
    );

    Ok(config)
    // END_BLOCK_VALIDATE_CONFIGURATION
}

impl TryFrom<CliArgs> for AppConfig {
    type Error = ConfigError;

    fn try_from(value: CliArgs) -> Result<Self, Self::Error> {
        validate_non_empty_token(&value.auth_token)?;
        validate_positive("max_pending_intents", value.max_pending_intents)?;
        validate_positive("max_sessions", value.max_sessions)?;
        validate_positive_u64("iroh_connect_timeout_secs", value.iroh_connect_timeout_secs)?;
        validate_positive_u64("wss_connect_timeout_secs", value.wss_connect_timeout_secs)?;
        validate_positive_u64("socks5_total_timeout_secs", value.socks5_total_timeout_secs)?;
        validate_positive_u64("graceful_timeout_secs", value.graceful_timeout_secs)?;
        validate_positive_u64("force_kill_after_secs", value.force_kill_after_secs)?;
        validate_positive_u64("burst_alert_threshold", value.burst_alert_threshold)?;
        validate_positive_u64("burst_alert_window_secs", value.burst_alert_window_secs)?;
        validate_positive_u64(
            "burst_min_log_interval_secs",
            value.burst_min_log_interval_secs,
        )?;
        validate_positive("burst_ring_capacity", value.burst_ring_capacity)?;

        let graceful_timeout = Duration::from_secs(value.graceful_timeout_secs);
        let force_kill_after = Duration::from_secs(value.force_kill_after_secs);
        if graceful_timeout > force_kill_after {
            error!(
                graceful_timeout_secs = value.graceful_timeout_secs,
                force_kill_after_secs = value.force_kill_after_secs,
                "[Config][loadConfig][BLOCK_VALIDATE_CONFIGURATION] invalid shutdown ordering"
            );
            return Err(ConfigError::InvalidShutdownOrdering);
        }

        let runtime_mode = match value.mode {
            ModeArgs::Client {
                listen_addr,
                remote_wss_url,
                tls_trust_anchor_path,
                tls_server_name_override,
            } => {
                if remote_wss_url.scheme() != "wss" {
                    error!(
                        scheme = remote_wss_url.scheme(),
                        "[Config][loadConfig][BLOCK_VALIDATE_CONFIGURATION] invalid remote WSS scheme"
                    );
                    return Err(ConfigError::InvalidRemoteWssScheme);
                }

                RuntimeMode::Client(ClientConfig {
                    listen_addr,
                    remote_wss_url,
                    tls: tls_trust_anchor_path.map(|trust_anchor_path| ClientTlsConfig {
                        trust_anchor_path,
                        server_name_override: tls_server_name_override,
                    }),
                })
            }
            ModeArgs::Server {
                listen_addr,
                tls_cert_path,
                tls_key_path,
            } => {
                if tls_cert_path.as_os_str().is_empty() || tls_key_path.as_os_str().is_empty() {
                    error!(
                        "[Config][loadConfig][BLOCK_VALIDATE_CONFIGURATION] empty TLS path provided"
                    );
                    return Err(ConfigError::EmptyTlsPath);
                }

                RuntimeMode::Server(ServerConfig {
                    listen_addr,
                    tls_cert_path,
                    tls_key_path,
                })
            }
            ModeArgs::OrigdstLive {
                listener_addr,
                payload_capacity_bytes,
                operator_uid,
                preserve_baseline_proxy_addr,
            } => {
                validate_positive("payload_capacity_bytes", payload_capacity_bytes)?;
                validate_positive_u32("operator_uid", operator_uid)?;

                RuntimeMode::OrigDstLive(OrigDstLiveConfig {
                    listener_addr,
                    payload_capacity_bytes,
                    operator_uid,
                    preserve_baseline_proxy_addr,
                })
            }
        };

        Ok(AppConfig {
            runtime_mode,
            auth_token: value.auth_token,
            limits: LimitsConfig {
                max_pending_intents: value.max_pending_intents,
                max_sessions: value.max_sessions,
            },
            timeouts: TimeoutConfig {
                iroh_connect_timeout: Duration::from_secs(value.iroh_connect_timeout_secs),
                wss_connect_timeout: Duration::from_secs(value.wss_connect_timeout_secs),
                socks5_total_timeout: Duration::from_secs(value.socks5_total_timeout_secs),
                graceful_timeout,
                force_kill_after,
            },
            burst_detection: BurstDetectionConfig {
                alert_threshold: value.burst_alert_threshold,
                alert_window: Duration::from_secs(value.burst_alert_window_secs),
                min_log_interval: Duration::from_secs(value.burst_min_log_interval_secs),
                ring_capacity: value.burst_ring_capacity,
            },
        })
    }
}

fn validate_non_empty_token(token: &str) -> Result<(), ConfigError> {
    if token.trim().is_empty() {
        return Err(ConfigError::EmptyAuthToken);
    }
    Ok(())
}

fn validate_positive(field: &'static str, value: usize) -> Result<(), ConfigError> {
    if value == 0 {
        return Err(ConfigError::NonPositiveValue { field });
    }
    Ok(())
}

fn validate_positive_u64(field: &'static str, value: u64) -> Result<(), ConfigError> {
    if value == 0 {
        return Err(ConfigError::NonPositiveValue { field });
    }
    Ok(())
}

fn validate_positive_u32(field: &'static str, value: u32) -> Result<(), ConfigError> {
    if value == 0 {
        return Err(ConfigError::NonPositiveValue { field });
    }
    Ok(())
}

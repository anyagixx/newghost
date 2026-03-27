// FILE: src/cli/mod.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Select runtime mode, load configuration, initialize observability, and coordinate graceful shutdown sequencing.
//   SCOPE: Startup bootstrap, client or server mode selection, foundation dependency assembly, and local shutdown-state coordination.
//   DEPENDS: std, thiserror, tracing, src/config/mod.rs, src/obs/mod.rs, src/auth/mod.rs, src/tls/mod.rs
//   LINKS: M-CLI, M-CONFIG, M-OBS, M-AUTH, M-TLS, V-M-CLI, DF-CLIENT-BOOT, DF-SHUTDOWN
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   ApplicationRunResult - typed startup output for client or server mode
//   ApplicationMode - stable runtime mode label
//   StartupArtifacts - initialized foundation handles returned by run_from
//   ShutdownCoordinator - local shutdown state machine for accept-stop and drain phases
//   run_from - bootstrap config, observability, auth, optional TLS, and startup mode
//   coordinate_shutdown - drive shutdown phases in deterministic order
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Created Phase 1 CLI bootstrap and shutdown orchestration with tests.
// END_CHANGE_SUMMARY

use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use thiserror::Error;
use tracing::info;

use crate::auth::{AuthPolicy, AuthPolicyConfig};
use crate::config::{load_config_from, AppConfig, RuntimeMode};
use crate::obs::{init_observability, ObservabilityConfig, ObservabilityHandles};
use crate::tls::{TlsConfig, TlsContextHandle, TlsError};

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationMode {
    Client,
    Server,
}

#[derive(Clone)]
pub struct StartupArtifacts {
    pub config: AppConfig,
    pub observability: ObservabilityHandles,
    pub auth_policy: AuthPolicy,
    pub tls_context: Option<TlsContextHandle>,
}

#[derive(Clone)]
pub struct ApplicationRunResult {
    pub mode: ApplicationMode,
    pub startup: StartupArtifacts,
    pub shutdown: ShutdownCoordinator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownConfig {
    pub graceful_timeout: Duration,
    pub force_kill_after: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownSnapshot {
    pub state: ShutdownState,
    pub accepts_stopped: bool,
    pub drains_requested: bool,
    pub transports_released: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownState {
    Running,
    AcceptsStopped,
    Draining,
    TransportReleased,
}

#[derive(Clone)]
pub struct ShutdownCoordinator {
    inner: Arc<Mutex<ShutdownSnapshot>>,
    config: ShutdownConfig,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("configuration failed: {0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("observability initialization failed: {0}")]
    Observability(#[from] crate::obs::ObservabilityError),
    #[error("auth initialization failed: {0}")]
    Auth(#[from] crate::auth::AuthPolicyError),
    #[error("TLS initialization failed: {0}")]
    Tls(#[from] TlsError),
}

impl ApplicationRunResult {
    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            ApplicationMode::Client => "client",
            ApplicationMode::Server => "server",
        }
    }
}

impl ShutdownCoordinator {
    pub fn new(config: ShutdownConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ShutdownSnapshot {
                state: ShutdownState::Running,
                accepts_stopped: false,
                drains_requested: false,
                transports_released: false,
            })),
            config,
        }
    }

    pub fn snapshot(&self) -> ShutdownSnapshot {
        self.inner
            .lock()
            .expect("shutdown coordinator mutex poisoned")
            .clone()
    }

    pub fn can_accept_new_work(&self) -> bool {
        !self.snapshot().accepts_stopped
    }
}

// START_CONTRACT: run_from
//   PURPOSE: Bootstrap the process and route execution into client or server mode.
//   INPUTS: { args: Iterator<Item = OsString> - command-line arguments including binary name }
//   OUTPUTS: { Result<ApplicationRunResult, CliError> - startup result with initialized foundation artifacts }
//   SIDE_EFFECTS: [loads config, initializes tracing, auth policy, and optional TLS context]
//   LINKS: [M-CLI, M-CONFIG, M-OBS, M-AUTH, M-TLS, V-M-CLI]
// END_CONTRACT: run_from
pub fn run_from<I, T>(args: I) -> Result<ApplicationRunResult, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    // START_BLOCK_START_APPLICATION
    let config = load_config_from(args)?;
    let observability = init_observability(ObservabilityConfig::from_app_config(&config))?;
    let auth_policy = AuthPolicy::from_config(AuthPolicyConfig::from_app_config(&config))?;

    let mode = match config.runtime_mode {
        RuntimeMode::Client(_) => ApplicationMode::Client,
        RuntimeMode::Server(_) => ApplicationMode::Server,
    };

    let tls_context = match &config.runtime_mode {
        RuntimeMode::Client(_) => None,
        RuntimeMode::Server(server_config) => Some(TlsContextHandle::from_config(&TlsConfig {
            cert_path: server_config.tls_cert_path.clone(),
            key_path: server_config.tls_key_path.clone(),
            trust_anchor_path: server_config.tls_cert_path.clone(),
        })?),
    };

    let shutdown = ShutdownCoordinator::new(ShutdownConfig {
        graceful_timeout: config.timeouts.graceful_timeout,
        force_kill_after: config.timeouts.force_kill_after,
    });

    let result = ApplicationRunResult {
        mode,
        startup: StartupArtifacts {
            config,
            observability,
            auth_policy,
            tls_context,
        },
        shutdown,
    };

    info!(
        mode = result.mode_label(),
        has_tls = result.startup.tls_context.is_some(),
        "[CliApp][run][BLOCK_START_APPLICATION] application startup initialized"
    );

    Ok(result)
    // END_BLOCK_START_APPLICATION
}

// START_CONTRACT: coordinate_shutdown
//   PURPOSE: Drive accept-stop, drain, and transport-release shutdown phases.
//   INPUTS: { coordinator: &ShutdownCoordinator - mutable shutdown state holder }
//   OUTPUTS: { ShutdownSnapshot - final shutdown state after local orchestration }
//   SIDE_EFFECTS: [updates shutdown state and emits structured shutdown marker]
//   LINKS: [M-CLI, V-M-CLI, DF-SHUTDOWN]
// END_CONTRACT: coordinate_shutdown
pub fn coordinate_shutdown(coordinator: &ShutdownCoordinator) -> ShutdownSnapshot {
    // START_BLOCK_COORDINATE_SHUTDOWN
    let mut snapshot = coordinator
        .inner
        .lock()
        .expect("shutdown coordinator mutex poisoned");

    snapshot.accepts_stopped = true;
    snapshot.state = ShutdownState::AcceptsStopped;

    snapshot.drains_requested = true;
    snapshot.state = ShutdownState::Draining;

    snapshot.transports_released = true;
    snapshot.state = ShutdownState::TransportReleased;

    info!(
        graceful_timeout_secs = coordinator.config.graceful_timeout.as_secs(),
        force_kill_after_secs = coordinator.config.force_kill_after.as_secs(),
        "[CliApp][coordinateShutdown][BLOCK_COORDINATE_SHUTDOWN] coordinated shutdown phases"
    );

    snapshot.clone()
    // END_BLOCK_COORDINATE_SHUTDOWN
}

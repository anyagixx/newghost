// FILE: src/cli/mod.rs
// VERSION: 0.1.10
// START_MODULE_CONTRACT
//   PURPOSE: Select runtime mode, load configuration, initialize observability, launch the selected runtime surface, and coordinate graceful shutdown sequencing plus client-side inbound datagram delivery, WSS return-handler wiring, and one governed live origdst-helper launch surface with an explicit transparent-socket requirement boundary.
//   SCOPE: Startup bootstrap, client or server mode selection, live origdst-helper entrypoint and launcher wiring, transparent-socket requirement logging, foundation dependency assembly, runtime listener launch, session-manager timing bootstrap, client-side inbound datagram delivery wiring, WSS inbound-handler registration, and local shutdown-state coordination.
//   DEPENDS: std, async-trait, http, thiserror, tokio, tokio-util, tracing, src/config/mod.rs, src/obs/mod.rs, src/auth/mod.rs, src/tls/mod.rs, src/wss_gateway/mod.rs, src/socks5/mod.rs, src/proxy_bridge/mod.rs, src/session/mod.rs, src/transport/adapter_contract.rs, src/transport/task_tracker.rs, src/udp_origdst/mod.rs
//   LINKS: M-CLI, M-CONFIG, M-OBS, M-AUTH, M-TLS, M-WSS-GATEWAY, M-SOCKS5, M-PROXY-BRIDGE, M-SESSION, M-ORIGDST-LIVE-ENTRYPOINT-CONTRACT, M-ORIGDST-LIVE-LAUNCHER, M-TPROXY-PRIV-LAUNCH-DELTA, V-M-CLI, V-M-ORIGDST-LIVE-ENTRYPOINT-CONTRACT, V-M-ORIGDST-LIVE-LAUNCHER, V-M-TPROXY-PRIV-LAUNCH-DELTA, DF-CLIENT-BOOT, DF-SHUTDOWN
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   ApplicationRunResult - typed startup output for client or server mode
//   ApplicationMode - stable runtime mode label
//   StartupArtifacts - initialized foundation handles returned by run_from
//   SessionManagerConfig - session-aware idle and shutdown timings derived during bootstrap
//   ShutdownCoordinator - local shutdown state machine for accept-stop and drain phases
//   ClientDatagramInboundTarget - client runtime datagram sink that returns inbound replies to the owning local UDP relay socket
//   OrigDstLiveLaunchResult - bounded live helper launch metadata for one packet window
//   CliRuntimeError - typed runtime-launch and shutdown surface errors after bootstrap
//   run_origdst_live_until_cancelled - bind and run one governed live origdst helper listener until cancellation
//   run_from - bootstrap config, observability, auth, optional TLS, and selected-mode startup artifacts
//   run_until_shutdown_from - launch the selected runtime surface and keep it alive until cancellation
//   coordinate_shutdown - drive shutdown phases in deterministic order
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.10 - Added reply-path client-delivery and client-drop anchors so Phase-48 can separate successful local inbound delivery from bounded drop reasons.
// END_CHANGE_SUMMARY

use std::ffi::OsString;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use http::Uri;
use thiserror::Error;
use tokio::net::{lookup_host, TcpListener};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::auth::{AuthPolicy, AuthPolicyConfig};
use crate::config::{load_config_from, AppConfig, OrigDstLiveConfig, RuntimeMode};
use crate::obs::{init_observability, ObservabilityConfig, ObservabilityHandles};
use crate::proxy_bridge::{ProxyBridge, ProxyBridgeConfig};
use crate::session::{
    DatagramDispatchTarget, DatagramRuntimeBridge, DatagramTransportSelector,
    DatagramTransportSelectorConfig, EffectHandler, MetricEffectTarget, MetricEvent,
    SessionManager, SessionManagerConfig, SessionRegistry, TimerCommand, TimerEffectTarget,
    TransportSelector, TransportSelectorConfig, UdpAssociationRegistry,
};
use crate::socks5::udp_associate::{encode_udp_datagram, UdpRelaySocketRegistry};
use crate::socks5::{Socks5Proxy, Socks5ProxyConfig};
use crate::tls::{ClientTlsConfig, TlsConfig, TlsContextHandle, TlsError};
use crate::transport::adapter_contract::{TransportAdapter, TransportRequest};
use crate::transport::datagram_contract::DatagramAssociationId;
use crate::transport::stream::ResolvedStream;
use crate::transport::task_tracker::AdapterTaskTracker;
use crate::udp_origdst::{
    RecoveredUdpTuple, UdpOrigDstError, UdpOrigDstGovernedHandoff, UdpOrigDstRuntime,
};
use crate::wss_gateway::{DatagramInboundHandler, GatewayConfig, WssGateway};

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationMode {
    Client,
    Server,
    OrigDstLive,
}

#[derive(Clone)]
pub struct StartupArtifacts {
    pub config: AppConfig,
    pub observability: ObservabilityHandles,
    pub auth_policy: AuthPolicy,
    pub session_config: SessionManagerConfig,
    pub tls_context: Option<TlsContextHandle>,
}

#[derive(Clone)]
pub struct ApplicationRunResult {
    pub mode: ApplicationMode,
    pub startup: StartupArtifacts,
    pub shutdown: ShutdownCoordinator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrigDstLiveLaunchResult {
    pub listener_addr: SocketAddr,
    pub payload_capacity_bytes: usize,
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

#[derive(Debug, Error)]
pub enum CliRuntimeError {
    #[error("{0}")]
    Bootstrap(#[from] CliError),
    #[error("invalid remote endpoint: {0}")]
    InvalidRemoteEndpoint(String),
    #[error("failed to resolve remote server address: {0}")]
    RemoteAddressResolution(String),
    #[error("listener bind failed: {0}")]
    ListenerBind(String),
    #[error("server runtime failed: {0}")]
    ServerRuntime(String),
    #[error("client runtime failed: {0}")]
    ClientRuntime(String),
    #[error("origdst live runtime failed: {0}")]
    OrigDstLiveRuntime(String),
}

impl ApplicationRunResult {
    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            ApplicationMode::Client => "client",
            ApplicationMode::Server => "server",
            ApplicationMode::OrigDstLive => "origdst-live",
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

#[derive(Clone, Default)]
struct NoopTimerTarget;

#[derive(Clone, Default)]
struct NoopMetricTarget;

#[derive(Clone)]
struct UnavailableTransportAdapter {
    task_tracker: Arc<AdapterTaskTracker>,
}

#[derive(Clone)]
struct ClientDatagramInboundTarget {
    registry: Arc<UdpAssociationRegistry>,
    relay_sockets: UdpRelaySocketRegistry,
}

#[derive(Clone, Default)]
struct LoggingOrigDstLiveHandoff;

#[derive(Debug, Error)]
enum ClientDatagramInboundTargetError {
    #[error("udp association not found: {0}")]
    AssociationNotFound(DatagramAssociationId),
    #[error("relay-client mismatch for association {association_id}: expected {expected}, got {actual}")]
    RelayClientMismatch {
        association_id: DatagramAssociationId,
        expected: SocketAddr,
        actual: SocketAddr,
    },
    #[error("owning relay socket not found for {0}")]
    MissingRelaySocket(SocketAddr),
    #[error("failed to encode inbound relay packet: {0}")]
    Encode(String),
    #[error("failed to deliver inbound relay packet: {0}")]
    Send(String),
}

impl UnavailableTransportAdapter {
    fn new() -> Self {
        Self {
            task_tracker: Arc::new(AdapterTaskTracker::new("unavailable")),
        }
    }
}

#[async_trait]
impl TransportAdapter for UnavailableTransportAdapter {
    type Error = std::io::Error;

    async fn open_stream(
        &self,
        _request: &TransportRequest,
        _cancel: CancellationToken,
    ) -> Result<ResolvedStream, Self::Error> {
        Err(std::io::Error::other(
            "iroh runtime transport is not configured for this launch",
        ))
    }

    fn task_tracker(&self) -> &AdapterTaskTracker {
        self.task_tracker.as_ref()
    }
}

#[async_trait]
impl TimerEffectTarget for NoopTimerTarget {
    async fn execute(&self, _command: TimerCommand) {}
}

impl MetricEffectTarget for NoopMetricTarget {
    fn emit(&self, _event: MetricEvent) {}
}

impl ClientDatagramInboundTarget {
    fn new(registry: Arc<UdpAssociationRegistry>, relay_sockets: UdpRelaySocketRegistry) -> Self {
        Self {
            registry,
            relay_sockets,
        }
    }
}

#[async_trait]
impl UdpOrigDstGovernedHandoff for LoggingOrigDstLiveHandoff {
    async fn forward_recovered_tuple(
        &self,
        tuple: RecoveredUdpTuple,
        payload: Vec<u8>,
    ) -> Result<(), UdpOrigDstError> {
        info!(
            client_source = %tuple.client_source_addr,
            helper_listener = %tuple.helper_listener_addr,
            original_target = ?tuple.original_target,
            payload_len = payload.len(),
            "[OrigDstLiveLauncher][forwardRecoveredTuple][BLOCK_ORIGDST_LIVE_LAUNCHER] origdst live governed handoff observed"
        );
        Ok(())
    }
}

#[async_trait]
impl DatagramDispatchTarget for ClientDatagramInboundTarget {
    type Error = ClientDatagramInboundTargetError;

    async fn dispatch(
        &self,
        envelope: &crate::transport::datagram_contract::DatagramEnvelope,
    ) -> Result<(), Self::Error> {
        let association = match self.registry.get(envelope.association_id) {
            Some(association) => association,
            None => {
                info!(
                    association_id = envelope.association_id,
                    relay_client_addr = %envelope.relay_client_addr,
                    target = ?envelope.target,
                    payload_len = envelope.payload.len(),
                    "[CallReply][clientDrop][BLOCK_CALL_REPLY_CLIENT_DROP] dropped inbound reply because the owning UDP association no longer exists"
                );
                return Err(ClientDatagramInboundTargetError::AssociationNotFound(
                    envelope.association_id,
                ));
            }
        };
        if association.expected_client_addr != envelope.relay_client_addr {
            info!(
                association_id = envelope.association_id,
                relay_addr = %association.relay_addr,
                expected = %association.expected_client_addr,
                actual = %envelope.relay_client_addr,
                target = ?envelope.target,
                payload_len = envelope.payload.len(),
                "[CallReply][clientDrop][BLOCK_CALL_REPLY_CLIENT_DROP] dropped inbound reply because relay-client ownership no longer matches the preserved association"
            );
            return Err(ClientDatagramInboundTargetError::RelayClientMismatch {
                association_id: envelope.association_id,
                expected: association.expected_client_addr,
                actual: envelope.relay_client_addr,
            });
        }
        let relay_socket = match self.relay_sockets.socket_for(association.relay_addr) {
            Some(relay_socket) => relay_socket,
            None => {
                info!(
                    association_id = envelope.association_id,
                    relay_addr = %association.relay_addr,
                    relay_client_addr = %association.expected_client_addr,
                    target = ?envelope.target,
                    payload_len = envelope.payload.len(),
                    "[CallReply][clientDrop][BLOCK_CALL_REPLY_CLIENT_DROP] dropped inbound reply because the owning local relay socket no longer exists"
                );
                return Err(ClientDatagramInboundTargetError::MissingRelaySocket(
                    association.relay_addr,
                ));
            }
        };
        let packet = match encode_udp_datagram(envelope) {
            Ok(packet) => packet,
            Err(err) => {
                info!(
                    association_id = envelope.association_id,
                    relay_addr = %association.relay_addr,
                    relay_client_addr = %association.expected_client_addr,
                    target = ?envelope.target,
                    payload_len = envelope.payload.len(),
                    "[CallReply][clientDrop][BLOCK_CALL_REPLY_CLIENT_DROP] dropped inbound reply because the relay packet could not be encoded for client delivery"
                );
                return Err(ClientDatagramInboundTargetError::Encode(err.to_string()));
            }
        };
        relay_socket
            .send_to(&packet, association.expected_client_addr)
            .await
            .map_err(|err| {
                info!(
                    association_id = envelope.association_id,
                    relay_addr = %association.relay_addr,
                    relay_client_addr = %association.expected_client_addr,
                    target = ?envelope.target,
                    payload_len = envelope.payload.len(),
                    "[CallReply][clientDrop][BLOCK_CALL_REPLY_CLIENT_DROP] dropped inbound reply because the owning local relay socket rejected delivery"
                );
                ClientDatagramInboundTargetError::Send(err.to_string())
            })?;
        info!(
            association_id = envelope.association_id,
            relay_addr = %association.relay_addr,
            relay_client_addr = %association.expected_client_addr,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[CliApp][deliverInboundDatagram][BLOCK_DELIVER_INBOUND_DATAGRAM] delivered governed inbound datagram into the owning local UDP relay socket"
        );
        info!(
            association_id = envelope.association_id,
            relay_addr = %association.relay_addr,
            relay_client_addr = %association.expected_client_addr,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[CallDownstream][reply][BLOCK_CALL_DOWNSTREAM_REPLY] delivered downstream inbound reply into the owning local UDP relay socket"
        );
        info!(
            association_id = envelope.association_id,
            relay_addr = %association.relay_addr,
            relay_client_addr = %association.expected_client_addr,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[CallReply][clientDelivery][BLOCK_CALL_REPLY_CLIENT_DELIVERY] delivered reply-path inbound datagram into the owning local UDP relay socket"
        );
        Ok(())
    }
}

#[async_trait]
impl DatagramInboundHandler for ClientDatagramInboundTarget {
    async fn handle_inbound(
        &self,
        envelope: crate::transport::datagram_contract::DatagramEnvelope,
    ) -> Result<(), String> {
        self.dispatch(&envelope).await.map_err(|err| err.to_string())
    }
}

// START_CONTRACT: run_from
//   PURPOSE: Bootstrap the process and prepare startup artifacts for the selected runtime mode.
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
        RuntimeMode::OrigDstLive(_) => ApplicationMode::OrigDstLive,
    };

    let tls_context = match &config.runtime_mode {
        RuntimeMode::Client(client_config) => client_config
            .tls
            .as_ref()
            .map(|tls| {
                TlsContextHandle::from_client_config(&ClientTlsConfig {
                    trust_anchor_path: tls.trust_anchor_path.clone(),
                })
            })
            .transpose()?,
        RuntimeMode::Server(server_config) => Some(TlsContextHandle::from_config(&TlsConfig {
            cert_path: server_config.tls_cert_path.clone(),
            key_path: server_config.tls_key_path.clone(),
            trust_anchor_path: server_config.tls_cert_path.clone(),
        })?),
        RuntimeMode::OrigDstLive(_) => None,
    };

    let shutdown = ShutdownCoordinator::new(ShutdownConfig {
        graceful_timeout: config.timeouts.graceful_timeout,
        force_kill_after: config.timeouts.force_kill_after,
    });
    let session_config = SessionManagerConfig::from_app_config(&config);

    let result = ApplicationRunResult {
        mode,
        startup: StartupArtifacts {
            config,
            observability,
            auth_policy,
            session_config,
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

// START_CONTRACT: run_until_shutdown_from
//   PURPOSE: Launch the selected runtime surface and keep it alive until cancellation is requested.
//   INPUTS: { args: Iterator<Item = OsString> - command-line arguments including binary name, cancel: CancellationToken - process-level shutdown signal boundary }
//   OUTPUTS: { Result<ApplicationRunResult, CliRuntimeError> - initialized startup artifacts after runtime launch and coordinated shutdown }
//   SIDE_EFFECTS: [binds runtime listeners, spawns runtime tasks, and coordinates shutdown on cancellation]
//   LINKS: [M-CLI, M-WSS-GATEWAY, M-SOCKS5, M-PROXY-BRIDGE, M-SESSION, M-ORIGDST-LIVE-ENTRYPOINT-CONTRACT, M-ORIGDST-LIVE-LAUNCHER, V-M-CLI, V-M-ORIGDST-LIVE-ENTRYPOINT-CONTRACT, V-M-ORIGDST-LIVE-LAUNCHER]
// END_CONTRACT: run_until_shutdown_from
pub async fn run_until_shutdown_from<I, T>(
    args: I,
    cancel: CancellationToken,
) -> Result<ApplicationRunResult, CliRuntimeError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    // START_BLOCK_RUN_SELECTED_MODE
    let startup = run_from(args)?;
    match &startup.startup.config.runtime_mode {
        RuntimeMode::Server(server_config) => {
            run_server_mode(&startup, server_config.listen_addr, cancel.clone()).await?;
        }
        RuntimeMode::Client(client_config) => {
            run_client_mode(&startup, client_config.listen_addr, cancel.clone()).await?;
        }
        RuntimeMode::OrigDstLive(origdst_live_config) => {
            run_origdst_live_until_cancelled(
                origdst_live_config,
                cancel.clone(),
                LoggingOrigDstLiveHandoff,
            )
            .await?;
        }
    }

    coordinate_shutdown(&startup.shutdown);
    info!(
        mode = startup.mode_label(),
        "[CliApp][runRuntime][BLOCK_RUN_SELECTED_MODE] runtime exited after coordinated shutdown"
    );
    Ok(startup)
    // END_BLOCK_RUN_SELECTED_MODE
}

// START_CONTRACT: run_origdst_live_until_cancelled
//   PURPOSE: Bind and run one governed live repo-local origdst helper listener until cancellation while keeping process-launch and listener-bind anchors separate.
//   INPUTS: { config: &OrigDstLiveConfig - explicit live helper listener and baseline-preserve shape, cancel: CancellationToken - bounded packet-window cancellation boundary, handoff: H - governed recovered-tuple sink for the live helper process }
//   OUTPUTS: { Result<OrigDstLiveLaunchResult, CliRuntimeError> - bound listener metadata when the helper exits cleanly after cancellation }
//   SIDE_EFFECTS: [binds one UDP listener, emits stable launch and listener-bind logs, and runs the live origdst recovery loop until cancellation]
//   LINKS: [M-ORIGDST-LIVE-ENTRYPOINT-CONTRACT, M-ORIGDST-LIVE-LAUNCHER, V-M-ORIGDST-LIVE-ENTRYPOINT-CONTRACT, V-M-ORIGDST-LIVE-LAUNCHER]
// END_CONTRACT: run_origdst_live_until_cancelled
pub async fn run_origdst_live_until_cancelled<H>(
    config: &OrigDstLiveConfig,
    cancel: CancellationToken,
    handoff: H,
) -> Result<OrigDstLiveLaunchResult, CliRuntimeError>
where
    H: UdpOrigDstGovernedHandoff,
{
    // START_BLOCK_ORIGDST_LIVE_ENTRYPOINT
    info!(
        listener_addr = %config.listener_addr,
        payload_capacity_bytes = config.payload_capacity_bytes,
        operator_uid = config.operator_uid,
        preserve_baseline_proxy_addr = %config.preserve_baseline_proxy_addr,
        transparent_socket_required = config.transparent_socket_mode == crate::config::OrigDstTransparentSocketMode::Required,
        "[OrigDstLiveEntrypoint][runOrigDstLiveUntilCancelled][BLOCK_ORIGDST_LIVE_ENTRYPOINT] origdst live entrypoint starting"
    );
    // END_BLOCK_ORIGDST_LIVE_ENTRYPOINT

    // START_BLOCK_ORIGDST_LIVE_LAUNCHER
    let socket = UdpSocket::bind(config.listener_addr)
        .map_err(|error| CliRuntimeError::OrigDstLiveRuntime(error.to_string()))?;
    let listener_addr = socket
        .local_addr()
        .map_err(|error| CliRuntimeError::OrigDstLiveRuntime(error.to_string()))?;

    info!(
        listener_addr = %listener_addr,
        payload_capacity_bytes = config.payload_capacity_bytes,
        transparent_socket_required = config.transparent_socket_mode == crate::config::OrigDstTransparentSocketMode::Required,
        "[OrigDstLiveLauncher][runOrigDstLiveUntilCancelled][BLOCK_ORIGDST_LIVE_LAUNCHER] origdst live listener bound"
    );

    let runtime = UdpOrigDstRuntime::new(handoff);
    runtime
        .run_linux_ipv4_listener_until_cancelled(
            socket,
            config.payload_capacity_bytes,
            config.transparent_socket_mode == crate::config::OrigDstTransparentSocketMode::Required,
            cancel,
        )
        .await
        .map_err(|error| CliRuntimeError::OrigDstLiveRuntime(error.to_string()))?;

    info!(
        listener_addr = %listener_addr,
        "[OrigDstLiveLauncher][runOrigDstLiveUntilCancelled][BLOCK_ORIGDST_LIVE_LAUNCHER] origdst live listener stopped after cancellation"
    );

    Ok(OrigDstLiveLaunchResult {
        listener_addr,
        payload_capacity_bytes: config.payload_capacity_bytes,
    })
    // END_BLOCK_ORIGDST_LIVE_LAUNCHER
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

async fn run_server_mode(
    startup: &ApplicationRunResult,
    listen_addr: SocketAddr,
    cancel: CancellationToken,
) -> Result<(), CliRuntimeError> {
    let gateway = build_server_gateway(startup, listen_addr)?;
    let listener = TcpListener::bind(listen_addr)
        .await
        .map_err(|err| CliRuntimeError::ListenerBind(err.to_string()))?;
    let server_task = tokio::spawn({
        let gateway = gateway.clone();
        async move { gateway.run_server(listener).await }
    });
    tokio::pin!(server_task);

    info!(
        mode = "server",
        listen_addr = %listen_addr,
        "[CliApp][runRuntime][BLOCK_RUN_SERVER_MODE] server runtime bound listener"
    );

    tokio::select! {
        server_result = &mut server_task => {
            let joined = server_result
                .map_err(|err| CliRuntimeError::ServerRuntime(err.to_string()))?;
            joined.map_err(|err| CliRuntimeError::ServerRuntime(err.to_string()))
        }
        _ = cancel.cancelled() => {
            gateway.stop_accept();
            let joined = server_task
                .await
                .map_err(|err| CliRuntimeError::ServerRuntime(err.to_string()))?;
            joined.map_err(|err| CliRuntimeError::ServerRuntime(err.to_string()))
        }
    }
}

async fn run_client_mode(
    startup: &ApplicationRunResult,
    listen_addr: SocketAddr,
    cancel: CancellationToken,
) -> Result<(), CliRuntimeError> {
    let app_config = &startup.startup.config;
    let socks5_config = Socks5ProxyConfig::from_app_config(app_config)
        .ok_or_else(|| CliRuntimeError::ClientRuntime("missing client socks5 config".to_string()))?;
    let (intent_tx, intent_rx) = mpsc::channel(app_config.limits.max_pending_intents);
    let wss_gateway = build_client_gateway(startup).await?;
    let datagram_registry = Arc::new(UdpAssociationRegistry::new(app_config.limits.max_sessions));
    let relay_sockets = UdpRelaySocketRegistry::default();
    let client_inbound_target =
        ClientDatagramInboundTarget::new(datagram_registry.clone(), relay_sockets.clone());
    wss_gateway.set_datagram_inbound_handler(Arc::new(client_inbound_target.clone()));
    let datagram_selector = DatagramTransportSelector::new(
        wss_gateway.clone(),
        DatagramTransportSelectorConfig {
            wss_timeout: app_config.timeouts.wss_connect_timeout,
        },
    );
    let datagram_runtime_target = Arc::new(DatagramRuntimeBridge::new(
        datagram_registry.clone(),
        datagram_selector,
        client_inbound_target,
    ));
    let proxy = Socks5Proxy::new(socks5_config, intent_tx.clone())
        .with_udp_runtime_target(datagram_runtime_target)
        .with_udp_relay_socket_registry(relay_sockets);
    let selector = TransportSelector::new(
        UnavailableTransportAdapter::new(),
        wss_gateway,
        TransportSelectorConfig {
            iroh_timeout: app_config.timeouts.iroh_connect_timeout,
            wss_timeout: app_config.timeouts.wss_connect_timeout,
            safety_timeout: app_config.timeouts.iroh_connect_timeout
                + app_config.timeouts.wss_connect_timeout
                + Duration::from_secs(1),
        },
    );
    let registry = Arc::new(SessionRegistry::new(app_config.limits.max_sessions));
    let effect_handler = EffectHandler::new(
        registry.clone(),
        NoopTimerTarget,
        NoopMetricTarget,
    );
    let manager = Arc::new(SessionManager::new(
        registry,
        selector,
        effect_handler,
        startup.startup.session_config.clone(),
    ));
    let bridge = ProxyBridge::new(
        ProxyBridgeConfig {
            pump_buffer_bytes: 8 * 1024,
            total_request_timeout: app_config.timeouts.socks5_total_timeout,
        },
        manager.clone(),
    );
    let listener_task = tokio::spawn({
        let proxy = proxy.clone();
        async move { proxy.run_listener().await }
    });
    let worker_task = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.run_worker(intent_rx).await }
    });
    tokio::pin!(listener_task);

    info!(
        mode = "client",
        listen_addr = %listen_addr,
        "[CliApp][runRuntime][BLOCK_RUN_CLIENT_MODE] client runtime bound socks5 listener"
    );

    let runtime_result = tokio::select! {
        listener_result = &mut listener_task => {
            let joined = listener_result
                .map_err(|err| CliRuntimeError::ClientRuntime(err.to_string()))?;
            joined.map_err(|err| CliRuntimeError::ClientRuntime(err.to_string()))
        }
        _ = cancel.cancelled() => Ok(()),
    };

    proxy.stop_accept();
    bridge.stop_accept();
    drop(intent_tx);

    let listener_joined = listener_task
        .await
        .map_err(|err| CliRuntimeError::ClientRuntime(err.to_string()))?;
    bridge.drain_all().await;
    let _ = worker_task.await;
    let _ = manager.shutdown().await;

    runtime_result?;
    listener_joined.map_err(|err| CliRuntimeError::ClientRuntime(err.to_string()))
}

fn build_server_gateway(
    startup: &ApplicationRunResult,
    listen_addr: SocketAddr,
) -> Result<WssGateway, CliRuntimeError> {
    let tls_context = startup
        .startup
        .tls_context
        .clone()
        .ok_or_else(|| CliRuntimeError::ServerRuntime("server TLS context missing".to_string()))?;
    let websocket_uri: Uri = "wss://localhost/tunnel"
        .parse()
        .map_err(|err: http::uri::InvalidUri| CliRuntimeError::ServerRuntime(err.to_string()))?;

    Ok(WssGateway::new(GatewayConfig {
        server_addr: listen_addr,
        server_name: "localhost".to_string(),
        websocket_uri,
        auth_token: startup.startup.config.auth_token.clone(),
        tls_context,
        auth_policy: startup.startup.auth_policy.clone(),
        metrics: startup.startup.observability.metrics.clone(),
    }))
}

async fn build_client_gateway(
    startup: &ApplicationRunResult,
) -> Result<WssGateway, CliRuntimeError> {
    let client_config = match &startup.startup.config.runtime_mode {
        RuntimeMode::Client(client_config) => client_config,
        RuntimeMode::Server(_) | RuntimeMode::OrigDstLive(_) => {
            return Err(CliRuntimeError::ClientRuntime(
                "client gateway requested outside client mode".to_string(),
            ))
        }
    };
    let remote_socket_addr = resolve_remote_socket_addr(&client_config.remote_wss_url).await?;
    let server_name = client_config
        .tls
        .as_ref()
        .and_then(|tls| tls.server_name_override.clone())
        .or_else(|| client_config.remote_wss_url.host_str().map(ToOwned::to_owned))
        .ok_or_else(|| {
            CliRuntimeError::InvalidRemoteEndpoint(
                "remote WSS URL must include a host".to_string(),
            )
        })?;
    let websocket_uri = client_config
        .remote_wss_url
        .as_str()
        .parse()
        .map_err(|err: http::uri::InvalidUri| CliRuntimeError::InvalidRemoteEndpoint(err.to_string()))?;
    let tls_context = startup
        .startup
        .tls_context
        .clone()
        .ok_or_else(|| CliRuntimeError::ClientRuntime("client TLS context missing".to_string()))?;

    Ok(WssGateway::new(GatewayConfig {
        server_addr: remote_socket_addr,
        server_name,
        websocket_uri,
        auth_token: startup.startup.config.auth_token.clone(),
        tls_context,
        auth_policy: startup.startup.auth_policy.clone(),
        metrics: startup.startup.observability.metrics.clone(),
    }))
}

async fn resolve_remote_socket_addr(url: &url::Url) -> Result<SocketAddr, CliRuntimeError> {
    let host = url.host_str().ok_or_else(|| {
        CliRuntimeError::InvalidRemoteEndpoint("remote WSS URL must include a host".to_string())
    })?;
    let port = url.port_or_known_default().ok_or_else(|| {
        CliRuntimeError::InvalidRemoteEndpoint(
            "remote WSS URL must include a known or explicit port".to_string(),
        )
    })?;
    let mut resolved = lookup_host((host, port))
        .await
        .map_err(|err| CliRuntimeError::RemoteAddressResolution(err.to_string()))?;
    resolved.next().ok_or_else(|| {
        CliRuntimeError::RemoteAddressResolution(format!(
            "no socket addresses resolved for {host}:{port}"
        ))
    })
}

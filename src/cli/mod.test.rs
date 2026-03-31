// FILE: src/cli/mod.test.rs
// VERSION: 0.1.8
// START_MODULE_CONTRACT
//   PURPOSE: Verify deterministic CLI bootstrap, runtime launch, UDP-capable client bootstrap, client-side inbound reply delivery, origdst-live helper launch, live-launch smoke, non-OUTPUT live-smoke launch shape, and shutdown sequencing for governed startup paths.
//   SCOPE: Client startup, server startup, optional client TLS bootstrap, runtime listener binding, raw UDP delivery through the live client bootstrap, association-owned inbound UDP delivery, origdst-live listener launch, live tuple-recovery smoke, non-OUTPUT launch-shape proof, and shutdown ordering.
//   DEPENDS: async-trait, src/cli/mod.rs, src/tls/mod.rs, src/wss_gateway/mod.rs, src/socks5/mod.rs, src/socks5/udp_associate.rs, src/session/datagram_manager.rs, src/proxy_bridge/udp_relay.rs, src/udp_origdst/mod.rs
//   LINKS: V-M-CLI, V-M-TLS, V-M-ORIGDST-LIVE-ENTRYPOINT-CONTRACT, V-M-ORIGDST-LIVE-LAUNCHER, V-M-ORIGDST-LIVE-SMOKE, V-M-TPROXY-NONOUTPUT-SMOKE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   selects_client_mode_on_valid_startup - proves baseline client bootstrap
//   selects_client_mode_and_builds_tls_when_trust_anchor_is_configured - proves optional client TLS bootstrap
//   selects_server_mode_and_builds_tls_on_valid_startup - proves server TLS bootstrap
//   selects_origdst_live_mode_on_valid_startup - proves the new governed live helper startup shape
//   server_runtime_binds_listener_until_cancelled - proves server runtime stays alive and binds a socket until cancellation
//   client_runtime_binds_socks5_listener_until_cancelled - proves client runtime stays alive and binds a SOCKS5 socket until cancellation
//   origdst_live_runtime_binds_listener_until_cancelled - proves the live helper launcher emits a bound listener and stays alive until cancellation
//   origdst_live_smoke_proves_launch_listener_tuple_handoff_and_preserved_baseline - proves the live helper packet stays outside Telegram UI while preserving the ordinary baseline and forwarding one recovered tuple
//   origdst_live_nonoutput_smoke_proves_launch_shape_and_preserved_baseline - proves the Phase-45 non-OUTPUT smoke packet keeps launch proof, route-mark plan proof, local-delivery plan proof, and preserved baseline proof outside Telegram UI
//   client_runtime_forwards_udp_datagram_through_runtime_bridge - proves the live client bootstrap wires UDP ingress into the datagram runtime bridge and reaches a real UDP target
//   client_inbound_target_delivers_reply_into_owned_udp_socket - proves client-side inbound delivery returns one governed datagram into the owning local UDP relay socket
//   shutdown_stops_accepts_before_drain_and_release - proves deterministic shutdown ordering
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.8 - Added a bounded Phase-45 non-OUTPUT live-smoke launch-shape test so privileged launch, route-mark planning, local-delivery planning, and preserved baseline proof stay separate before any Telegram packet.
// END_CHANGE_SUMMARY

use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::net::TcpListener as StdTcpListener;
use std::net::UdpSocket as StdUdpSocket;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::{
    coordinate_shutdown, run_from, run_origdst_live_until_cancelled, run_until_shutdown_from,
    ApplicationMode,
    ClientDatagramInboundTarget, ShutdownState,
};
use crate::session::{DatagramDispatchTarget, UdpAssociationRegistry};
use crate::socks5::udp_associate::{encode_udp_datagram, UdpRelaySocketRegistry};
use crate::transport::datagram_contract::{DatagramEnvelope, DatagramTarget};
use crate::udp_origdst::{RecoveredUdpTuple, UdpOrigDstError, UdpOrigDstGovernedHandoff};
use crate::config::{OrigDstLiveConfig, OrigDstTransparentSocketMode};
use crate::udp_origdst::linux::plan_linux_nonoutput_tproxy;

#[derive(Clone, Default)]
struct RecordingOrigDstLiveHandoff {
    calls: Arc<Mutex<Vec<(RecoveredUdpTuple, Vec<u8>)>>>,
}

#[async_trait]
impl UdpOrigDstGovernedHandoff for RecordingOrigDstLiveHandoff {
    async fn forward_recovered_tuple(
        &self,
        tuple: RecoveredUdpTuple,
        payload: Vec<u8>,
    ) -> Result<(), UdpOrigDstError> {
        self.calls
            .lock()
            .expect("recording origdst handoff lock poisoned")
            .push((tuple, payload));
        Ok(())
    }
}

fn write_server_tls_fixture() -> (tempfile::TempDir, String, String) {
    let dir = tempdir().expect("tempdir should build");

    let key_pair = KeyPair::generate().expect("key pair should build");
    let mut params = CertificateParams::new(vec!["localhost".to_string()])
        .expect("certificate params should build");
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, "localhost");
    params.distinguished_name = distinguished_name;
    let cert = params
        .self_signed(&key_pair)
        .expect("certificate should build");

    let cert_path = dir.path().join("server-cert.pem");
    let key_path = dir.path().join("server-key.pem");
    fs::write(&cert_path, cert.pem()).expect("cert should write");
    fs::write(&key_path, key_pair.serialize_pem()).expect("key should write");

    (
        dir,
        cert_path.display().to_string(),
        key_path.display().to_string(),
    )
}

#[test]
fn selects_client_mode_on_valid_startup() {
    let run_result = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "client",
        "--remote-wss-url",
        "wss://example.com/tunnel",
    ])
    .expect("client startup should succeed");

    assert_eq!(run_result.mode, ApplicationMode::Client);
    assert!(run_result.startup.tls_context.is_none());
    assert_eq!(
        run_result.startup.session_config.idle_timeout,
        std::time::Duration::from_secs(10)
    );
    assert!(run_result.shutdown.can_accept_new_work());
}

#[test]
fn selects_client_mode_and_builds_tls_when_trust_anchor_is_configured() {
    let (_dir, cert_path, _key_path) = write_server_tls_fixture();

    let run_result = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "client",
        "--remote-wss-url",
        "wss://example.com/tunnel",
        "--tls-trust-anchor-path",
        cert_path.as_str(),
        "--tls-server-name-override",
        "example.com",
    ])
    .expect("client startup with trust anchor should succeed");

    assert_eq!(run_result.mode, ApplicationMode::Client);
    assert!(run_result.startup.tls_context.is_some());
    assert_eq!(
        run_result.startup.tls_context.expect("tls context").leaf_subject,
        "CN=localhost"
    );
}

#[test]
fn selects_server_mode_and_builds_tls_on_valid_startup() {
    let (_dir, cert_path, key_path) = write_server_tls_fixture();

    let run_result = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "server",
        "--tls-cert-path",
        cert_path.as_str(),
        "--tls-key-path",
        key_path.as_str(),
    ])
    .expect("server startup should succeed");

    assert_eq!(run_result.mode, ApplicationMode::Server);
    assert!(run_result.startup.tls_context.is_some());
    assert_eq!(
        run_result.startup.session_config.graceful_shutdown_timeout,
        std::time::Duration::from_secs(60)
    );
}

#[test]
fn selects_origdst_live_mode_on_valid_startup() {
    let run_result = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "origdst-live",
    ])
    .expect("origdst-live startup should succeed");

    assert_eq!(run_result.mode, ApplicationMode::OrigDstLive);
    assert!(run_result.startup.tls_context.is_none());
    assert!(run_result.shutdown.can_accept_new_work());
}

fn reserve_local_addr() -> String {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("ephemeral listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    drop(listener);
    addr.to_string()
}

async fn wait_for_listener(addr: &str) {
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return;
        }
        sleep(Duration::from_millis(20)).await;
    }

    panic!("listener {addr} did not become reachable");
}

async fn wait_for_udp_listener_bound(addr: &str) {
    for _ in 0..50 {
        match StdUdpSocket::bind(addr) {
            Ok(socket) => {
                drop(socket);
                sleep(Duration::from_millis(20)).await;
            }
            Err(_) => return,
        }
    }

    panic!("udp listener {addr} did not become reachable");
}

fn reserve_local_udp_addr() -> SocketAddr {
    let socket =
        StdUdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .expect("ephemeral udp listener should bind");
    let addr = socket.local_addr().expect("udp local addr should resolve");
    drop(socket);
    addr
}

async fn socks5_udp_associate(listen_addr: &str) -> (TcpStream, SocketAddr) {
    let mut control = TcpStream::connect(listen_addr)
        .await
        .expect("control channel should connect");
    control
        .write_all(&[0x05, 0x01, 0x00])
        .await
        .expect("auth greeting should write");
    let mut auth_reply = [0_u8; 2];
    control
        .read_exact(&mut auth_reply)
        .await
        .expect("auth reply should read");
    assert_eq!(auth_reply, [0x05, 0x00]);

    control
        .write_all(&[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .expect("udp associate request should write");
    let mut reply = [0_u8; 10];
    control
        .read_exact(&mut reply)
        .await
        .expect("udp associate reply should read");
    assert_eq!(reply[0], 0x05);
    assert_eq!(reply[1], 0x00);
    assert_eq!(reply[2], 0x00);
    assert_eq!(reply[3], 0x01);

    let relay_addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(reply[4], reply[5], reply[6], reply[7])),
        u16::from_be_bytes([reply[8], reply[9]]),
    );
    (control, relay_addr)
}

#[tokio::test]
async fn server_runtime_binds_listener_until_cancelled() {
    let (_dir, cert_path, key_path) = write_server_tls_fixture();
    let listen_addr = reserve_local_addr();
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let cert_path = cert_path.clone();
        let key_path = key_path.clone();
        let listen_addr = listen_addr.clone();
        async move {
            run_until_shutdown_from(
                [
                    "n0wss",
                    "--auth-token",
                    "token-12345",
                    "server",
                    "--listen-addr",
                    listen_addr.as_str(),
                    "--tls-cert-path",
                    cert_path.as_str(),
                    "--tls-key-path",
                    key_path.as_str(),
                ],
                cancel,
            )
            .await
        }
    });

    wait_for_listener(&listen_addr).await;
    cancel.cancel();

    task.await
        .expect("server runtime task should join")
        .expect("server runtime should shut down cleanly");
}

#[tokio::test]
async fn client_runtime_binds_socks5_listener_until_cancelled() {
    let (_dir, cert_path, _key_path) = write_server_tls_fixture();
    let listen_addr = reserve_local_addr();
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let cert_path = cert_path.clone();
        let listen_addr = listen_addr.clone();
        async move {
            run_until_shutdown_from(
                [
                    "n0wss",
                    "--auth-token",
                    "token-12345",
                    "client",
                    "--listen-addr",
                    listen_addr.as_str(),
                    "--remote-wss-url",
                    "wss://127.0.0.1:7443/tunnel",
                    "--tls-trust-anchor-path",
                    cert_path.as_str(),
                ],
                cancel,
            )
            .await
        }
    });

    wait_for_listener(&listen_addr).await;
    cancel.cancel();

    task.await
        .expect("client runtime task should join")
        .expect("client runtime should shut down cleanly");
}

#[tokio::test]
async fn origdst_live_runtime_binds_listener_until_cancelled() {
    let listen_addr = reserve_local_udp_addr().to_string();
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let listen_addr = listen_addr.clone();
        async move {
            run_until_shutdown_from(
                [
                    "n0wss",
                    "--auth-token",
                    "token-12345",
                    "origdst-live",
                    "--listener-addr",
                    listen_addr.as_str(),
                ],
                cancel,
            )
            .await
        }
    });

    wait_for_udp_listener_bound(&listen_addr).await;
    cancel.cancel();

    let result = task
        .await
        .expect("origdst live runtime task should join")
        .expect("origdst live runtime should shut down cleanly");
    assert_eq!(result.mode, ApplicationMode::OrigDstLive);
}

#[tokio::test]
async fn origdst_live_smoke_proves_launch_listener_tuple_handoff_and_preserved_baseline() {
    let startup = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "origdst-live",
    ])
    .expect("origdst-live startup should succeed");
    assert_eq!(startup.mode, ApplicationMode::OrigDstLive);

    let baseline_listener = StdTcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind preserved baseline listener");
    let baseline_addr = baseline_listener
        .local_addr()
        .expect("preserved baseline listener addr");
    let baseline_thread = thread::spawn(move || {
        let (_stream, _) = baseline_listener.accept().expect("accept preserved baseline probe");
    });

    let helper_addr = reserve_local_udp_addr();
    let config = OrigDstLiveConfig {
        listener_addr: helper_addr,
        payload_capacity_bytes: 128,
        operator_uid: 1000,
        preserve_baseline_proxy_addr: baseline_addr,
        transparent_socket_mode: OrigDstTransparentSocketMode::Disabled,
    };
    let handoff = RecordingOrigDstLiveHandoff::default();
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let handoff = handoff.clone();
        let config = config.clone();
        async move { run_origdst_live_until_cancelled(&config, cancel, handoff).await }
    });

    wait_for_udp_listener_bound(&helper_addr.to_string()).await;
    let sender = StdUdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind smoke sender");
    let sender_addr = sender.local_addr().expect("smoke sender addr");
    sender
        .send_to(b"live-smoke", helper_addr)
        .expect("send smoke packet");

    for _ in 0..20 {
        if handoff
            .calls
            .lock()
            .expect("recording origdst handoff lock poisoned")
            .len()
            == 1
        {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    let calls = handoff
        .calls
        .lock()
        .expect("recorded origdst calls lock poisoned");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.client_source_addr, sender_addr);
    assert_eq!(calls[0].0.helper_listener_addr, helper_addr);
    assert_eq!(calls[0].0.original_target, DatagramTarget::Ip(helper_addr));
    assert_eq!(calls[0].1, b"live-smoke".to_vec());
    drop(calls);

    let _baseline_probe =
        TcpStream::connect(baseline_addr).await.expect("preserved baseline should stay reachable");
    baseline_thread.join().expect("baseline thread join");

    cancel.cancel();
    let launch = task
        .await
        .expect("origdst live smoke task should join")
        .expect("origdst live smoke should shut down cleanly");
    assert_eq!(launch.listener_addr, helper_addr);
    assert_eq!(launch.payload_capacity_bytes, 128);
}

#[tokio::test]
async fn origdst_live_nonoutput_smoke_proves_launch_shape_and_preserved_baseline() {
    // START_BLOCK_TPROXY_NONOUTPUT_SMOKE
    let startup = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "origdst-live",
        "--transparent-socket-mode",
        "required",
    ])
    .expect("origdst-live non-output startup should succeed");
    assert_eq!(startup.mode, ApplicationMode::OrigDstLive);

    let baseline_listener = StdTcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind non-output preserved baseline listener");
    let baseline_addr = baseline_listener
        .local_addr()
        .expect("non-output preserved baseline listener addr");
    let baseline_thread = thread::spawn(move || {
        let (_stream, _) = baseline_listener
            .accept()
            .expect("accept non-output preserved baseline probe");
    });

    let helper_addr = reserve_local_udp_addr();
    let nonoutput_plan = plan_linux_nonoutput_tproxy(helper_addr);
    assert_eq!(nonoutput_plan.listener_addr, helper_addr);
    assert_eq!(nonoutput_plan.host_output_marker, "output-owner-mark-only");
    assert_eq!(nonoutput_plan.route_marker, "policy-routing-fwmark");
    assert_eq!(nonoutput_plan.ingress_marker, "veth-netns-ingress");
    assert_eq!(nonoutput_plan.interception_chain_marker, "prerouting-tproxy");
    assert_eq!(nonoutput_plan.route_localnet_marker, "route-localnet");
    assert_eq!(nonoutput_plan.rp_filter_marker, "rp-filter-relaxed");
    assert!(nonoutput_plan.requires_transparent_socket);

    let config = OrigDstLiveConfig {
        listener_addr: helper_addr,
        payload_capacity_bytes: 128,
        operator_uid: 1000,
        preserve_baseline_proxy_addr: baseline_addr,
        transparent_socket_mode: OrigDstTransparentSocketMode::Disabled,
    };
    let handoff = RecordingOrigDstLiveHandoff::default();
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        let handoff = handoff.clone();
        let config = config.clone();
        async move { run_origdst_live_until_cancelled(&config, cancel, handoff).await }
    });

    sleep(Duration::from_millis(50)).await;
    assert!(
        handoff
            .calls
            .lock()
            .expect("recording origdst handoff lock poisoned")
            .is_empty()
    );

    let _baseline_probe = TcpStream::connect(baseline_addr)
        .await
        .expect("non-output preserved baseline should stay reachable");
    baseline_thread.join().expect("non-output baseline thread join");

    cancel.cancel();
    let launch = task
        .await
        .expect("non-output origdst live smoke task should join")
        .expect("non-output origdst live smoke should shut down cleanly");
    assert_eq!(launch.listener_addr, helper_addr);
    assert_eq!(launch.payload_capacity_bytes, 128);
    // END_BLOCK_TPROXY_NONOUTPUT_SMOKE
}

#[tokio::test]
async fn client_runtime_forwards_udp_datagram_through_runtime_bridge() {
    let (_dir, cert_path, key_path) = write_server_tls_fixture();
    let server_addr = reserve_local_addr();
    let client_addr = reserve_local_addr();
    let server_cancel = CancellationToken::new();
    let client_cancel = CancellationToken::new();

    let server_task = tokio::spawn({
        let server_cancel = server_cancel.clone();
        let cert_path = cert_path.clone();
        let key_path = key_path.clone();
        let server_addr = server_addr.clone();
        async move {
            run_until_shutdown_from(
                [
                    "n0wss",
                    "--auth-token",
                    "token-12345",
                    "server",
                    "--listen-addr",
                    server_addr.as_str(),
                    "--tls-cert-path",
                    cert_path.as_str(),
                    "--tls-key-path",
                    key_path.as_str(),
                ],
                server_cancel,
            )
            .await
        }
    });

    wait_for_listener(&server_addr).await;

    let client_task = tokio::spawn({
        let client_cancel = client_cancel.clone();
        let cert_path = cert_path.clone();
        let client_addr = client_addr.clone();
        let server_url = format!("wss://{server_addr}/tunnel");
        async move {
            run_until_shutdown_from(
                [
                    "n0wss",
                    "--auth-token",
                    "token-12345",
                    "client",
                    "--listen-addr",
                    client_addr.as_str(),
                    "--remote-wss-url",
                    server_url.as_str(),
                    "--tls-trust-anchor-path",
                    cert_path.as_str(),
                    "--tls-server-name-override",
                    "localhost",
                ],
                client_cancel,
            )
            .await
        }
    });

    wait_for_listener(&client_addr).await;

    let remote = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("remote udp target should bind");
    let remote_addr = remote.local_addr().expect("remote addr should resolve");
    let (_control, relay_addr) = socks5_udp_associate(&client_addr).await;

    let udp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("local udp socket should bind");
    let payload = b"cli-phase25-udp".to_vec();
    let mut packet = Vec::with_capacity(3 + 1 + 4 + 2 + payload.len());
    packet.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    packet.extend_from_slice(&match remote_addr.ip() {
        IpAddr::V4(ipv4) => ipv4.octets(),
        IpAddr::V6(_) => panic!("test uses ipv4 target"),
    });
    packet.extend_from_slice(&remote_addr.port().to_be_bytes());
    packet.extend_from_slice(&payload);
    udp.send_to(&packet, relay_addr)
        .await
        .expect("udp datagram should send to relay");

    let mut buffer = [0_u8; 128];
    let (bytes_read, _source) =
        tokio::time::timeout(Duration::from_secs(2), remote.recv_from(&mut buffer))
            .await
            .expect("runtime bridge should reach remote udp target in time")
            .expect("remote recv should succeed");
    assert_eq!(&buffer[..bytes_read], payload.as_slice());

    client_cancel.cancel();
    server_cancel.cancel();

    client_task
        .await
        .expect("client runtime task should join")
        .expect("client runtime should shut down cleanly");
    server_task
        .await
        .expect("server runtime task should join")
        .expect("server runtime should shut down cleanly");
}

#[tokio::test]
async fn client_inbound_target_delivers_reply_into_owned_udp_socket() {
    let registry = std::sync::Arc::new(UdpAssociationRegistry::new(1));
    let relay_sockets = UdpRelaySocketRegistry::default();
    let client_udp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("client udp should bind");
    let client_addr = client_udp.local_addr().expect("client addr should resolve");
    let relay_socket = std::sync::Arc::new(
        UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("relay socket should bind"),
    );
    let relay_addr = relay_socket.local_addr().expect("relay addr should resolve");
    relay_sockets.register(relay_addr, relay_socket);

    let (association_id, _) = registry
        .open_association(relay_addr, client_addr, std::time::Instant::now())
        .expect("association should open");
    let target = ClientDatagramInboundTarget::new(registry, relay_sockets);
    let envelope = DatagramEnvelope {
        association_id,
        relay_client_addr: client_addr,
        target: DatagramTarget::Ip(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)),
        payload: b"phase26-inbound".to_vec(),
    };

    target
        .dispatch(&envelope)
        .await
        .expect("inbound target should deliver datagram");

    let mut buffer = [0_u8; 128];
    let (bytes_read, source) = client_udp.recv_from(&mut buffer).await.expect("recv reply");
    assert_eq!(source, relay_addr);
    assert_eq!(
        &buffer[..bytes_read],
        encode_udp_datagram(&envelope)
            .expect("packet should encode")
            .as_slice()
    );
}

#[test]
fn shutdown_stops_accepts_before_drain_and_release() {
    let run_result = run_from([
        "n0wss",
        "--auth-token",
        "token-12345",
        "client",
        "--remote-wss-url",
        "wss://example.com/tunnel",
    ])
    .expect("client startup should succeed");

    assert!(run_result.shutdown.can_accept_new_work());

    let snapshot = coordinate_shutdown(&run_result.shutdown);

    assert_eq!(snapshot.state, ShutdownState::TransportReleased);
    assert!(snapshot.accepts_stopped);
    assert!(snapshot.drains_requested);
    assert!(snapshot.transports_released);
    assert!(!run_result.shutdown.can_accept_new_work());
}

// FILE: tests/release_smoke.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Provide a narrow release smoke path that proves startup, one proxied request, reply mapping, and graceful shutdown.
//   SCOPE: Client-mode bootstrap, SOCKS5 request parsing, bridge worker execution, one transport-backed ping or pong exchange, and shutdown confirmation.
//   DEPENDS: async-trait, tokio, src/cli/mod.rs, src/socks5/mod.rs, src/proxy_bridge/mod.rs, src/session/mod.rs, src/transport/stream.rs
//   LINKS: V-M-SMOKE-E2E, DF-LOCAL-SMOKE, DF-RELEASE-SHUTDOWN
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   release_smoke_proves_startup_proxy_path_and_shutdown - deterministic release smoke scenario
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added a deterministic release smoke test covering startup, one proxied request, reply mapping, and graceful shutdown.
// END_CHANGE_SUMMARY

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use n0wss::cli::{coordinate_shutdown, run_from, ApplicationMode, ShutdownState};
use n0wss::proxy_bridge::{ProxyBridge, ProxyBridgeConfig};
use n0wss::session::{
    SessionControl, SessionEvent, SessionHandle, SessionManagerError, SessionRecord,
    SessionRegistry, SessionRequest, SessionState,
};
use n0wss::socks5::{ProxyError, Socks5Proxy, Socks5Reply};
use n0wss::transport::adapter_contract::TransportRequest;
use n0wss::transport::stream::{
    BoxedRead, BoxedWrite, ResolvedStream, ShutdownError, TransportKind, TransportStream,
};
use tokio::io::{duplex, split, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct SmokeStream {
    stream: DuplexStream,
    peer_label: String,
}

#[async_trait]
impl TransportStream for SmokeStream {
    fn split(self: Box<Self>) -> (BoxedRead, BoxedWrite) {
        let (read_half, write_half) = split(self.stream);
        (Box::pin(read_half), Box::pin(write_half))
    }

    fn peer_label(&self) -> &str {
        &self.peer_label
    }

    async fn shutdown(mut self: Box<Self>, _timeout: Duration) -> Result<(), ShutdownError> {
        let _ = self.stream.shutdown().await;
        Ok(())
    }
}

struct SmokeSessionControl {
    next_session_id: AtomicU64,
    resolved: Mutex<Option<ResolvedStream>>,
    events: Mutex<Vec<(u64, SessionEvent)>>,
}

impl SmokeSessionControl {
    fn new(resolved: ResolvedStream) -> Self {
        Self {
            next_session_id: AtomicU64::new(1),
            resolved: Mutex::new(Some(resolved)),
            events: Mutex::new(Vec::new()),
        }
    }

    fn events(&self) -> Vec<(u64, SessionEvent)> {
        self.events.lock().expect("events lock").clone()
    }
}

#[async_trait]
impl SessionControl for SmokeSessionControl {
    fn register_session(
        &self,
        request: &SessionRequest,
    ) -> Result<(u64, SessionHandle), SessionManagerError> {
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let registry = SessionRegistry::new(1);
        let (_, handle) = registry.insert(
            registry.try_reserve().expect("reservation"),
            SessionRecord::new(
                SessionState::Active {
                    since: request.started_at,
                    stream_count: 0,
                },
                request.started_at,
            ),
        );
        Ok((session_id, handle))
    }

    async fn resolve_stream(
        &self,
        _session_id: u64,
        request: &TransportRequest,
        _cancel: CancellationToken,
    ) -> Result<ResolvedStream, SessionManagerError> {
        let mut resolved = self.resolved.lock().expect("resolved lock");
        let stream = resolved.take().expect("single smoke stream");
        assert_eq!(request.peer_label, "request-1");
        Ok(stream)
    }

    async fn handle_event(
        &self,
        session_id: u64,
        event: SessionEvent,
    ) -> Result<(), SessionManagerError> {
        self.events.lock().expect("events lock").push((session_id, event));
        Ok(())
    }
}

async fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let client = TcpStream::connect(addr).await.expect("client connect");
    let (server, _) = listener.accept().await.expect("server accept");
    (client, server)
}

fn socks5_connect_frame(domain: &str, port: u16) -> Vec<u8> {
    let mut frame = vec![0x05, 0x01, 0x00, 0x03, domain.len() as u8];
    frame.extend_from_slice(domain.as_bytes());
    frame.extend_from_slice(&port.to_be_bytes());
    frame
}

#[tokio::test]
async fn release_smoke_proves_startup_proxy_path_and_shutdown() {
    let startup = run_from([
        "n0wss",
        "--auth-token",
        "release-token",
        "client",
        "--remote-wss-url",
        "wss://example.com/tunnel",
    ])
    .expect("startup should succeed");

    assert_eq!(startup.mode, ApplicationMode::Client);
    assert!(startup.shutdown.can_accept_new_work());

    let (remote_local, mut remote_peer) = duplex(1024);
    let session = Arc::new(SmokeSessionControl::new(ResolvedStream {
        stream: Box::new(SmokeStream {
            stream: remote_local,
            peer_label: "release-peer".to_string(),
        }),
        transport_kind: TransportKind::Wss,
    }));

    let bridge = ProxyBridge::new(
        ProxyBridgeConfig {
            pump_buffer_bytes: 1024,
            total_request_timeout: Duration::from_secs(2),
        },
        session.clone(),
    );
    let (tx, rx) = mpsc::channel(1);
    let worker = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.run_worker(rx).await }
    });

    let (mut client_side, server_side) = tcp_pair().await;
    let parse_task = tokio::spawn(async move { Socks5Proxy::parse_request(server_side).await });

    client_side
        .write_all(&[0x05, 0x01, 0x00])
        .await
        .expect("send greeting");
    let mut method_reply = [0_u8; 2];
    client_side
        .read_exact(&mut method_reply)
        .await
        .expect("read method reply");
    assert_eq!(method_reply, [0x05, 0x00]);

    client_side
        .write_all(&socks5_connect_frame("example.com", 443))
        .await
        .expect("send connect request");
    let intent = parse_task
        .await
        .expect("parse task join")
        .expect("proxy intent");
    tx.send(intent).await.expect("queue smoke intent");

    let mut success_reply = [0_u8; 10];
    client_side
        .read_exact(&mut success_reply)
        .await
        .expect("read success reply");
    assert_eq!(success_reply[1], Socks5Reply::Succeeded as u8);

    let mapped = Socks5Proxy::map_reply(&ProxyError::TransportFailed("infra".to_string()));
    assert_eq!(mapped, Some(Socks5Reply::GeneralFailure));

    client_side
        .write_all(b"ping")
        .await
        .expect("write ping");
    let mut forwarded = [0_u8; 4];
    remote_peer
        .read_exact(&mut forwarded)
        .await
        .expect("read forwarded ping");
    assert_eq!(&forwarded, b"ping");

    remote_peer
        .write_all(b"pong")
        .await
        .expect("write pong");
    let mut returned = [0_u8; 4];
    client_side
        .read_exact(&mut returned)
        .await
        .expect("read pong");
    assert_eq!(&returned, b"pong");

    drop(client_side);
    drop(remote_peer);

    let shutdown_snapshot = coordinate_shutdown(&startup.shutdown);
    assert_eq!(shutdown_snapshot.state, ShutdownState::TransportReleased);

    bridge.stop_accept();
    drop(tx);
    bridge.drain_all().await;
    tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .expect("worker should stop")
        .expect("worker join");

    assert_eq!(
        session.events(),
        vec![(1, SessionEvent::StreamClosed)]
    );
}

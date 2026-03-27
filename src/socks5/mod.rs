// FILE: src/socks5/mod.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Expose a local SOCKS5 ingress surface, normalize CONNECT requests into ProxyIntent work items, and map pre-pump failures to exact client-visible replies.
//   SCOPE: SOCKS5 listener bootstrap, no-auth negotiation, CONNECT parsing, ProxyIntent creation, bounded queue enqueue, reply mapping, and accept-loop shutdown.
//   DEPENDS: std, thiserror, tokio, tokio-util, tracing, src/config/mod.rs, src/obs/mod.rs, src/session/mod.rs
//   LINKS: M-SOCKS5, V-M-SOCKS5, DF-SOCKS5-INTENT, DF-REPLY-MAPPING
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   Socks5ProxyConfig - bind address and timeout policy for local SOCKS5 ingress
//   Socks5Proxy - local listener and bounded intent-queue producer
//   ProxyIntent - normalized CONNECT request ready for bridge processing
//   ProxyProtocol - currently supported proxy protocol discriminator
//   TargetAddr - parsed target address from SOCKS5 CONNECT
//   ProxyError - pre-pump and post-pump error surface with exact reply mapping
//   ErrorCategory - infrastructure, policy, target, or post-reply classification
//   Socks5Reply - exact SOCKS5 reply codes used for client-visible errors
//   run_listener - bind and accept local SOCKS5 connections
//   parse_request - parse SOCKS5 handshake and CONNECT request into ProxyIntent
//   map_reply - map ProxyError to an exact SOCKS5 reply or None after reply emission
//   stop_accept - stop accepting new local connections during shutdown
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added the Phase 4 SOCKS5 ingress boundary with ProxyIntent parsing, fast-fail queue saturation, and exhaustive reply mapping.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::AppConfig;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;

#[cfg(test)]
#[path = "reply_mapping.test.rs"]
mod reply_mapping_tests;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Socks5ProxyConfig {
    pub listen_addr: SocketAddr,
    pub total_timeout: Duration,
}

#[derive(Debug)]
pub struct ProxyIntent {
    pub target: TargetAddr,
    pub client_stream: TcpStream,
    pub protocol_kind: ProxyProtocol,
    pub request_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetAddr {
    Ip(SocketAddr),
    Domain(String, u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyProtocol {
    Socks5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Socks5Reply {
    Succeeded = 0x00,
    GeneralFailure = 0x01,
    NotAllowedByRuleset = 0x02,
    NetworkUnreachable = 0x03,
    HostUnreachable = 0x04,
    ConnectionRefused = 0x05,
    CommandNotSupported = 0x07,
    AddressTypeNotSupported = 0x08,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorCategory {
    Infrastructure,
    Policy,
    Target,
    PostReply,
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("intent queue full")]
    IntentQueueFull,
    #[error("session limit reached")]
    SessionLimitReached,
    #[error("transport failed: {0}")]
    TransportFailed(String),
    #[error("egress denied: {0}")]
    EgressDenied(String),
    #[error("target unreachable: {0}")]
    TargetUnreachable(std::io::Error),
    #[error("pump failed: {0}")]
    PumpFailed(std::io::Error),
    #[error("operation cancelled")]
    Cancelled,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Socks5Error {
    #[error("unsupported SOCKS version: {0}")]
    UnsupportedVersion(u8),
    #[error("no acceptable auth method")]
    NoAcceptableAuthMethod,
    #[error("unsupported command: {0}")]
    UnsupportedCommand(u8),
    #[error("unsupported address type: {0}")]
    UnsupportedAddressType(u8),
    #[error("invalid target address")]
    InvalidTargetAddress,
    #[error("io failed: {0}")]
    Io(String),
}

#[derive(Clone)]
pub struct Socks5Proxy {
    config: Socks5ProxyConfig,
    intent_tx: mpsc::Sender<ProxyIntent>,
    accept_token: CancellationToken,
}

impl Socks5ProxyConfig {
    pub fn from_app_config(config: &AppConfig) -> Option<Self> {
        match &config.runtime_mode {
            crate::config::RuntimeMode::Client(client) => Some(Self {
                listen_addr: client.listen_addr,
                total_timeout: config.timeouts.socks5_total_timeout,
            }),
            crate::config::RuntimeMode::Server(_) => None,
        }
    }
}

impl Socks5Proxy {
    pub fn new(config: Socks5ProxyConfig, intent_tx: mpsc::Sender<ProxyIntent>) -> Self {
        Self {
            config,
            intent_tx,
            accept_token: CancellationToken::new(),
        }
    }

    pub fn stop_accept(&self) {
        self.accept_token.cancel();
    }

    // START_CONTRACT: run_listener
    //   PURPOSE: Bind and accept local SOCKS5 connections until shutdown is requested.
    //   INPUTS: { none }
    //   OUTPUTS: { Result<(), Socks5Error> - listener loop termination status }
    //   SIDE_EFFECTS: [binds a local TCP listener, accepts sockets, and spawns per-connection handlers]
    //   LINKS: [M-SOCKS5, V-M-SOCKS5]
    // END_CONTRACT: run_listener
    pub async fn run_listener(&self) -> Result<(), Socks5Error> {
        let listener = TcpListener::bind(self.config.listen_addr)
            .await
            .map_err(|err| Socks5Error::Io(err.to_string()))?;
        let accept_token = self.accept_token.clone();

        loop {
            tokio::select! {
                _ = accept_token.cancelled() => break Ok(()),
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(|err| Socks5Error::Io(err.to_string()))?;
                    let proxy = self.clone();
                    tokio::spawn(async move {
                        let _ = proxy.handle_connection(stream).await;
                    });
                }
            }
        }
    }

    async fn handle_connection(&self, stream: TcpStream) -> Result<(), Socks5Error> {
        let result = tokio::time::timeout(
            self.config.total_timeout,
            self.handle_connection_inner(stream),
        )
        .await;
        match result {
            Ok(inner) => inner,
            Err(_) => Err(Socks5Error::Io("socks5 total timeout exceeded".to_string())),
        }
    }

    async fn handle_connection_inner(&self, stream: TcpStream) -> Result<(), Socks5Error> {
        let intent = Self::parse_request(stream).await?;
        match self.intent_tx.try_send(intent) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(returned_intent)) => {
                let mut stream = returned_intent.client_stream;
                Self::send_reply(&mut stream, Socks5Reply::GeneralFailure).await?;
                warn!(
                    request_id = returned_intent.request_id,
                    "[Socks5Proxy][mapReply][BLOCK_MAP_REPLY_CODE] intent queue full"
                );
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(returned_intent)) => {
                let mut stream = returned_intent.client_stream;
                Self::send_reply(&mut stream, Socks5Reply::GeneralFailure).await?;
                Ok(())
            }
        }
    }

    // START_CONTRACT: parse_request
    //   PURPOSE: Parse a SOCKS5 handshake and CONNECT request into a normalized ProxyIntent.
    //   INPUTS: { stream: TcpStream - accepted local client socket }
    //   OUTPUTS: { Result<ProxyIntent, Socks5Error> - normalized CONNECT intent or deterministic protocol error }
    //   SIDE_EFFECTS: [reads from and writes to the client socket]
    //   LINKS: [M-SOCKS5, V-M-SOCKS5]
    // END_CONTRACT: parse_request
    pub async fn parse_request(mut stream: TcpStream) -> Result<ProxyIntent, Socks5Error> {
        // START_BLOCK_PARSE_SOCKS5_REQUEST
        let mut greeting = [0_u8; 2];
        stream
            .read_exact(&mut greeting)
            .await
            .map_err(|err| Socks5Error::Io(err.to_string()))?;

        if greeting[0] != 0x05 {
            return Err(Socks5Error::UnsupportedVersion(greeting[0]));
        }

        let methods_len = greeting[1] as usize;
        let mut methods = vec![0_u8; methods_len];
        stream
            .read_exact(&mut methods)
            .await
            .map_err(|err| Socks5Error::Io(err.to_string()))?;

        if !methods.contains(&0x00) {
            stream
                .write_all(&[0x05, 0xff])
                .await
                .map_err(|err| Socks5Error::Io(err.to_string()))?;
            return Err(Socks5Error::NoAcceptableAuthMethod);
        }

        stream
            .write_all(&[0x05, 0x00])
            .await
            .map_err(|err| Socks5Error::Io(err.to_string()))?;

        let mut request_header = [0_u8; 4];
        stream
            .read_exact(&mut request_header)
            .await
            .map_err(|err| Socks5Error::Io(err.to_string()))?;

        if request_header[0] != 0x05 {
            return Err(Socks5Error::UnsupportedVersion(request_header[0]));
        }
        if request_header[1] != 0x01 {
            return Err(Socks5Error::UnsupportedCommand(request_header[1]));
        }

        let target = match request_header[3] {
            0x01 => {
                let mut ipv4 = [0_u8; 4];
                let mut port = [0_u8; 2];
                stream
                    .read_exact(&mut ipv4)
                    .await
                    .map_err(|err| Socks5Error::Io(err.to_string()))?;
                stream
                    .read_exact(&mut port)
                    .await
                    .map_err(|err| Socks5Error::Io(err.to_string()))?;
                let socket_addr = SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(ipv4[0], ipv4[1], ipv4[2], ipv4[3])),
                    u16::from_be_bytes(port),
                );
                TargetAddr::Ip(socket_addr)
            }
            0x03 => {
                let mut len = [0_u8; 1];
                stream
                    .read_exact(&mut len)
                    .await
                    .map_err(|err| Socks5Error::Io(err.to_string()))?;
                let mut domain = vec![0_u8; len[0] as usize];
                let mut port = [0_u8; 2];
                stream
                    .read_exact(&mut domain)
                    .await
                    .map_err(|err| Socks5Error::Io(err.to_string()))?;
                stream
                    .read_exact(&mut port)
                    .await
                    .map_err(|err| Socks5Error::Io(err.to_string()))?;
                let domain =
                    String::from_utf8(domain).map_err(|_| Socks5Error::InvalidTargetAddress)?;
                TargetAddr::Domain(domain, u16::from_be_bytes(port))
            }
            atyp => return Err(Socks5Error::UnsupportedAddressType(atyp)),
        };

        let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        info!(
            request_id,
            target = ?target,
            "[Socks5Proxy][parseRequest][BLOCK_PARSE_SOCKS5_REQUEST] parsed proxy intent"
        );

        Ok(ProxyIntent {
            target,
            client_stream: stream,
            protocol_kind: ProxyProtocol::Socks5,
            request_id,
        })
        // END_BLOCK_PARSE_SOCKS5_REQUEST
    }

    // START_CONTRACT: map_reply
    //   PURPOSE: Convert one ProxyError into an exact SOCKS5 reply or no-reply post-pump outcome.
    //   INPUTS: { error: &ProxyError - classified proxy failure }
    //   OUTPUTS: { Option<Socks5Reply> - exact reply code or None when success reply was already sent }
    //   SIDE_EFFECTS: [emits structured mapping log]
    //   LINKS: [M-SOCKS5, V-M-SOCKS5]
    // END_CONTRACT: map_reply
    pub fn map_reply(error: &ProxyError) -> Option<Socks5Reply> {
        // START_BLOCK_MAP_REPLY_CODE
        let reply = match error {
            ProxyError::IntentQueueFull => Some(Socks5Reply::GeneralFailure),
            ProxyError::SessionLimitReached => Some(Socks5Reply::GeneralFailure),
            ProxyError::TransportFailed(_) => Some(Socks5Reply::GeneralFailure),
            ProxyError::EgressDenied(_) => Some(Socks5Reply::NotAllowedByRuleset),
            ProxyError::TargetUnreachable(err) => Some(Self::map_target_error(err)),
            ProxyError::PumpFailed(_) => None,
            ProxyError::Cancelled => Some(Socks5Reply::GeneralFailure),
        };

        info!(
            category = ?error.category(),
            reply = ?reply,
            "[Socks5Proxy][mapReply][BLOCK_MAP_REPLY_CODE] mapped proxy error to reply"
        );
        reply
        // END_BLOCK_MAP_REPLY_CODE
    }

    pub async fn send_reply(stream: &mut TcpStream, reply: Socks5Reply) -> Result<(), Socks5Error> {
        let response = [
            0x05,
            reply as u8,
            0x00,
            0x01,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
        ];
        stream
            .write_all(&response)
            .await
            .map_err(|err| Socks5Error::Io(err.to_string()))?;
        stream
            .shutdown()
            .await
            .map_err(|err| Socks5Error::Io(err.to_string()))
    }

    fn map_target_error(error: &std::io::Error) -> Socks5Reply {
        let kind = error.kind();
        if kind == std::io::ErrorKind::ConnectionRefused {
            Socks5Reply::ConnectionRefused
        } else if kind == std::io::ErrorKind::TimedOut
            || kind == std::io::ErrorKind::AddrNotAvailable
        {
            Socks5Reply::HostUnreachable
        } else if kind == std::io::ErrorKind::NetworkUnreachable {
            Socks5Reply::NetworkUnreachable
        } else {
            Socks5Reply::GeneralFailure
        }
    }
}

impl ProxyError {
    pub fn category(&self) -> ErrorCategory {
        match self {
            ProxyError::IntentQueueFull => ErrorCategory::Infrastructure,
            ProxyError::SessionLimitReached => ErrorCategory::Infrastructure,
            ProxyError::TransportFailed(_) => ErrorCategory::Infrastructure,
            ProxyError::EgressDenied(_) => ErrorCategory::Policy,
            ProxyError::TargetUnreachable(_) => ErrorCategory::Target,
            ProxyError::PumpFailed(_) => ErrorCategory::PostReply,
            ProxyError::Cancelled => ErrorCategory::Infrastructure,
        }
    }
}

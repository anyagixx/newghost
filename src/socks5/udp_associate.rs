// FILE: src/socks5/udp_associate.rs
// VERSION: 0.1.4
// START_MODULE_CONTRACT
//   PURPOSE: Negotiate governed SOCKS5 UDP ASSOCIATE relay binds, normalize SOCKS5 UDP relay packets, encode inbound relay replies, retain live relay sockets, and drive the local UDP runtime receive loop into a bounded handoff target.
//   SCOPE: Local UDP relay bind allocation, SOCKS5 UDP ASSOCIATE success replies, UDP relay packet parsing, UDP relay packet encoding, source validation, fragmentation rejection, datagram-envelope validation, relay-socket retention, runtime source learning, and live relay-loop forwarding.
//   DEPENDS: async-trait, std, thiserror, tokio, tokio-util, tracing, src/socks5/mod.rs, src/transport/datagram_contract.rs
//   LINKS: M-SOCKS5-UDP-ASSOCIATE, V-M-SOCKS5-UDP-ASSOCIATE, DF-SOCKS5-UDP-ASSOCIATE, DF-UDP-OUTBOUND
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   UdpAssociateRecord - governed UDP relay bind plus owning relay socket
//   UdpAssociateError - deterministic UDP ASSOCIATE and relay-packet failure surface
//   UdpRelaySocketRegistry - shared live relay-socket retention used by inbound delivery to the owning local UDP socket
//   UdpRelayRuntimeTarget - bounded live handoff target for normalized UDP relay packets
//   handle_udp_associate - negotiate one SOCKS5 UDP ASSOCIATE request and return the governed relay bind
//   parse_udp_datagram - normalize one SOCKS5 UDP relay packet into the shared datagram contract and emit a bounded normalization anchor
//   encode_udp_datagram - encode one governed inbound datagram into the SOCKS5 UDP relay packet shape expected by the owning local client
//   run_udp_relay_runtime_loop - read live UDP relay packets and forward normalized datagrams into the configured runtime handoff
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.4 - Bound UDP ASSOCIATE relay sockets to the control socket local address so LAN-facing listeners can return a reachable relay bind without breaking localhost callers.
// END_CHANGE_SUMMARY

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::socks5::{Socks5Error, TargetAddr};
use crate::transport::datagram_contract::{
    DatagramAssociationId, DatagramEnvelope, DatagramError, DatagramTarget,
};

#[cfg(test)]
#[path = "udp_associate.test.rs"]
mod tests;

#[derive(Debug)]
pub struct UdpAssociateRecord {
    pub relay_addr: SocketAddr,
    pub relay_socket: Arc<UdpSocket>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UdpAssociateError {
    #[error("udp associate IO failed: {0}")]
    Io(String),
    #[error("invalid reserved field in UDP relay packet")]
    InvalidReservedField,
    #[error("unsupported UDP fragmentation value: {0}")]
    UnsupportedFragmentation(u8),
    #[error("unsupported UDP address type: {0}")]
    UnsupportedAddressType(u8),
    #[error("invalid UDP target address")]
    InvalidTargetAddress,
    #[error("foreign UDP source: expected {expected}, got {actual}")]
    ForeignSource {
        expected: SocketAddr,
        actual: SocketAddr,
    },
    #[error("datagram contract violation: {0}")]
    DatagramContract(#[from] DatagramError),
}

#[derive(Clone, Default)]
pub struct UdpRelaySocketRegistry {
    sockets: Arc<Mutex<HashMap<SocketAddr, Arc<UdpSocket>>>>,
}

impl UdpRelaySocketRegistry {
    pub fn register(&self, relay_addr: SocketAddr, relay_socket: Arc<UdpSocket>) {
        self.sockets
            .lock()
            .expect("udp relay socket registry lock poisoned")
            .insert(relay_addr, relay_socket);
    }

    pub fn socket_for(&self, relay_addr: SocketAddr) -> Option<Arc<UdpSocket>> {
        self.sockets
            .lock()
            .expect("udp relay socket registry lock poisoned")
            .get(&relay_addr)
            .cloned()
    }

    pub fn remove(&self, relay_addr: SocketAddr) -> Option<Arc<UdpSocket>> {
        self.sockets
            .lock()
            .expect("udp relay socket registry lock poisoned")
            .remove(&relay_addr)
    }
}

#[async_trait]
pub trait UdpRelayRuntimeTarget: Send + Sync + 'static {
    async fn forward_runtime_datagram(
        &self,
        relay_addr: SocketAddr,
        expected_client_addr: SocketAddr,
        target: DatagramTarget,
        payload: Vec<u8>,
    ) -> Result<(), UdpAssociateError>;
}

// START_CONTRACT: handle_udp_associate
//   PURPOSE: Allocate a governed local UDP relay bind for one SOCKS5 UDP ASSOCIATE request and emit the corresponding success reply.
//   INPUTS: { stream: &mut TcpStream - accepted SOCKS5 control socket after command validation }
//   OUTPUTS: { Result<UdpAssociateRecord, UdpAssociateError> - owned relay socket and the bind address returned to the client }
//   SIDE_EFFECTS: [binds a local UDP socket, writes a SOCKS5 success reply, and emits a stable log anchor]
//   LINKS: [M-SOCKS5-UDP-ASSOCIATE, V-M-SOCKS5-UDP-ASSOCIATE]
// END_CONTRACT: handle_udp_associate
pub async fn handle_udp_associate(
    stream: &mut TcpStream,
) -> Result<UdpAssociateRecord, UdpAssociateError> {
    // START_BLOCK_HANDLE_UDP_ASSOCIATE
    let control_local_addr = stream
        .local_addr()
        .map_err(|err| UdpAssociateError::Io(err.to_string()))?;
    let relay_bind_ip = match control_local_addr.ip() {
        IpAddr::V4(ipv4) if ipv4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        ip => ip,
    };
    let relay_socket = Arc::new(
        UdpSocket::bind(SocketAddr::new(relay_bind_ip, 0))
            .await
            .map_err(|err| UdpAssociateError::Io(err.to_string()))?,
    );
    let relay_addr = relay_socket
        .local_addr()
        .map_err(|err| UdpAssociateError::Io(err.to_string()))?;

    let mut reply = vec![0x05, 0x00, 0x00];
    match relay_addr.ip() {
        IpAddr::V4(ipv4) => {
            reply.push(0x01);
            reply.extend_from_slice(&ipv4.octets());
        }
        IpAddr::V6(ipv6) => {
            reply.push(0x04);
            reply.extend_from_slice(&ipv6.octets());
        }
    }
    reply.extend_from_slice(&relay_addr.port().to_be_bytes());
    stream
        .write_all(&reply)
        .await
        .map_err(|err| UdpAssociateError::Io(err.to_string()))?;

    info!(
        control_local_addr = %control_local_addr,
        relay_addr = %relay_addr,
        "[Socks5Proxy][handleUdpAssociate][BLOCK_HANDLE_UDP_ASSOCIATE] allocated governed UDP relay bind"
    );

    Ok(UdpAssociateRecord {
        relay_addr,
        relay_socket,
    })
    // END_BLOCK_HANDLE_UDP_ASSOCIATE
}

// START_CONTRACT: parse_udp_datagram
//   PURPOSE: Parse one SOCKS5 UDP relay packet into the shared datagram contract under deterministic source and fragmentation rules.
//   INPUTS: { source: SocketAddr - actual UDP sender address, expected_source: SocketAddr - association-owned sender address, packet: &[u8] - raw SOCKS5 UDP relay packet, association_id: DatagramAssociationId - stable association identity }
//   OUTPUTS: { Result<DatagramEnvelope, UdpAssociateError> - normalized datagram envelope or deterministic protocol rejection }
//   SIDE_EFFECTS: [emits a warning when foreign or unsupported UDP traffic is rejected]
//   LINKS: [M-SOCKS5-UDP-ASSOCIATE, M-DATAGRAM-CONTRACT, V-M-SOCKS5-UDP-ASSOCIATE]
// END_CONTRACT: parse_udp_datagram
pub fn parse_udp_datagram(
    source: SocketAddr,
    expected_source: SocketAddr,
    packet: &[u8],
    association_id: DatagramAssociationId,
) -> Result<DatagramEnvelope, UdpAssociateError> {
    if source != expected_source {
        warn!(
            expected_source = %expected_source,
            actual_source = %source,
            "[Socks5Proxy][handleUdpAssociate][BLOCK_HANDLE_UDP_ASSOCIATE] rejected foreign UDP source"
        );
        return Err(UdpAssociateError::ForeignSource {
            expected: expected_source,
            actual: source,
        });
    }

    if packet.len() < 4 {
        return Err(UdpAssociateError::InvalidReservedField);
    }

    if packet[0] != 0x00 || packet[1] != 0x00 {
        return Err(UdpAssociateError::InvalidReservedField);
    }

    if packet[2] != 0x00 {
        warn!(
            fragment = packet[2],
            "[Socks5Proxy][handleUdpAssociate][BLOCK_HANDLE_UDP_ASSOCIATE] rejected fragmented UDP relay packet"
        );
        return Err(UdpAssociateError::UnsupportedFragmentation(packet[2]));
    }

    let (target, payload_offset) = match packet[3] {
        0x01 => {
            if packet.len() < 10 {
                return Err(UdpAssociateError::InvalidTargetAddress);
            }
            let ip = IpAddr::V4(Ipv4Addr::new(packet[4], packet[5], packet[6], packet[7]));
            let port = u16::from_be_bytes([packet[8], packet[9]]);
            (DatagramTarget::Ip(SocketAddr::new(ip, port)), 10)
        }
        0x03 => {
            if packet.len() < 5 {
                return Err(UdpAssociateError::InvalidTargetAddress);
            }
            let domain_len = packet[4] as usize;
            let domain_end = 5 + domain_len;
            if packet.len() < domain_end + 2 {
                return Err(UdpAssociateError::InvalidTargetAddress);
            }
            let domain = String::from_utf8(packet[5..domain_end].to_vec())
                .map_err(|_| UdpAssociateError::InvalidTargetAddress)?;
            let port = u16::from_be_bytes([packet[domain_end], packet[domain_end + 1]]);
            (DatagramTarget::Domain(domain, port), domain_end + 2)
        }
        atyp => return Err(UdpAssociateError::UnsupportedAddressType(atyp)),
    };

    let envelope = DatagramEnvelope {
        association_id,
        relay_client_addr: source,
        target,
        payload: packet[payload_offset..].to_vec(),
    };
    envelope.validate()?;
    info!(
        association_id,
        target = ?envelope.target,
        payload_len = envelope.payload.len(),
        "[Socks5Proxy][parseUdpDatagram][BLOCK_PARSE_UDP_DATAGRAM] normalized governed UDP relay packet"
    );
    Ok(envelope)
}

// START_CONTRACT: encode_udp_datagram
//   PURPOSE: Encode one governed inbound datagram into the SOCKS5 UDP relay packet shape expected by the owning local client.
//   INPUTS: { envelope: &DatagramEnvelope - normalized inbound datagram envelope for one owned association }
//   OUTPUTS: { Result<Vec<u8>, UdpAssociateError> - encoded SOCKS5 UDP relay packet ready for local relay delivery }
//   SIDE_EFFECTS: [none]
//   LINKS: [M-SOCKS5-UDP-ASSOCIATE, M-DATAGRAM-CONTRACT, V-M-SOCKS5-UDP-ASSOCIATE]
// END_CONTRACT: encode_udp_datagram
pub fn encode_udp_datagram(
    envelope: &DatagramEnvelope,
) -> Result<Vec<u8>, UdpAssociateError> {
    let mut packet = vec![0x00, 0x00, 0x00];
    match &envelope.target {
        DatagramTarget::Ip(addr) => match addr.ip() {
            IpAddr::V4(ipv4) => {
                packet.push(0x01);
                packet.extend_from_slice(&ipv4.octets());
                packet.extend_from_slice(&addr.port().to_be_bytes());
            }
            IpAddr::V6(_) => return Err(UdpAssociateError::UnsupportedAddressType(0x04)),
        },
        DatagramTarget::Domain(domain, port) => {
            if domain.len() > u8::MAX as usize {
                return Err(UdpAssociateError::InvalidTargetAddress);
            }
            packet.push(0x03);
            packet.push(domain.len() as u8);
            packet.extend_from_slice(domain.as_bytes());
            packet.extend_from_slice(&port.to_be_bytes());
        }
    }
    packet.extend_from_slice(&envelope.payload);
    Ok(packet)
}

// START_CONTRACT: run_udp_relay_runtime_loop
//   PURPOSE: Read live packets from the governed UDP relay socket, normalize them, and forward them into one bounded runtime handoff target until cancellation.
//   INPUTS: { record: UdpAssociateRecord - owned governed relay socket, client_hint: TargetAddr - client-provided UDP endpoint hint from the control request, forward_target: Arc<dyn UdpRelayRuntimeTarget> - bounded runtime handoff target, cancel: CancellationToken - shutdown boundary for the relay loop }
//   OUTPUTS: { Result<(), UdpAssociateError> - ok when the loop stops cleanly or forwarding stays within deterministic protocol bounds }
//   SIDE_EFFECTS: [reads the local UDP relay socket, emits a stable runtime-loop anchor, and forwards normalized packets]
//   LINKS: [M-SOCKS5-UDP-ASSOCIATE, V-M-SOCKS5-UDP-ASSOCIATE]
// END_CONTRACT: run_udp_relay_runtime_loop
pub async fn run_udp_relay_runtime_loop(
    record: UdpAssociateRecord,
    client_hint: TargetAddr,
    forward_target: Arc<dyn UdpRelayRuntimeTarget>,
    cancel: CancellationToken,
) -> Result<(), UdpAssociateError> {
    // START_BLOCK_SOCKS5_UDP_RUNTIME_LOOP
    let UdpAssociateRecord {
        relay_addr,
        relay_socket,
    } = record;
    let mut expected_source = hinted_client_addr(&client_hint);
    let mut packet = vec![0_u8; 65_535];

    loop {
        let received = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            received = relay_socket.recv_from(&mut packet) => {
                received.map_err(|err| UdpAssociateError::Io(err.to_string()))?
            }
        };

        let (packet_len, source) = received;
        let learned_source = expected_source.unwrap_or(source);
        let envelope = parse_udp_datagram(
            source,
            learned_source,
            &packet[..packet_len],
            0,
        )?;
        expected_source.get_or_insert(learned_source);

        info!(
            relay_addr = %relay_addr,
            expected_client_addr = %learned_source,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[Socks5UdpRuntimeLoop][runRelayLoop][BLOCK_SOCKS5_UDP_RUNTIME_LOOP] forwarding normalized governed UDP relay packet into runtime handoff"
        );

        forward_target
            .forward_runtime_datagram(
                relay_addr,
                learned_source,
                envelope.target,
                envelope.payload,
            )
            .await?;
    }
    // END_BLOCK_SOCKS5_UDP_RUNTIME_LOOP
}

impl From<Socks5Error> for UdpAssociateError {
    fn from(value: Socks5Error) -> Self {
        UdpAssociateError::Io(value.to_string())
    }
}

fn hinted_client_addr(client_hint: &TargetAddr) -> Option<SocketAddr> {
    match client_hint {
        TargetAddr::Ip(addr) if !addr.ip().is_unspecified() && addr.port() != 0 => Some(*addr),
        _ => None,
    }
}

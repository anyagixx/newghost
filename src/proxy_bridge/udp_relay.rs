// FILE: src/proxy_bridge/udp_relay.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Open real UDP sockets on the server side, relay outbound datagrams to remote targets, and return inbound packets to the owning association.
//   SCOPE: Outbound target resolution, UDP socket binding, outbound relay, inbound receive, and foreign-source rejection.
//   DEPENDS: std, thiserror, tokio, tracing, src/transport/datagram_contract.rs
//   LINKS: M-UDP-EGRESS-RELAY, V-M-UDP-EGRESS-RELAY, DF-UDP-OUTBOUND, DF-UDP-INBOUND
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   UdpRelayRecord - one active server-side UDP relay socket bound to an owning association and remote peer
//   UdpRelayError - deterministic outbound or inbound UDP relay failure surface
//   relay_outbound_datagram - send one governed outbound datagram to the resolved remote UDP target
//   relay_inbound_datagram - receive one inbound UDP packet and map it back to the owning association
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added explicit server-side UDP relay helpers so datagram transport work has deterministic outbound and inbound relay evidence.
// END_CHANGE_SUMMARY

use std::net::SocketAddr;

use thiserror::Error;
use tokio::net::{lookup_host, UdpSocket};
use tracing::{info, warn};

use crate::transport::datagram_contract::{DatagramAssociationId, DatagramEnvelope, DatagramTarget};

#[cfg(test)]
#[path = "udp_relay.test.rs"]
mod tests;

pub struct UdpRelayRecord {
    pub association_id: DatagramAssociationId,
    pub relay_socket: UdpSocket,
    pub remote_peer: SocketAddr,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UdpRelayError {
    #[error("udp relay target resolution failed: {0}")]
    ResolveFailed(String),
    #[error("udp relay bind failed: {0}")]
    BindFailed(String),
    #[error("udp relay send failed: {0}")]
    SendFailed(String),
    #[error("udp relay receive failed: {0}")]
    ReceiveFailed(String),
    #[error("unexpected inbound source: expected {expected}, got {actual}")]
    UnexpectedSource {
        expected: SocketAddr,
        actual: SocketAddr,
    },
}

// START_CONTRACT: relay_outbound_datagram
//   PURPOSE: Bind one relay socket, resolve the remote UDP peer, and send one outbound datagram for the owning association.
//   INPUTS: { envelope: &DatagramEnvelope - normalized outbound datagram with association and target metadata }
//   OUTPUTS: { Result<UdpRelayRecord, UdpRelayError> - active relay socket and resolved remote peer for later inbound validation }
//   SIDE_EFFECTS: [binds a UDP socket, sends one datagram, and emits the outbound relay log anchor]
//   LINKS: [M-UDP-EGRESS-RELAY, V-M-UDP-EGRESS-RELAY]
// END_CONTRACT: relay_outbound_datagram
pub async fn relay_outbound_datagram(
    envelope: &DatagramEnvelope,
) -> Result<UdpRelayRecord, UdpRelayError> {
    // START_BLOCK_RELAY_UDP_OUTBOUND
    let remote_peer = resolve_target(&envelope.target).await?;
    let relay_socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|err| UdpRelayError::BindFailed(err.to_string()))?;
    relay_socket
        .send_to(&envelope.payload, remote_peer)
        .await
        .map_err(|err| UdpRelayError::SendFailed(err.to_string()))?;

    info!(
        association_id = envelope.association_id,
        remote_peer = %remote_peer,
        payload_len = envelope.payload.len(),
        "[UdpEgressRelay][relayOutbound][BLOCK_RELAY_UDP_OUTBOUND] relayed outbound UDP datagram"
    );

    Ok(UdpRelayRecord {
        association_id: envelope.association_id,
        relay_socket,
        remote_peer,
    })
    // END_BLOCK_RELAY_UDP_OUTBOUND
}

// START_CONTRACT: relay_inbound_datagram
//   PURPOSE: Receive one inbound UDP packet from the expected remote peer and map it back to the owning association.
//   INPUTS: { relay: &UdpRelayRecord - active relay socket plus owning association and expected source }
//   OUTPUTS: { Result<DatagramEnvelope, UdpRelayError> - inbound datagram mapped back to the owning association }
//   SIDE_EFFECTS: [reads one UDP packet from the relay socket and emits the inbound relay log anchor]
//   LINKS: [M-UDP-EGRESS-RELAY, V-M-UDP-EGRESS-RELAY]
// END_CONTRACT: relay_inbound_datagram
pub async fn relay_inbound_datagram(
    relay: &UdpRelayRecord,
) -> Result<DatagramEnvelope, UdpRelayError> {
    // START_BLOCK_RELAY_UDP_INBOUND
    let mut buffer = vec![0_u8; 65_535];
    let (bytes_read, source) = relay
        .relay_socket
        .recv_from(&mut buffer)
        .await
        .map_err(|err| UdpRelayError::ReceiveFailed(err.to_string()))?;
    if source != relay.remote_peer {
        warn!(
            association_id = relay.association_id,
            expected = %relay.remote_peer,
            actual = %source,
            "[UdpEgressRelay][relayInbound][BLOCK_RELAY_UDP_INBOUND] rejected unexpected inbound UDP source"
        );
        return Err(UdpRelayError::UnexpectedSource {
            expected: relay.remote_peer,
            actual: source,
        });
    }

    info!(
        association_id = relay.association_id,
        remote_peer = %source,
        payload_len = bytes_read,
        "[UdpEgressRelay][relayInbound][BLOCK_RELAY_UDP_INBOUND] relayed inbound UDP datagram"
    );

    Ok(DatagramEnvelope {
        association_id: relay.association_id,
        relay_client_addr: relay
            .relay_socket
            .local_addr()
            .map_err(|err| UdpRelayError::ReceiveFailed(err.to_string()))?,
        target: DatagramTarget::Ip(source),
        payload: buffer[..bytes_read].to_vec(),
    })
    // END_BLOCK_RELAY_UDP_INBOUND
}

async fn resolve_target(target: &DatagramTarget) -> Result<SocketAddr, UdpRelayError> {
    match target {
        DatagramTarget::Ip(addr) => Ok(*addr),
        DatagramTarget::Domain(domain, port) => lookup_host((domain.as_str(), *port))
            .await
            .map_err(|err| UdpRelayError::ResolveFailed(err.to_string()))?
            .next()
            .ok_or_else(|| UdpRelayError::ResolveFailed("no resolved UDP target".to_string())),
    }
}

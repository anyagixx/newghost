// FILE: src/wss_gateway/datagram.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Encapsulate governed datagrams inside WSS frames and recover them on the opposite side without altering the existing stream tunnel path.
//   SCOPE: Datagram frame encoding, decoding, binary-message validation, and send helper logic for the WSS datagram path.
//   DEPENDS: futures-util, thiserror, tokio-tungstenite, tracing, src/transport/datagram_contract.rs
//   LINKS: M-WSS-DATAGRAM-GATEWAY, V-M-WSS-DATAGRAM-GATEWAY, DF-UDP-OUTBOUND, DF-UDP-INBOUND
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   WssDatagramError - deterministic frame encoding and decoding failure surface
//   encode_datagram - encode one transport-agnostic datagram into a WSS binary frame
//   decode_datagram - decode one WSS binary frame into the shared datagram contract
//   send_datagram - send one datagram as a WSS binary message with a stable log anchor
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added explicit WSS datagram framing helpers so UDP-capable transport work has a bounded binary carrier contract.
// END_CHANGE_SUMMARY

use futures_util::Sink;
use futures_util::SinkExt;
use thiserror::Error;
use tokio_tungstenite::tungstenite::Message;
use tracing::info;

use crate::transport::datagram_contract::{DatagramEnvelope, DatagramTarget};

#[cfg(test)]
#[path = "datagram.test.rs"]
mod tests;

const DATAGRAM_FRAME_VERSION: u8 = 1;
const TARGET_KIND_IP_V4: u8 = 1;
const TARGET_KIND_DOMAIN: u8 = 3;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WssDatagramError {
    #[error("invalid datagram frame version: {0}")]
    InvalidVersion(u8),
    #[error("unsupported datagram target kind: {0}")]
    UnsupportedTargetKind(u8),
    #[error("invalid datagram frame")]
    InvalidFrame,
    #[error("non-binary websocket message is not a datagram frame")]
    NonBinaryFrame,
    #[error("datagram send failed: {0}")]
    SendFailed(String),
}

pub fn encode_datagram(envelope: &DatagramEnvelope) -> Result<Vec<u8>, WssDatagramError> {
    let mut frame = Vec::with_capacity(64 + envelope.payload.len());
    frame.push(DATAGRAM_FRAME_VERSION);
    frame.extend_from_slice(&envelope.association_id.to_be_bytes());
    match &envelope.target {
        DatagramTarget::Ip(addr) => match addr.ip() {
            std::net::IpAddr::V4(ipv4) => {
                frame.push(TARGET_KIND_IP_V4);
                frame.extend_from_slice(&ipv4.octets());
                frame.extend_from_slice(&addr.port().to_be_bytes());
            }
            std::net::IpAddr::V6(_) => return Err(WssDatagramError::UnsupportedTargetKind(4)),
        },
        DatagramTarget::Domain(domain, port) => {
            if domain.len() > u8::MAX as usize {
                return Err(WssDatagramError::InvalidFrame);
            }
            frame.push(TARGET_KIND_DOMAIN);
            frame.push(domain.len() as u8);
            frame.extend_from_slice(domain.as_bytes());
            frame.extend_from_slice(&port.to_be_bytes());
        }
    }
    let relay_ip = envelope.relay_client_addr.ip().to_string();
    if relay_ip.len() > u16::MAX as usize {
        return Err(WssDatagramError::InvalidFrame);
    }
    frame.extend_from_slice(&(relay_ip.len() as u16).to_be_bytes());
    frame.extend_from_slice(relay_ip.as_bytes());
    frame.extend_from_slice(&envelope.relay_client_addr.port().to_be_bytes());
    frame.extend_from_slice(&(envelope.payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&envelope.payload);
    Ok(frame)
}

pub fn decode_datagram(frame: &[u8]) -> Result<DatagramEnvelope, WssDatagramError> {
    if frame.len() < 1 + 8 + 1 {
        return Err(WssDatagramError::InvalidFrame);
    }

    if frame[0] != DATAGRAM_FRAME_VERSION {
        return Err(WssDatagramError::InvalidVersion(frame[0]));
    }

    let association_id = u64::from_be_bytes(
        frame[1..9]
            .try_into()
            .map_err(|_| WssDatagramError::InvalidFrame)?,
    );
    let mut cursor = 9;

    let target = match frame[cursor] {
        TARGET_KIND_IP_V4 => {
            if frame.len() < cursor + 1 + 4 + 2 {
                return Err(WssDatagramError::InvalidFrame);
            }
            let ip = std::net::Ipv4Addr::new(
                frame[cursor + 1],
                frame[cursor + 2],
                frame[cursor + 3],
                frame[cursor + 4],
            );
            let port = u16::from_be_bytes([frame[cursor + 5], frame[cursor + 6]]);
            cursor += 7;
            DatagramTarget::Ip(std::net::SocketAddr::new(std::net::IpAddr::V4(ip), port))
        }
        TARGET_KIND_DOMAIN => {
            if frame.len() < cursor + 2 {
                return Err(WssDatagramError::InvalidFrame);
            }
            let domain_len = frame[cursor + 1] as usize;
            let domain_start = cursor + 2;
            let domain_end = domain_start + domain_len;
            if frame.len() < domain_end + 2 {
                return Err(WssDatagramError::InvalidFrame);
            }
            let domain = String::from_utf8(frame[domain_start..domain_end].to_vec())
                .map_err(|_| WssDatagramError::InvalidFrame)?;
            let port = u16::from_be_bytes([frame[domain_end], frame[domain_end + 1]]);
            cursor = domain_end + 2;
            DatagramTarget::Domain(domain, port)
        }
        other => return Err(WssDatagramError::UnsupportedTargetKind(other)),
    };

    if frame.len() < cursor + 2 {
        return Err(WssDatagramError::InvalidFrame);
    }
    let relay_ip_len = u16::from_be_bytes([frame[cursor], frame[cursor + 1]]) as usize;
    cursor += 2;
    if frame.len() < cursor + relay_ip_len + 2 + 4 {
        return Err(WssDatagramError::InvalidFrame);
    }
    let relay_ip = std::str::from_utf8(&frame[cursor..cursor + relay_ip_len])
        .map_err(|_| WssDatagramError::InvalidFrame)?
        .parse()
        .map_err(|_| WssDatagramError::InvalidFrame)?;
    cursor += relay_ip_len;
    let relay_port = u16::from_be_bytes([frame[cursor], frame[cursor + 1]]);
    cursor += 2;
    let payload_len = u32::from_be_bytes(
        frame[cursor..cursor + 4]
            .try_into()
            .map_err(|_| WssDatagramError::InvalidFrame)?,
    ) as usize;
    cursor += 4;
    if frame.len() < cursor + payload_len {
        return Err(WssDatagramError::InvalidFrame);
    }

    Ok(DatagramEnvelope {
        association_id,
        relay_client_addr: std::net::SocketAddr::new(relay_ip, relay_port),
        target,
        payload: frame[cursor..cursor + payload_len].to_vec(),
    })
}

// START_CONTRACT: send_datagram
//   PURPOSE: Encode and send one governed datagram as a WSS binary frame.
//   INPUTS: { sink: &mut S - websocket sink for tungstenite messages, envelope: &DatagramEnvelope - normalized datagram contract }
//   OUTPUTS: { Result<(), WssDatagramError> - ok when the datagram frame is sent successfully }
//   SIDE_EFFECTS: [writes one websocket message and emits the stable datagram log anchor]
//   LINKS: [M-WSS-DATAGRAM-GATEWAY, V-M-WSS-DATAGRAM-GATEWAY]
// END_CONTRACT: send_datagram
pub async fn send_datagram<S>(sink: &mut S, envelope: &DatagramEnvelope) -> Result<(), WssDatagramError>
where
    S: Sink<Message> + Unpin,
    S::Error: std::error::Error,
{
    // START_BLOCK_SEND_WSS_DATAGRAM
    let frame = encode_datagram(envelope)?;
    sink.send(Message::Binary(frame.into()))
        .await
        .map_err(|err| WssDatagramError::SendFailed(err.to_string()))?;
    info!(
        association_id = envelope.association_id,
        target = ?envelope.target,
        payload_len = envelope.payload.len(),
        "[WssDatagramGateway][sendDatagram][BLOCK_SEND_WSS_DATAGRAM] sent governed WSS datagram"
    );
    Ok(())
    // END_BLOCK_SEND_WSS_DATAGRAM
}

pub fn decode_message(message: Message) -> Result<DatagramEnvelope, WssDatagramError> {
    match message {
        Message::Binary(bytes) => decode_datagram(&bytes),
        _ => Err(WssDatagramError::NonBinaryFrame),
    }
}

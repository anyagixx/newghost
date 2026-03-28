// FILE: src/wss_gateway/datagram.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify that governed datagrams are encoded into WSS binary frames and decoded back without losing association or target metadata.
//   SCOPE: Domain-target roundtrip, IPv4-target roundtrip, non-binary rejection, and send helper emission.
//   DEPENDS: src/wss_gateway/datagram.rs, src/transport/datagram_contract.rs
//   LINKS: V-M-WSS-DATAGRAM-GATEWAY
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   domain_datagram_roundtrips_through_binary_frame - proves association identity and domain target survive frame encoding
//   ipv4_datagram_roundtrips_through_binary_frame - proves IPv4 targets survive frame encoding
//   non_binary_messages_are_rejected - proves text frames cannot be misread as datagram frames
//   send_datagram_emits_one_binary_message - proves the send helper produces one binary WSS message
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added deterministic WSS datagram framing tests so the UDP carrier boundary stays explicit and reviewable.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Sink;
use tokio_tungstenite::tungstenite::Message;

use super::{decode_datagram, decode_message, encode_datagram, send_datagram, WssDatagramError};
use crate::transport::datagram_contract::{DatagramEnvelope, DatagramTarget};

#[derive(Default)]
struct VecSink {
    messages: Vec<Message>,
}

impl Sink<Message> for VecSink {
    type Error = std::io::Error;

    fn poll_ready(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        self.get_mut().messages.push(item);
        Ok(())
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

fn sample_domain_envelope() -> DatagramEnvelope {
    DatagramEnvelope {
        association_id: 7,
        relay_client_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54000),
        target: DatagramTarget::Domain("example.com".to_string(), 443),
        payload: vec![1, 2, 3, 4],
    }
}

#[test]
fn domain_datagram_roundtrips_through_binary_frame() {
    let envelope = sample_domain_envelope();
    let frame = encode_datagram(&envelope).expect("encode");
    let decoded = decode_datagram(&frame).expect("decode");
    assert_eq!(decoded, envelope);
}

#[test]
fn ipv4_datagram_roundtrips_through_binary_frame() {
    let envelope = DatagramEnvelope {
        association_id: 8,
        relay_client_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54001),
        target: DatagramTarget::Ip(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
            3478,
        )),
        payload: vec![9, 8, 7],
    };
    let frame = encode_datagram(&envelope).expect("encode");
    let decoded = decode_datagram(&frame).expect("decode");
    assert_eq!(decoded, envelope);
}

#[test]
fn non_binary_messages_are_rejected() {
    let error = decode_message(Message::Text("not-binary".into())).expect_err("reject text");
    assert_eq!(error, WssDatagramError::NonBinaryFrame);
}

#[tokio::test]
async fn send_datagram_emits_one_binary_message() {
    let envelope = sample_domain_envelope();
    let mut sink = VecSink::default();

    send_datagram(&mut sink, &envelope).await.expect("send datagram");
    assert_eq!(sink.messages.len(), 1);
    let decoded = decode_message(sink.messages.remove(0)).expect("decode sent message");
    assert_eq!(decoded, envelope);
}

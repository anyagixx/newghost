// FILE: src/transport/datagram_contract.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify that the governed datagram contract stays transport-agnostic, bounded, and explicit about association ownership and target metadata.
//   SCOPE: Successful envelope validation, payload bound rejection, and empty-domain rejection.
//   DEPENDS: src/transport/datagram_contract.rs
//   LINKS: V-M-DATAGRAM-CONTRACT
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   valid_ip_envelope_passes_validation - proves bounded IP-target datagrams are accepted
//   oversized_payload_is_rejected - proves datagram payload size stays bounded
//   empty_domain_target_is_rejected - proves domain targets remain explicit and non-empty
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added explicit datagram-contract tests so UDP-capable work begins from deterministic envelope validation.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use super::{
    DatagramEnvelope, DatagramError, DatagramTarget, MAX_DATAGRAM_PAYLOAD_BYTES,
};

fn relay_client() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 55_000)
}

#[test]
fn valid_ip_envelope_passes_validation() {
    let envelope = DatagramEnvelope {
        association_id: 7,
        relay_client_addr: relay_client(),
        target: DatagramTarget::Ip(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(149, 154, 167, 51)),
            3478,
        )),
        payload: vec![1, 2, 3, 4],
    };

    assert_eq!(envelope.validate(), Ok(()));
}

#[test]
fn oversized_payload_is_rejected() {
    let envelope = DatagramEnvelope {
        association_id: 9,
        relay_client_addr: relay_client(),
        target: DatagramTarget::Ip(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9999)),
        payload: vec![0_u8; MAX_DATAGRAM_PAYLOAD_BYTES + 1],
    };

    assert_eq!(envelope.validate(), Err(DatagramError::PayloadTooLarge));
}

#[test]
fn empty_domain_target_is_rejected() {
    let envelope = DatagramEnvelope {
        association_id: 11,
        relay_client_addr: relay_client(),
        target: DatagramTarget::Domain(String::new(), 443),
        payload: vec![9, 8, 7],
    };

    assert_eq!(envelope.validate(), Err(DatagramError::EmptyDomainTarget));
}

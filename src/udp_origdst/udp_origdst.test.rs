// FILE: src/udp_origdst/udp_origdst.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify the repo-local original-destination helper contract surface leaves stable tuple-level evidence labels.
//   SCOPE: Helper contract tuple-evidence labeling checks.
//   DEPENDS: src/udp_origdst/mod.rs, src/transport/datagram_contract.rs
//   LINKS: V-M-UDP-ORIGDST-CONTRACT
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   tuple_evidence_label_includes_tuple_boundaries - proves tuple evidence labels keep source, listener, target, and payload length visible
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added a deterministic contract-level check for tuple evidence labels in the repo-local helper surface.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::transport::datagram_contract::DatagramTarget;

use super::{tuple_evidence_label, RecoveredUdpTuple};

#[test]
fn tuple_evidence_label_includes_tuple_boundaries() {
    let tuple = RecoveredUdpTuple {
        client_source_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 40000),
        helper_listener_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12000),
        original_target: DatagramTarget::Ip(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(91, 99, 128, 146)),
            55123,
        )),
        payload_len: 27,
    };

    let label = tuple_evidence_label(&tuple);

    assert!(label.contains("127.0.0.1:40000"));
    assert!(label.contains("127.0.0.1:12000"));
    assert!(label.contains("91.99.128.146:55123"));
    assert!(label.ends_with("|27"));
}

// FILE: src/udp_origdst/linux.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Isolate Linux-specific socket and original-destination recovery surfaces for the repo-local UDP helper.
//   SCOPE: Recovery marker definitions, listener-plan metadata, and Linux-specific recovery strategy descriptions for intercepted UDP tuples.
//   DEPENDS: std, src/transport/datagram_contract.rs, src/udp_origdst/mod.rs
//   LINKS: M-UDP-ORIGDST-LINUX-ADAPTER, V-M-UDP-ORIGDST-LINUX-ADAPTER, DF-UDP-ORIGDST-RECOVERY
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   IPV4_ORIGINAL_DST_MARKER - Linux IPv4 original-destination recovery marker
//   IPV4_RECV_ORIGINAL_DST_MARKER - Linux IPv4 recvmsg control-message marker
//   IPV6_RECV_ORIGINAL_DST_MARKER - Linux IPv6 recvmsg control-message marker
//   LinuxOrigDstSocketPlan - one bounded socket-plan description for tuple recovery
//   LinuxOrigDstRecoveryStrategy - one explicit Linux recovery strategy class
//   planLinuxOrigDstSocket - build one bounded Linux recovery plan for a helper listener address
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added the Linux-specific original-destination recovery surface so Phase-41 can keep platform recovery separate from the generic helper runtime.
// END_CHANGE_SUMMARY

use std::net::SocketAddr;

pub const IPV4_ORIGINAL_DST_MARKER: &str = "SO_ORIGINAL_DST";
pub const IPV4_RECV_ORIGINAL_DST_MARKER: &str = "IP_RECVORIGDSTADDR";
pub const IPV6_RECV_ORIGINAL_DST_MARKER: &str = "IPV6_RECVORIGDSTADDR";
pub const RECVMSG_API_MARKER: &str = "recvmsg";
pub const CONTROL_MESSAGE_API_MARKER: &str = "cmsg";

#[cfg(test)]
#[path = "linux.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinuxOrigDstRecoveryStrategy {
    Ipv4SocketOption,
    Ipv4RecvMsgControlMessage,
    Ipv6RecvMsgControlMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxOrigDstSocketPlan {
    pub listener_addr: SocketAddr,
    pub strategy: LinuxOrigDstRecoveryStrategy,
    pub requires_recvmsg: bool,
}

// START_CONTRACT: planLinuxOrigDstSocket
//   PURPOSE: Build one bounded Linux recovery plan for a helper listener address so tuple recovery can stay explicit before runtime implementation begins.
//   INPUTS: { listener_addr: SocketAddr - repo-local helper listener address, prefers_ipv6: bool - whether the recovery plan should target IPv6-first tuple recovery }
//   OUTPUTS: { LinuxOrigDstSocketPlan - explicit Linux recovery strategy for the helper listener }
//   SIDE_EFFECTS: [none]
//   LINKS: [M-UDP-ORIGDST-LINUX-ADAPTER, V-M-UDP-ORIGDST-LINUX-ADAPTER]
// END_CONTRACT: planLinuxOrigDstSocket
pub fn plan_linux_origdst_socket(
    listener_addr: SocketAddr,
    prefers_ipv6: bool,
) -> LinuxOrigDstSocketPlan {
    // START_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
    let strategy = if prefers_ipv6 {
        LinuxOrigDstRecoveryStrategy::Ipv6RecvMsgControlMessage
    } else {
        LinuxOrigDstRecoveryStrategy::Ipv4RecvMsgControlMessage
    };
    LinuxOrigDstSocketPlan {
        listener_addr,
        requires_recvmsg: true,
        strategy,
    }
    // END_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
}

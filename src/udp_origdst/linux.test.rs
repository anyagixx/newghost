// FILE: src/udp_origdst/linux.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify the Linux original-destination adapter keeps explicit recovery-plan markers visible.
//   SCOPE: Linux recovery-plan strategy and marker checks.
//   DEPENDS: src/udp_origdst/linux.rs
//   LINKS: V-M-UDP-ORIGDST-LINUX-ADAPTER
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   plan_linux_origdst_socket_prefers_ipv6_recvmsg_strategy - proves IPv6 planning stays on an explicit recvmsg control-message surface
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added a deterministic Linux adapter plan check for the repo-local helper branch.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use super::{
    plan_linux_origdst_socket, LinuxOrigDstRecoveryStrategy, CONTROL_MESSAGE_API_MARKER,
    IPV4_ORIGINAL_DST_MARKER, IPV4_RECV_ORIGINAL_DST_MARKER, IPV6_RECV_ORIGINAL_DST_MARKER,
    RECVMSG_API_MARKER,
};

#[test]
fn plan_linux_origdst_socket_prefers_ipv6_recvmsg_strategy() {
    let plan = plan_linux_origdst_socket(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12000),
        true,
    );

    assert_eq!(
        plan.strategy,
        LinuxOrigDstRecoveryStrategy::Ipv6RecvMsgControlMessage
    );
    assert!(plan.requires_recvmsg);
    assert_eq!(IPV4_ORIGINAL_DST_MARKER, "SO_ORIGINAL_DST");
    assert_eq!(IPV4_RECV_ORIGINAL_DST_MARKER, "IP_RECVORIGDSTADDR");
    assert_eq!(IPV6_RECV_ORIGINAL_DST_MARKER, "IPV6_RECVORIGDSTADDR");
    assert_eq!(RECVMSG_API_MARKER, "recvmsg");
    assert_eq!(CONTROL_MESSAGE_API_MARKER, "cmsg");
}

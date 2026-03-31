// FILE: src/udp_origdst/linux.test.rs
// VERSION: 0.1.3
// START_MODULE_CONTRACT
//   PURPOSE: Verify the Linux original-destination adapter keeps explicit recovery-plan, non-OUTPUT TPROXY topology markers, transparent-socket, and control-message markers visible and parses recovered IPv4 tuples deterministically.
//   SCOPE: Linux recovery-plan strategy, non-OUTPUT TPROXY planning markers, transparent-socket enablement boundary, marker, and control-message parsing checks.
//   DEPENDS: libc, src/udp_origdst/linux.rs
//   LINKS: V-M-UDP-ORIGDST-LINUX-ADAPTER, V-M-TPROXY-PRIV-LAUNCH-DELTA, V-M-TPROXY-NONOUTPUT-LINUX-DELTA
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   udp_origdst_linux_plan_socket_prefers_ipv6_recvmsg_strategy - proves IPv6 planning stays on an explicit recvmsg control-message surface
//   udp_origdst_linux_plans_nonoutput_tproxy_with_namespace_prerouting_markers - proves the non-OUTPUT branch keeps one exact owner-mark, route, ingress, and PREROUTING anchor set
//   udp_origdst_linux_transparent_socket_enablement_is_explicit - proves Linux transparent-socket enablement is observable as either success or a bounded privilege failure
//   udp_origdst_linux_enables_ipv4_original_dst_option - proves Linux socket setup can enable original-destination ancillary data on a UDP socket
//   udp_origdst_linux_recovers_ipv4_original_destination_from_control_message - proves ancillary control parsing yields the expected original IPv4 destination tuple
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.3 - Added a bounded non-OUTPUT TPROXY planning check so Phase-45 can lock one exact veth/netns PREROUTING topology before smoke.
// END_CHANGE_SUMMARY

use std::mem::size_of;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::os::fd::AsRawFd;

use crate::transport::datagram_contract::DatagramTarget;

use super::{
    enable_ipv4_recv_original_dst, enable_ipv4_transparent_socket, plan_linux_nonoutput_tproxy,
    plan_linux_origdst_socket, recv_recovered_ipv4_datagram,
    LinuxOrigDstRecoveryStrategy, CONTROL_MESSAGE_API_MARKER, IPV4_ORIGINAL_DST_MARKER,
    IPV4_RECV_ORIGINAL_DST_MARKER, IPV4_TRANSPARENT_SOCKET_MARKER,
    IPV6_RECV_ORIGINAL_DST_MARKER, RECVMSG_API_MARKER, TPROXY_OUTPUT_OWNER_MARK_ONLY_MARKER,
    TPROXY_POLICY_ROUTE_MARKER, TPROXY_PREROUTING_CHAIN_MARKER,
    TPROXY_VETH_NETNS_INGRESS_MARKER,
};

#[test]
fn udp_origdst_linux_plan_socket_prefers_ipv6_recvmsg_strategy() {
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

#[test]
fn udp_origdst_linux_plans_nonoutput_tproxy_with_namespace_prerouting_markers() {
    let listener_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 10073);
    let plan = plan_linux_nonoutput_tproxy(listener_addr);

    assert_eq!(plan.listener_addr, listener_addr);
    assert_eq!(plan.host_output_marker, TPROXY_OUTPUT_OWNER_MARK_ONLY_MARKER);
    assert_eq!(plan.route_marker, TPROXY_POLICY_ROUTE_MARKER);
    assert_eq!(plan.ingress_marker, TPROXY_VETH_NETNS_INGRESS_MARKER);
    assert_eq!(plan.interception_chain_marker, TPROXY_PREROUTING_CHAIN_MARKER);
    assert!(plan.requires_transparent_socket);
}

#[test]
fn udp_origdst_linux_transparent_socket_enablement_is_explicit() {
    let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind udp socket");

    match enable_ipv4_transparent_socket(&socket) {
        Ok(()) => {
            let mut enabled: libc::c_int = 0;
            let mut enabled_len = size_of::<libc::c_int>() as libc::socklen_t;
            let rc = unsafe {
                libc::getsockopt(
                    socket.as_raw_fd(),
                    libc::IPPROTO_IP,
                    libc::IP_TRANSPARENT,
                    &mut enabled as *mut _ as *mut libc::c_void,
                    &mut enabled_len as *mut libc::socklen_t,
                )
            };

            assert_eq!(rc, 0);
            assert_eq!(enabled, 1);
        }
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("Operation not permitted")
                    || message.contains("Permission denied")
                    || message.contains(IPV4_TRANSPARENT_SOCKET_MARKER),
                "unexpected transparent-socket failure: {message}"
            );
        }
    }
}

#[test]
fn udp_origdst_linux_enables_ipv4_original_dst_option() {
    let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind udp socket");

    enable_ipv4_recv_original_dst(&socket).expect("enable original-destination ancillary data");

    let mut enabled: libc::c_int = 0;
    let mut enabled_len = size_of::<libc::c_int>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_RECVORIGDSTADDR,
            &mut enabled as *mut _ as *mut libc::c_void,
            &mut enabled_len as *mut libc::socklen_t,
        )
    };

    assert_eq!(rc, 0);
    assert_eq!(enabled, 1);
}

#[test]
fn udp_origdst_linux_recovers_ipv4_original_destination_from_control_message() {
    let receiver = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind receiver");
    enable_ipv4_recv_original_dst(&receiver).expect("enable original destination");
    let sender = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .expect("bind sender");
    let receiver_addr = receiver.local_addr().expect("receiver addr");
    let sender_addr = sender.local_addr().expect("sender addr");
    sender
        .send_to(b"ping", receiver_addr)
        .expect("send ping");

    let recovered = recv_recovered_ipv4_datagram(&receiver, receiver_addr, 32)
        .expect("recover first recvmsg packet");

    assert_eq!(recovered.payload, b"ping");
    assert_eq!(recovered.tuple.client_source_addr, sender_addr);
    assert_eq!(recovered.tuple.helper_listener_addr, receiver_addr);
    assert_eq!(
        recovered.tuple.original_target,
        DatagramTarget::Ip(receiver_addr)
    );
    assert_eq!(recovered.tuple.payload_len, 4);
}

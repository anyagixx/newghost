// FILE: src/udp_origdst/linux.rs
// VERSION: 0.1.4
// START_MODULE_CONTRACT
//   PURPOSE: Isolate Linux-specific socket, transparent-socket, original-destination recovery, and bounded non-OUTPUT TPROXY planning surfaces for the repo-local UDP helper.
//   SCOPE: Recovery marker definitions, listener-plan metadata, non-OUTPUT TPROXY topology markers, namespace local-delivery sysctl markers, Linux socket-option enablement, Linux transparent-socket enablement, Linux recvmsg-based original-destination parsing, and Linux-specific recovery strategy descriptions for intercepted UDP tuples.
//   DEPENDS: libc, std, src/transport/datagram_contract.rs, src/udp_origdst/mod.rs
//   LINKS: M-UDP-ORIGDST-LINUX-ADAPTER, M-TPROXY-PRIV-LAUNCH-DELTA, M-TPROXY-NONOUTPUT-LINUX-DELTA, V-M-UDP-ORIGDST-LINUX-ADAPTER, V-M-TPROXY-PRIV-LAUNCH-DELTA, V-M-TPROXY-NONOUTPUT-LINUX-DELTA, DF-UDP-ORIGDST-RECOVERY
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   IPV4_ORIGINAL_DST_MARKER - Linux IPv4 original-destination recovery marker
//   IPV4_RECV_ORIGINAL_DST_MARKER - Linux IPv4 recvmsg control-message marker
//   IPV6_RECV_ORIGINAL_DST_MARKER - Linux IPv6 recvmsg control-message marker
//   IPV4_TRANSPARENT_SOCKET_MARKER - Linux IPv4 transparent-socket marker
//   TPROXY_OUTPUT_OWNER_MARK_ONLY_MARKER - host-side OUTPUT owner-mark-only steering marker for the non-OUTPUT branch
//   TPROXY_POLICY_ROUTE_MARKER - host-side fwmark and policy-route proof marker for the non-OUTPUT branch
//   TPROXY_VETH_NETNS_INGRESS_MARKER - isolated veth/netns ingress proof marker for the non-OUTPUT branch
//   TPROXY_PREROUTING_CHAIN_MARKER - namespace PREROUTING TPROXY proof marker for the non-OUTPUT branch
//   TPROXY_ROUTE_LOCALNET_MARKER - namespace route_localnet proof marker for loopback-targeted TPROXY delivery
//   TPROXY_RPFILTER_RELAX_MARKER - namespace rp_filter relaxation proof marker for non-OUTPUT TPROXY delivery
//   LinuxRecoveredDatagram - one recvmsg packet plus recovered original destination metadata
//   LinuxOrigDstSocketPlan - one bounded socket-plan description for tuple recovery
//   LinuxNonOutputTproxyPlan - one bounded non-OUTPUT TPROXY topology plan for the live helper branch
//   LinuxOrigDstRecoveryStrategy - one explicit Linux recovery strategy class
//   planLinuxOrigDstSocket - build one bounded Linux recovery plan for a helper listener address
//   planLinuxNonOutputTproxy - build one bounded non-OUTPUT TPROXY plan for the live helper listener
//   enableIpv4TransparentSocket - enable Linux IPv4 transparent-socket mode on one UDP socket
//   enableIpv4RecvOriginalDst - enable Linux IPv4 original-destination ancillary data on one UDP socket
//   recvRecoveredIpv4Datagram - receive one UDP packet with Linux ancillary data and recover the original IPv4 destination
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.4 - Added explicit route_localnet and rp_filter markers after Phase-45 live packet proved PREROUTING hits but no helper delivery until namespace local-delivery policy was relaxed.
// END_CHANGE_SUMMARY

use std::io;
use std::mem::{size_of, zeroed};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::os::fd::AsRawFd;

use crate::transport::datagram_contract::DatagramTarget;
use crate::udp_origdst::{RecoveredUdpTuple, UdpOrigDstError};

pub const IPV4_ORIGINAL_DST_MARKER: &str = "SO_ORIGINAL_DST";
pub const IPV4_RECV_ORIGINAL_DST_MARKER: &str = "IP_RECVORIGDSTADDR";
pub const IPV6_RECV_ORIGINAL_DST_MARKER: &str = "IPV6_RECVORIGDSTADDR";
pub const IPV4_TRANSPARENT_SOCKET_MARKER: &str = "IP_TRANSPARENT";
pub const RECVMSG_API_MARKER: &str = "recvmsg";
pub const CONTROL_MESSAGE_API_MARKER: &str = "cmsg";
pub const DEFAULT_ORIGDST_CONTROL_LEN: usize = 128;
pub const TPROXY_OUTPUT_OWNER_MARK_ONLY_MARKER: &str = "output-owner-mark-only";
pub const TPROXY_POLICY_ROUTE_MARKER: &str = "policy-routing-fwmark";
pub const TPROXY_VETH_NETNS_INGRESS_MARKER: &str = "veth-netns-ingress";
pub const TPROXY_PREROUTING_CHAIN_MARKER: &str = "prerouting-tproxy";
pub const TPROXY_ROUTE_LOCALNET_MARKER: &str = "route-localnet";
pub const TPROXY_RPFILTER_RELAX_MARKER: &str = "rp-filter-relaxed";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxNonOutputTproxyPlan {
    pub listener_addr: SocketAddr,
    pub host_output_marker: &'static str,
    pub route_marker: &'static str,
    pub ingress_marker: &'static str,
    pub interception_chain_marker: &'static str,
    pub route_localnet_marker: &'static str,
    pub rp_filter_marker: &'static str,
    pub requires_transparent_socket: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRecoveredDatagram {
    pub tuple: RecoveredUdpTuple,
    pub payload: Vec<u8>,
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

// START_CONTRACT: planLinuxNonOutputTproxy
//   PURPOSE: Build one bounded non-OUTPUT TPROXY topology plan so the live helper branch can freeze its route and interception anchors before smoke.
//   INPUTS: { listener_addr: SocketAddr - repo-local helper listener address }
//   OUTPUTS: { LinuxNonOutputTproxyPlan - explicit non-OUTPUT TPROXY topology markers for the bounded live branch }
//   SIDE_EFFECTS: [none]
//   LINKS: [M-TPROXY-NONOUTPUT-LINUX-DELTA, V-M-TPROXY-NONOUTPUT-LINUX-DELTA]
// END_CONTRACT: planLinuxNonOutputTproxy
pub fn plan_linux_nonoutput_tproxy(listener_addr: SocketAddr) -> LinuxNonOutputTproxyPlan {
    // START_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
    LinuxNonOutputTproxyPlan {
        listener_addr,
        host_output_marker: TPROXY_OUTPUT_OWNER_MARK_ONLY_MARKER,
        route_marker: TPROXY_POLICY_ROUTE_MARKER,
        ingress_marker: TPROXY_VETH_NETNS_INGRESS_MARKER,
        interception_chain_marker: TPROXY_PREROUTING_CHAIN_MARKER,
        route_localnet_marker: TPROXY_ROUTE_LOCALNET_MARKER,
        rp_filter_marker: TPROXY_RPFILTER_RELAX_MARKER,
        requires_transparent_socket: true,
    }
    // END_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
}

// START_CONTRACT: enableIpv4RecvOriginalDst
//   PURPOSE: Enable Linux IPv4 original-destination ancillary data on one UDP socket before recvmsg-based tuple recovery begins.
//   INPUTS: { socket: &UdpSocket - bound UDP socket owned by the repo-local helper }
//   OUTPUTS: { Result<(), UdpOrigDstError> - ok when the socket is configured to expose original-destination ancillary data }
//   SIDE_EFFECTS: [mutates Linux socket options on the provided file descriptor]
//   LINKS: [M-UDP-ORIGDST-LINUX-ADAPTER, V-M-UDP-ORIGDST-LINUX-ADAPTER]
// END_CONTRACT: enableIpv4RecvOriginalDst
pub fn enable_ipv4_recv_original_dst(socket: &UdpSocket) -> Result<(), UdpOrigDstError> {
    // START_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
    let enable: libc::c_int = 1;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_RECVORIGDSTADDR,
            &enable as *const _ as *const libc::c_void,
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result != 0 {
        return Err(UdpOrigDstError::RecoveryFailed(io::Error::last_os_error().to_string()));
    }
    Ok(())
    // END_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
}

// START_CONTRACT: enableIpv4TransparentSocket
//   PURPOSE: Enable Linux IPv4 transparent-socket mode on one UDP socket before TPROXY-backed live interception begins.
//   INPUTS: { socket: &UdpSocket - bound UDP socket owned by the repo-local helper }
//   OUTPUTS: { Result<(), UdpOrigDstError> - ok when the socket is configured for Linux transparent-socket semantics }
//   SIDE_EFFECTS: [mutates Linux socket options on the provided file descriptor]
//   LINKS: [M-TPROXY-PRIV-LAUNCH-DELTA, V-M-TPROXY-PRIV-LAUNCH-DELTA]
// END_CONTRACT: enableIpv4TransparentSocket
pub fn enable_ipv4_transparent_socket(socket: &UdpSocket) -> Result<(), UdpOrigDstError> {
    // START_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
    let enable: libc::c_int = 1;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_TRANSPARENT,
            &enable as *const _ as *const libc::c_void,
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result != 0 {
        return Err(UdpOrigDstError::RecoveryFailed(io::Error::last_os_error().to_string()));
    }
    Ok(())
    // END_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
}

// START_CONTRACT: recvRecoveredIpv4Datagram
//   PURPOSE: Receive one UDP packet with recvmsg, recover its original IPv4 destination from Linux ancillary data, and return explicit tuple metadata plus payload.
//   INPUTS: { socket: &UdpSocket - repo-local helper listener with IP_RECVORIGDSTADDR enabled, helper_listener_addr: SocketAddr - listener address used by the helper, payload_capacity: usize - maximum payload bytes to receive in one packet }
//   OUTPUTS: { Result<LinuxRecoveredDatagram, UdpOrigDstError> - recovered tuple metadata plus payload bytes }
//   SIDE_EFFECTS: [reads one packet from the provided UDP socket using recvmsg and Linux ancillary control messages]
//   LINKS: [M-UDP-ORIGDST-LINUX-ADAPTER, V-M-UDP-ORIGDST-LINUX-ADAPTER]
// END_CONTRACT: recvRecoveredIpv4Datagram
pub fn recv_recovered_ipv4_datagram(
    socket: &UdpSocket,
    helper_listener_addr: SocketAddr,
    payload_capacity: usize,
) -> Result<LinuxRecoveredDatagram, UdpOrigDstError> {
    // START_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
    let mut payload = vec![0_u8; payload_capacity];
    let mut control = vec![0_u8; DEFAULT_ORIGDST_CONTROL_LEN];
    let mut source_storage: libc::sockaddr_storage = unsafe { zeroed() };
    let mut iov = libc::iovec {
        iov_base: payload.as_mut_ptr() as *mut libc::c_void,
        iov_len: payload.len(),
    };
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_name = &mut source_storage as *mut _ as *mut libc::c_void;
    message.msg_namelen = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr() as *mut libc::c_void;
    message.msg_controllen = control.len();

    let received = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, 0) };
    if received < 0 {
        let error = io::Error::last_os_error();
        return match error.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                Err(UdpOrigDstError::ReceiveWouldBlock)
            }
            _ => Err(UdpOrigDstError::RecoveryFailed(error.to_string())),
        };
    }
    if message.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(UdpOrigDstError::RecoveryFailed(
            "linux original-destination control buffer truncated".to_string(),
        ));
    }

    let payload_len = received as usize;
    payload.truncate(payload_len);
    let client_source_addr = socket_addr_from_storage(&source_storage, message.msg_namelen)?;
    let original_target = DatagramTarget::Ip(parse_original_dst_from_msghdr(&message)?);

    Ok(LinuxRecoveredDatagram {
        tuple: RecoveredUdpTuple {
            client_source_addr,
            helper_listener_addr,
            original_target,
            payload_len,
        },
        payload,
    })
    // END_BLOCK_UDP_ORIGDST_LINUX_ADAPTER
}

fn parse_original_dst_from_msghdr(
    message: &libc::msghdr,
) -> Result<SocketAddr, UdpOrigDstError> {
    let mut current = message.msg_control as *const u8;
    let control_end = unsafe { current.add(message.msg_controllen) };
    while !current.is_null()
        && unsafe { current.add(size_of::<libc::cmsghdr>()) } <= control_end
    {
        let header = unsafe { &*(current as *const libc::cmsghdr) };
        if header.cmsg_level == libc::IPPROTO_IP && header.cmsg_type == libc::IP_RECVORIGDSTADDR {
            let data_len = (header.cmsg_len as usize).saturating_sub(size_of::<libc::cmsghdr>());
            if data_len < size_of::<libc::sockaddr_in>() {
                return Err(UdpOrigDstError::RecoveryFailed(
                    "linux original-destination control message too short".to_string(),
                ));
            }
            let data_ptr =
                unsafe { current.add(cmsg_align(size_of::<libc::cmsghdr>())) } as *const libc::sockaddr_in;
            let sockaddr = unsafe { *data_ptr };
            return Ok(socket_addr_from_sockaddr_in(sockaddr));
        }

        let next = unsafe { current.add(cmsg_align(header.cmsg_len as usize)) };
        if next <= current || next > control_end {
            break;
        }
        current = next;
    }

    Err(UdpOrigDstError::RecoveryFailed(
        "linux original-destination control message missing".to_string(),
    ))
}

fn socket_addr_from_storage(
    storage: &libc::sockaddr_storage,
    name_len: libc::socklen_t,
) -> Result<SocketAddr, UdpOrigDstError> {
    if name_len as usize >= size_of::<libc::sockaddr_in>() && storage.ss_family as libc::c_int == libc::AF_INET {
        let sockaddr = unsafe { *(storage as *const _ as *const libc::sockaddr_in) };
        return Ok(socket_addr_from_sockaddr_in(sockaddr));
    }

    Err(UdpOrigDstError::RecoveryFailed(
        "unsupported Linux UDP source address family".to_string(),
    ))
}

fn socket_addr_from_sockaddr_in(sockaddr: libc::sockaddr_in) -> SocketAddr {
    SocketAddr::new(
        IpAddr::V4(Ipv4Addr::from(u32::from_be(sockaddr.sin_addr.s_addr))),
        u16::from_be(sockaddr.sin_port),
    )
}

fn cmsg_align(length: usize) -> usize {
    let align = size_of::<usize>();
    (length + align - 1) & !(align - 1)
}

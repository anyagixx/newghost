#!/usr/bin/env bash
# FILE: scripts/udp_probe.sh
# VERSION: 0.1.0
# START_MODULE_CONTRACT
#   PURPOSE: Run one bounded SOCKS5 UDP ASSOCIATE probe against n0wss and report association, outbound, and inbound evidence as stable fields.
#   SCOPE: Argument parsing, SOCKS5 no-auth negotiation, UDP ASSOCIATE negotiation, bounded datagram send, optional reply wait, and stable evidence output.
#   DEPENDS: bash, python3
#   LINKS: M-CALLS-UDP-PROBE-TOOLING, V-M-CALLS-UDP-PROBE-TOOLING, DF-CALLS-CONTROLLED-UDP-PROBE
# END_MODULE_CONTRACT
#
# START_MODULE_MAP
#   main - parse bounded probe arguments and delegate to the Python probe runtime
# END_MODULE_MAP
#
# START_CHANGE_SUMMARY
#   LAST_CHANGE: v0.1.0 - Added a bounded operator UDP probe tool so Telegram calls diagnosis can exercise SOCKS5 UDP ASSOCIATE independently of Telegram Desktop UI.
# END_CHANGE_SUMMARY

set -euo pipefail

# START_BLOCK_MAIN
exec python3 - "$@" <<'PY'
import socket
import struct
import sys
from typing import Tuple


def fail(message: str, stage: str, code: int = 1) -> None:
    print(f"probe_status=failed")
    print(f"failure_stage={stage}")
    print(f"failure_reason={message}")
    raise SystemExit(code)


def parse_args(argv):
    socks5 = "127.0.0.1:1080"
    target = None
    payload = "n0wss-udp-probe"
    timeout = 3.0

    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--socks5":
            i += 1
            socks5 = argv[i]
        elif arg == "--target":
            i += 1
            target = argv[i]
        elif arg == "--payload":
            i += 1
            payload = argv[i]
        elif arg == "--timeout":
            i += 1
            timeout = float(argv[i])
        else:
            fail(f"unknown argument {arg}", "arg-parse", 2)
        i += 1

    if target is None:
        fail("missing required --target host:port", "arg-parse", 2)

    return socks5, target, payload.encode("utf-8"), timeout


def split_host_port(value: str) -> Tuple[str, int]:
    if value.count(":") == 0:
        fail(f"expected host:port, got {value}", "arg-parse", 2)
    host, port = value.rsplit(":", 1)
    try:
        return host, int(port)
    except ValueError as exc:
        fail(f"invalid port in {value}: {exc}", "arg-parse", 2)


def recv_exact(sock: socket.socket, count: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < count:
        chunk = sock.recv(count - len(chunks))
        if not chunk:
            fail("unexpected EOF from SOCKS5 control channel", "tcp-recv")
        chunks.extend(chunk)
    return bytes(chunks)


def encode_target(host: str, port: int) -> bytes:
    try:
        packed = socket.inet_aton(host)
        return b"\x01" + packed + struct.pack("!H", port)
    except OSError:
        encoded = host.encode("idna")
        if len(encoded) > 255:
            fail("target host too long for SOCKS5 domain encoding", "target-encode")
        return b"\x03" + bytes([len(encoded)]) + encoded + struct.pack("!H", port)


def decode_socks_addr(data: bytes, offset: int = 0):
    atyp = data[offset]
    cursor = offset + 1
    if atyp == 0x01:
        host = socket.inet_ntoa(data[cursor:cursor + 4])
        cursor += 4
    elif atyp == 0x03:
        size = data[cursor]
        cursor += 1
        host = data[cursor:cursor + size].decode("idna")
        cursor += size
    elif atyp == 0x04:
        host = socket.inet_ntop(socket.AF_INET6, data[cursor:cursor + 16])
        cursor += 16
    else:
        fail(f"unsupported SOCKS5 ATYP 0x{atyp:02x}", "reply-parse")
    port = struct.unpack("!H", data[cursor:cursor + 2])[0]
    cursor += 2
    return host, port, cursor


def main(argv):
    socks5_raw, target_raw, payload, timeout = parse_args(argv)
    socks5_host, socks5_port = split_host_port(socks5_raw)
    target_host, target_port = split_host_port(target_raw)

    tcp = socket.create_connection((socks5_host, socks5_port), timeout=timeout)
    tcp.settimeout(timeout)

    tcp.sendall(b"\x05\x01\x00")
    auth_reply = recv_exact(tcp, 2)
    if auth_reply != b"\x05\x00":
        fail(f"unexpected auth reply {auth_reply.hex()}", "auth")

    udp_request = b"\x05\x03\x00" + encode_target("0.0.0.0", 0)
    tcp.sendall(udp_request)

    reply_prefix = recv_exact(tcp, 4)
    if reply_prefix[0] != 0x05:
        fail(f"unexpected UDP ASSOCIATE version {reply_prefix[0]:#x}", "udp-associate")
    if reply_prefix[1] != 0x00:
        fail(f"udp associate reply code {reply_prefix[1]:#x}", "udp-associate")
    if reply_prefix[2] != 0x00:
        fail(f"unexpected reserved reply byte {reply_prefix[2]:#x}", "udp-associate")

    atyp = reply_prefix[3]
    if atyp == 0x01:
        relay_tail = recv_exact(tcp, 6)
    elif atyp == 0x03:
        domain_len = recv_exact(tcp, 1)[0]
        relay_tail = bytes([domain_len]) + recv_exact(tcp, domain_len + 2)
    elif atyp == 0x04:
        relay_tail = recv_exact(tcp, 18)
    else:
        fail(f"unsupported relay ATYP 0x{atyp:02x}", "udp-associate")

    relay_reply = reply_prefix + relay_tail
    relay_host, relay_port, _ = decode_socks_addr(relay_reply, 3)

    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.settimeout(timeout)
    udp.bind(("127.0.0.1", 0))
    local_udp_host, local_udp_port = udp.getsockname()

    udp_packet = b"\x00\x00\x00" + encode_target(target_host, target_port) + payload
    udp.sendto(udp_packet, (relay_host, relay_port))

    print("probe_status=association-ok")
    print(f"socks5_addr={socks5_host}:{socks5_port}")
    print(f"auth_reply={auth_reply.hex()}")
    print(f"udp_associate_reply={relay_reply.hex()}")
    print(f"relay_addr={relay_host}:{relay_port}")
    print(f"local_udp_addr={local_udp_host}:{local_udp_port}")
    print(f"target_addr={target_host}:{target_port}")
    print(f"payload_len={len(payload)}")
    print("outbound_result=sent")

    try:
        response, responder = udp.recvfrom(65535)
    except socket.timeout:
        print("inbound_result=timeout")
        return

    if len(response) < 4:
        fail("reply shorter than SOCKS5 UDP header", "udp-recv")
    if response[2] != 0x00:
        fail(f"fragmented reply not supported: {response[2]:#x}", "udp-recv")

    reply_host, reply_port, cursor = decode_socks_addr(response, 3)
    payload_bytes = response[cursor:]
    print("probe_status=reply-received")
    print(f"relay_reply_from={responder[0]}:{responder[1]}")
    print(f"inbound_target={reply_host}:{reply_port}")
    print(f"inbound_payload_len={len(payload_bytes)}")
    try:
        print(f"inbound_payload_utf8={payload_bytes.decode('utf-8')}")
    except UnicodeDecodeError:
        print(f"inbound_payload_hex={payload_bytes.hex()}")


if __name__ == "__main__":
    main(sys.argv[1:])
PY
# END_BLOCK_MAIN

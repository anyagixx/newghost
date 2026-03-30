// FILE: src/lib.rs
// VERSION: 0.1.3
// START_MODULE_CONTRACT
//   PURPOSE: Expose the currently implemented GRACE modules for the n0wss crate.
//   SCOPE: Library module registration and re-exports.
//   DEPENDS: src/auth/mod.rs, src/cli/mod.rs, src/config/mod.rs, src/iroh_adapter/mod.rs, src/obs/mod.rs, src/proxy_bridge/mod.rs, src/session/mod.rs, src/socks5/mod.rs, src/tls/mod.rs, src/transport/mod.rs, src/udp_origdst/mod.rs, src/wss_gateway/mod.rs
//   LINKS: M-CRATE-ROOT, V-M-CRATE-ROOT, M-AUTH, M-CLI, M-CONFIG, M-IROH-ADAPTER, M-OBS, M-PROXY-BRIDGE, M-SESSION, M-SOCKS5, M-TLS, M-UDP-ORIGDST-CONTRACT, M-WSS-GATEWAY
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   cli - startup and graceful shutdown orchestration module
//   config - configuration loading and validation module
//   obs - observability bootstrap and burst detection module
//   session - session core state machine and typed effect contracts
//   socks5 - local SOCKS5 ingress, proxy intent parsing, and reply mapping
//   proxy_bridge - queue-driven bridge over generic resolved streams
//   auth - handshake authentication and redaction module
//   transport - shared transport contracts used by transport adapters
//   tls - TLS material loading and runtime context module
//   udp_origdst - repo-local original-destination helper contract and Linux adapter surface
//   wss_gateway - WSS transport adapter and server boundary
//   iroh_adapter - iroh transport adapter and release boundary
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.3 - Registered the repo-local udp_origdst module so Phase-41 can introduce an explicit original-destination helper surface without hiding it outside the crate root.
// END_CHANGE_SUMMARY

pub mod auth;
pub mod cli;
pub mod config;
pub mod iroh_adapter;
pub mod obs;
pub mod proxy_bridge;
pub mod session;
pub mod socks5;
pub mod tls;
pub mod transport;
pub mod udp_origdst;
pub mod wss_gateway;

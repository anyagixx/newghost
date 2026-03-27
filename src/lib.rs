// FILE: src/lib.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Expose the currently implemented GRACE modules for the n0wss crate.
//   SCOPE: Library module registration and re-exports.
//   DEPENDS: src/config/mod.rs
//   LINKS: M-CONFIG
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   config - configuration loading and validation module
//   obs - observability bootstrap and burst detection module
//   auth - handshake authentication and redaction module
// END_MODULE_MAP

pub mod auth;
pub mod config;
pub mod obs;

// FILE: src/transport/mod.rs
// VERSION: 0.1.4
// START_MODULE_CONTRACT
//   PURPOSE: Aggregate the governed transport helper modules behind one stable Rust module surface without adding transport logic.
//   SCOPE: Top-level transport module exports for adapter contracts, datagram contracts, transport streams, and adapter task tracking helpers.
//   DEPENDS: src/transport/adapter_contract.rs, src/transport/datagram_contract.rs, src/transport/stream.rs, src/transport/task_tracker.rs
//   LINKS: M-TRANSPORT-MOD, V-M-TRANSPORT-MOD, M-TRANSPORT-ADAPTER-CONTRACT, M-DATAGRAM-CONTRACT, M-TRANSPORT-STREAM, M-TRANSPORT-TASK-TRACKER
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   adapter_contract - shared adapter contract and TransportRequest surface
//   datagram_contract - shared UDP datagram envelope and association surface
//   stream - transport-agnostic stream and resolved-stream surface
//   task_tracker - adapter-scoped task tracking helpers
// END_MODULE_MAP
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.4 - Added the governed datagram contract export so UDP-capable transport helpers share one stable module surface.
// END_CHANGE_SUMMARY

pub mod adapter_contract;
pub mod datagram_contract;
pub mod stream;
pub mod task_tracker;

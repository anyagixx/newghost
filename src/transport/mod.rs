// FILE: src/transport/mod.rs
// VERSION: 0.1.2
// START_MODULE_CONTRACT
//   PURPOSE: Aggregate the governed transport helper modules behind one stable Rust module surface without adding transport logic.
//   SCOPE: Top-level transport module exports for adapter contracts, transport streams, and adapter task tracking helpers.
//   DEPENDS: src/transport/adapter_contract.rs, src/transport/stream.rs, src/transport/task_tracker.rs
//   LINKS: M-TRANSPORT-MOD, V-M-TRANSPORT-MOD, M-TRANSPORT-ADAPTER-CONTRACT, M-TRANSPORT-STREAM, M-TRANSPORT-TASK-TRACKER
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   adapter_contract - shared adapter contract and TransportRequest surface
//   stream - transport-agnostic stream and resolved-stream surface
//   task_tracker - adapter-scoped task tracking helpers
// END_MODULE_MAP
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.3 - Promoted the transport module wrapper into GRACE governance so the exported transport surface is tracked explicitly in plan, graph, and verification artifacts.
// END_CHANGE_SUMMARY

pub mod adapter_contract;
pub mod stream;
pub mod task_tracker;

// FILE: src/transport/mod.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Expose thin shared transport contracts used by WSS and iroh adapters without becoming a separate GRACE module.
//   SCOPE: Module registration for transport stream, adapter, and task-tracker contracts.
//   DEPENDS: src/transport/stream.rs, src/transport/adapter_contract.rs, src/transport/task_tracker.rs
//   LINKS: M-WSS-GATEWAY, M-IROH-ADAPTER, M-PROXY-BRIDGE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   stream - transport-agnostic stream contract and resolved stream metadata
//   adapter_contract - shared adapter trait for transport implementations
//   task_tracker - adapter-scoped task tracking wrapper
// END_MODULE_MAP

pub mod adapter_contract;
pub mod stream;
pub mod task_tracker;

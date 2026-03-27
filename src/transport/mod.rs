// FILE: src/transport/mod.rs
// VERSION: 0.1.1
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.2 - Clarified why this wrapper stays outside first-class GRACE governance while still aggregating the governed transport contracts.
// END_CHANGE_SUMMARY
//
// This wrapper intentionally stays outside the GRACE module graph.
// The governed transport contracts live in:
// - src/transport/stream.rs
// - src/transport/adapter_contract.rs
// - src/transport/task_tracker.rs
// This file exists only as a Rust module aggregator and should not gain transport logic.

pub mod adapter_contract;
pub mod stream;
pub mod task_tracker;

// FILE: src/transport/mod.rs
// VERSION: 0.1.1
// This wrapper intentionally stays outside the GRACE module graph.
// The governed transport contracts live in:
// - src/transport/stream.rs
// - src/transport/adapter_contract.rs
// - src/transport/task_tracker.rs

pub mod adapter_contract;
pub mod stream;
pub mod task_tracker;

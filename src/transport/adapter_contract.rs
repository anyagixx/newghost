// FILE: src/transport/adapter_contract.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Define the shared adapter contract for transport implementations.
//   SCOPE: Adapter open_stream behavior, typed transport request, and adapter-scoped task tracker access.
//   DEPENDS: async-trait, tokio-util, src/transport/stream.rs, src/transport/task_tracker.rs
//   LINKS: M-WSS-GATEWAY, M-IROH-ADAPTER, M-SESSION
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   TransportRequest - minimal request used by transport adapters
//   TransportAdapter - shared adapter trait
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added the missing change summary block so the shared adapter contract satisfies GRACE governed-file markup requirements.
// END_CHANGE_SUMMARY

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::transport::stream::ResolvedStream;
use crate::transport::task_tracker::AdapterTaskTracker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportRequest {
    pub peer_label: String,
}

#[async_trait]
pub trait TransportAdapter: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn open_stream(
        &self,
        request: &TransportRequest,
        cancel: CancellationToken,
    ) -> Result<ResolvedStream, Self::Error>;

    fn task_tracker(&self) -> &AdapterTaskTracker;
}

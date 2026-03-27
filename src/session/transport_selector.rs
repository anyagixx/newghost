// FILE: src/session/transport_selector.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Choose iroh first and fall back to WSS sequentially under bounded timeouts and explicit cancellation.
//   SCOPE: Transport selector configuration, sequential adapter attempts, safety timeout, and combined failure diagnostics.
//   DEPENDS: std, thiserror, tokio, tokio-util, src/transport/adapter_contract.rs, src/transport/stream.rs
//   LINKS: M-SESSION, V-M-SESSION, DF-TRANSPORT-FALLBACK, VF-003
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   TransportSelectorConfig - per-attempt and safety timeout configuration
//   TransportSelectError - selector cancellation, safety timeout, and combined failure diagnostics
//   TransportSelector - sequential iroh then WSS selection strategy
//   open_stream - resolve one transport stream with explicit fallback behavior
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added sequential iroh then WSS transport resolution with bounded timeouts and combined diagnostics.
// END_CHANGE_SUMMARY

use std::time::Duration;

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::transport::adapter_contract::{TransportAdapter, TransportRequest};
use crate::transport::stream::ResolvedStream;

#[cfg(test)]
#[path = "transport_selector.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportSelectorConfig {
    pub iroh_timeout: Duration,
    pub wss_timeout: Duration,
    pub safety_timeout: Duration,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransportSelectError {
    #[error("transport selection cancelled")]
    Cancelled,
    #[error("transport selector safety timeout exceeded")]
    ContractViolation,
    #[error("all transport attempts failed")]
    AllFailed {
        iroh_err: Option<String>,
        wss_err: Option<String>,
    },
}

pub struct TransportSelector<I, W> {
    iroh: I,
    wss: W,
    config: TransportSelectorConfig,
}

impl<I, W> TransportSelector<I, W> {
    pub fn new(iroh: I, wss: W, config: TransportSelectorConfig) -> Self {
        Self { iroh, wss, config }
    }
}

impl<I, W> TransportSelector<I, W>
where
    I: TransportAdapter,
    W: TransportAdapter,
{
    // START_CONTRACT: open_stream
    //   PURPOSE: Resolve one transport stream by trying iroh first and WSS second without parallel attempts.
    //   INPUTS: { request: &TransportRequest - stable peer label for diagnostics, cancel: CancellationToken - caller cancellation boundary }
    //   OUTPUTS: { Result<ResolvedStream, TransportSelectError> - resolved stream or combined failure diagnostics }
    //   SIDE_EFFECTS: [opens transport adapter attempts under bounded timeouts]
    //   LINKS: [M-SESSION, M-IROH-ADAPTER, M-WSS-GATEWAY, V-M-SESSION]
    // END_CONTRACT: open_stream
    pub async fn open_stream(
        &self,
        request: &TransportRequest,
        cancel: CancellationToken,
    ) -> Result<ResolvedStream, TransportSelectError> {
        tokio::time::timeout(
            self.config.safety_timeout,
            self.open_stream_inner(request, cancel),
        )
        .await
        .map_err(|_| TransportSelectError::ContractViolation)?
    }

    // START_CONTRACT: open_stream_inner
    //   PURPOSE: Apply the sequential transport policy without the outer safety timeout wrapper.
    //   INPUTS: { request: &TransportRequest - stable peer label for diagnostics, cancel: CancellationToken - caller cancellation boundary }
    //   OUTPUTS: { Result<ResolvedStream, TransportSelectError> - resolved stream or combined failure diagnostics }
    //   SIDE_EFFECTS: [starts one iroh attempt and, if needed, one WSS attempt]
    //   LINKS: [M-SESSION, V-M-SESSION]
    // END_CONTRACT: open_stream_inner
    async fn open_stream_inner(
        &self,
        request: &TransportRequest,
        cancel: CancellationToken,
    ) -> Result<ResolvedStream, TransportSelectError> {
        // START_BLOCK_SELECT_TRANSPORT
        if cancel.is_cancelled() {
            return Err(TransportSelectError::Cancelled);
        }

        let iroh_cancel = cancel.child_token();
        match self
            .attempt_with_timeout(
                &self.iroh,
                request,
                iroh_cancel,
                self.config.iroh_timeout,
                &cancel,
            )
            .await
        {
            AttemptOutcome::Resolved(stream) => Ok(stream),
            AttemptOutcome::Cancelled => Err(TransportSelectError::Cancelled),
            AttemptOutcome::Failed(iroh_err) => {
                let wss_cancel = cancel.child_token();
                match self
                    .attempt_with_timeout(
                        &self.wss,
                        request,
                        wss_cancel,
                        self.config.wss_timeout,
                        &cancel,
                    )
                    .await
                {
                    AttemptOutcome::Resolved(stream) => Ok(stream),
                    AttemptOutcome::Cancelled => Err(TransportSelectError::Cancelled),
                    AttemptOutcome::Failed(wss_err) => Err(TransportSelectError::AllFailed {
                        iroh_err: Some(iroh_err),
                        wss_err: Some(wss_err),
                    }),
                }
            }
        }
        // END_BLOCK_SELECT_TRANSPORT
    }

    async fn attempt_with_timeout<A>(
        &self,
        adapter: &A,
        request: &TransportRequest,
        attempt_cancel: CancellationToken,
        timeout: Duration,
        root_cancel: &CancellationToken,
    ) -> AttemptOutcome
    where
        A: TransportAdapter,
    {
        tokio::select! {
            _ = root_cancel.cancelled() => {
                attempt_cancel.cancel();
                AttemptOutcome::Cancelled
            }
            result = tokio::time::timeout(timeout, adapter.open_stream(request, attempt_cancel.clone())) => {
                match result {
                    Ok(Ok(stream)) => AttemptOutcome::Resolved(stream),
                    Ok(Err(err)) => {
                        if root_cancel.is_cancelled() {
                            AttemptOutcome::Cancelled
                        } else {
                            AttemptOutcome::Failed(err.to_string())
                        }
                    }
                    Err(_) => {
                        attempt_cancel.cancel();
                        AttemptOutcome::Failed(format!("timeout after {}ms", timeout.as_millis()))
                    }
                }
            }
        }
    }
}

enum AttemptOutcome {
    Resolved(ResolvedStream),
    Cancelled,
    Failed(String),
}

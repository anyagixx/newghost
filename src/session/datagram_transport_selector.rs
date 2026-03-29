// FILE: src/session/datagram_transport_selector.rs
// VERSION: 0.1.2
// START_MODULE_CONTRACT
//   PURPOSE: Select the governed datagram transport path, initially with WSS-backed datagrams and an explicit extension point for later parity work.
//   SCOPE: Datagram selector configuration, WSS-backed path invocation, explicit cancellation, bounded timeout, and phase-scoped failure diagnostics.
//   DEPENDS: async-trait, std, thiserror, tokio, tokio-util, tracing, src/transport/datagram_contract.rs
//   LINKS: M-DATAGRAM-TRANSPORT-SELECTOR, V-M-DATAGRAM-TRANSPORT-SELECTOR, DF-UDP-OUTBOUND
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   DatagramTransportKind - bounded datagram carrier kinds currently approved by the phase
//   DatagramTransportResolution - resolved datagram carrier information for one association
//   DatagramTransportSelectorConfig - timeout boundary for the approved datagram carrier
//   DatagramTransportSelectError - explicit cancellation, timeout, and WSS-only failure diagnostics
//   WssDatagramPath - trait for the approved WSS-backed datagram carrier
//   DatagramTransportSelector - bounded selector that currently delegates only to the WSS-backed datagram path
//   select_transport - resolve the approved datagram carrier for one association
//   emit_outbound_datagram - emit one outbound datagram through the approved bounded carrier
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.2 - Added a bounded outbound emission helper so repair waves can prove carrier-side datagram handoff separately from local dispatch.
// END_CHANGE_SUMMARY

use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::transport::datagram_contract::{DatagramAssociationId, DatagramEnvelope};

#[cfg(test)]
#[path = "datagram_transport_selector.test.rs"]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatagramTransportKind {
    WssDatagram,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatagramTransportResolution {
    pub association_id: DatagramAssociationId,
    pub transport_kind: DatagramTransportKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatagramTransportSelectorConfig {
    pub wss_timeout: Duration,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DatagramTransportSelectError {
    #[error("datagram transport selection cancelled")]
    Cancelled,
    #[error("wss-backed datagram path timed out after {0}ms")]
    WssTimeout(u128),
    #[error("wss-backed datagram path failed: {0}")]
    WssFailed(String),
}

#[async_trait]
pub trait WssDatagramPath: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn open_path(
        &self,
        association_id: DatagramAssociationId,
        cancel: CancellationToken,
    ) -> Result<(), Self::Error>;

    async fn emit_datagram(
        &self,
        envelope: &DatagramEnvelope,
        cancel: CancellationToken,
    ) -> Result<(), Self::Error>;
}

pub struct DatagramTransportSelector<W> {
    wss: W,
    config: DatagramTransportSelectorConfig,
}

impl<W> DatagramTransportSelector<W> {
    pub fn new(wss: W, config: DatagramTransportSelectorConfig) -> Self {
        Self { wss, config }
    }
}

impl<W> DatagramTransportSelector<W>
where
    W: WssDatagramPath,
{
    // START_CONTRACT: select_transport
    //   PURPOSE: Resolve the approved datagram carrier for one UDP association while making the initial WSS-only scope explicit.
    //   INPUTS: { association_id: DatagramAssociationId - active governed UDP association, cancel: CancellationToken - caller cancellation boundary }
    //   OUTPUTS: { Result<DatagramTransportResolution, DatagramTransportSelectError> - resolved datagram carrier or bounded selector failure }
    //   SIDE_EFFECTS: [invokes the approved WSS-backed carrier under a bounded timeout and emits a stable selector log anchor]
    //   LINKS: [M-DATAGRAM-TRANSPORT-SELECTOR, V-M-DATAGRAM-TRANSPORT-SELECTOR]
    // END_CONTRACT: select_transport
    pub async fn select_transport(
        &self,
        association_id: DatagramAssociationId,
        cancel: CancellationToken,
    ) -> Result<DatagramTransportResolution, DatagramTransportSelectError> {
        // START_BLOCK_SELECT_DATAGRAM_TRANSPORT
        if cancel.is_cancelled() {
            warn!(
                association_id,
                "[DatagramTransportSelector][selectTransport][BLOCK_SELECT_DATAGRAM_TRANSPORT] datagram transport selection cancelled before carrier open"
            );
            return Err(DatagramTransportSelectError::Cancelled);
        }

        let attempt_cancel = cancel.child_token();
        tokio::select! {
            _ = cancel.cancelled() => {
                attempt_cancel.cancel();
                warn!(
                    association_id,
                    "[DatagramTransportSelector][selectTransport][BLOCK_SELECT_DATAGRAM_TRANSPORT] datagram transport selection cancelled during carrier open"
                );
                Err(DatagramTransportSelectError::Cancelled)
            }
            result = tokio::time::timeout(
                self.config.wss_timeout,
                self.wss.open_path(association_id, attempt_cancel.clone()),
            ) => {
                match result {
                    Ok(Ok(())) => {
                        info!(
                            association_id,
                            transport_kind = ?DatagramTransportKind::WssDatagram,
                            "[DatagramTransportSelector][selectTransport][BLOCK_SELECT_DATAGRAM_TRANSPORT] resolved datagram transport"
                        );
                        Ok(DatagramTransportResolution {
                            association_id,
                            transport_kind: DatagramTransportKind::WssDatagram,
                        })
                    }
                    Ok(Err(err)) => {
                        warn!(
                            association_id,
                            error = %err,
                            "[DatagramTransportSelector][selectTransport][BLOCK_SELECT_DATAGRAM_TRANSPORT] WSS-backed datagram path failed"
                        );
                        Err(DatagramTransportSelectError::WssFailed(err.to_string()))
                    }
                    Err(_) => {
                        attempt_cancel.cancel();
                        warn!(
                            association_id,
                            timeout_ms = self.config.wss_timeout.as_millis(),
                            "[DatagramTransportSelector][selectTransport][BLOCK_SELECT_DATAGRAM_TRANSPORT] WSS-backed datagram path timed out"
                        );
                        Err(DatagramTransportSelectError::WssTimeout(
                            self.config.wss_timeout.as_millis(),
                        ))
                    }
                }
            }
        }
        // END_BLOCK_SELECT_DATAGRAM_TRANSPORT
    }

    // START_CONTRACT: emit_outbound_datagram
    //   PURPOSE: Emit one governed outbound datagram through the approved WSS-backed datagram carrier after bounded transport selection.
    //   INPUTS: { envelope: &DatagramEnvelope - normalized outbound datagram contract, cancel: CancellationToken - caller cancellation boundary }
    //   OUTPUTS: { Result<DatagramTransportResolution, DatagramTransportSelectError> - resolved carrier metadata when the datagram is emitted successfully }
    //   SIDE_EFFECTS: [invokes the approved WSS-backed datagram carrier under a bounded timeout and emits the stable datagram transport log anchor]
    //   LINKS: [M-DATAGRAM-TRANSPORT-SELECTOR, V-M-DATAGRAM-TRANSPORT-SELECTOR]
    // END_CONTRACT: emit_outbound_datagram
    pub async fn emit_outbound_datagram(
        &self,
        envelope: &DatagramEnvelope,
        cancel: CancellationToken,
    ) -> Result<DatagramTransportResolution, DatagramTransportSelectError> {
        // START_BLOCK_EMIT_OUTBOUND_DATAGRAM
        if cancel.is_cancelled() {
            warn!(
                association_id = envelope.association_id,
                "[DatagramTransportSelector][emitOutboundDatagram][BLOCK_EMIT_OUTBOUND_DATAGRAM] datagram emission cancelled before carrier open"
            );
            return Err(DatagramTransportSelectError::Cancelled);
        }

        let attempt_cancel = cancel.child_token();
        tokio::select! {
            _ = cancel.cancelled() => {
                attempt_cancel.cancel();
                warn!(
                    association_id = envelope.association_id,
                    "[DatagramTransportSelector][emitOutboundDatagram][BLOCK_EMIT_OUTBOUND_DATAGRAM] datagram emission cancelled during carrier open"
                );
                Err(DatagramTransportSelectError::Cancelled)
            }
            result = tokio::time::timeout(
                self.config.wss_timeout,
                self.wss.emit_datagram(envelope, attempt_cancel.clone()),
            ) => {
                match result {
                    Ok(Ok(())) => {
                        info!(
                            association_id = envelope.association_id,
                            target = ?envelope.target,
                            payload_len = envelope.payload.len(),
                            transport_kind = ?DatagramTransportKind::WssDatagram,
                            "[DatagramTransportSelector][emitOutboundDatagram][BLOCK_EMIT_OUTBOUND_DATAGRAM] emitted governed outbound datagram through selected transport"
                        );
                        Ok(DatagramTransportResolution {
                            association_id: envelope.association_id,
                            transport_kind: DatagramTransportKind::WssDatagram,
                        })
                    }
                    Ok(Err(err)) => {
                        warn!(
                            association_id = envelope.association_id,
                            error = %err,
                            "[DatagramTransportSelector][emitOutboundDatagram][BLOCK_EMIT_OUTBOUND_DATAGRAM] WSS-backed datagram emission failed"
                        );
                        Err(DatagramTransportSelectError::WssFailed(err.to_string()))
                    }
                    Err(_) => {
                        attempt_cancel.cancel();
                        warn!(
                            association_id = envelope.association_id,
                            timeout_ms = self.config.wss_timeout.as_millis(),
                            "[DatagramTransportSelector][emitOutboundDatagram][BLOCK_EMIT_OUTBOUND_DATAGRAM] WSS-backed datagram emission timed out"
                        );
                        Err(DatagramTransportSelectError::WssTimeout(
                            self.config.wss_timeout.as_millis(),
                        ))
                    }
                }
            }
        }
        // END_BLOCK_EMIT_OUTBOUND_DATAGRAM
    }
}

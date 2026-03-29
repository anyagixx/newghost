// FILE: src/session/datagram_manager.rs
// VERSION: 0.1.1
// START_MODULE_CONTRACT
//   PURPOSE: Coordinate UDP association lifecycle, outbound or inbound datagram dispatch, and session-side cleanup rules.
//   SCOPE: Association open, outbound dispatch, inbound dispatch, activity refresh, and explicit association close over the governed UDP registry.
//   DEPENDS: async-trait, std, thiserror, tracing, src/session/udp_registry.rs, src/transport/datagram_contract.rs
//   LINKS: M-DATAGRAM-SESSION-MANAGER, V-M-DATAGRAM-SESSION-MANAGER, DF-UDP-OUTBOUND, DF-UDP-INBOUND, DF-UDP-ASSOCIATION-LIFECYCLE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   DatagramDispatchTarget - abstract outbound or inbound datagram sink used by the manager
//   DatagramSessionError - deterministic lifecycle and dispatch failure surface
//   DatagramSessionManager - orchestrate association open, outbound, inbound, and close trajectories
//   open_association - create one session-owned UDP association
//   forward_outbound_datagram - dispatch one outbound datagram while refreshing activity and emit bounded manager-side dispatch anchors
//   forward_inbound_datagram - dispatch one inbound datagram while refreshing activity and emit bounded manager-side dispatch anchors
//   close_association - close one UDP association and free its owned relay state
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added bounded outbound and inbound dispatch anchors so repair waves can distinguish local relay acceptance from manager-side datagram progression.
// END_CHANGE_SUMMARY

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use thiserror::Error;
use tracing::info;

use crate::session::udp_registry::{
    UdpAssociationLimitReached, UdpAssociationNotFound, UdpAssociationRecord,
    UdpAssociationRegistry,
};
use crate::transport::datagram_contract::{DatagramAssociationId, DatagramEnvelope};

#[cfg(test)]
#[path = "datagram_manager.test.rs"]
mod tests;

#[async_trait]
pub trait DatagramDispatchTarget: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn dispatch(&self, envelope: &DatagramEnvelope) -> Result<(), Self::Error>;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DatagramSessionError {
    #[error("udp association limit reached")]
    AssociationLimitReached,
    #[error("udp association not found: {0}")]
    AssociationNotFound(DatagramAssociationId),
    #[error("datagram dispatch failed: {0}")]
    DispatchFailed(String),
}

pub struct DatagramSessionManager<O, I> {
    registry: Arc<UdpAssociationRegistry>,
    outbound_target: O,
    inbound_target: I,
}

impl<O, I> DatagramSessionManager<O, I> {
    pub fn new(registry: Arc<UdpAssociationRegistry>, outbound_target: O, inbound_target: I) -> Self {
        Self {
            registry,
            outbound_target,
            inbound_target,
        }
    }
}

impl<O, I> DatagramSessionManager<O, I>
where
    O: DatagramDispatchTarget,
    I: DatagramDispatchTarget,
{
    // START_CONTRACT: open_association
    //   PURPOSE: Open one session-owned UDP association under deterministic registry ownership rules.
    //   INPUTS: { relay_addr: SocketAddr - governed relay bind, expected_client_addr: SocketAddr - only allowed UDP client source, now: Instant - current activity timestamp }
    //   OUTPUTS: { Result<(DatagramAssociationId, UdpAssociationRecord), DatagramSessionError> - association id and stored ownership record }
    //   SIDE_EFFECTS: [allocates one UDP association slot and emits the stable open-association log anchor]
    //   LINKS: [M-DATAGRAM-SESSION-MANAGER, M-UDP-ASSOCIATION-REGISTRY, V-M-DATAGRAM-SESSION-MANAGER]
    // END_CONTRACT: open_association
    pub fn open_association(
        &self,
        relay_addr: SocketAddr,
        expected_client_addr: SocketAddr,
        now: Instant,
    ) -> Result<(DatagramAssociationId, UdpAssociationRecord), DatagramSessionError> {
        // START_BLOCK_OPEN_DATAGRAM_ASSOCIATION
        let (association_id, record) = self
            .registry
            .open_association(relay_addr, expected_client_addr, now)
            .map_err(|UdpAssociationLimitReached::AtCapacity| {
                DatagramSessionError::AssociationLimitReached
            })?;

        info!(
            association_id,
            relay_addr = %record.relay_addr,
            expected_client_addr = %record.expected_client_addr,
            "[DatagramSessionManager][openAssociation][BLOCK_OPEN_DATAGRAM_ASSOCIATION] opened datagram association"
        );

        Ok((association_id, record))
        // END_BLOCK_OPEN_DATAGRAM_ASSOCIATION
    }

    pub async fn forward_outbound_datagram(
        &self,
        envelope: DatagramEnvelope,
        now: Instant,
    ) -> Result<(), DatagramSessionError> {
        // START_BLOCK_FORWARD_OUTBOUND_DATAGRAM
        self.registry
            .touch_association(envelope.association_id, now)
            .map_err(map_not_found)?;
        info!(
            association_id = envelope.association_id,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[DatagramSessionManager][forwardOutboundDatagram][BLOCK_FORWARD_OUTBOUND_DATAGRAM] dispatching outbound datagram"
        );
        self.outbound_target
            .dispatch(&envelope)
            .await
            .map_err(|err| DatagramSessionError::DispatchFailed(err.to_string()))?;
        info!(
            association_id = envelope.association_id,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[DatagramSessionManager][forwardOutboundDatagram][BLOCK_FORWARD_OUTBOUND_DATAGRAM] outbound datagram reached manager dispatch target"
        );
        Ok(())
        // END_BLOCK_FORWARD_OUTBOUND_DATAGRAM
    }

    // START_CONTRACT: forward_inbound_datagram
    //   PURPOSE: Dispatch one inbound datagram through the session-owned inbound target while refreshing activity and leaving a bounded manager-side trace.
    //   INPUTS: { envelope: DatagramEnvelope - normalized inbound datagram for one association, now: Instant - current activity timestamp }
    //   OUTPUTS: { Result<(), DatagramSessionError> - ok when the inbound datagram reaches the configured inbound target }
    //   SIDE_EFFECTS: [refreshes association activity and emits a stable inbound manager-dispatch log anchor]
    //   LINKS: [M-DATAGRAM-SESSION-MANAGER, V-M-DATAGRAM-SESSION-MANAGER]
    // END_CONTRACT: forward_inbound_datagram
    pub async fn forward_inbound_datagram(
        &self,
        envelope: DatagramEnvelope,
        now: Instant,
    ) -> Result<(), DatagramSessionError> {
        // START_BLOCK_FORWARD_INBOUND_DATAGRAM
        self.registry
            .touch_association(envelope.association_id, now)
            .map_err(map_not_found)?;
        info!(
            association_id = envelope.association_id,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[DatagramSessionManager][forwardInboundDatagram][BLOCK_FORWARD_INBOUND_DATAGRAM] dispatching inbound datagram"
        );
        self.inbound_target
            .dispatch(&envelope)
            .await
            .map_err(|err| DatagramSessionError::DispatchFailed(err.to_string()))?;
        info!(
            association_id = envelope.association_id,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[DatagramSessionManager][forwardInboundDatagram][BLOCK_FORWARD_INBOUND_DATAGRAM] inbound datagram reached manager dispatch target"
        );
        Ok(())
        // END_BLOCK_FORWARD_INBOUND_DATAGRAM
    }

    pub fn close_association(
        &self,
        association_id: DatagramAssociationId,
    ) -> Result<UdpAssociationRecord, DatagramSessionError> {
        self.registry
            .close_association(association_id)
            .map_err(map_not_found)
    }
}

fn map_not_found(error: UdpAssociationNotFound) -> DatagramSessionError {
    match error {
        UdpAssociationNotFound::Missing(association_id) => {
            DatagramSessionError::AssociationNotFound(association_id)
        }
    }
}

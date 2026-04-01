// FILE: src/session/datagram_manager.rs
// VERSION: 0.1.4
// START_MODULE_CONTRACT
//   PURPOSE: Coordinate UDP association lifecycle, outbound or inbound datagram dispatch, runtime handoff bridging, and session-side cleanup rules.
//   SCOPE: Association open, outbound dispatch, inbound dispatch, activity refresh, explicit association close, selector-backed outbound dispatch, and SOCKS5 runtime handoff bridging over the governed UDP registry.
//   DEPENDS: async-trait, std, thiserror, tokio-util, tracing, src/session/udp_registry.rs, src/session/datagram_transport_selector.rs, src/socks5/udp_associate.rs, src/transport/datagram_contract.rs
//   LINKS: M-DATAGRAM-SESSION-MANAGER, V-M-DATAGRAM-SESSION-MANAGER, DF-UDP-OUTBOUND, DF-UDP-INBOUND, DF-UDP-ASSOCIATION-LIFECYCLE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   DatagramDispatchTarget - abstract outbound or inbound datagram sink used by the manager
//   DatagramSessionError - deterministic lifecycle and dispatch failure surface
//   DatagramSessionManager - orchestrate association open, outbound, inbound, and close trajectories
//   DatagramSelectorDispatch - selector-backed outbound dispatch target used by the runtime bridge
//   DatagramSelectorDispatchError - bounded selector-backed dispatch failure surface
//   DatagramRuntimeBridge - SOCKS5 runtime handoff bridge from normalized UDP packets into manager-owned outbound dispatch
//   open_association - create one session-owned UDP association
//   ensure_outbound_association - open or reuse the session-owned UDP association for one governed relay and client endpoint pair
//   accept_outbound_datagram - bind local relay ownership to one outbound datagram and forward it through the manager dispatch path
//   forward_outbound_datagram - dispatch one outbound datagram while refreshing activity and emit bounded manager-side dispatch anchors
//   forward_inbound_datagram - dispatch one inbound datagram while refreshing activity and emit bounded manager-side dispatch anchors
//   close_association - close one UDP association and free its owned relay state
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.4 - Added a downstream-continuation anchor after manager-owned outbound dispatch so Phase-47 can classify progress above governed handoff without reopening transport diagnosis.
// END_CHANGE_SUMMARY

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::session::datagram_transport_selector::{
    DatagramTransportSelectError, DatagramTransportSelector, WssDatagramPath,
};
use crate::session::udp_registry::{
    UdpAssociationLimitReached, UdpAssociationNotFound, UdpAssociationRecord,
    UdpAssociationRegistry,
};
use crate::socks5::udp_associate::{UdpAssociateError, UdpRelayRuntimeTarget};
use crate::transport::datagram_contract::{DatagramAssociationId, DatagramEnvelope, DatagramError, DatagramTarget};

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
    #[error("datagram contract violation: {0}")]
    ContractViolation(#[from] DatagramError),
    #[error("datagram dispatch failed: {0}")]
    DispatchFailed(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DatagramSelectorDispatchError {
    #[error("datagram transport selection failed: {0}")]
    TransportSelectionFailed(#[from] DatagramTransportSelectError),
}

pub struct DatagramSelectorDispatch<W> {
    selector: DatagramTransportSelector<W>,
}

impl<W> DatagramSelectorDispatch<W> {
    pub fn new(selector: DatagramTransportSelector<W>) -> Self {
        Self { selector }
    }
}

pub struct DatagramRuntimeBridge<W, I> {
    manager: DatagramSessionManager<DatagramSelectorDispatch<W>, I>,
}

impl<W, I> DatagramRuntimeBridge<W, I> {
    pub fn new(
        registry: Arc<UdpAssociationRegistry>,
        selector: DatagramTransportSelector<W>,
        inbound_target: I,
    ) -> Self {
        Self {
            manager: DatagramSessionManager::new(
                registry,
                DatagramSelectorDispatch::new(selector),
                inbound_target,
            ),
        }
    }

    pub fn manager(&self) -> &DatagramSessionManager<DatagramSelectorDispatch<W>, I> {
        &self.manager
    }
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

    // START_CONTRACT: ensure_outbound_association
    //   PURPOSE: Reuse one existing governed UDP association for the same relay and client endpoints or open a fresh one when no owned association exists.
    //   INPUTS: { relay_addr: SocketAddr - governed relay bind, expected_client_addr: SocketAddr - only allowed UDP client source, now: Instant - current activity timestamp }
    //   OUTPUTS: { Result<(DatagramAssociationId, UdpAssociationRecord), DatagramSessionError> - resolved association id and its current ownership record }
    //   SIDE_EFFECTS: [may open a new association or refresh activity on an existing one, and emits a stable ownership-resolution anchor]
    //   LINKS: [M-DATAGRAM-SESSION-MANAGER, M-UDP-ASSOCIATION-REGISTRY, V-M-DATAGRAM-SESSION-MANAGER]
    // END_CONTRACT: ensure_outbound_association
    pub fn ensure_outbound_association(
        &self,
        relay_addr: SocketAddr,
        expected_client_addr: SocketAddr,
        now: Instant,
    ) -> Result<(DatagramAssociationId, UdpAssociationRecord), DatagramSessionError> {
        // START_BLOCK_ENSURE_OUTBOUND_ASSOCIATION
        if let Some((association_id, record)) = self
            .registry
            .find_by_endpoints(relay_addr, expected_client_addr)
        {
            self.registry
                .touch_association(association_id, now)
                .map_err(map_not_found)?;
            info!(
                association_id,
                relay_addr = %relay_addr,
                expected_client_addr = %expected_client_addr,
                "[DatagramSessionManager][ensureOutboundAssociation][BLOCK_ENSURE_OUTBOUND_ASSOCIATION] reused datagram association for outbound dispatch"
            );
            let mut refreshed = record;
            refreshed.last_activity = now;
            return Ok((association_id, refreshed));
        }

        let opened = self.open_association(relay_addr, expected_client_addr, now)?;
        info!(
            association_id = opened.0,
            relay_addr = %relay_addr,
            expected_client_addr = %expected_client_addr,
            "[DatagramSessionManager][ensureOutboundAssociation][BLOCK_ENSURE_OUTBOUND_ASSOCIATION] opened fresh datagram association for outbound dispatch"
        );
        Ok(opened)
        // END_BLOCK_ENSURE_OUTBOUND_ASSOCIATION
    }

    // START_CONTRACT: accept_outbound_datagram
    //   PURPOSE: Resolve or open the owned UDP association for one governed relay ingress event, normalize the resulting envelope, and forward it through the outbound manager dispatch path.
    //   INPUTS: { relay_addr: SocketAddr - governed relay bind that received the packet, expected_client_addr: SocketAddr - only allowed UDP source, target: DatagramTarget - normalized UDP target, payload: Vec<u8> - UDP payload bytes, now: Instant - current activity timestamp }
    //   OUTPUTS: { Result<DatagramEnvelope, DatagramSessionError> - validated outbound envelope after successful manager-side dispatch }
    //   SIDE_EFFECTS: [may open or reuse an association, forwards one outbound datagram, and emits a stable local-handoff anchor]
    //   LINKS: [M-DATAGRAM-SESSION-MANAGER, V-M-DATAGRAM-SESSION-MANAGER]
    // END_CONTRACT: accept_outbound_datagram
    pub async fn accept_outbound_datagram(
        &self,
        relay_addr: SocketAddr,
        expected_client_addr: SocketAddr,
        target: DatagramTarget,
        payload: Vec<u8>,
        now: Instant,
    ) -> Result<DatagramEnvelope, DatagramSessionError> {
        // START_BLOCK_ACCEPT_OUTBOUND_DATAGRAM
        let (association_id, _) = self.ensure_outbound_association(relay_addr, expected_client_addr, now)?;
        let envelope = DatagramEnvelope {
            association_id,
            relay_client_addr: expected_client_addr,
            target,
            payload,
        };
        envelope.validate()?;
        info!(
            association_id,
            relay_addr = %relay_addr,
            relay_client_addr = %expected_client_addr,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[DatagramSessionManager][acceptOutboundDatagram][BLOCK_ACCEPT_OUTBOUND_DATAGRAM] accepted governed outbound datagram for manager dispatch"
        );
        self.forward_outbound_datagram(envelope.clone(), now).await?;
        Ok(envelope)
        // END_BLOCK_ACCEPT_OUTBOUND_DATAGRAM
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
        info!(
            association_id = envelope.association_id,
            target = ?envelope.target,
            payload_len = envelope.payload.len(),
            "[CallDownstream][continuation][BLOCK_CALL_DOWNSTREAM_CONTINUATION] observed downstream outbound continuation beyond governed handoff"
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

#[async_trait]
impl<W> DatagramDispatchTarget for DatagramSelectorDispatch<W>
where
    W: WssDatagramPath,
{
    type Error = DatagramSelectorDispatchError;

    async fn dispatch(&self, envelope: &DatagramEnvelope) -> Result<(), Self::Error> {
        self.selector
            .emit_outbound_datagram(envelope, CancellationToken::new())
            .await?;
        Ok(())
    }
}

#[async_trait]
impl<W, I> UdpRelayRuntimeTarget for DatagramRuntimeBridge<W, I>
where
    W: WssDatagramPath,
    I: DatagramDispatchTarget + 'static,
{
    async fn forward_runtime_datagram(
        &self,
        relay_addr: SocketAddr,
        expected_client_addr: SocketAddr,
        target: DatagramTarget,
        payload: Vec<u8>,
    ) -> Result<(), UdpAssociateError> {
        self.manager
            .accept_outbound_datagram(
                relay_addr,
                expected_client_addr,
                target,
                payload,
                Instant::now(),
            )
            .await
            .map(|_| ())
            .map_err(map_runtime_bridge_error)
    }
}

fn map_not_found(error: UdpAssociationNotFound) -> DatagramSessionError {
    match error {
        UdpAssociationNotFound::Missing(association_id) => {
            DatagramSessionError::AssociationNotFound(association_id)
        }
    }
}

fn map_runtime_bridge_error(error: DatagramSessionError) -> UdpAssociateError {
    match error {
        DatagramSessionError::ContractViolation(inner) => UdpAssociateError::DatagramContract(inner),
        DatagramSessionError::AssociationLimitReached => {
            UdpAssociateError::Io("udp association limit reached".to_string())
        }
        DatagramSessionError::AssociationNotFound(association_id) => {
            UdpAssociateError::Io(format!("udp association not found: {association_id}"))
        }
        DatagramSessionError::DispatchFailed(message) => UdpAssociateError::Io(message),
    }
}

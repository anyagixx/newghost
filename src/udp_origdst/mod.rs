// FILE: src/udp_origdst/mod.rs
// VERSION: 0.1.1
// START_MODULE_CONTRACT
//   PURPOSE: Define the repo-local helper contract for transparently intercepted UDP, original-destination recovery, and governed handoff into the existing datagram path.
//   SCOPE: Helper configuration, recovered-tuple metadata, helper error taxonomy, Linux-adapter export, governed-handoff trait surface, recovered-tuple runtime forwarding, and repo-local runtime contract helpers.
//   DEPENDS: async-trait, std, thiserror, tracing, src/transport/datagram_contract.rs, src/session/datagram_manager.rs, src/udp_origdst/linux.rs
//   LINKS: M-UDP-ORIGDST-CONTRACT, M-UDP-ORIGDST-RUNTIME, M-UDP-ORIGDST-LINUX-ADAPTER, V-M-UDP-ORIGDST-CONTRACT, DF-UDP-ORIGDST-RECOVERY, DF-UDP-ORIGDST-GOVERNED-HANDOFF
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   UdpOrigDstHelperConfig - bounded helper listener and baseline-preserve settings
//   RecoveredUdpTuple - one transparently intercepted UDP packet plus its recovered original destination metadata
//   UdpOrigDstError - deterministic repo-local helper contract error surface
//   UdpOrigDstRuntime - bounded runtime that validates one recovered tuple and forwards it into the governed handoff target
//   UdpOrigDstGovernedHandoff - bounded runtime contract for forwarding one recovered tuple into the governed datagram path
//   UdpOrigDstRecoverySurface - bounded contract over recovery of one original destination tuple
//   tupleEvidenceLabel - emit one stable tuple evidence label for logs and smoke packets
//   forwardRecoveredDatagram - validate one recovered tuple and forward it into the governed handoff target
//   linux - Linux-specific original-destination recovery surface
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added the bounded repo-local runtime surface so recovered tuples can be validated and forwarded into governed handoff with stable trace anchors.
// END_CHANGE_SUMMARY

use std::net::SocketAddr;

use async_trait::async_trait;
use thiserror::Error;
use tracing::info;

use crate::transport::datagram_contract::{DatagramTarget, MAX_DATAGRAM_PAYLOAD_BYTES};

pub mod linux;

#[cfg(test)]
#[path = "udp_origdst.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpOrigDstHelperConfig {
    pub listener_addr: SocketAddr,
    pub preserve_baseline_proxy_addr: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredUdpTuple {
    pub client_source_addr: SocketAddr,
    pub helper_listener_addr: SocketAddr,
    pub original_target: DatagramTarget,
    pub payload_len: usize,
}

pub struct UdpOrigDstRuntime<H> {
    handoff: H,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UdpOrigDstError {
    #[error("original-destination tuple recovery failed: {0}")]
    RecoveryFailed(String),
    #[error("governed handoff failed: {0}")]
    GovernedHandoffFailed(String),
    #[error("invalid recovered tuple: {0}")]
    InvalidRecoveredTuple(String),
    #[error("recovered payload length mismatch: expected {expected}, got {actual}")]
    PayloadLengthMismatch { expected: usize, actual: usize },
    #[error("recovered payload exceeds maximum supported size")]
    PayloadTooLarge,
}

#[async_trait]
pub trait UdpOrigDstGovernedHandoff: Send + Sync + 'static {
    async fn forward_recovered_tuple(
        &self,
        tuple: RecoveredUdpTuple,
        payload: Vec<u8>,
    ) -> Result<(), UdpOrigDstError>;
}

pub trait UdpOrigDstRecoverySurface: Send + Sync + 'static {
    fn recover_original_destination(
        &self,
        helper_listener_addr: SocketAddr,
        client_source_addr: SocketAddr,
        payload_len: usize,
        original_target: DatagramTarget,
    ) -> Result<RecoveredUdpTuple, UdpOrigDstError>;
}

impl<H> UdpOrigDstRuntime<H>
where
    H: UdpOrigDstGovernedHandoff,
{
    pub fn new(handoff: H) -> Self {
        Self { handoff }
    }

    // START_CONTRACT: forwardRecoveredDatagram
    //   PURPOSE: Validate one recovered tuple and forward its payload into the governed handoff target with stable tuple-level trace anchors.
    //   INPUTS: { tuple: RecoveredUdpTuple - recovered tuple metadata from the repo-local helper, payload: Vec<u8> - intercepted UDP payload bytes }
    //   OUTPUTS: { Result<(), UdpOrigDstError> - ok when the recovered tuple reaches the governed handoff target }
    //   SIDE_EFFECTS: [emits stable runtime and governed-handoff log anchors]
    //   LINKS: [M-UDP-ORIGDST-RUNTIME, V-M-UDP-ORIGDST-RUNTIME]
    // END_CONTRACT: forwardRecoveredDatagram
    pub async fn forward_recovered_datagram(
        &self,
        tuple: RecoveredUdpTuple,
        payload: Vec<u8>,
    ) -> Result<(), UdpOrigDstError> {
        // START_BLOCK_UDP_ORIGDST_RUNTIME
        if tuple.payload_len != payload.len() {
            return Err(UdpOrigDstError::PayloadLengthMismatch {
                expected: tuple.payload_len,
                actual: payload.len(),
            });
        }

        if payload.len() > MAX_DATAGRAM_PAYLOAD_BYTES {
            return Err(UdpOrigDstError::PayloadTooLarge);
        }

        let tuple_label = tuple_evidence_label(&tuple);
        info!(
            tuple = %tuple_label,
            target = ?tuple.original_target,
            payload_len = payload.len(),
            "[UdpOrigDstRuntime][forwardRecoveredDatagram][BLOCK_UDP_ORIGDST_RUNTIME] accepted recovered UDP tuple for governed handoff"
        );
        // END_BLOCK_UDP_ORIGDST_RUNTIME

        self.handoff
            .forward_recovered_tuple(tuple.clone(), payload)
            .await?;

        // START_BLOCK_UDP_ORIGDST_GOVERNED_HANDOFF
        info!(
            tuple = %tuple_label,
            target = ?tuple.original_target,
            payload_len = tuple.payload_len,
            "[UdpOrigDstRuntime][forwardRecoveredDatagram][BLOCK_UDP_ORIGDST_GOVERNED_HANDOFF] forwarded recovered UDP tuple into governed handoff"
        );
        Ok(())
        // END_BLOCK_UDP_ORIGDST_GOVERNED_HANDOFF
    }
}

// START_CONTRACT: tupleEvidenceLabel
//   PURPOSE: Emit one stable label for tuple-level logs and smoke packets so recovery and governed handoff can be correlated deterministically.
//   INPUTS: { tuple: &RecoveredUdpTuple - recovered tuple metadata for one transparently intercepted UDP packet }
//   OUTPUTS: { String - stable tuple evidence label suitable for logs and packet summaries }
//   SIDE_EFFECTS: [none]
//   LINKS: [M-UDP-ORIGDST-CONTRACT, V-M-UDP-ORIGDST-CONTRACT]
// END_CONTRACT: tupleEvidenceLabel
pub fn tuple_evidence_label(tuple: &RecoveredUdpTuple) -> String {
    // START_BLOCK_UDP_ORIGDST_CONTRACT
    format!(
        "{}|{}|{:?}|{}",
        tuple.client_source_addr,
        tuple.helper_listener_addr,
        tuple.original_target,
        tuple.payload_len
    )
    // END_BLOCK_UDP_ORIGDST_CONTRACT
}

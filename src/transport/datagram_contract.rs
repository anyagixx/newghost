// FILE: src/transport/datagram_contract.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Define the transport-agnostic datagram envelope, association identity, size bounds, and error taxonomy for UDP-capable proxy flows.
//   SCOPE: Datagram association identifiers, relay-source metadata, target addressing, payload bounds, and contract-level normalization checks.
//   DEPENDS: std, thiserror
//   LINKS: M-DATAGRAM-CONTRACT, V-M-DATAGRAM-CONTRACT, DF-SOCKS5-UDP-ASSOCIATE, DF-UDP-OUTBOUND, DF-UDP-INBOUND
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   DatagramAssociationId - stable UDP association identity
//   DatagramTarget - normalized UDP target address
//   DatagramEnvelope - one transport-agnostic UDP datagram plus addressing metadata
//   DatagramError - contract-level datagram validation errors
//   validate - enforce bounded payload and non-empty-target invariants
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added the governed datagram contract so UDP-capable work can share one explicit envelope and validation surface.
// END_CHANGE_SUMMARY

use std::net::SocketAddr;

use thiserror::Error;

#[cfg(test)]
#[path = "datagram_contract.test.rs"]
mod tests;

pub const MAX_DATAGRAM_PAYLOAD_BYTES: usize = 65_507;

pub type DatagramAssociationId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatagramTarget {
    Ip(SocketAddr),
    Domain(String, u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatagramEnvelope {
    pub association_id: DatagramAssociationId,
    pub relay_client_addr: SocketAddr,
    pub target: DatagramTarget,
    pub payload: Vec<u8>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DatagramError {
    #[error("datagram payload exceeds the maximum supported size")]
    PayloadTooLarge,
    #[error("domain targets must not be empty")]
    EmptyDomainTarget,
}

impl DatagramEnvelope {
    // START_CONTRACT: validate
    //   PURPOSE: Enforce bounded datagram contract invariants before transport-specific processing begins.
    //   INPUTS: { &self: DatagramEnvelope - normalized UDP datagram plus addressing metadata }
    //   OUTPUTS: { Result<(), DatagramError> - ok when the envelope is valid for later transport handling }
    //   SIDE_EFFECTS: [none]
    //   LINKS: [M-DATAGRAM-CONTRACT, V-M-DATAGRAM-CONTRACT]
    // END_CONTRACT: validate
    pub fn validate(&self) -> Result<(), DatagramError> {
        // START_BLOCK_DATAGRAM_CONTRACT
        if self.payload.len() > MAX_DATAGRAM_PAYLOAD_BYTES {
            return Err(DatagramError::PayloadTooLarge);
        }

        if let DatagramTarget::Domain(domain, _) = &self.target {
            if domain.is_empty() {
                return Err(DatagramError::EmptyDomainTarget);
            }
        }

        Ok(())
        // END_BLOCK_DATAGRAM_CONTRACT
    }
}

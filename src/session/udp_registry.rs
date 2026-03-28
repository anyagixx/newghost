// FILE: src/session/udp_registry.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Track active UDP associations, governed relay endpoints, idle timers, and deterministic cleanup for datagram traffic.
//   SCOPE: Capacity reservation, association open/get/touch/close, idle reaping, and stable association-count reporting.
//   DEPENDS: std, thiserror, tokio, tracing, src/transport/datagram_contract.rs
//   LINKS: M-UDP-ASSOCIATION-REGISTRY, V-M-UDP-ASSOCIATION-REGISTRY, DF-UDP-ASSOCIATION-LIFECYCLE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   UdpAssociationRecord - mutable UDP association ownership and relay metadata
//   UdpAssociationRegistry - capacity-aware UDP association storage
//   UdpAssociationLimitReached - deterministic capacity exhaustion error
//   UdpAssociationNotFound - deterministic lookup failure error
//   open_association - register one active UDP association
//   touch_association - refresh the activity timestamp for one association
//   close_association - remove one UDP association and release its reserved slot
//   reap_idle - close stale UDP associations whose idle lifetime exceeds the configured bound
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added a dedicated UDP association registry so datagram ownership and cleanup stay deterministic before transport integration work begins.
// END_CHANGE_SUMMARY

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::info;

use crate::transport::datagram_contract::DatagramAssociationId;

#[cfg(test)]
#[path = "udp_registry.test.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpAssociationRecord {
    pub relay_addr: SocketAddr,
    pub expected_client_addr: SocketAddr,
    pub last_activity: Instant,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UdpAssociationLimitReached {
    #[error("udp association registry is at capacity")]
    AtCapacity,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UdpAssociationNotFound {
    #[error("udp association not found: {0}")]
    Missing(DatagramAssociationId),
}

#[derive(Debug)]
struct UdpAssociationReservation {
    permit: OwnedSemaphorePermit,
}

struct StoredAssociation {
    record: UdpAssociationRecord,
    permit: OwnedSemaphorePermit,
}

pub struct UdpAssociationRegistry {
    associations: Mutex<HashMap<DatagramAssociationId, StoredAssociation>>,
    semaphore: Arc<Semaphore>,
    next_association_id: AtomicU64,
}

impl UdpAssociationRegistry {
    pub fn new(max_associations: usize) -> Self {
        Self {
            associations: Mutex::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(max_associations)),
            next_association_id: AtomicU64::new(1),
        }
    }

    fn try_reserve(&self) -> Result<UdpAssociationReservation, UdpAssociationLimitReached> {
        let permit = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| UdpAssociationLimitReached::AtCapacity)?;
        Ok(UdpAssociationReservation { permit })
    }

    // START_CONTRACT: open_association
    //   PURPOSE: Register one active UDP association and reserve deterministic ownership capacity.
    //   INPUTS: { relay_addr: SocketAddr - governed local relay bind, expected_client_addr: SocketAddr - only allowed UDP source for this association, now: Instant - current activity timestamp }
    //   OUTPUTS: { Result<(DatagramAssociationId, UdpAssociationRecord), UdpAssociationLimitReached> - association id and stored metadata }
    //   SIDE_EFFECTS: [consumes one capacity slot and emits a stable registry log anchor]
    //   LINKS: [M-UDP-ASSOCIATION-REGISTRY, V-M-UDP-ASSOCIATION-REGISTRY]
    // END_CONTRACT: open_association
    pub fn open_association(
        &self,
        relay_addr: SocketAddr,
        expected_client_addr: SocketAddr,
        now: Instant,
    ) -> Result<(DatagramAssociationId, UdpAssociationRecord), UdpAssociationLimitReached> {
        // START_BLOCK_UDP_ASSOCIATION_REGISTRY
        let reservation = self.try_reserve()?;
        let association_id = self.next_association_id.fetch_add(1, Ordering::Relaxed);
        let record = UdpAssociationRecord {
            relay_addr,
            expected_client_addr,
            last_activity: now,
        };

        self.associations
            .lock()
            .expect("udp association registry lock poisoned")
            .insert(
                association_id,
                StoredAssociation {
                    record: record.clone(),
                    permit: reservation.permit,
                },
            );

        info!(
            association_id,
            relay_addr = %relay_addr,
            expected_client_addr = %expected_client_addr,
            "[UdpAssociationRegistry][openAssociation][BLOCK_UDP_ASSOCIATION_REGISTRY] registered UDP association"
        );

        Ok((association_id, record))
        // END_BLOCK_UDP_ASSOCIATION_REGISTRY
    }

    pub fn get(&self, association_id: DatagramAssociationId) -> Option<UdpAssociationRecord> {
        self.associations
            .lock()
            .expect("udp association registry lock poisoned")
            .get(&association_id)
            .map(|stored| stored.record.clone())
    }

    pub fn touch_association(
        &self,
        association_id: DatagramAssociationId,
        now: Instant,
    ) -> Result<(), UdpAssociationNotFound> {
        let mut associations = self
            .associations
            .lock()
            .expect("udp association registry lock poisoned");
        let stored = associations
            .get_mut(&association_id)
            .ok_or(UdpAssociationNotFound::Missing(association_id))?;
        stored.record.last_activity = now;
        Ok(())
    }

    pub fn close_association(
        &self,
        association_id: DatagramAssociationId,
    ) -> Result<UdpAssociationRecord, UdpAssociationNotFound> {
        let stored = self
            .associations
            .lock()
            .expect("udp association registry lock poisoned")
            .remove(&association_id)
            .ok_or(UdpAssociationNotFound::Missing(association_id))?;
        let record = stored.record;
        drop(stored.permit);
        Ok(record)
    }

    pub fn reap_idle(
        &self,
        max_idle: Duration,
        now: Instant,
    ) -> Vec<DatagramAssociationId> {
        let mut associations = self
            .associations
            .lock()
            .expect("udp association registry lock poisoned");
        let mut removed = Vec::new();

        associations.retain(|association_id, stored| {
            let idle_for = now.saturating_duration_since(stored.record.last_activity);
            let keep = idle_for <= max_idle;
            if !keep {
                removed.push(*association_id);
            }
            keep
        });

        removed
    }

    pub fn association_count(&self) -> usize {
        self.associations
            .lock()
            .expect("udp association registry lock poisoned")
            .len()
    }

    pub fn available_slots(&self) -> usize {
        self.semaphore.available_permits()
    }
}

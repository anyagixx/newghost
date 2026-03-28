// FILE: src/session/udp_registry.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify UDP association capacity control, deterministic close behavior, idle cleanup, and activity refresh semantics.
//   SCOPE: Association open, capacity exhaustion, close, touch, and idle reap behavior.
//   DEPENDS: src/session/udp_registry.rs
//   LINKS: V-M-UDP-ASSOCIATION-REGISTRY
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   open_and_close_release_capacity - proves explicit close returns registry capacity
//   capacity_exhaustion_is_deterministic - proves registry rejects opens beyond the configured limit
//   touch_updates_only_target_association - proves activity refresh mutates only the addressed association
//   reap_idle_closes_stale_associations - proves stale associations are reaped while fresh ones remain
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added deterministic UDP association registry tests so later datagram work can rely on bounded ownership semantics.
// END_CHANGE_SUMMARY

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use super::{UdpAssociationLimitReached, UdpAssociationNotFound, UdpAssociationRegistry};

fn socket(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

#[test]
fn open_and_close_release_capacity() {
    let registry = UdpAssociationRegistry::new(1);
    let now = Instant::now();

    let (association_id, record) = registry
        .open_association(socket(40000), socket(50000), now)
        .expect("association should open");
    assert_eq!(record.relay_addr, socket(40000));
    assert_eq!(registry.available_slots(), 0);
    assert_eq!(registry.association_count(), 1);

    let closed = registry
        .close_association(association_id)
        .expect("association should close");
    assert_eq!(closed.expected_client_addr, socket(50000));
    assert_eq!(registry.available_slots(), 1);
    assert_eq!(registry.association_count(), 0);
}

#[test]
fn capacity_exhaustion_is_deterministic() {
    let registry = UdpAssociationRegistry::new(1);
    registry
        .open_association(socket(40000), socket(50000), Instant::now())
        .expect("first association should open");

    assert_eq!(
        registry
            .open_association(socket(40001), socket(50001), Instant::now())
            .expect_err("second association should be rejected"),
        UdpAssociationLimitReached::AtCapacity
    );
}

#[test]
fn touch_updates_only_target_association() {
    let registry = UdpAssociationRegistry::new(2);
    let now = Instant::now();
    let later = now + Duration::from_secs(5);

    let (first_id, first_record) = registry
        .open_association(socket(40000), socket(50000), now)
        .expect("first association");
    let (second_id, second_record) = registry
        .open_association(socket(40001), socket(50001), now)
        .expect("second association");

    registry
        .touch_association(first_id, later)
        .expect("touch first association");

    assert_eq!(
        registry.get(first_id).expect("first snapshot").last_activity,
        later
    );
    assert_eq!(
        registry.get(second_id).expect("second snapshot").last_activity,
        second_record.last_activity
    );
    assert_eq!(first_record.expected_client_addr, socket(50000));
}

#[test]
fn reap_idle_closes_stale_associations() {
    let registry = UdpAssociationRegistry::new(2);
    let now = Instant::now();

    let (stale_id, _) = registry
        .open_association(socket(40000), socket(50000), now - Duration::from_secs(120))
        .expect("stale association");
    let (fresh_id, _) = registry
        .open_association(socket(40001), socket(50001), now - Duration::from_secs(5))
        .expect("fresh association");

    let removed = registry.reap_idle(Duration::from_secs(30), now);

    assert_eq!(removed, vec![stale_id]);
    assert!(registry.get(stale_id).is_none());
    assert!(registry.get(fresh_id).is_some());
    assert_eq!(registry.association_count(), 1);
    assert_eq!(registry.available_slots(), 1);
}

#[test]
fn touching_missing_association_is_explicit() {
    let registry = UdpAssociationRegistry::new(1);
    let error = registry
        .touch_association(9, Instant::now())
        .expect_err("missing association should be explicit");
    assert_eq!(error, UdpAssociationNotFound::Missing(9));
}

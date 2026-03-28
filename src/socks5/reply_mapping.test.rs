// FILE: src/socks5/reply_mapping.test.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Verify deterministic SOCKS5 reply mapping across infrastructure, policy, target, and post-reply failures.
//   SCOPE: Reply-code mapping, error-category stability, and the invariant that all pre-pump failures stay client-visible.
//   DEPENDS: src/socks5/mod.rs
//   LINKS: V-M-SOCKS5, VF-006, VF-007
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   infrastructure_failures_map_to_general_failure - proves infrastructure failures map to general failure replies
//   policy_denied_maps_to_ruleset_reply - proves policy denials map to ruleset replies
//   target_refused_maps_to_connection_refused - proves refused targets map to connection-refused replies
//   target_timeout_maps_to_host_unreachable - proves timed-out targets map to host-unreachable replies
//   pump_failed_returns_no_reply - proves post-reply pump failure does not emit a second client reply
//   categories_remain_exhaustive_and_stable - proves error categories stay exhaustive and stable
//   all_pre_pump_variants_map_to_some_reply - proves every pre-pump error remains client-visible
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added GRACE markup so reply-mapping invariants remain explicit for autonomous verification agents.
// END_CHANGE_SUMMARY

use super::{ErrorCategory, ProxyError, Socks5Proxy, Socks5Reply};

#[test]
fn infrastructure_failures_map_to_general_failure() {
    let queue_full = Socks5Proxy::map_reply(&ProxyError::IntentQueueFull);
    let session_limit = Socks5Proxy::map_reply(&ProxyError::SessionLimitReached);
    let transport = Socks5Proxy::map_reply(&ProxyError::TransportFailed("down".to_string()));
    let cancelled = Socks5Proxy::map_reply(&ProxyError::Cancelled);

    assert_eq!(queue_full, Some(Socks5Reply::GeneralFailure));
    assert_eq!(session_limit, Some(Socks5Reply::GeneralFailure));
    assert_eq!(transport, Some(Socks5Reply::GeneralFailure));
    assert_eq!(cancelled, Some(Socks5Reply::GeneralFailure));
}

#[test]
fn policy_denied_maps_to_ruleset_reply() {
    let reply = Socks5Proxy::map_reply(&ProxyError::EgressDenied("blocked".to_string()));
    assert_eq!(reply, Some(Socks5Reply::NotAllowedByRuleset));
}

#[test]
fn target_refused_maps_to_connection_refused() {
    let reply = Socks5Proxy::map_reply(&ProxyError::TargetUnreachable(std::io::Error::new(
        std::io::ErrorKind::ConnectionRefused,
        "refused",
    )));
    assert_eq!(reply, Some(Socks5Reply::ConnectionRefused));
}

#[test]
fn target_timeout_maps_to_host_unreachable() {
    let reply = Socks5Proxy::map_reply(&ProxyError::TargetUnreachable(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "timeout",
    )));
    assert_eq!(reply, Some(Socks5Reply::HostUnreachable));
}

#[test]
fn pump_failed_returns_no_reply() {
    let reply = Socks5Proxy::map_reply(&ProxyError::PumpFailed(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "broken pipe",
    )));
    assert_eq!(reply, None);
}

#[test]
fn categories_remain_exhaustive_and_stable() {
    assert_eq!(
        ProxyError::IntentQueueFull.category(),
        ErrorCategory::Infrastructure
    );
    assert_eq!(
        ProxyError::EgressDenied("blocked".to_string()).category(),
        ErrorCategory::Policy
    );
    assert_eq!(
        ProxyError::TargetUnreachable(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"))
            .category(),
        ErrorCategory::Target
    );
    assert_eq!(
        ProxyError::PumpFailed(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "broken"
        ))
        .category(),
        ErrorCategory::PostReply
    );
}

#[test]
fn all_pre_pump_variants_map_to_some_reply() {
    let pre_pump_errors = [
        ProxyError::IntentQueueFull,
        ProxyError::SessionLimitReached,
        ProxyError::TransportFailed("all failed".to_string()),
        ProxyError::EgressDenied("blocked".to_string()),
        ProxyError::TargetUnreachable(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "refused",
        )),
        ProxyError::Cancelled,
    ];

    for error in &pre_pump_errors {
        assert!(
            Socks5Proxy::map_reply(error).is_some(),
            "pre-pump error must produce a client-visible reply: {error:?}"
        );
    }
}

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

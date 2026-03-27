use super::{ErrorCategory, ProxyError, Socks5Proxy, Socks5Reply};

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

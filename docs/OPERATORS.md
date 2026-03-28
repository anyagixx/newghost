# Operators Guide

## Managed Deployment Layout

Phase-9 fixes one canonical deployment layout for managed Linux hosts. Do not introduce alternate ad hoc paths once managed deployment begins.

### Shared Paths

| Surface | Canonical Path | Notes |
|---|---|---|
| release binary | `/opt/n0wss/n0wss` | Active executable for both roles |
| working root | `/opt/n0wss` | Owned by `root:root` |
| cert directory | `/opt/n0wss/certs` | Server cert and client trust copy live here |
| runtime env directory | `/opt/n0wss/env` | Reserved for service-managed env files |
| runtime state directory | `/opt/n0wss/run` | Reserved for future pid or transient state if needed |
| logs | `/var/log/n0wss-*.log` | Bounded by logrotate in later Phase-9 steps |

### Server Role

| Surface | Canonical Path |
|---|---|
| binary | `/opt/n0wss/n0wss` |
| server cert | `/opt/n0wss/certs/server.pem` |
| server key | `/opt/n0wss/certs/server.key` |
| auth source | `/opt/n0wss/env/server.env` |
| log file | `/var/log/n0wss-server.log` |
| service file | `/etc/systemd/system/n0wss-server.service` |

The server role owns the externally reachable WSS listener and must not read client-only override values from a separate layout.

### Client Role

| Surface | Canonical Path |
|---|---|
| binary | `/opt/n0wss/n0wss` |
| trust anchor | `/opt/n0wss/certs/server.pem` |
| auth source | `/opt/n0wss/env/client.env` |
| log file | `/var/log/n0wss-client.log` |
| bad-auth log file | `/var/log/n0wss-client-bad-auth.log` |
| service file | `/etc/systemd/system/n0wss-client.service` |

The client role owns the local SOCKS5 ingress and targets the managed server endpoint through the pinned trust shape only.

### Ownership And Modes

Canonical ownership and mode rules:

- `/opt/n0wss` and subdirectories: `root:root`
- `n0wss` binary: mode `0755`
- certificate files: mode `0644` unless stricter host policy requires tighter read rules
- private key files and env files containing auth material: mode `0600`
- service files: mode `0644`
- logs stay writable by the service runtime user or by the service manager policy chosen in the unit files

Operational invariants:

- one binary path per host role
- one canonical log target per service role
- no secret material in repository-tracked files
- no service file may point outside the approved `/opt/n0wss`, `/etc/systemd/system`, and `/var/log` layout without a new GRACE plan update

## Release Readiness

Before opening the first GitHub release or handing the repository to external testers, run:

```bash
cargo clippy --all-targets --all-features
cargo test
```

Narrow checks that map cleanly to the implemented modules:

```bash
cargo test config
cargo test cli
cargo test wss_gateway
cargo test iroh_adapter
cargo test session
cargo test socks5
cargo test proxy_bridge
```

Current release posture:

- source-first release
- no standalone binary target documented yet
- GitHub docs and CI must stay aligned with the commands above

If docs or workflows mention `cargo run`, a binary name, or a smoke target that does not exist, treat that as release drift and block publication.

## Burst Detection Tuning

Burst thresholds are deployment-time tuning values, not architecture constants.

Default guidance:

- `alert_threshold = 50`
  Use `10` for single-user or local development environments.
- `alert_window_secs = 1`
  Keeps short spikes visible without alerting on isolated failures.
- `min_log_interval_secs = 5`
  Limits sustained burst logging to at most 12 entries per minute.

Tune after observing real traffic for 24 hours:

1. If `peak_rate_per_sec` never exceeds `10`, lower `alert_threshold`.
2. If `peak_rate_per_sec` regularly exceeds the threshold during normal load, raise it.
3. If `intent_queue_len / intent_queue_capacity` stays above `0.8`, increase queue capacity before loosening burst alerts.
4. If `total_rejected` remains `0`, keep defaults and avoid unnecessary tuning.

Key metrics:

- `peak_rate_per_sec`
- `intents_rejected_queue_full`
- `intent_queue_len`
- `intent_queue_capacity`

Operational rule:

- Tune thresholds first.
- Increase queue capacity second.
- Treat repeated queue saturation as a capacity problem, not only a logging problem.

## Quick Runtime Shapes

These are the currently validated runtime argument shapes from the CLI/config tests.

Client mode:

```text
n0wss --auth-token <token> client --remote-wss-url wss://example.com/tunnel
```

Server mode:

```text
n0wss --auth-token <token> server --tls-cert-path certs/server.pem --tls-key-path certs/server.key
```

Do not publish examples with a non-`wss` remote URL or with missing TLS paths for server mode.

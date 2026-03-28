# n0wss

`n0wss` is a GRACE-governed Rust codebase for a WSS-backed proxy tunnel with an optional iroh transport path. The current repository state is source-first: the core modules, tests, managed deployment surface, and release gates are implemented.

## Current Scope

- Client and server runtime configuration parsing
- Observability bootstrap and burst detection
- Auth policy and redaction boundaries
- TLS loading for server mode
- WSS and iroh transport adapters
- Session orchestration, SOCKS5 ingress, and proxy bridge logic

## Repository Layout

- `src/config` - typed runtime configuration and validation
- `src/obs` - tracing, metrics, and burst detection
- `src/auth` - handshake authentication and redaction
- `src/tls` - TLS material loading and validation
- `src/wss_gateway` - WSS transport adapter
- `src/iroh_adapter` - iroh transport adapter
- `src/session` - state machine, registry, selector, and orchestrator
- `src/socks5` - SOCKS5 parsing and reply mapping
- `src/proxy_bridge` - queue-driven bridge and bidirectional pump
- `docs/` - GRACE requirements, plan, graph, verification, and operator notes

## Local Verification

Minimum release-ready checks:

```bash
cargo clippy --all-targets --all-features
cargo test
```

Useful narrow checks while iterating:

```bash
cargo test config
cargo test cli
cargo test wss_gateway
cargo test iroh_adapter
cargo test session
cargo test socks5
cargo test proxy_bridge
```

## Runtime Argument Shapes

The current runtime entry surface is the typed CLI parser exercised by tests and by `src/cli::run_from`.

Client mode:

```text
n0wss --auth-token <token> client --remote-wss-url wss://example.com/tunnel
```

Client mode with pinned live trust:

```text
n0wss --auth-token <token> client --remote-wss-url wss://example.com/tunnel --tls-trust-anchor-path certs/live-ca.pem --tls-server-name-override edge.example.com
```

Server mode:

```text
n0wss --auth-token <token> server --tls-cert-path certs/server.pem --tls-key-path certs/server.key
```

Important validated options:

- `--max-pending-intents`
- `--max-sessions`
- `--tls-trust-anchor-path`
- `--tls-server-name-override`
- `--iroh-connect-timeout-secs`
- `--wss-connect-timeout-secs`
- `--socks5-total-timeout-secs`
- `--graceful-timeout-secs`
- `--force-kill-after-secs`
- `--burst-alert-threshold`
- `--burst-alert-window-secs`
- `--burst-min-log-interval-secs`
- `--burst-ring-capacity`

`client` mode requires a `wss://` remote URL and may pin trust with `--tls-trust-anchor-path`. `server` mode requires both TLS paths.

## Managed Deployment Surface

Phase-9 introduces the first governed managed deployment surface:

- `scripts/deploy-live.sh` for repeatable role-aware staging
- `deploy/systemd/n0wss-server.service` and `deploy/systemd/n0wss-client.service` for managed startup and restart
- `deploy/logrotate/n0wss` for bounded log retention
- `docs/OPERATORS.md` for install, `systemctl`, `journalctl`, logrotate, and rollback procedures

## Telegram Compatibility Profile

`n0wss` can be presented to Telegram as a standard local SOCKS5 proxy when the managed client runtime is already healthy. The expected shape is still:

- Telegram points to a local SOCKS5 listener such as `127.0.0.1:1080`
- `n0wss` remains the tunnel runtime behind that listener
- the remote path stays the same governed WSS-backed tunnel already verified for generic SOCKS5 traffic

Bounded claim surface:

- supported claim: Telegram can be tested as a SOCKS5-aware application over the existing `n0wss` tunnel
- unsupported claim: `n0wss` defines a Telegram-specific client protocol
- unverified claim: `n0wss` guarantees bypass of every possible Telegram blocking regime or acts like a whole-device VPN

The Telegram-specific verification wave is about client compatibility evidence, not about inventing a new app protocol.

## Release Notes

The next public release is prepared as source-first:

- GitHub source release
- CI-enforced `clippy` and `cargo test`
- operator guidance in [docs/OPERATORS.md](/home/truffle/Загрузки/newghost/docs/OPERATORS.md)

Standalone packaged distribution is still deferred, but the governed `n0wss` binary, managed deployment workflow, and operator runbook are already implemented in-repo.

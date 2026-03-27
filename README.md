# n0wss

`n0wss` is a GRACE-governed Rust codebase for a WSS-backed proxy tunnel with an optional iroh transport path. The current repository state is source-first: the core modules, tests, and release gates are implemented, while the standalone binary wrapper and public deployment flow are still being finalized.

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

Server mode:

```text
n0wss --auth-token <token> server --tls-cert-path certs/server.pem --tls-key-path certs/server.key
```

Important validated options:

- `--max-pending-intents`
- `--max-sessions`
- `--iroh-connect-timeout-secs`
- `--wss-connect-timeout-secs`
- `--socks5-total-timeout-secs`
- `--graceful-timeout-secs`
- `--force-kill-after-secs`
- `--burst-alert-threshold`
- `--burst-alert-window-secs`
- `--burst-min-log-interval-secs`
- `--burst-ring-capacity`

`client` mode requires a `wss://` remote URL. `server` mode requires both TLS paths.

## Release Notes

The first public release is intended to be source-first:

- GitHub source release
- CI-enforced `clippy` and `cargo test`
- operator guidance in [docs/OPERATORS.md](/home/zverev/Загрузки/newghost/docs/OPERATORS.md)

Binary packaging is intentionally deferred until the runtime surface is finalized.

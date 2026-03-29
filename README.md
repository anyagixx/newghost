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

For the verified local-workstation Telegram Desktop example, that local `127.0.0.1:1080` listener is reached through an SSH forward to the managed client host, not through a directly local `n0wss` bind:

- local operator workstation -> `ssh -L 127.0.0.1:1080:127.0.0.1:1080 root@$N0WSS_CLIENT_HOST`
- remote managed client host -> `n0wss-client` listener on `127.0.0.1:1080`

Bounded claim surface:

- supported claim: Telegram can be tested as a SOCKS5-aware application over the existing `n0wss` tunnel, and the repository now contains an explicit UDP-capable architecture for later Telegram calls verification
- unsupported claim: `n0wss` defines a Telegram-specific client protocol
- unverified claim: the newly added UDP-capable path already proves working Telegram Desktop voice or video calls before the dedicated calls wave runs
- unverified claim: `n0wss` guarantees bypass of every possible Telegram blocking regime or acts like a whole-device VPN

Verified envelope as of 2026-03-29:

- Telegram Desktop was exercised through a governed SOCKS5 path backed by the managed `n0wss` client and server services
- the verified local Desktop path explicitly includes the SSH forward to the remote managed client listener before Telegram is pointed at `127.0.0.1:1080`
- separate initial-connect and reconnect packets were observed on Telegram network IPs with anchored SOCKS5 parse, transport selection, bridge pump, and accepted WSS handshake evidence
- on rebuilt hosts, Telegram Desktop text messages, photo send, ordinary media send, and large-file transfer were green through that same governed path
- the rebuilt-host acceptance wave still left Telegram Desktop voice and video calls outside the proven envelope; it stalled at Telegram key exchange before any governed UDP-capable path existed
- the evidence applies to the tested desktop build and host setup only

Current calls-profile status:

- Phase-18 adds a governed UDP-capable architecture to the repository:
  `SOCKS5 UDP ASSOCIATE` ingress, datagram association ownership, a bounded WSS-backed datagram carrier, and server-side UDP relay helpers
- this is an implementation baseline for a later dedicated calls wave, not a retroactive proof that the old key-exchange failure is solved
- until the dedicated Telegram calls wave is run, Telegram Desktop voice and video calls remain under validation rather than green
- the current governed live calls handoff for that later wave is still the SSH-forwarded local Desktop path through `127.0.0.1:1080` to the managed client host at `178.104.104.208`, backed by the WSS server at `91.99.128.146:7443`
- the claim surface is still limited to the tested Desktop setup and must not be widened into universal unblock or all-network call support

The Telegram-specific verification wave is about client compatibility evidence, not about inventing a new app protocol.

## Release Notes

The current public release baseline is source-first patch `v0.3.2`:

- GitHub source release
- bounded rebuilt-host Telegram Desktop baseline
- operator guidance in [docs/OPERATORS.md](/home/truffle/Загрузки/newghost/docs/OPERATORS.md)
- explicit SSH-forwarded Desktop path through `127.0.0.1:1080`
- explicit no-go for voice and video call support until a later UDP-capable phase exists

GitHub publication shape:

- the release tag `v0.3.2` already points at the approved stable baseline
- the separate `master` branch sync step exists only so GitHub default-branch browsing shows the same baseline commit in the file tree view
- that sync publishes the released `v0.3.2` snapshot only and excludes newer local planning commits created after the release tag
- that branch sync is not a new release and does not add new runtime capability beyond `v0.3.2`

Standalone packaged distribution is still deferred, but the governed `n0wss` binary, managed deployment workflow, and operator runbook are already implemented in-repo.

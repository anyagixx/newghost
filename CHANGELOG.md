# Changelog

## [0.2.0] - 2026-03-28

Live-capable WSS proxy release with deployable runtime packaging.

Included in this release:

- Deployable `n0wss` binary entrypoint for server and client runtime modes
- Client-side TLS trust bootstrap for real `wss://` connectivity
- Long-lived runtime launch paths with graceful coordinated shutdown
- Target-aware WSS relay path that opens outbound TCP streams on the server side
- Two-host live-wave validation with success path, auth rejection path, and shutdown evidence
- GRACE governance expansion for crate root, transport wrapper, and helper-module graph coverage

Verification baseline:

- `cargo test`
- controlled live-wave execution for `Gate-Live-1`

Notes:

- GitHub source release is published from this tag by `.github/workflows/release.yml`.

## [0.1.0] - 2026-03-27

Initial GRACE-governed release candidate.

Included in this release:

- Foundation modules for config, observability, auth, TLS, and CLI bootstrap
- Transport adapters for WSS and iroh
- Session state machine, effect routing, registry, and transport selection
- SOCKS5 ingress with exact reply mapping
- Proxy bridge with queue-driven pumping and shutdown semantics
- Release-readiness documentation and GRACE integrity cleanup

Verification baseline:

- `cargo clippy --all-targets --all-features`
- `cargo test`

Notes:

- The repository is currently source-first. A standalone binary wrapper and broader deployment packaging are deferred to a later release.

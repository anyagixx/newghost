# Changelog

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

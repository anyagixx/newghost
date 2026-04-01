# Changelog

## [0.3.3] - 2026-04-02

Patch release for the deeper Telegram Desktop calls diagnosis after the published `v0.3.2` baseline.

Included in this release:

- Completed bounded Phase-47, Phase-48, and Phase-49 diagnosis for Telegram Desktop voice and video attempts on the preserved `SOCKS5 127.0.0.1:1080` path
- Narrowed the voice-call blocker from a generic post-handoff gap to `reply-path blocker`, then to `server-ingress blocker`, and finally to `eligibility absence`
- Added governed downstream, reply-path, and server-ingress trace anchors plus direct source-adjacent assertions in `cli`, `session`, `proxy_bridge`, and `wss_gateway` tests
- Synchronized GRACE shared artifacts with the completed calls-diagnosis waves and the planned `Phase-50` pre-eligibility branch

Verification baseline:

- `cargo test`
- `cargo test cli -- --nocapture`
- `cargo test datagram_manager -- --nocapture`
- `cargo test udp_relay -- --nocapture`
- `cargo test wss_gateway -- --nocapture`
- targeted `grace-refresh` after the Phase-50 planning and verification packet

Notes:

- This release does not claim green Telegram voice or video call support.
- The current honest calls verdict remains bounded: real media tuples and governed handoff are proven, but the first unresolved voice layer is still `eligibility absence` above accepted WSS handshake evidence.

## [0.3.2] - 2026-03-29

Baseline publication release for the rebuilt-host Telegram Desktop acceptance wave after `v0.3.1`.

Included in this release:

- Rebuilt-host acceptance evidence for the governed Telegram Desktop path through `ssh -L 127.0.0.1:1080:127.0.0.1:1080`
- Bounded final decision showing green text messaging, photo send, ordinary media send, and large-file transfer on the tested two-host environment
- Separated final evidence packet for readiness, basic acceptance, media success, and call-path failure
- Release baseline sync before the future UDP-capable Telegram calls architecture phase

Verification baseline:

- `Gate-Phase-14`
- bounded final Telegram Desktop acceptance packet on rebuilt hosts
- scoped GRACE integrity sync before the release baseline gate

Notes:

- This release publishes the current verified local baseline to GitHub; it does not claim Telegram voice or video call support.
- Telegram Desktop calls remain outside the proven envelope and are deferred to a later UDP-capable phase.

## [0.3.1] - 2026-03-28

Patch release for the Telegram Desktop forward clarification wave after the published `v0.3.0` baseline.

Included in this release:

- Explicit operator guidance for the verified local Telegram Desktop path through `ssh -L 127.0.0.1:1080:127.0.0.1:1080`
- Telegram verification contract updated so `LV-007` requires forward proof and forward-liveness triage before tunnel blame
- Knowledge graph synchronized with the clarified forwarded Desktop path and failure-classification order

Verification baseline:

- `Gate-Phase-12`
- targeted GRACE refresh for the Telegram forward clarification scope
- scoped GRACE review pass for the same scope

Notes:

- This is a patch-level documentation and verification correction release after `v0.3.0`; it does not introduce a new transport or deployment wave.

## [0.3.0] - 2026-03-28

Managed deployment and release-hardening update for the WSS proxy runtime.

Included in this release:

- Governed managed deployment script for role-aware staging to Linux hosts
- Managed `systemd` units for server and client runtime lifecycle
- Bounded log rotation policy for server, client, and bad-auth evidence files
- Operator runbook for install, restart, rollback, and evidence capture
- Second managed live wave with repeated smoke, restart recovery, and redacted bad-auth rejection evidence
- GRACE release-preparation governance for substantive test files, release-facing docs, and tag alignment

Verification baseline:

- `cargo clippy --all-targets --all-features`
- `cargo test`
- managed live verification for `Gate-Phase-9`

Notes:

- The next GitHub source release should be published from tag `v0.3.0` by `.github/workflows/release.yml`.

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

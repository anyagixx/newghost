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

## Secret Hygiene

Phase-9 forbids plaintext-oriented secret handling as part of the standard operator workflow.

Approved secret source types:

- root-readable env files stored on the host under `/opt/n0wss/env`
- transient shell environment variables provided through an operator-controlled session
- out-of-band secret delivery handled outside the repository and copied directly onto the host

Disallowed secret source types:

- repository-tracked files containing live auth values
- chat transcripts or tickets containing reusable raw secrets
- shell scripts with embedded live auth values, passwords, or private keys

Canonical managed secret surfaces:

| Secret Surface | Approved Location | Mode | Notes |
|---|---|---|---|
| server auth token | `/opt/n0wss/env/server.env` | `0600` | Loaded only by the server service or deploy workflow |
| client auth token | `/opt/n0wss/env/client.env` | `0600` | Loaded only by the client service or deploy workflow |
| private key | `/opt/n0wss/certs/server.key` | `0600` | Never copied into logs, docs, or reusable transcripts |

Redaction rules:

- describe secret source type or file path, not the secret value
- if a reject-path example needs evidence, show only redacted material such as masked prefixes
- never paste a live auth token, SSH password, GitHub token, or PEM private key into repository docs

Rotation follow-up is mandatory when a live secret was ever stored unsafely:

1. replace the leaked token or password
2. update the host-local env file with the new value
3. verify the old value is no longer accepted
4. remove the unsafe source artifact from the operator workflow

Operational rule:

- if an operator must choose between speed and secret hygiene, secret hygiene wins and the rollout waits

## Managed Service Workflow

This is the governed Phase-9 operator path. Do not bypass it with one-off `scp`, ad hoc `nohup`, or manual service definitions once managed deployment is in use.

### Install Or Update

Server role:

```bash
bash scripts/deploy-live.sh \
  --role server \
  --host <server-host> \
  --binary target/release/n0wss \
  --env-file /secure-inputs/server.env \
  --server-cert /secure-inputs/server.pem \
  --server-key /secure-inputs/server.key
```

Client role:

```bash
bash scripts/deploy-live.sh \
  --role client \
  --host <client-host> \
  --binary target/release/n0wss \
  --env-file /secure-inputs/client.env \
  --trust-anchor /secure-inputs/server.pem
```

Dry-run before the first managed rollout:

```bash
bash scripts/deploy-live.sh --dry-run --role client --host example.invalid --binary target/release/n0wss --env-file /secure-inputs/client.env --trust-anchor /secure-inputs/server.pem
systemd-analyze verify deploy/systemd/n0wss-server.service deploy/systemd/n0wss-client.service
logrotate -d deploy/logrotate/n0wss
```

### Service Lifecycle

Install units on the host from the governed files under `deploy/systemd/`, then reload the service manager:

```bash
sudo install -m 0644 deploy/systemd/n0wss-server.service /etc/systemd/system/n0wss-server.service
sudo install -m 0644 deploy/systemd/n0wss-client.service /etc/systemd/system/n0wss-client.service
sudo systemctl daemon-reload
```

Managed lifecycle commands:

```bash
sudo systemctl enable --now n0wss-server
sudo systemctl enable --now n0wss-client
sudo systemctl restart n0wss-server
sudo systemctl restart n0wss-client
sudo systemctl stop n0wss-server
sudo systemctl stop n0wss-client
```

### Evidence Capture

Use both service-manager evidence and runtime-anchor evidence:

```bash
systemctl is-active n0wss-server
systemctl is-active n0wss-client
systemctl show n0wss-server -p MainPID --no-pager
systemctl show n0wss-client -p MainPID --no-pager
journalctl -u n0wss-server -n 100 --no-pager
journalctl -u n0wss-client -n 100 --no-pager
tail -n 100 /var/log/n0wss-server.log
tail -n 100 /var/log/n0wss-client.log
tail -n 100 /var/log/n0wss-client-bad-auth.log 2>/dev/null || true
```

### Rollback

Rollback must be bounded and explicit:

1. stop the affected service with `systemctl stop`
2. restore the previous known-good binary or env file under the same canonical path
3. start the service again with `systemctl start`
4. capture `journalctl`, `systemctl show ... MainPID`, and bounded log tails before declaring rollback successful

Do not change the service name, working directory, or log target during rollback. If those need to change, it is not a rollback; it is a new GRACE plan update.

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

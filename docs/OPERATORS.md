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

Before opening the next GitHub release or handing the repository to external testers, run:

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
- governed `n0wss` binary target and managed deployment workflow are implemented in-repo
- GitHub docs and CI must stay aligned with the commands above

If docs or workflows mention `cargo run`, a binary name, or a smoke target that does not exist, treat that as release drift and block publication.

### GitHub Source Release

After `Gate-Phase-10` closes green, the prepared source-release path is:

```bash
git status --short
git tag v0.3.0
git push origin v0.3.0
```

Expected outcome:

- GitHub Actions workflow `.github/workflows/release.yml` runs on the pushed tag
- the published release body is sourced from `CHANGELOG.md`
- the published notes align with the `0.3.0` entry and the current source-first release posture

## Telegram Readiness

Phase-11 does not introduce a new Telegram protocol. The governed operator path is to expose the existing managed client runtime as a local SOCKS5 proxy and only then test Telegram against that listener.

### Telegram Proxy Settings

Use Telegram as a standard SOCKS5 client:

- host: `127.0.0.1`
- port: `1080` unless the managed client service is configured for a different governed SOCKS5 port
- proxy type: `SOCKS5`
- no Telegram-specific transport mode or custom `n0wss` protocol setting is required

### Managed Preflight

Do not start the Telegram app-side check until the managed client runtime is already healthy:

```bash
systemctl is-active n0wss-client
systemctl show n0wss-client -p MainPID --no-pager
ss -ltnp | grep ":1080" || true
journalctl -u n0wss-client -n 100 --no-pager
tail -n 100 /var/log/n0wss-client.log
```

Required readiness outcome:

- `n0wss-client` is active
- the local SOCKS5 listener is bound on the governed port
- startup evidence includes `[CliApp][run][BLOCK_START_APPLICATION]`
- startup evidence includes `[CliApp][runRuntime][BLOCK_RUN_CLIENT_MODE]`

If this preflight is not green, stop here. A Telegram-side retry without readiness proof is not valid GRACE evidence.

### Local Workstation Versus Remote Host

There are two valid operator shapes, and they are not interchangeable:

- local-workstation Telegram Desktop: Telegram runs on your own machine, while `n0wss-client` listens on the remote managed client host
- remote-host Telegram: Telegram runs directly on the same host where `n0wss-client` is listening

For the current verified Desktop example, use the first shape. That means `127.0.0.1:1080` is not available locally by itself. You must first forward your local port to the remote managed listener:

```bash
ssh -N -L 127.0.0.1:1080:127.0.0.1:1080 root@$N0WSS_CLIENT_HOST
```

Keep that SSH session open during the Telegram test. Before configuring Telegram, prove the local forwarded bind exists:

```bash
ss -ltnp | grep ":1080" || true
```

If Telegram is running directly on the remote client host, skip the SSH forward and point Telegram at the host-local listener there.

### Bounded Telegram App Check

After readiness is green:

1. if Telegram runs on the local workstation, start `ssh -N -L 127.0.0.1:1080:127.0.0.1:1080 root@$N0WSS_CLIENT_HOST` and keep it alive
2. if Telegram runs on the local workstation, confirm the forwarded local bind exists with `ss -ltnp | grep ":1080" || true`
3. configure Telegram Desktop to use SOCKS5 at `127.0.0.1:1080`
4. perform one initial connect attempt
5. capture a bounded client log tail and, when tunnel activity appears, a bounded server log tail
6. restart Telegram Desktop or trigger one reconnect attempt through the same SOCKS5 settings
7. if reconnect stalls at `Connecting` on the local workstation, verify the SSH forward is still alive before classifying a tunnel-side defect
8. capture a second bounded packet for reconnect

Do not blend the initial connect and reconnect evidence into one transcript.

### Final Acceptance Handoff

The current final-acceptance environment is:

- remote WSS server host: `91.99.128.146`
- remote managed client host: `178.104.104.208`
- local Telegram Desktop proxy target: `127.0.0.1:1080`
- proxy type: `SOCKS5`
- username: empty
- password: empty

The controlled operator sequence is:

1. wait until the controller explicitly says the environment is ready
2. on the local workstation, start and keep alive:

```bash
ssh -N -L 127.0.0.1:1080:127.0.0.1:1080 root@178.104.104.208
```

3. prove the local forwarded bind exists:

```bash
ss -ltnp | grep ":1080" || true
```

4. in Telegram Desktop set:
   - host: `127.0.0.1`
   - port: `1080`
   - proxy type: `SOCKS5`
   - no username
   - no password
5. perform the acceptance actions in this order:
   - basic connect and message send
   - photo send
   - large file send
   - voice call if available
   - video call if available
6. if Telegram shows `Connecting`, do not change settings first; verify the `ssh -N -L ...` process is still alive
7. report each result in order so the controller can keep separate evidence packets

The controller must not ask the operator to start Telegram testing before service readiness, local forward proof, and the pre-handoff smoke are already green.

Observed rebuilt-host acceptance outcome on 2026-03-29:

- basic connect and text messages: green
- photo and ordinary media send: green
- large-file transfer: green
- voice and video calls: not green; the call reached ringing and answer state but then stayed at Telegram key exchange
- classification: basic SOCKS5 or TCP tunnel behavior stayed healthy while the call-specific media path remained outside the currently proven envelope

This observed outcome is the operator-facing baseline for release `v0.3.2` and must not be described as working Telegram call support.

Current Telegram calls profile during Phase-18:

- the repository now contains a governed UDP-capable path for later call validation:
  `SOCKS5 UDP ASSOCIATE`, datagram association ownership, a bounded WSS-backed datagram carrier, and server-side UDP relay helpers
- this changes the architecture baseline, but it does not by itself convert calls into a verified green path
- public wording must still separate the already green text or file envelope from the new UDP media envelope that still needs a dedicated verification wave
- until `LV-009 TelegramCallsWave` is executed, voice and video calls remain under validation for the tested Telegram Desktop setup

### Phase-19 Live Calls Handoff

The current live calls environment is:

- remote WSS server host: `91.99.128.146`
- remote managed client host: `178.104.104.208`
- remote server listener: `0.0.0.0:7443`
- remote managed client SOCKS5 listener: `127.0.0.1:1080`
- local Telegram Desktop proxy target: `127.0.0.1:1080`
- proxy type: `SOCKS5`
- username: empty
- password: empty

The controlled operator sequence for `LV-010` is:

1. wait until the controller explicitly says the live calls environment is ready
2. on the local workstation, start and keep alive:

```bash
ssh -N -L 127.0.0.1:1080:127.0.0.1:1080 root@178.104.104.208
```

3. prove the forwarded local bind exists:

```bash
ss -ltnp | grep ":1080" || true
```

4. in Telegram Desktop set:
   - host: `127.0.0.1`
   - port: `1080`
   - proxy type: `SOCKS5`
   - no username
   - no password
5. run one voice call and one video call as separate attempts
6. after the first completed or failed call attempt, run one fresh second call attempt through the same SOCKS5 settings
7. if the call UI reaches ringing or answer state, do not treat that as final success by itself; report it as signaling evidence only
8. if Telegram shows `Connecting` or the call stalls, verify the `ssh -N -L ...` process is still alive before changing settings
9. report voice, video, and second-call outcomes separately so the controller can keep separate packets

The controller must not ask the operator to start the live calls wave before fresh-host baseline, governed deploy, service readiness, and pre-handoff smoke are already green.

Observed live calls outcome on 2026-03-29:

- voice call: not green
- video call: not green
- repeated call attempt: not green
- user-visible symptom: call stays at Telegram key exchange
- bounded classification: Telegram signaling remained active on the already proven TCP path, but the live logs did not show
  `[Socks5Proxy][handleUdpAssociate][BLOCK_HANDLE_UDP_ASSOCIATE]`
  or any later datagram markers
- first divergent layer: UDP ingress, before datagram transport selection or UDP relay

This means the new UDP-capable repository baseline is present on the hosts, but the tested Telegram Desktop setup still did not yield a governed SOCKS5 UDP ASSOCIATE trajectory during the live wave.

Observed post-fix rerun outcome on 2026-03-29:

- repaired baseline `5d8b598` was redeployed to both managed hosts
- raw post-fix probe on `ghost-cli` succeeded for `SOCKS5 UDP ASSOCIATE` and produced
  `[Socks5Proxy][handleUdpAssociate][BLOCK_HANDLE_UDP_ASSOCIATE]`
- repeated local-workstation Telegram Desktop voice and video calls still stayed at Telegram key exchange
- during those SSH-forwarded Desktop reruns, live logs still did not show governed UDP markers
- bounded inference: the ingress defect is fixed on the managed client runtime, but the current SSH-forwarded local Desktop operator path is still not a proven end-to-end calls path

Current topology target after Phase-20:

- historical packet: local Telegram Desktop -> `ssh -L 127.0.0.1:1080:127.0.0.1:1080` -> remote managed `n0wss-client`
- next execution target: local Telegram Desktop -> truly local `n0wss-client` listener on the same workstation
- bounded reason for the topology change: the managed client runtime now proves `SOCKS5 UDP ASSOCIATE` in isolation, so the next wave must remove the SSH-forwarded operator path from the calls trajectory before blaming deeper datagram code or the external network
- until the local-client topology wave is executed, do not treat the old SSH-forwarded Desktop path as the default calls runbook anymore

### Phase-21 Local Client Runbook Target

The next Telegram calls wave must use this operator shape:

- Telegram Desktop and `n0wss-client` run on the same local workstation
- Telegram Desktop points to a truly local SOCKS5 listener on `127.0.0.1:1080`
- the older `ssh -L 127.0.0.1:1080:127.0.0.1:1080 ...` path is not part of the new calls trajectory
- the old SSH-forwarded packet remains historical comparison evidence only

Operator boundaries for the next wave:

1. do not keep the old `ssh -L` forward alive while testing the local-client topology
2. prove that `127.0.0.1:1080` is owned by the local `n0wss-client` process before opening Telegram
3. keep Telegram Desktop on SOCKS5 `127.0.0.1:1080` with no username and no password
4. run voice and video attempts only after the controller confirms local raw `UDP ASSOCIATE` readiness on the same workstation

### Phase-21 Local Client Bootstrap Shape

The next wave uses one bounded local bootstrap:

- local binary: the current `n0wss` build on the Telegram Desktop workstation
- local auth source: one local env file exporting `N0WSS_AUTH_TOKEN`
- local remote endpoint: one `N0WSS_REMOTE_WSS_URL`
- local trust anchor: one local copy of the server trust anchor
- local listener target: `127.0.0.1:1080`

Planned local launch shape:

```bash
set -a
source /secure-inputs/client.env
set +a

./target/release/n0wss \
  --auth-token "$N0WSS_AUTH_TOKEN" \
  client \
  --listen-addr 127.0.0.1:1080 \
  --remote-wss-url "$N0WSS_REMOTE_WSS_URL" \
  --tls-trust-anchor-path /secure-inputs/server.pem
```

Planned local cleanup shape before reruns:

```bash
pkill -f 'n0wss .* client' || true
ss -ltnp | grep ':1080' || true
```

Bootstrap boundaries:

1. do not start more than one local `n0wss-client` for the calls wave
2. do not reuse a stale listener from an older local experiment
3. keep the trust anchor local and explicit instead of silently reusing remote host paths
4. if local bootstrap changes, treat it as a new readiness packet before opening Telegram

Observed local readiness packet on 2026-03-29:

- old `ssh -L` listener was removed before the local-client wave
- local `n0wss-client` bound `127.0.0.1:1080` on the Telegram Desktop workstation
- local TCP smoke through `curl --proxy socks5h://127.0.0.1:1080 https://example.com -I` returned `HTTP/2 200`
- local raw `SOCKS5 UDP ASSOCIATE` probe returned a success reply and allocated a governed relay bind
- bounded classification: the next local calls wave may now treat listener ingress as green and must promote the next missing datagram marker if calls still fail

Observed first local-client topology outcome on 2026-03-29:

- the old `ssh -L` path stayed disabled during the test
- local `n0wss-client` owned `127.0.0.1:1080` on the Telegram Desktop workstation
- local process logs showed fresh SOCKS5 requests toward Telegram IPs under `149.154.*`
- remote WSS server accepted fresh handshakes from the workstation IP `188.255.118.217`
- user-visible symptom: Telegram Desktop still showed `Connecting`, and voice or video calls did not go green
- no governed UDP markers were observed during that local-client wave
- bounded comparison result: removing the SSH-forwarded operator topology did not widen the calls evidence envelope by itself

### Telegram Calls Wave Runbook

Use this runbook only after the normal Telegram Desktop SOCKS5 path is already green for text and file traffic on the same setup.

Preflight for the calls wave:

1. confirm `n0wss-client` and `n0wss-server` are both active before opening Telegram
2. for the local-workstation Desktop shape, start and keep alive:
   `ssh -N -L 127.0.0.1:1080:127.0.0.1:1080 root@$N0WSS_CLIENT_HOST`
3. prove the forwarded local listener exists before the call attempt:
   `ss -ltnp | grep ':1080' || true`
4. keep Telegram Desktop on SOCKS5 `127.0.0.1:1080` with no username and no password

Execution order for `LV-009 TelegramCallsWave`:

1. start one voice call or video call and note whether ringing and answer state appear
2. while the call is progressing, capture logs for:
   `[Socks5Proxy][handleUdpAssociate][BLOCK_HANDLE_UDP_ASSOCIATE]`
   `[DatagramSessionManager][openAssociation][BLOCK_OPEN_DATAGRAM_ASSOCIATION]`
   `[DatagramTransportSelector][selectTransport][BLOCK_SELECT_DATAGRAM_TRANSPORT]`
   `[WssDatagramGateway][sendDatagram][BLOCK_SEND_WSS_DATAGRAM]`
   `[UdpEgressRelay][relayOutbound][BLOCK_RELAY_UDP_OUTBOUND]`
   `[UdpEgressRelay][relayInbound][BLOCK_RELAY_UDP_INBOUND]`
3. separate signaling green from media green:
   signaling green means the call reaches ringing or answer state
   media green means UDP association, transport, outbound relay, and inbound relay markers all appear for the same association
4. end the first call, wait for cleanup, then trigger one more call attempt through the same SOCKS5 settings
5. on the second call, prove a fresh UDP association is opened instead of silently reusing stale state

If the call stalls:

- first verify the SSH forward is still alive for the local-workstation variant
- if signaling is green but no UDP ASSOCIATE marker appears, classify the first divergent layer at UDP ingress
- if UDP ASSOCIATE appears but no datagram transport marker follows, classify the first divergent layer at datagram transport selection
- if outbound relay appears without inbound relay, classify the first divergent layer at remote relay or remote media path
- if a raw managed-host UDP ASSOCIATE probe is green but the SSH-forwarded local Desktop wave still produces no UDP marker, classify the first divergent layer at the current operator topology before changing the transport core again

Repository publication note:

- tag `v0.3.2` already captures this approved baseline
- a later push of `master` is only a GitHub branch-visibility sync so the default branch shows the same stable tree
- that sync must publish the released `v0.3.2` snapshot only and must exclude newer local planning commits that were created after the tag
- that branch sync must not be described as a new runtime wave or a new release beyond `v0.3.2`

### Telegram Evidence Packet Shape

For Telegram calls keep four separable packets:

- call-setup packet:
  call type, local forward state when applicable, whether ringing and answer state appeared, and the first appearance of `[Socks5Proxy][handleUdpAssociate][BLOCK_HANDLE_UDP_ASSOCIATE]`
- media-flow packet:
  association id, datagram transport marker, WSS datagram marker, outbound relay marker, inbound relay marker, and whether two-way media was actually observed
- reconnect packet:
  second call attempt summary, fresh local forward proof when applicable, and evidence that a fresh UDP association was opened instead of reusing stale state
- call-failure packet:
  expected evidence, observed evidence, first divergent block, and the exact next repair action

Calls-packet templates for `LV-009`:

- call-setup packet:
  expected evidence: Telegram call reaches ringing or answer state and opens one governed UDP association
  observed evidence: operator call summary, local forward liveness, client log tail, server log tail, and the first UDP ASSOCIATE marker if present
  first divergent block: first missing UDP ASSOCIATE marker after signaling turns green
- media-flow packet:
  expected evidence: the same association produces datagram transport selection, WSS datagram send, outbound relay, and inbound relay markers
  observed evidence: association id, outbound marker state, inbound marker state, and operator note about real media
  first divergent block: first missing datagram transport, outbound relay, or inbound relay marker
- reconnect packet:
  expected evidence: a second call attempt opens a fresh UDP association after the first call ends
  observed evidence: second-call action summary, fresh local forward proof, fresh logs, and the new association id
  first divergent block: first sign of stale association reuse or missing second association-open marker
- call-failure packet:
  expected evidence: signaling green, UDP ASSOCIATE green, datagram transport green, and bounded media-path evidence
  observed evidence: Telegram result, forward state, association state, datagram logs, relay logs, and operator media note
  first divergent block: first missing UDP ingress, datagram transport, outbound relay, inbound relay, or app-side media confirmation
  next action: repair only that first divergent layer before replaying the calls wave

Final rebuilt-host packet on 2026-03-29:

- readiness packet:
  server and client services were green on the rebuilt hosts before handoff, with managed listeners and startup anchors already confirmed
- basic-acceptance packet:
  Telegram Desktop connected through the governed SSH-forwarded SOCKS5 path and text-message delivery was green
- media-success packet:
  photo send, ordinary media send, and large-file transfer were green
- call-failure packet:
  expected evidence: voice and video calls complete over the same governed path
  observed evidence: call ringing and answer state succeeded, but Telegram stayed at key exchange while message and file paths remained green
  first divergent block: the old bounded envelope stopped at the CONNECT-oriented SOCKS5 or TCP path and did not yet prove the later call media path
  next action: use the new UDP-capable Phase-18 architecture and run the dedicated Telegram calls wave instead of replaying the already green TCP or SOCKS5 acceptance path

Classification rule:

- if Telegram never triggers `[Socks5Proxy][parseRequest][BLOCK_PARSE_SOCKS5_REQUEST]`, first check whether the SSH forward is still alive for the local-workstation variant, then classify the failure as app-side misconfiguration, forward-liveness failure, or readiness-side failure
- if SOCKS5 parse appears but no `[SessionManager][resolveStream][BLOCK_SELECT_TRANSPORT]` follows, classify the divergence at transport resolution
- if transport selection appears but no `[ProxyBridge][pumpBidirectional][BLOCK_PUMP_BIDIRECTIONAL]` follows, classify the divergence at the bridge or remote path

Bounded claim rule:

- a green Telegram wave proves SOCKS5 client compatibility for the tested Telegram build and host setup
- a future green Telegram calls wave would need separate UDP media evidence before voice or video calls can be described as supported
- it does not prove universal unblock behavior across all Telegram builds, all networks, or all blocking regimes
- the current verified example is Telegram Desktop on the local operator workstation routed into the managed client host listener through a governed local SOCKS5 forward

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

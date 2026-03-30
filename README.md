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
- the dedicated live Telegram calls wave on 2026-03-29 still did not reach a green calls result on the tested setup: signaling stayed on the proven TCP path, but no governed UDP ingress markers appeared during the observed key-exchange stall
- the current governed live calls handoff for that later wave is still the SSH-forwarded local Desktop path through `127.0.0.1:1080` to the managed client host at `178.104.104.208`, backed by the WSS server at `91.99.128.146:7443`
- the post-fix rerun on 2026-03-29 proved that live `SOCKS5 UDP ASSOCIATE` ingress now works on the managed client host itself, but the SSH-forwarded local Desktop call attempts still did not emit governed UDP markers
- inference from that bounded evidence: the old ingress defect is fixed, but the current local-workstation `ssh -L 127.0.0.1:1080:127.0.0.1:1080` handoff is still not a proven end-to-end Telegram calls path
- Phase-21 changes the target verification topology: the next calls wave must use a truly local `n0wss-client` on the Telegram Desktop workstation instead of the historical SSH-forwarded path
- the older SSH-forwarded Desktop packet remains valid historical evidence and comparison baseline; it is not the next execution target
- operator runbook consequence: the next calls wave must start with a truly local SOCKS5 listener on the Telegram Desktop workstation and must not keep the old `ssh -L` path enabled in parallel
- the next bootstrap shape is intentionally bounded: one local `n0wss-client` process, one local trust anchor, one local SOCKS5 listener, and explicit cleanup before reruns
- the first local-client topology wave on 2026-03-29 proved that Telegram Desktop did reach the truly local `n0wss-client` path: local `127.0.0.1:1080` was owned by `n0wss`, local Telegram generated SOCKS5 requests toward Telegram IPs, and the remote WSS server accepted handshakes from the workstation IP
- changing the operator topology still did not produce governed UDP markers or a green Telegram session, so the earlier no-go is no longer attributable only to the old SSH-forwarded path
- Phase-22 freezes the remaining bounded hypotheses after that result:
  Telegram Desktop may still avoid the proxy-governed UDP media path, may bypass or stall before media on the tested build, or may still hit a deeper governed datagram or remote-media boundary that is not visible from the UI alone
- external filtering remains only a later hypothesis; it is not allowed to become the leading explanation before a controlled UDP probe and remote-media reachability packet are captured
- the completed Phase-22 packet on 2026-03-29 narrowed the boundary further:
  a bounded live call capture showed TCP traffic to the local SOCKS5 listener and a live WSS uplink to `91.99.128.146:7443`, but no UDP on local port `1080` during the Telegram call attempt
- the same phase proved that controlled UDP ingress is alive and outbound probe traffic can be sent, but the controlled round trip is still incomplete:
  `scripts/udp_probe.sh` returned `probe_status=association-ok`, `outbound_result=sent`, and `inbound_result=timeout`, while a bounded remote capture on UDP port `55123` saw zero packets
- bounded root-cause classification after Phase-22:
  Telegram Desktop calls on the tested setup remain outside the proven proxy-governed UDP envelope, and the governed datagram path still lacks a proven inbound round trip
- Phase-23 now narrows the next diagnostic boundary further:
  the next wave is not a Telegram UI wave, but a controlled datagram round-trip wave that must separate four layers explicitly:
  `UDP ASSOCIATE` success, outbound datagram emission, remote echo-target ingress, and inbound reply return
- this keeps the next blocker classification transport-scoped:
  if the remote echo target still sees no packet, the unresolved layer remains inside the controlled datagram path; if remote ingress appears but the probe still times out, the unresolved layer moves to inbound return rather than back to Telegram UI
- current bounded blocking boundary after the first Phase-23 packet:
  local `UDP ASSOCIATE` negotiation and governed relay-bind allocation are green, but the first still-unresolved datagram layer remains before any proven remote echo-target ingress and therefore before any inbound reply return
- current bounded next-step decision after the same packet:
  the next phase should be a datagram-path repair or deeper relay-probe wave, because the controlled packet still stops before proven remote echo-target ingress; another Telegram Desktop rerun on the same setup would still be blind
- external filtering is still not the first unresolved layer; the next phase should isolate datagram round-trip behavior and app-specific media handoff before any network-workaround phase is approved
- Phase-24 now narrows that next step further into a repair-only wave:
  the allowed scope is the transport segment between proven local UDP ingress and the first proven remote echo-target ingress
- Phase-24 must not reopen Telegram UI diagnosis while that repair scope is unresolved:
  the expected progression is local dispatch evidence, WSS datagram emission evidence, server-side relay outbound evidence, and only then remote echo-target ingress or inbound reply evidence
- `outbound_result=sent` remains a bounded local outcome until one of those deeper layers is proven for the same controlled probe packet
- the stopped Phase-24 wave on 2026-03-29 narrowed the blocker again:
  repaired helper surfaces now exist for local manager handoff, selector emit, and relay outbound, but the live runtime still does not call them
- bounded runtime-glue classification after that stop packet:
  there is still no proven production `WssDatagramPath`, no proven live client UDP receive loop feeding the manager, and no proven server-side runtime datagram ingress
- Phase-25 is therefore not another datagram diagnosis wave and not a Telegram rerun:
  it is a runtime-glue wave that must wire the repaired helpers into the real client and server path before any new controlled probe or Telegram-specific rerun is treated as meaningful
- the completed Phase-25 runtime-glue wave on 2026-03-29 advanced the controlled packet beyond that stopped boundary:
  live client logs now prove `BLOCK_PARSE_UDP_DATAGRAM`, `BLOCK_FORWARD_OUTBOUND_DATAGRAM`, and `BLOCK_SEND_WSS_DATAGRAM`, while live server logs prove `SERVER_DATAGRAM_RECEIVED` and `BLOCK_RELAY_UDP_OUTBOUND`
- the same bounded rerun still ended with `inbound_result=timeout`, so the post-glue decision remains narrow:
  outbound runtime glue is now proven through server-side relay outbound, but inbound reply return is still unresolved and a new Telegram-specific calls wave would still be premature
- Phase-26 is therefore not another Telegram or UI-facing verification wave:
  it is an inbound-return wave that must separately prove server-side reply receive, WSS return emission, client-side local delivery, and only then `reply-received`
- the Phase-26 baseline is locked to the completed Phase-25 tuple:
  `SERVER_DATAGRAM_RECEIVED`, `BLOCK_RELAY_UDP_OUTBOUND`, and `inbound_result=timeout`
- the completed Phase-26 inbound-return wave on 2026-03-29 tightened the boundary further with two bounded packets:
  the public-IP probe to `91.99.128.146:55123` still timed out with no echo-target ingress, while the loopback probe to `127.0.0.1:55123` on the server host proved deeper progress because the echo target received `phase26-loopback`
- the same loopback packet still ended with `inbound_result=timeout`, and no deeper inbound markers appeared:
  no `SERVER_DATAGRAM_INBOUND_RECEIVED`, no `SERVER_DATAGRAM_RETURN_EMITTED`, no `BLOCK_RELAY_UDP_INBOUND`, no `BLOCK_FORWARD_INBOUND_DATAGRAM`, and no `BLOCK_DELIVER_INBOUND_DATAGRAM`
- the bounded Phase-26 decision therefore stays transport-scoped:
  outbound delivery to a deterministic server-local echo target is now proven, but the first unresolved layer remains the server-side inbound receive and return-emission path, so another Telegram-specific wave is still blocked
- the completed Phase-27 server-inbound-return wave on 2026-03-29 moved the controlled packet beyond that boundary:
  after redeploying binary `ce49836d3c19a3b881927e97653f001f936ea649d0337fb3aa8ee1e767535b15` to `ghost-srv` and `ghost-cli`, the bounded live `phase27-probe` to `127.0.0.1:55123` produced `probe_status=reply-received`
- the same bounded evidence packet is now fully anchored end to end:
  server-local echo evidence recorded `received=b'phase27-probe'` and `replied_to=...`, live server logs recorded `SERVER_DATAGRAM_INBOUND_RECEIVED` plus `SERVER_DATAGRAM_RETURN_EMITTED`, and live client logs recorded `BLOCK_DELIVER_INBOUND_DATAGRAM`
- the bounded Phase-27 decision is therefore narrower and greener than the old transport diagnosis:
  the controlled datagram round-trip is now proven through reply return, so the next justified wave is a new Telegram-specific verification phase rather than another datagram repair
- the new post-Phase-27 Telegram calls boundary is therefore app-facing, not transport-facing:
  any fresh voice, video, or reconnect failure on the same deployment window must be classified against a transport-green baseline rather than reopening generic datagram diagnosis
- the next Telegram calls rerun is allowed only when the same live window still proves one bounded `phase27-probe` packet with `reply-received` before the first manual call attempt
- the next calls decision must stay explicitly split if needed:
  voice, video, and reconnect may diverge on the tested Telegram Desktop setup even after the controlled datagram round-trip is green
- the completed Phase-28 evidence packet on 2026-03-29 now stays explicitly separated from the older pre-Phase-27 no-go waves:
  readiness kept the same-window `phase27-probe` green, voice reached answer plus key-exchange emoji before falling back to `Соединение...`, video stalled at `Обмен ключами шифрования...` and dropped, and the reconnect audio attempt repeated the same key-exchange symptom
- the comparison against the older calls no-go waves is now narrower:
  before Phase-27 the first unresolved layer still overlapped generic transport uncertainty, but the new Phase-28 packet keeps the failure boundary app-facing because the same deployment window already preserved the green controlled datagram round-trip
- the completed Phase-28 decision on 2026-03-29 therefore stays bounded and non-green for the tested Telegram Desktop setup:
  voice, video, and reconnect all remained below a green media packet even though the transport baseline stayed green in the same window
- the remaining blocker is now classified as app-facing rather than transport-facing:
  voice advanced far enough to show answer plus key-exchange emoji before `Соединение...`, while video and reconnect still stalled at key exchange; none of those packets justify reopening generic datagram repair
- the next approved diagnostic boundary after Phase-28 is therefore narrower and explicitly Telegram-specific:
  controlled datagram transport stays green, while the tested Desktop setup still needs a media-behavior phase that explains how calls fail above that transport baseline
- the new hypothesis packet must preserve both sides at once:
  the same deployment window already proved one bounded green `phase27-probe`, and the completed Phase-28 calls packet still ended as app-facing no-go for voice, video, and reconnect
- the completed Phase-29 comparison packet on 2026-03-30 is narrower than Phase-28 without becoming greener:
  both bounded media packets now reused the exact Desktop handoff profile, preserved their own capture packets, and converged on the same `signaling-only stall` class above the green transport baseline
- the bounded Phase-29 decision therefore still remains no-go for the tested Desktop setup:
  the preserved transport-green packet stays intact, but the media-behavior packet now points to a Telegram-specific workaround or alternate app-topology phase rather than another generic transport repair
- the bounded Phase-30 workaround hypothesis now freezes that next branch more tightly:
  generic datagram transport remains closed as green baseline evidence, and the only approved next question is whether one alternate app topology changes the Telegram no-go class on the tested Desktop setup
- the chosen alternate topology for Phase-30 is intentionally narrow:
  Telegram Desktop will return to a truly local `n0wss-client` listener on the same workstation instead of the exact Phase-29 SSH-forwarded Desktop route, while the same-window `phase27-probe` precondition stays mandatory
- the comparison rule remains explicit:
  if that alternate topology still reproduces the same media no-go class, the result strengthens the Telegram-specific boundary rather than reopening transport work
- the completed Phase-30 workaround packet on 2026-03-30 now answers that exact question:
  the alternate topology was real, the same-window controlled baseline stayed green, but both alternate voice and alternate video still stalled at `Обмен ключами шифрования`
- the bounded Phase-30 decision therefore stays narrow and non-green:
  switching from the Phase-29 SSH-forwarded Desktop route to a truly local `n0wss-client` listener did not change the Telegram no-go class for the tested Desktop setup
- the next justified branch is now narrower than alternate topology:
  future work must target a more Telegram-specific workaround above the preserved green transport baseline, not another generic transport repair and not a repeat of the same topology swap
- the bounded Phase-31 deeper-workaround hypothesis now freezes that branch even tighter:
  Phase-29 and Phase-30 already proved that neither the SSH-forwarded Desktop route nor the truly local `n0wss-client` route changed the Telegram no-go class, so future work must stay inside one Telegram-specific app variant at a time
- the preserved green baseline still stays separate from that workaround branch:
  the same tested setup keeps text messages, media files, large files, and the controlled datagram `reply-received` packet as already-green evidence; Phase-31 must not widen its edits into generic proxy, transport, or file-transfer regressions
- the next decision is therefore variant-only:
  each new wave may change exactly one Telegram-specific app variable, then compare that packet directly against the completed Phase-29 and Phase-30 no-go packets
- the blocked Phase-31 Desktop packet is now part of the historical baseline:
  the tested Desktop build exposes no separate calls-proxy toggle, so that app variant is unavailable rather than transport-broken
- the next justified branch is now mobile-only:
  future workaround work may move to a Telegram Mobile build with an explicit `Use proxy for calls` toggle, but any result there must stay separate from Desktop claims
- the bounded Phase-32 mobile packet is now also no-go:
  on the tested Android setup with a dedicated LAN-facing listener at `192.168.31.241:11080` and `Use proxy for calls = enabled`, Telegram Mobile connected to SOCKS5 but text messages became high-latency, media files no longer sent or received, and both voice and video stalled at key exchange without a green media path
- the mobile comparison stays strict:
  the chosen mobile variant changed the no-go class for neither voice nor video, and it degraded the ordinary app path relative to the preserved Desktop envelope instead of improving it
- the next decision therefore stays narrow:
  the tested mobile calls-proxy variant is still no-go and must not widen into any support claim for Desktop, Android generally, or generic proxy compatibility
- the bounded Phase-33 terminal matrix now freezes all spent Telegram branches together:
  tested Desktop media-behavior no-go from Phase-29, tested Desktop alternate-topology no-go from Phase-30, blocked Desktop-only app variant from Phase-31, and tested Android mobile calls-proxy no-go from Phase-32 are now one exact historical matrix above the preserved green transport baseline
- the bounded Phase-33 branch screen is now exhausted for the tested variants:
  no genuinely new Telegram-specific branch remains without changing the tested app family, build family, or another major operator variable, so another workaround rerun on the same Desktop or Android variants would only spend more manual effort on an already-spent branch
- the explicit stop criteria are therefore frozen:
  stop Telegram workaround exploration whenever a proposed next wave would only replay the tested Desktop route, the tested truly local Desktop route, the unavailable Desktop-only calls-proxy toggle, or the tested Android `Use proxy for calls = enabled` route without a genuinely new bounded variable
- the bounded Phase-33 final decision on 2026-03-30 is terminal for the tested variants:
  stop workaround exploration as no-go for the tested Telegram Desktop and tested Android variants, keep the green transport baseline as already proven, and do not reopen generic transport diagnosis
- the safe operator end-state is also frozen:
  keep only the preserved Desktop listener on `127.0.0.1:1080`, do not leave any temporary LAN-facing mobile listener running, and treat future Telegram work as a fresh branch only if it starts from a genuinely new bounded variant rather than another rerun of the spent variants
- the bounded Phase-34 attribution hypothesis now narrows the next question again without reopening any spent workaround branch:
  on the preserved Desktop baseline, Telegram voice and video still reach key exchange but the evidence set still does not say whether media next attempts the governed SOCKS path, a direct path outside SOCKS, or no real media path at all
- the attribution phase therefore stays strictly above the green transport baseline:
  it does not repair generic datagram transport, does not retry the old Desktop or Android workaround variants, and does not change the already-working `127.0.0.1:1080` path for text messages, media files, or large files
- the only approved new deliverable is attribution evidence:
  separate workstation and server packets must explain where the first positive media-path evidence appears, or honestly classify the result as `insufficient evidence`
- the bounded Phase-34 Desktop voice-attribution packet on 2026-03-30 is now recorded:
  during one preserved-baseline Desktop voice call the UI progressed through `Запрос` -> `Вызов` -> `Обмен ключами шифрования` and then dropped, workstation loopback and uplink captures both stayed busy on the governed `127.0.0.1:1080` and WSS `91.99.128.146:7443` surfaces, but the broader workstation capture also showed fresh direct UDP attempts to Telegram-owned `91.108.*` addresses on ports `598`, `599`, and `1400` while the server-side correlation packet stayed limited to new WSS handshakes without fresh datagram markers such as `SERVER_DATAGRAM_RECEIVED` or `SERVER_DATAGRAM_INBOUND_RECEIVED`
- the current best bounded reading for that voice packet is therefore attribution, not repair:
  first positive non-UI evidence appeared outside the governed SOCKS envelope, so the packet currently points to candidate direct-media behavior outside SOCKS rather than another generic transport defect on the already-working Desktop baseline
- the bounded Phase-34 Desktop video-attribution packet on 2026-03-30 converged on the same class:
  the UI progressed through `Запрос` -> `Вызов` -> `Обмен ключами шифрования` -> `Ошибка соединения` and then dropped, the same bounded window again showed fresh WSS traffic to `91.99.128.146:7443` plus fresh direct traffic to Telegram infrastructure such as `91.108.56.*` and `149.154.167.*`, while the server-side correlation packet still showed only fresh WSS handshakes without fresh governed datagram markers
- the bounded Phase-34 classifier is now strong enough to stop guessing:
  both voice and video packets point to `direct-media outside SOCKS`, not to `governed-media attempt`, not to `signaling-only stall`, and not to generic n0wss transport failure on the preserved Desktop baseline
- the bounded Phase-34 attribution decision is therefore narrower than a new repair phase:
  the next justified branch is only a Telegram-specific/app-behavior branch that explains or works around direct media outside the governed SOCKS envelope; no generic transport repair and no replay of the spent Desktop or Android workaround branches is justified by the current evidence
- the old Phase-35 forced-topology packet is now explicitly superseded as a contract mismatch:
  the isolated namespace helper proved only SOCKS-only containment with blocked direct egress, not true transparent forced routing, so that packet must not be reused as if it had already tested transparent interception of Telegram media
- the bounded Phase-36 transparent-routing branch is now the only justified topology follow-up:
  because the current classifier still points to `direct-media outside SOCKS`, the next valid experiment must preserve the normal `127.0.0.1:1080` Desktop baseline for text messages and files while proving transparent interception, local governed handoff, and fresh attribution evidence inside an isolated Telegram-specific routing surface
- the first blocked Phase-36 packet has now narrowed the next blocker one level deeper:
  the current system already has isolated netns launch, preserved Desktop baseline, and governed SOCKS/WSS transport, but it still lacks one explicit transparent interception helper surface between isolated Telegram egress and the governed local handoff, so the next justified branch is helper-only rather than another blind Telegram rerun or a generic transport repair
- the old Phase-37 helper bootstrap branch is now explicitly unsafe and superseded:
  after the 2026-03-30 logout incident, copied live Telegram Desktop session state is no longer an allowed experiment surface on this workstation, so no future calls branch may clone or reuse the authenticated `tdata` from the ordinary Desktop profile
- the ordinary Telegram baseline remains the only approved live operator path:
  keep the already-working Desktop path through `127.0.0.1:1080` for text messages, media files, and large files, and treat any temporary MTProto or other recovery proxy used only to restore the ordinary Telegram account after provider blocking as operator recovery evidence rather than calls-branch progress
- the new Phase-38 branch is therefore calls-only and safety-first:
  it does not try to re-prove generic Telegram reachability, does not weaken the ordinary message or file path, and does not spend any new voice or video packet until one separate safe experiment window exists without live-session cloning
- the bounded Phase-38 auth and readiness contract is now fixed before any new calls attempt:
  no safe calls packet is valid while a candidate experiment window still shows `Connection...`, `Reconnecting to proxy`, a QR or login bootstrap screen, or an unstable partial-dialog surface; only a stable dialog-ready safe window may advance to the next voice or video packet
- the old Phase-24 tail is now explicitly superseded:
  helper-level repair rerun, repair evidence, and repair decision no longer define the next execution queue because the first unresolved layer has already moved deeper into inbound return
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

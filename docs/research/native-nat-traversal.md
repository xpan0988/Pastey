# Native cross-NAT connectivity for Pastey — technical decision report

Status: research and decision, 2026-10-12, kept as the record of why iroh was chosen and what was measured. **Historical:** the internet carrier now exists, and [Connectivity](../layers/layer-4-connectivity.md) is the current design. It differs from this report in four ways:
- **Relay fallback carries Bridge traffic.** There is no direct-only gate (§5 item 6, §7 D): the product goal became "works across networks, direct where possible". The UI says which path is in use.
- **No separate signature on the binding.** The carrier card travels as Room Control `bridge_membership.carrier_card` and is trusted because that session's keys were established by the pinned device key (§5 item 2).
- **Address lookup with n0's relays.** With the default relays the endpoint publishes its home relay on n0's DNS so a peer whose relay changed is still found; with a custom relay nothing is published (§5 item 1, §6 Metadata).
- **The setting is device-wide** under Settings → General, not per Bridge (§5 item 7).

The Phase 0 experiment crate has been removed: `tools/net-probe` supersedes it (direct, relay-only and unpinned-key scenarios, plus HTTP over QUIC and recovery), and its STUN probe is now `tools/net-probe/stun_probe.py`. It remains in git history.

Evidence levels used below:
- **Measured**: run on this Mac for this report.
- **Source**: read in the code — Pastey at `eaf2e1e`, or iroh 1.3.0 from crates.io.
- **Upstream**: n0's documentation or its third-party measurements.
- **Assumption**: plausible but not verified.

## 1. Recommendation in one paragraph

Add **iroh 1.x as a narrow connectivity adapter underneath the unchanged Bridge protocol**. Do not replace the Bridge, its HTTP routes, its encryption, its session binding or its authority model.

- **Bootstrap:** n0's free public relays are used only for bootstrap — reaching the other device, learning its reflexive address, the QUIC handshake, and hole-punch coordination.
- **Direct-only data:** an application-level gate admits Bridge traffic only while iroh has selected a direct (IP) path.
- **Identity:** a remote iroh `EndpointId` is used only after the paired device has signed it inside an already-authenticated Bridge session. It is a reachability pin, never an authority.
- **Failure:** when no direct path forms (UDP blocked, both sides behind hard NAT), Pastey says so and keeps the existing overlay-address path as the manual fallback.

This is the smallest design that works for two devices that are both behind NAT. It needs no infrastructure that Pastey operates and no rewrite of Bridge, Agent or Transfer code. Its costs are a dependency on n0's free relays, whose terms say they are for "development and hobby use only", and a heavy dependency tree.

## 2. Current Pastey networking (source)

| Area | What exists | Relevance to NAT traversal |
|---|---|---|
| LAN discovery | UDP broadcast on fixed port 48392 (`discovery.rs`); join-request listener on an ephemeral TCP port | LAN-only by design |
| Bridge server | One axum HTTP/1.1 server per Bridge on an **ephemeral TCP port bound to 0.0.0.0** (`transfer.rs` `start_room_server`). Routes: `join`, `items`, `control-events`, `transfers/{start,chunks,finish,cancel}`, `diagnostics/*` | Both devices are **servers and clients**; each must be able to reach the other's listener |
| Peer endpoint | `(source IP, advertised port)` recorded by `join_handler` from `ConnectInfo`, stored as `peer_host`/`peer_port` | Assumes routable IPs; **the main adaptation point** |
| Transport security | Plain HTTP. Security is app-layer: per-start X25519 transport keys, ChaCha20-Poly1305 payloads, encrypted Room Control envelopes bound to both keys, replay ids, session binding (`crypto.rs`, `room_control.rs`, `host_identity.rs`) | The link may be untrusted, so **any byte pipe can carry it** |
| Device identity | Ed25519 key, session attestation (`device_identity.rs`) | Reusable to bind a connectivity key to a device |
| Liveness/reconnect | 2 s reconcile loop, HTTP ping with 1.8 s timeout, 3 failures before invalidation, reconnect by LAN or remembered-address discovery (`bridge_lifecycle.rs`) | Reusable; a tunnel only needs to look like an endpoint |
| Cross-network today | User-supplied overlay address (Tailscale/WireGuard), private and overlay IPv4 ranges only, resolved names checked (`discovery.rs` `validate_peer_address`, `discover_room_at`) | Works now, but needs an external VPN |
| Loopback trust | None: no room-server route grants anything to a loopback source. `join_handler` uses the source IP only to record the endpoint | Makes a loopback forwarder safe, with one fix (see §6) |

**What blocks NAT traversal is not the protocol but the addressing model.** Each side opens TCP connections to the other's listener at `(ip, port)`. Behind NAT, inbound TCP fails, and TCP hole punching relies on simultaneous open, which consumer NATs and firewalls handle much less reliably than UDP. Carrying the existing HTTP traffic inside one UDP-based connection that either side can open streams on removes the problem without touching the protocol.

## 3. Candidates

| Candidate | Rendezvous needed | Hard-NAT / no-UDP fallback | Fit with Pastey | Verdict |
|---|---|---|---|---|
| **iroh 1.3** (QUIC multipath on n0's `noq`, NAT traversal inside QUIC, relays, UPnP/NAT-PMP/PCP portmapper) | Relay (n0 public, free) or self-hosted `iroh-relay` | Relay can carry encrypted data; Pastey would gate it off | Dial by public key; bi-streams in both directions map onto the Bridge's two-way HTTP; Rust-native; cross-compiles for Windows GNU (measured) | **Chosen** |
| STUN + own UDP punch + quinn + Mainline-DHT signalling | Public STUN plus the DHT (no operator) | None; no portmapper; DHT lookups are slow and best-effort | We would write and maintain the hard part: candidate gathering, punching timing, NAT keepalives, network-change healing | Rejected: rewrites what iroh already does well; lower success |
| ICE/WebRTC (`str0m`, `webrtc-rs`) | Signalling channel we must provide; TURN needs a server | TURN only | DTLS/SCTP data channels and RTP machinery we do not need; SCTP is a poor fit for bulk transfer | Rejected: more stack, no rendezvous answer |
| rust-libp2p (AutoNAT + DCUtR) | Publicly reachable circuit-relay-v2 peers (in practice IPFS-network relays we neither run nor control) | Relay with tight reservation limits | Swarm/behaviour model mismatches a two-device Bridge; ProbeLab measured about 70% punch success across the IPFS network, with older reports of worse rust-libp2p results | Rejected |
| Improve the overlay workflow (Tailscale etc.) | The overlay's own coordination and relays | The overlay's relays | Zero engineering; works today | Keep as fallback; fails the "no external VPN" requirement |
| Plain TCP hole punching for the existing HTTP | Rendezvous still needed | None | Keeps TCP, but needs simultaneous open | Rejected: much lower success than UDP |

iroh facts verified for this report:
- **License:** MIT OR Apache-2.0 (Source).
- **Release:** 1.0 on 2026-06-15 with stable wire guarantees; 1.3.0 is current (Upstream/Source).
- **Path API:** `Connection::paths()`, `path_events()` and `paths_stream()`, with per-path `is_ip()`, `is_relay()`, `is_selected()`, `rtt()` and `stats().udp_tx` (Source).
- **Relays:** four public relays — US-East, US-West, EU-Central, Asia-Pacific (`defaults.rs`).
- **No relay, no traversal:** with relays disabled there is no address discovery or hole-punch coordination, only paths to addresses already known (Source/Upstream).

## 4. Zero-cost infrastructure feasibility

**A rendezvous is unavoidable.** When both devices are behind NAT, neither can receive an unsolicited packet. Each must learn its own public (reflexive) address from a server on the internet, and the two must exchange those addresses through a channel both can reach. Durable identity and pairing authenticate the devices, but they do not make either one reachable.

| Option | Cost | Operator | Terms / limits |
|---|---|---|---|
| n0 public relays (iroh default) | $0 | n0.computer | "Suitable for development and hobby use only"; rate-limited; no SLA; only the latest stable iroh is supported; relays see IPs, timing and volume but not content |
| n0 DNS/pkarr address lookup (`dns.iroh.link`) | $0 | n0.computer | Publishes the endpoint's addresses; **not needed**, since Pastey can exchange its relay hint over the authenticated Bridge |
| Mainline DHT (BEP 44, via pkarr) | $0 | Nobody (public DHT) | Slow; records are public; iroh still needs a relay to learn its reflexive address |
| Public STUN (Google, Cloudflare) | $0 | Google, Cloudflare | No contractual terms for third-party use; gives the reflexive address only |
| n0 managed relay / self-hosted `iroh-relay` | about $19/month, or a VM | n0 or us | Production-grade; excluded by the current budget |

**Conclusion:** the only zero-cost option needing no new servers is n0's public relays, which are third-party infrastructure under development/hobby terms. That is acceptable for the PoC and for personal use. **Before a public release the relay must change**, either to a managed plan or to a self-hosted relay; with iroh that is a configuration change (`RelayMode::Custom`), not a code change. The design keeps relays out of the data path, so even a small paid relay would carry only bootstrap traffic.

## 5. Recommended architecture

```text
 Bridge (unchanged): axum room server, reqwest clients, Room Control, Transfer, Agent
        │  HTTP to peer endpoint (ip, port)
        ▼
 peer endpoint = 127.0.0.1:<local forwarder port>      ← only the addressing changes
        │  each TCP connection = one QUIC bi-stream: header {bridge id}
        ▼
 connectivity (new module): iroh Endpoint, pinned remote EndpointId per Bridge peer,
                            direct-only gate, path events → liveness/status
        │  QUIC over UDP: direct path, hole-punched with n0 relay coordination
        ▼
 peer's connectivity: accepts only pinned ids; opens 127.0.0.1:<that Bridge's room port>
```

**Reuse unchanged:**
- All room-server routes and the HTTP clients.
- Transport keys, Room Control encryption and replay protection, session binding and device attestation.
- Grants, admission, the native-approval relay, Transfer and its integrity checks.
- Liveness pings, reconnect state and Burn.

**Add:**

1. **`connectivity.rs`** (new module):
   - A process-wide iroh `Endpoint` with its own Ed25519 secret key, stored wrapped like the device key and separate from it.
   - The address-lookup publisher is disabled, so nothing is published to n0 DNS.
   - ALPN `pastey/bridge/1`.
   - An `EndpointHooks::after_handshake` check that accepts only `EndpointId`s pinned to an active, unburned Bridge.
2. **Binding exchange:** a new authenticated Room Control event, for example `bridge_connectivity.endpoint`, sent over an existing LAN or overlay session.
   - It carries `{endpointId, relayHint}` plus a device-identity signature over (Bridge, signer and verifier HostRef, session pair, endpointId).
   - The receiver pins the EndpointId only if the signature verifies against the **already trusted or attested device key for that peer** in the current session.
   - It is stored per Bridge peer and erased at Burn.
3. **Loopback forwarder:**
   - Per connected peer, a listener on `127.0.0.1:0`.
   - Each accepted connection opens a bi-stream with a fixed header naming the Bridge.
   - The receiving side forwards **only** to the room-server port of a Bridge pinned to that remote EndpointId, never to any other local port.
4. **Endpoint adaptation:**
   - `join_handler` and `update_room_peer` must record `127.0.0.1:<local forwarder port for that peer>` for connections the forwarder delivered, instead of `(source ip, advertised port)`.
   - The forwarder keeps a registry from the loopback source port it used to the peer, and `join_handler` consults it. This is the one change inside the Bridge.
5. **`bridge_lifecycle::reconnect_room`:** after the LAN and remembered-address discovery fail, and only when a pinned EndpointId exists, dial it. Reconnect then rejoins through the tunnel, refreshing transport keys exactly as today.
6. **Direct-only gate:**
   - The forwarder accepts streams only while the selected path `is_ip()`.
   - When `path_events` reports the selection moving to the relay, new streams stop. In-flight requests then fail as today's network errors do, so Transfer's existing receipt-ambiguity rules apply unchanged.
   - Status is reported truthfully: *direct*, *no direct path (withheld)*, or *unreachable*.
7. **UI:** a Bridge network-panel setting "Connect across the internet", **off by default**. When enabled it says that n0's free relay servers are used to set up connections, and that messages and files travel only directly. The panel shows the path state.

**Affected Rust modules:**
- `transfer.rs`: `join_handler`, endpoint recording.
- `bridge_lifecycle.rs`: reconnect fallback; liveness also consults connection state.
- `storage.rs`: pinned EndpointId and relay hint per Bridge peer; Burn deletion.
- `room_control.rs`: one new allowed event kind and its validation.
- `device_identity.rs`: sign and verify the binding.
- `host_runtime.rs`: holds the connectivity service.
- `commands.rs`: setting and status.
- New `connectivity.rs`.

**New dependency:** `iroh = "1.3"`, which adds about 115 crates not currently in Pastey's lock plus about 131 version-divergent duplicates (Measured). The isolated release binary was 14.8 MB (Measured).

## 6. Security

- **A NAT identifier is never authority.** The pinned EndpointId only answers "may this QUIC peer reach our room server?". Every Bridge, Agent, grant and approval decision still requires the existing session keys, attestation and authorization. An attacker with a valid iroh key that is not pinned is refused right after the handshake (Measured: "refused unpinned peer"). The same QUIC TLS handshake proves possession of the pinned key, so the pin cannot be spoofed.
- **Binding provenance.** Pins are created only from a device-signed event inside an authenticated session, never from a ticket, address, relay or DNS record.
- **Forwarder scope.** The forwarder reaches only the room-server ports of Bridges pinned to that peer. It must never become a general localhost proxy: no nearby join listener, no other services. Tests must assert this.
- **Net effect on exposure.** Today the room server's unauthenticated `join` route is reachable by anyone on the LAN who knows the room id. Over the tunnel it is reachable only by the pinned peer, so the tunnel is stricter than the LAN.
- **New attack surface.** iroh's UDP socket, QUIC handshake processing and relay client. Those are mature components, but they are new code in the process, and QUIC rate limiting should be configured. Windows firewall prompts are expected on first UDP bind (Assumption; test physically).
- **Metadata.** n0 sees both public IPs, connection times and the volume of bootstrap traffic, but not content; content is end-to-end encrypted by QUIC and again by the Bridge layer. The design publishes nothing to n0 DNS.
- **Revocation.** Burn, device revocation and Bridge revocation must close the iroh connection and delete the pin. A reconnect must not revive a revoked Bridge: the pin lookup filters burned Bridges, just as shared folders do.

## 7. Answers to the critical questions

**A. New transport?** Yes, at the link layer only: QUIC over UDP, provided by iroh. The Bridge protocol is encapsulated, not rewritten. Long-term, HTTP could run directly over QUIC streams (hyper over iroh streams) to remove the loopback hop. Consolidating everything onto QUIC is optional, not required.

**B. Initial discovery.** Both devices are first paired the existing way, on a LAN or an overlay. While that session is live they exchange signed EndpointIds and relay hints. Later, from different networks, either device dials the pinned EndpointId through the peer's home relay, which carries the encrypted QUIC handshake and the address exchange. Then both sides punch, and data moves only when a direct path is selected. A first-ever pairing across NAT would need an out-of-band ticket (EndpointId plus relay). That is possible later, but not in the MVP: the Bridge code must still authenticate the join, and a ticket provides only reachability.

**C. Difficult networks.**

| Network | Expected result |
|---|---|
| Ordinary home NAT | Endpoint-independent mapping is typical; direct path likely. Portmapping (UPnP/NAT-PMP/PCP) helps further. This Mac's current network measured endpoint-independent mapping across Google and Cloudflare STUN, without port preservation (Measured; filtering not measurable this way) |
| CGNAT (mobile hotspots) | Often punchable when mapping is endpoint-independent; fails when endpoint-dependent |
| Symmetric on one side only | iroh 1.0 fixed the hard-to-easy case in both directions (Upstream) |
| Symmetric on both sides | No direct path; withheld (truthful failure) |
| University or corporate Wi-Fi | Often blocks UDP or isolates clients; the relay would work over HTTPS, but direct will not, so the gate reports failure |
| UDP blocked | Never direct; truthful failure (the iroh FAQ says traffic would fall back to the relay) |
| IPv6 | Direct if both sides have global IPv6 and firewalls allow return traffic; iroh prefers IPv6. This Mac has no global IPv6 now (Measured) |

n0 reports about 9 in 10 connections going direct and about 95% of data volume direct (Upstream; their user mix, not ours). Expect lower success on campus and corporate networks.

**D. Strictly no relayed application data?**
- **What the gate achieves:** it can be enforced by the application, not at the packet level. In the measurements, during a direct 64 MiB transfer the relay path carried **3,726 bytes in 3 datagrams** (path probes) against 68.6 MB on the direct path. With no direct path, the relay carried **9,563 bytes in 17 datagrams over 8 s** (handshake and keepalives) and **0 application bytes**.
- **The residual risk:** if the direct path dies mid-transfer, QUIC may retransmit in-flight packets over the relay before the gate stops new streams. Those packets are already encrypted twice; the exposure is volume metadata, bounded by roughly one path-switch interval.
- **Making it strict:** that would need a custom `PathSelector`, which is still behind iroh's `unstable-custom-transports` feature, so it is not recommended for the MVP.

**E. Relation to META identity.** Covered in §6: the EndpointId is subordinate to the device identity and pinned per Bridge, and nothing in session binding, grants, replay protection or approvals changes.

**F. Is it justified versus the overlay?** The overlay already works and costs nothing to build, but every user must install and sign in to a VPN. The native path costs about 2–3 developer-weeks for the MVP (estimate in §9), plus ongoing iroh upgrades.

- **If the product goal is "no VPN for the user", it is justified.**
- **If the near-term goal is only the developer's own Mac↔Windows use, improving the overlay workflow wins.** One concrete improvement: recognise the peer's Tailscale name automatically.

## 8. PoC plan and acceptance criteria

Phase 0 is done (its crate is superseded by `tools/net-probe`; see Status).

- **Phase 0, on this Mac (done):** a STUN probe, and an iroh experiment using real n0 relays with two endpoints in one process. Results:

  | Scenario | Result |
  |---|---|
  | Direct | Selected path IP after 2.3–2.8 ms; 64 MiB at about 93 MiB/s locally; BLAKE3 digest intact; relay never selected |
  | Relay-only | Gate withheld payload; 0 application bytes |
  | Unpinned key | Refused |
  | Windows GNU `cargo check` | Passed |

  A local direct path is **not** cross-NAT evidence.

- **Phase 1, same LAN, Mac and Windows:** the same binary on both devices, with a CLI ticket exchange that stands in for the Bridge binding event. This confirms Windows runtime behavior and the firewall prompt.

- **Phase 2, cross-NAT:**
  - Windows at home; Mac on a phone hotspot (CGNAT), then on university Wi-Fi.
  - 20 attempts per network pair, recording:
    - time to the selected direct path;
    - the share of attempts that reach direct;
    - RTT;
    - 256 MiB stream throughput and digest;
    - relay `tx_bytes` before and after the gate.

- **Phase 3, adverse conditions:**
  - Block outbound UDP except DNS with macOS `pf`; expect "no direct path" within the gate timeout and 0 application bytes on the relay.
  - Switch Wi-Fi to hotspot mid-transfer and record path events, time to re-establish direct, and the transfer outcome.
  - Put the Mac to sleep and wake it.

- **Phase 4, only after phases 1–3:** wire the adapter into Bridge reconnect behind the off-by-default setting, and run the existing Bridge, Agent and Transfer suites over a tunnel between two local Hosts. This is a deterministic test in which the relay is disabled and the direct addresses are given.

| Acceptance criterion | Target |
|---|---|
| Home ↔ hotspot direct success | ≥ 8 of 10 attempts, direct path within 10 s |
| Integrity | 100% digest match on every completed stream |
| Throughput | ≥ 80% of the slower uplink measured with a speed test |
| Gate | 0 application bytes on relay paths when no direct path forms; reported as withheld within the timeout |
| Truthful status | Path classification (direct / withheld / unreachable) matches ground truth in every run |
| Network change | Direct path re-established within 30 s, or reported as failed; no silent stall |
| Authority | No Bridge, Agent or Transfer state changes from an unpinned EndpointId; existing suites green over the tunnel |

## 9. Complexity and maintenance

| Item | Estimate |
|---|---|
| `connectivity.rs` (endpoint, keys, hooks, gate, status) | about 600–900 lines |
| Forwarder and endpoint adaptation | about 300–400 lines |
| Binding event, storage and Burn | about 250 lines plus tests |
| UI and setting | about 150 lines |
| Tests (deterministic two-Host tunnel, refusal, gate, Burn) | about 600 lines |
| **MVP total** | **about 2–3 focused developer-weeks** |

Maintenance:
- **Forced upgrades:** n0 supports only the latest stable iroh on its public relays, so Pastey must keep iroh current; expect several upgrades a year.
- **Build cost:** a larger dependency tree and longer builds.
- **Physical testing:** Windows firewall and network-change behavior need physical tests on every significant upgrade.

## 10. Unknowns that need physical testing

- Real success rates for the user's networks (home router, phone hotspot CGNAT, university Wi-Fi), including filtering behavior.
- Whether iroh 1.3 re-runs NAT traversal reliably after a network change. 0.96 listed this as a known issue, and no 1.x fix note was found.
- Windows Defender Firewall prompts and behavior on public and private network profiles.
- Relay coordination latency from Australia: here iroh picked US-West (about 190–360 ms RTT) over the Asia-Pacific relay.
- Behavior across sleep and wake, and when one side's Pastey restarts.
- Whether n0 rate-limiting affects bootstrap at personal-use volumes.

Sources:
- [iroh 1.0 announcement coverage](https://pinggy.io/blog/iroh_1_0_dial_keys_not_ips/)
- [iroh relays concept](https://docs.iroh.computer/concepts/relays)
- [n0 public relay terms](https://docs.iroh.computer/iroh-services/relays/public)
- [iroh FAQ](https://docs.iroh.computer/about/faq)
- [iroh 0.96 release notes](https://www.iroh.computer/blog/iroh-0-96-0-the-quic-multipaths-to-1-0)
- [iroh 1.0.0-rc.1](https://iroh.computer/blog/iroh-1-0-0-rc-1)
- [Healing connections](https://www.iroh.computer/blog/healing-connections)
- [Shared relays pricing](https://www.iroh.computer/blog/shared-relays)
- [iroh repository](https://github.com/n0-computer/iroh)
- [docs.rs iroh](https://docs.rs/iroh/latest/iroh/)
- [Agaric iroh relay analysis](https://github.com/jfolcini/agaric/issues/4549)
- [ProbeLab DCUtR measurement](https://probelab.io/publications/large-scale-measurement-of-nat-traversal-for-the-decentralized-web-a-case-study-of-dcutr-in-ipfs/)
- [str0m](https://docs.rs/str0m)

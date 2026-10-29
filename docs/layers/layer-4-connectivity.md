# Layer 4 — Connectivity: LAN, overlay and the internet carrier

How a Bridge's two devices reach each other. [Layer 4](layer-4-bridge.md) owns the Bridge session (pairing, reconnect, Room Control, liveness, Burn); this page owns the paths underneath it. The source of truth is `src-tauri/src/connectivity.rs` (its module comment states the contract below), `connectivity/iroh_carrier.rs`, `bridge_lifecycle.rs` and `transfer.rs::join_handler`. Identifiers and bounds are in the [reference](../reference.md#internet-carrier-facts).

## Status and evidence

| Path | State | Evidence |
| --- | --- | --- |
| LAN (UDP discovery, TCP room servers) | Shipped, unchanged by the carrier | Automated suites; physical LAN acceptance in [Mac ↔ Windows acceptance](../platform/mac-windows-acceptance.md) parts B–D |
| Overlay address (Tailscale/WireGuard) | Shipped, unchanged | Automated; physical part G |
| Internet carrier (iroh: direct P2P, relay fallback) | **Shipped in 2.0, off by default** | Automated: deterministic two-Host tests over an in-memory carrier, and real iroh QUIC on loopback. Physical: a Mac and a Windows PC on separate networks (home Wi-Fi and a phone hotspot) kept and recovered a Bridge and ran remote Codex across them ([acceptance](../platform/mac-windows-acceptance.md#recorded-results-for-200)) |

This is evidence from one pair of devices and networks, not a claim of WAN reliability in general. NAT traversal success, relay throughput and network-change recovery depend on the networks and are measured only by part W (and, for raw path behaviour, the isolated `tools/net-probe`, part F). Throughput over the internet carrier is low in the tests so far: Test connection measured about 0.2–0.4 MB/s over a phone hotspot, and the cause (the hotspot uplink, the path, or Pastey) is not yet established.

## Transport–Session contract

A **carrier** moves bytes between two devices named by their carrier keys: it finds a path through NATs, prefers a direct one and falls back to a relay. A **Bridge session** is everything above it and is unchanged: room-server transport keys, the SPAKE2 pairing or signed reconnect that set them up, encrypted Room Control with its replay and lifetime rules, and every Review, grant, writer lock and task identity built on that.

1. **The carrier is a byte pipe to one room server.** A tunnel stream reaches only the room server of a Bridge that pins the stream's carrier key, unmodified. Every request on it is then authenticated by the Bridge exactly as on a LAN.
2. **Carrier keys are pinned only through an authenticated session.** A peer's carrier card arrives as a Room Control event in a session whose keys the pinned device key established. A carrier pin is valid only while the Bridge is unburned and still pinned to the same peer HostRef, so Burn, departure, forgetting the device and pairing another device revoke it with no extra step.
3. **Path changes are not session changes.** A network change, or moving between direct and relay, keeps the tunnel's loopback endpoint, so the Bridge session and its keys continue. Losing the carrier is an ordinary transport failure: requests fail as on a LAN, an ambiguous delivery stays ambiguous, a reconnect is a new authenticated session. Nothing is resent or rerun.
4. **Infrastructure never decides.** Relays and address lookup see carrier keys, IP addresses, timing and volume, never content. They cannot pin a key, admit a device, accept a request or complete a task.
5. **The LAN path is unchanged and preferred.** Discovery, pairing and reconnect on a LAN or overlay work as before; the carrier is tried alongside the LAN when a pinned Bridge reconnects, and a LAN answer wins.

What this means for Agents ([Layer 5](layer-5-agent.md)): executor Review, writer-lock checks, task identity, idempotency and reconciliation see a Bridge session, not a path. A continuation sent over the internet is the same `native_agent.invoke`; if the carrier drops, the requester's existing rules apply (not delivered, or receipt-ambiguous and reconciled), and nothing is queued for later delivery.

## How the internet path works

```text
 Mac                                                     Windows
 Bridge (unchanged) ── HTTP to 127.0.0.1:<forwarder> ─┐  ┌─ room server (unchanged)
                                                      │  │  127.0.0.1:<room port>
 connectivity: loopback forwarder per (Bridge, key)   │  │  connectivity: stream header
   each TCP connection = one QUIC bi-stream           ▼  ▲    names the Bridge; its pin
   header {pastey-bridge-tunnel-v1, room, bridgeId}  iroh QUIC   must name this key
                              direct (hole-punched, UPnP/NAT-PMP/PCP)  or  relay
```

1. **Turning it on.** Settings → General → Across the internet starts one iroh endpoint for the device, with its own carrier key (stored wrapped by the installation master key, separate from the device identity key).
2. **Learning the peer's card.** While a Bridge session is connected (on the LAN or overlay), each side announces its `CarrierCardV1` (carrier, key, home relay, direct addresses) as `bridge_membership.carrier_card`. The receiver pins it for the session's authenticated peer HostRef, only if the Bridge is pinned to exactly that device. It is announced again only when it changes.
3. **Admission.** After the QUIC handshake an inbound connection is refused unless a live Bridge pins its key (iroh may let the dialer's `connect` complete a moment before closing it; no stream of it is served). Each stream is checked again: its header must name a Bridge whose pin is this key.
4. **Reconnect.** A pinned Bridge without a session runs the LAN query and, when both devices have the carrier, the same signed reconnect query through the tunnel at once. A LAN answer wins. Either answer must be an offer signed by the pinned device key for this attempt; then the `/join` with the reconnect proof goes through the tunnel.
5. **The endpoint.** The Bridge stores `127.0.0.1:<forwarder port>` as the peer's endpoint. On the receiving side, `join_handler` recognises a join that came through a tunnel (by its loopback source port) and records its own forwarder toward the joining device, refusing the join unless that device is the one the tunnel's key is pinned to.
6. **Liveness.** A session through a tunnel is probed with a 5 s bound and ends after 4 failed probes (LAN: 1.8 s, 3), so relay latency and a path change of a few seconds do not rotate its keys; a session that is really gone still ends within about half a minute.
7. **Status.** Bridge details and Device details → Connection show whether the internet path is in use and whether it is direct or through a relay.

## Overlay address

Unchanged from before the carrier. Joining by code may name the creator's address on a private or overlay network the user runs (Tailscale, WireGuard), and any Bridge may remember its peer's address for reconnects. Pastey sends the same pairing or reconnect query by unicast to that address only and reads only replies from an address it resolved to; reconnect falls back to the LAN broadcast. Each reply is authenticated exactly as on the LAN; an address never authorizes anything. Public, loopback, link-local, broadcast and multicast addresses, and anything with a scheme, port or path, are refused. A name is used only through the private or overlay IPv4 addresses it resolves to at that moment; a name resolving only elsewhere is refused before anything is sent, and numeric forms a resolver reads as addresses (such as `0x08080808`) are refused as names. Room Control delivery allows 5 s to connect and 15 s in total; a timeout stays receipt-ambiguous. Remembered addresses end at the Burn cutoff. The room servers still bind all interfaces, so the Host's firewall decides which networks reach them; Pastey must not be port-forwarded to the internet (the carrier makes that unnecessary).

## Decisions

Each records what was chosen, against which alternative, and why. The [NAT traversal research](../research/native-nat-traversal.md) is the earlier analysis; where this differs, this page is current.

- **iroh 1.x behind a `Carrier` trait.** Pastey depends on its own `Carrier`/`CarrierLink`/`InboundSink` traits; iroh is one implementation (`iroh_carrier.rs`) and tests use an in-memory one with the same contract. Replacing iroh, or adding a second carrier, means a new implementation and a new carrier name in cards; nothing above `connectivity` changes. Chosen over STUN+own punching, WebRTC/ICE and libp2p for the reasons in the research report.
- **Relay fallback carries Bridge traffic.** The research proposed a direct-only gate. Pastey's goal for this phase is that a paired Bridge works across networks, with direct P2P where possible and relay fallback when necessary. Relayed bytes are QUIC-encrypted end to end and Bridge-encrypted again; the relay sees volume and timing. Measured on n0's free relays from one network, relay throughput was about 1.1 MiB/s at about 350 ms RTT (`tools/net-probe/RESULTS.md`): usable for control, Agent requests and small items, slow for large files. The UI says which path is in use.
- **The card is trusted because of the session it arrives in, not a separate signature.** Room Control events are encrypted to the current session's transport keys, which the pinned device key established (SPAKE2 at pairing binds them; reconnect signs them). An extra device signature over the card would prove the same thing. The receiver binds the pin to the authenticated session's HostRef and requires the Bridge's own pin to name it.
- **Loopback forwarder, not HTTP over QUIC.** Each TCP connection to a local forwarder becomes one QUIC stream, so the room server, its HTTP clients, Transfer and Room Control are unchanged, and LAN behaviour is untouched. The cost is one loopback hop and the join-source registry. HTTP directly on QUIC streams remains possible later.
- **Address lookup only with n0's relays.** With the default relays the endpoint publishes its home relay as a record signed by its key on n0's DNS, so a peer whose relay changed while the session was down is still found. With the user's own relay nothing is published to n0. The card exchanged over the Bridge is always the primary source.
- **Off by default, device-wide.** Turning it on is a device decision (it opens a UDP socket and contacts relays); each Bridge still needs the peer to have it on, and a Bridge without a peer pin never uses it.
- **No pairing across the internet in this phase.** First pairing stays on the LAN or overlay, where the 8-digit code is the SPAKE2 password. A code typed across the internet would need a rendezvous ticket; that is a separate design.
- **No offline queues.** A request to a device that cannot be reached fails or stays ambiguous as before; nothing waits to be sent later.

## Security and privacy

- A carrier key is reachability only. Every Bridge, Agent, grant and approval decision still requires the session keys and Host-local authority.
- Over the tunnel, the room server's routes are reachable only by the pinned peer, which is stricter than the LAN.
- Unpinned keys are refused after the handshake; a stream naming another Bridge, or a reconnect query for another Bridge or HostRef, gets nothing.
- Relay hints from a peer must be `https://`; the user's own relay must be `https://`.
- n0 (default relays and DNS) sees this device's IP addresses, its carrier key, connection times and volume; not content. n0 offers the public relays for development and hobby use, without an SLA. For dependable use, run an `iroh-relay` and set its address.
- Burn, departure and forgetting a device revoke the carrier pin with the Bridge pin and stop the Bridge's forwarders. An iroh connection that is already open stays until it idles out, but every new stream on it is refused.

## Limitations

- Not validated across physical networks (see Status).
- First pairing needs the LAN or an overlay.
- A session that started over the carrier stays on it when both devices come back to one network (iroh then usually finds a direct local path); the LAN path is used again at the next reconnect.
- Large transfers over a relay are slow and are not resumable; an interrupted transfer fails as on a LAN.
- Both sides symmetric NAT, or UDP blocked on one side: relay only. If the relay is unreachable too, the Bridge is not reachable across the internet.
- Liveness checks run one after another: a stalled tunnel probe delays the other Bridges' checks by up to 5 s per round.
- Windows Defender Firewall may ask about network access on first start of the carrier; the answer and the network profile matter (part W).
- iroh must stay current for n0's public relays, which support only the latest stable release.
- Mobile devices, push wake-up and offline delivery are out of scope.

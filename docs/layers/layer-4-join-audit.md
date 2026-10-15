# Layer 4 — Bridge join and reconnect audit (2026-10)

Status: audit of commit `eaf2e1e` (sections 1–6, kept as found), followed by the Stage 1 remediation (section 7) that implements option A. [Layer 4](layer-4-bridge.md) and the [reference](../reference.md#bridge-authentication-facts) are the authoritative description of the current behaviour.

Evidence labels: **Source** (read in code at the cited line), **Test** (a `characterization_*` test asserts the behaviour), **Estimate** (reasoned, not measured). No finding here was observed on a physical network.

## 1. Summary

The 8-digit Bridge code is used only to *find* a room; it is never proven. The room server's `/join` accepts any transport public key from anyone who knows the Bridge id, replaces the current peer, and binds whatever HostRef the request claims. Reconnect repeats the same unauthenticated exchange and does not compare the new peer with the old one. The Bridge id, port and key leak from the room query to anyone holding the code lookup value, and that value (an unsalted hash over 10^8 codes) is broadcast on the LAN on every code join and every LAN reconnect attempt.

What *is* sound: after a session exists, Room Control, managed/native transfers and device-identity proofs are bound to the stored transport keys. The problem is that nothing binds those keys to the code or to the peer device. An attacker who wins the join or reconnect therefore *becomes* the authenticated peer.

No finding lets an attacker skip executor Review or use an unattended grant: Review is still required on the executing Host, and unattended grants need a device-key signature that only the trusted device can make. The risks are confidentiality of Bridge traffic, integrity of what the user believes about the requester (the Review prompt names a HostRef the attacker chose), injection of ordinary items, and availability.

## 2. Message sequences

`→` is a request, `⇠` a reply. "Clear" means plaintext on the wire. Transport keys are per-room-server-start X25519 keys (`crypto.rs`), not long-term keys.

### 2.1 Nearby discovery and accept

```
A (requester)                                   B (acceptor)
  every 2 s, UDP broadcast :48392  ⇠──────────  nearby_beacon {device_id, display_name, platform,
                                                 app_version, listen_port, availability}        clear, unauthenticated
  POST http://B:listen_port/nearby/join-request ─→ {request_id, device_id, display_name,
                                                 platform, app_version, response_port}          clear, unauthenticated
                                                 B's user sees name/platform/version only and clicks Accept
                                                 B creates room, code, room server
  UDP → A:response_port  ⇠──────────────────────  join_response {request_id, room_id, room_code,
                                                 port, transport_public_key, device_name}       clear; A accepts any source
                                                                                                 with matching request_id
  POST http://B:port/rooms/{room_id}/join ──────→ {port, device_name, transport_public_key,
                                                 host_ref}                                      clear, unauthenticated
                                              ⇠  {device_name, expires_at, transport_public_key, host_ref}
  A stores key from join_response;               B stores key from the /join body and the
  HostRef from the /join reply                   TCP source IP; HostRef from the body
```

### 2.2 Join by code (LAN)

```
A (joiner)                                      B (creator, room server running)
  UDP broadcast 255.255.255.255:48392 ─────────→ discover_room {request_id, room_code_hash,
                                                 requester_device_id, expected_room_id=null}    clear, seen by every LAN host
                                              ⇠  room_offer {request_id, room_id, port, expires_at,
                                                 transport_public_key, device_name}             clear; A takes the first reply
                                                                                                 with matching request_id, any source
  POST /rooms/{room_id}/join  (as in 2.1) ─────→ …
```

### 2.3 Join by overlay address (unicast)

Same as 2.2, but the query is sent to the private/overlay IPv4 addresses the typed name resolves to, and only a reply whose source IP is one of them is accepted (`discover_room_unicast`). The overlay authenticates source IPs, so off-path spoofing is not possible there; the rest of the exchange is identical and equally unauthenticated.

### 2.4 Reconnect (restart or three failed liveness probes)

```
A                                               B
  liveness: POST /rooms/{id}/diagnostics/ping ─→ (no body)
                                              ⇠  {transportPublicKey}                            clear; compared with the stored
                                                                                                 key, which is public
  after 3 failures: peer → reconnecting, A rotates its room server (new transport key)
  every 2 s: unicast discover_room to remembered address (if any), else broadcast with
             expected_room_id = room id                                                         clear
                                              ⇠  room_offer                                      A checks room_id only
  POST /join  ──────────────────────────────────→ B: new peer_session_id, old row stale
  A stores key from room_offer and binds HostRef from the /join reply; nothing is compared
  with the previous session's key, HostRef or device identity.
```

### 2.5 Leave / Burn

`bridge_membership.departure` is a Room Control event: AEAD-encrypted under an event key wrapped with static-static X25519 between the sender's room-server key and the stored peer key, bound to both session refs, with expiry and a replay id. It is authenticated *to the current peer session*, so only whoever holds the current peer key can send it, which after a hijack is the attacker.

### 2.6 Where device identity is verified

The Ed25519 device key (`device_identity.rs`) is verified in exactly one place: an explicit `native_agent.device_identity` Room Control event (`commands.rs:1064` sends, `room_control.rs:1919` verifies with `verify_attestation`, `device_identity.rs:99`). The signature covers Bridge id, both HostRefs and `session_pair_ref` (`host_identity.rs:405`), which hashes both sides' transport-key session refs. Under a man-in-the-middle the two sides compute different `session_pair_ref` values, so a proof cannot be relayed. The verified device is used only for unattended-access grants (`unattended_access.rs`).

The device key is **not** used at join, reconnect, liveness, HostRef binding, or for ordinary items and transfers. A HostRef is `blake3("pastey-host-ref-v1\0" ‖ device_id)` (`host_identity.rs:21`), and `device_id` is in every nearby beacon (`discovery.rs:925`), so any LAN observer can compute and claim any nearby Host's HostRef.

## 3. Threat models

- **LAN, off-path.** A device on the same broadcast domain. It sees broadcasts (beacons, `discover_room`) and can send UDP/TCP to any LAN host, but cannot read unicast traffic between A and B.
- **LAN, on-path.** The same device after ARP spoofing, or on Wi-Fi where it can decrypt others' frames. It also reads all unicast HTTP, which is plaintext apart from the encrypted payloads.
- **Overlay member.** Another node on the user's Tailscale/WireGuard network. It sees no LAN broadcasts but can reach every listener bound to `0.0.0.0` (UDP 48392, the nearby join port, room servers). The overlay authenticates source IPs.
- **Future relay/internet attacker.** Anyone on the internet once Bridge traffic can be reached through a public relay (Stage 3). The relay itself sees only end-to-end encrypted QUIC. The risk is what a dialer, or a poisoned allowlist entry, can do after the handshake.

## 4. Findings

Severity is for the worst applicable threat model *today* (LAN or overlay). The relay column gives the severity if Stage 3 shipped without these fixes. C/I/A = confidentiality/integrity/availability. No finding bypasses executor Review.

| # | Finding | Location | Preconditions | Consequence | Today | Relay | Evidence |
|---|---|---|---|---|---|---|---|
| F1 | `/join` proves neither the code nor any prior identity. It overwrites the room's peer with the caller's key, source IP and claimed name, marks the previous session stale, and binds the claimed HostRef | `transfer.rs:2844`; `storage.rs:1214`, `:1465`, `:1490`, `:1808` | Reach the room server; know the Bridge id (F3, F4, F12) | C: the user's later sends go to the attacker. I: the attacker's Room Control events (Plan, Native Agent, departure) authenticate as the peer; Review prompts name the attacker-chosen HostRef. A: the real peer is evicted. Unattended grants are not reachable (need the trusted device's signature) | High | Critical (a poisoned peer can become a durable allowlisted EndpointId) | Test `transfer::join_characterization::characterization_join_without_proof_replaces_the_connected_peer` |
| F2 | The code lookup value is `blake3("pastey:code:v1:" ‖ code)`: no salt, no Bridge or installation input, 10^8 codes | `crypto.rs:27`, `:13` | Observe one value: any `discover_room` broadcast (code join, and every LAN reconnect attempt) | C: the code is recoverable offline. Estimate: 10^8 hashes is seconds to minutes of commodity CPU. Leads to F3/F1 for the lifetime of the Bridge, since the code never changes | High | High | Test `crypto::join_characterization::characterization_code_hash_is_unsalted_over_an_eight_digit_space` (asserts the construction; no enumeration is run) |
| F3 | The room query answers anyone presenting the lookup value with Bridge id, port and transport key; requests are not identified, rate-limited or locked out after wrong guesses | `discovery.rs:764`; listener `0.0.0.0:48392` at `:162` | Know or guess the lookup value; reach UDP 48392 (LAN or overlay) | C: hands over everything F1 needs. Overlay members can guess online with no broadcast access | Medium–High | n/a if discovery is not exposed through the relay | Test `discovery::join_characterization::characterization_room_query_answers_any_holder_of_the_code_hash` |
| F4 | Broadcast `discover_room` accepts the first `room_offer` with the matching `request_id` from **any** source, and does not check `expected_room_id` itself. `request_id` and `expected_room_id` are in the broadcast | `discovery.rs:696`, `:737`; callers `commands.rs:2836`, `bridge_lifecycle.rs:495` | LAN off-path; answer faster than the creator | C/I: the joiner or reconnecting Host joins the attacker's server. Combined with F1 on the other side this is a full man-in-the-middle that both users see as connected | High | n/a (LAN only) | Source only. An automated test needs a real broadcast to the fixed LAN port, so the fix should add a test seam and flip it |
| F5 | The joiner stores the key from the UDP offer and ignores the key in the authenticated-looking `/join` reply; the HostRef from the reply is bound unverified | `commands.rs:2885`, `:2974`; `bridge_lifecycle.rs:495` | F4, or on-path | I: no internal consistency check that the offer and `/join` reply came from the same server | Medium | Medium | Source |
| F6 | Reconnect is not pinned. The new session may have a different key, host and HostRef; nothing is compared with the previous peer or its device identity | `bridge_lifecycle.rs:495`; `storage.rs:1808` | F1 or F4 at any reconnect (every restart and network blip) | Same as F1, and the attack window recurs for the life of the Bridge rather than only at first join | High | Critical | Test `bridge_lifecycle::tests::characterization_reconnect_rebinds_a_different_key_and_host_ref` |
| F7 | HostRef is a self-asserted claim derived from `device_id`, which every beacon broadcasts | `host_identity.rs:21`; `discovery.rs:925` | LAN off-path | I: an attacker can claim a specific nearby Host's HostRef in F1/F6. It grants nothing alone, but `HostSessionBinding` and Review display trust it | Medium | Medium | Source; F1 test binds a claimed HostRef |
| F8 | `/items` and plain `/transfers/start` unwrap with whatever `sender_public_key` the request names and never compare it with the current peer key. Managed, native-v2 and Native Agent landings *do* check it | `transfer.rs:2893` (unwrap `:2921`), `:3118` (unwrap `:3293`) | Know the Bridge id; read the receiver's public key (F9 or room offer); reach the port | I: text and files land in the Bridge history and destination folder as if from the peer, with no join and no Review (ordinary data has none by design). A: unbounded item count | High | Low with an allowlist gate (only members reach the router) | Test `transfer::join_characterization::characterization_item_upload_from_a_non_member_key_is_stored` |
| F9 | Liveness and current-session resolution accept any server that echoes the stored *public* key; the ping endpoint answers anyone | `transfer.rs:3028`; `bridge_lifecycle.rs:318`, `:352` | Hold the peer's recorded IP (on-path or after DHCP reuse) | A/I of liveness state only: a dead route can look alive. No payload exposure, since encryption still uses the real key. [Layer 4](layer-4-bridge.md) calls this a "transport-key check" and the resolver a "transport-key proof"; it is not proof of possession | Low | Low | Tests `bridge_lifecycle::tests::characterization_liveness_probe_accepts_an_echo_of_the_public_key`, `transfer::join_characterization::characterization_liveness_ping_is_unauthenticated` |
| F10 | Nearby accept: the prompt shows only self-asserted name, platform and version. The acceptance carries `room_code` and Bridge id in clear UDP. The `/join` that follows is not tied to the approved request. Join requests are not rate-limited | `discovery.rs:365`, `:865`; `commands.rs:3064` | Off-path: spoof a name. On-path: read the code | I: the user may accept an impostor. On-path, a full man-in-the-middle. A: prompt spam | Medium | n/a (LAN only) | Source |
| F11 | Room-server HTTP is plaintext. Bridge id, paths, transfer ids, sizes, file names in metadata and device names are visible; payloads are AEAD-encrypted | `transfer.rs:288` and every `http://` call site | On-path | C: metadata, and the Bridge id needed for F1/F8 | Low–Medium | n/a (QUIC encrypts) | Source |
| F12 | Diagnostics benchmark routes accept chunk-sized bodies from anyone who knows the Bridge id | `transfer.rs:3078` | Know the Bridge id | A: CPU and bandwidth | Low | Low | Source |

Consistent with the research input: one current peer per Bridge in production (`room_control.rs` default-peer resolution, legacy `rooms.peer_*` columns), and `/join` and reconnect accepting any key. The research note also says a remote EndpointId would be accepted only "after the paired device has signed it inside an already-authenticated Bridge session". In code, that session is authenticated only to transport keys that F1/F4/F6 let an attacker choose. A device signature exchanged there pins *whoever was on the other end*, so it is trust-on-first-use (TOFU) until the join itself is authenticated.

## 5. Fix options

### Option A: PAKE join with the code as password, then reconnect pinned to the device key (recommended)

- **First join:** run a balanced PAKE (CPace or SPAKE2) keyed by the code and the Bridge id. Put the PAKE message *in the room query itself*, replacing `room_code_hash`, so nothing offline-checkable crosses the wire. Each running room answers with its own PAKE message plus a key-confirmation tag; the joiner keeps only the offer whose tag verifies.
- **Session binding:** both sides then exchange, under the derived key, their transport public keys, HostRefs and Ed25519 device public keys, each signed by the device key over the `session_pair_ref` transcript. The receiver stores the peer's device public key per Bridge.
- **Online guessing:** an active attacker gets one guess per run (1 in 10^8). The creator invalidates the code after a small number of failed confirmations.
- **Reconnect:** carry a device-key signature over (Bridge id, both HostRefs, new `session_pair_ref`, nonce) in `/join`, and accept it only if it verifies against the pinned key. The code is not needed after the first join.
- **Cheap route checks in the same change:** `/items` and plain transfer starts must require `sender_public_key ==` current peer key. `/join` must refuse to replace a connected peer without the reconnect proof. Broadcast discovery must check the offer's authentication and room id. The liveness ping becomes a MAC challenge under the session key.
- **Change surface:** discovery wire format (new message kind; the old one removed, or kept only for version detection), `JoinRoomRequest`/`JoinRoomResponse` additive fields, a new `bridge_peers`/rooms column for the pinned device key, one new dependency (`spake2` from RustCrypto, or a small CPace implementation on `curve25519-dalek`, which is already in the tree through `x25519-dalek`), and tests that flip F1–F9.
- **Session semantics:** unchanged. Every reconnect is still a new `peer_session_id`; old `HostSessionBinding`s still fail; stale routes are still rejected. A HostRef becomes a verified claim, signed by the pinned device key, instead of a free one. HostRef derivation does not change, so persisted Plans and grants keep their values.
- **Migration:** Bridges without a pinned key need one decision. Either the first reconnect after upgrade is TOFU (pin whatever key appears) or the user must re-join with the code. Re-join is safer; TOFU is invisible to the user. This is your call.

### Option B: code-derived MAC over both ephemeral keys, plus a short authentication string

- **First join:** derive `K = KDF(code, Bridge id)`. Each side MACs its ephemeral transport key and HostRef with `K`. Both screens show a short authentication string (SAS) hashed from both keys for the users to compare.
- **Weakness:** `K` has 27 bits of entropy, so one observed MAC lets a passive LAN observer recover the code offline, exactly as F2 does today. Against an active attacker, security then rests entirely on the users comparing the SAS on every join; the code adds almost nothing.
- **Reconnect:** must still be pinned to the device key as in Option A, so the remaining work is the same.
- **Change surface and session semantics:** a slightly smaller change surface (no PAKE dependency), the same semantics, and a UX change (a SAS to compare on every join).

### Recommendation

Option A. It is the only option where the 8-digit code is actually a password: passive observation reveals nothing, and an active attacker gets one guess in 10^8 per attempt. Pinning reconnect to the device key reuses the identity and signature format that already exists. Reconnect then needs neither the code nor the user. That is also the property Stage 3 needs before an iroh `EndpointId` may be pinned through a Bridge session.

The nearby flow needs a separate decision, because it has no code the requester types. Either the acceptor shows the code and the requester enters it (then use Option A), or both screens show a SAS. The current approach of sending the code in a clear UDP reply cannot be made safe against an on-path attacker.

## 6. Characterization tests (superseded)

The audit added seven `characterization_*` tests that asserted the insecure behaviour; F4, F5, F10, F11 and F12 were confirmed from source only. The remediation replaced each test with a regression test asserting that the attack is refused, and added tests for the source-only findings (section 7).

## 7. Remediation (Stage 1)

Option A is implemented in `src-tauri/src/bridge_auth.rs` with SPAKE2 from RustCrypto's `spake2` 0.4 (Ed25519 group). It is wired into discovery, `/join`, reconnect, the room routes and the renderer. Both devices must run the new version; an older peer can neither pair nor reconnect, which fails closed.

Decisions applied:
- **Existing Bridges** are not trusted on upgrade: without a pin they do not reconnect, and they keep their local data. Startup recovery already clears every legacy route and key. A user re-pairs by showing a new code ("Show new code" in the Bridge network panel) and entering it on the other device. A code for an existing local Bridge is accepted only from the same logical Host as that Bridge's last known or pinned peer.
- **Nearby**: the accepting device shows a fresh one-time code and the requester types it. The acceptance carries no secret.

| # | Status | How | Evidence (all local tests, loopback) |
|---|---|---|---|
| F1 | Fixed | `/join` requires a pairing proof (SPAKE2 key confirmation plus device signature) or a reconnect proof (pinned-key signature over both nonces); otherwise 403 and nothing changes | `transfer::join_authentication::a_join_without_proof_is_refused_and_replaces_nothing`, `a_forged_or_replayed_pairing_join_is_refused`, `pairing_with_the_code_admits_and_pins_exactly_that_device` |
| F2 | Fixed | No code-derived value is sent; the query carries a randomized SPAKE2 message only. `hash_code` remains as a local uniqueness index | `bridge_auth::tests::a_pair_query_carries_nothing_derived_from_the_code_alone` |
| F3 | Fixed | Each answered query is one guess. A code answers 20 queries, expires after 15 minutes, and closes on success. Room-id lookup by hash no longer exists | `transfer::join_authentication::wrong_spent_and_overguessed_codes_admit_no_one`, `storage::tests::a_pairing_code_answers_a_bounded_number_of_guesses_then_closes` |
| F4 | Fixed | A LAN answer is taken only if its key confirmation proves the code (or, for reconnect, the pinned key signed it); a joiner evaluates at most 16 offers per attempt | `discovery::tests::a_forged_lan_offer_is_ignored_and_the_genuine_one_accepted`, `pairing_with_a_wrong_code_finds_nothing_and_times_out_truthfully`, `bridge_auth::tests::a_pairing_attempt_stops_evaluating_after_its_offer_budget` |
| F5 | Fixed | The joiner requires the `/join` reply's key and HostRef to equal the confirmed offer's and verifies the Host's signature and confirmation | `VerifiedPairOffer::verify_ack`, exercised by every pairing test |
| F6 | Fixed | Reconnect needs a fresh signature by the pinned device key; replay and other keys are refused; unpinned Bridges do not reconnect | `transfer::join_authentication::reconnect_requires_fresh_proof_by_the_pinned_device`, `an_unpinned_legacy_bridge_admits_no_reconnect`, `bridge_lifecycle::tests::an_unpinned_bridge_does_not_attempt_reconnect` |
| F7 | Mitigated | HostRef is still derived from `device_id` and still a claim, but it is signed into the pairing transcript and pinned together with the device key; reconnect must present the same pair | pairing and reconnect tests above |
| F8 | Fixed | `/items` and `/transfers/start` require the sender key to be the single connected peer's | `transfer::join_authentication::items_and_file_starts_from_a_non_member_are_refused` |
| F9 | Fixed | Liveness is a signed request answered with a MAC proving the room-server key, only for the current peer | `bridge_lifecycle::tests::liveness_probe_rejects_an_echo_of_the_public_key`, `transfer::join_authentication::ping_answers_only_the_current_peer_with_proof_of_its_key` |
| F10 | Fixed | The acceptance carries no secret; the code is typed. Prompts are limited per address and in total. The prompt is still self-described, but accepting it admits no one by itself | `discovery::tests::nearby_join_response_carries_no_code_or_session_material`, `join_request_prompts_are_bounded_per_source_and_in_total` |
| F11 | Open (metadata) | Room-server HTTP stays plaintext. The Bridge id is no longer a capability, but on-path observers see metadata | — |
| F12 | Fixed | Benchmark routes, transfer finish and cancel require a signed, single-use peer request | `transfer::join_authentication::transfer_control_and_benchmarks_need_a_signed_peer_request` |

Burn and departure: the Burn authority cutoff deletes the pin and any open code before other cleanup, and an authenticated departure deletes the pin (`storage::tests::a_pinned_peer_identity_ends_with_burn_and_with_departure`). Native Agent authority is unchanged: it is still bound to the exact current `HostSessionBinding`, so re-pairing or reconnecting never revives an earlier binding.

Remaining risks (not fixed in Stage 1):
- **On-path availability and metadata.** An attacker on the path sees Bridge ids, transfer ids and sizes. It can make a transfer fail by sending a garbage chunk to a transfer id it saw. Chunk AEAD does not bind the chunk index, so an on-path attacker could also reorder chunks within one transfer, though it cannot forge content. Encrypting the transport (Stage 3 QUIC) or binding the chunk index as AEAD associated data would close this.
- **Pairing denial of service.** Anyone who can reach UDP 48392 can spend a code's 20 guesses or crowd the 16 offers a joiner evaluates. The user then shows a new code. Guessing success stays below 2·10⁻⁷ per code.
- **Self-described HostRef and nearby prompt.** A paired device can claim any HostRef at pairing, so the user's code entry decides which device is trusted. The nearby prompt's device name is unauthenticated.
- **Device identity rotation** (`rotate_device_identity`) breaks pinned reconnect until the Bridge is paired again. This is intended, and it is not yet explained in the UI.
- **Not validated on physical devices.** Everything above is local, in-process or loopback tests. Two-device LAN, overlay and nearby flows have not been run.

Run the focused suites (they bind loopback TCP/UDP ports):

```bash
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey -- bridge_auth join_authentication discovery:: bridge_lifecycle:: storage::tests::a_p commands::tests::a_code
```

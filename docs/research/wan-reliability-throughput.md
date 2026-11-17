# WAN reliability and throughput: investigation (2026-11-15)

Status: investigation, plus Steps 1 and 2 of the proposal (the adaptive in-flight window, and signed peer traffic as liveness), implemented afterwards on branch `tunnel-adaptive-transfer-window`; see [Step 1 result](#step-1-result-emulated) and [Step 2 result](#step-2-result-emulated). The findings in Phases 1–3 describe the code before that change. The benchmark harness and its results are in `tools/net-probe/`.

## What kind of evidence this is

Every number below that is marked **emulated** comes from `tools/net-probe/examples/wan_bench.rs`: two iroh 1.3 endpoints in one process on one Mac (Apple silicon, macOS 26.5), talking through a user-space shaper that adds latency, a rate limit, random loss and outages. It shows how the software behaves under a stated condition. It is **not** a measurement of any WAN, NAT, hotspot or public relay, and nothing here claims a WAN improvement.

The only physical evidence in the repository is still:

- `tools/net-probe/results/2026-10-15-mac-single-machine.jsonl`: both endpoints on one Mac, relay-only through n0's public relays, 8 MiB per run, about 1.1 MiB/s at about 350 ms RTT.
- The changelog's Test connection figures over a phone hotspot (0.2–0.4 MB/s), without the per-run records.

Full tables: [`tools/net-probe/results/2026-11-15-wan-bench-emulated.md`](../../tools/net-probe/results/2026-11-15-wan-bench-emulated.md) (102 runs).

## Phase 1: the data path as built

```text
sender                                                        receiver
transfer.rs: reqwest, HTTP/1.1                                axum room server
  4 MiB chunks, 8 in flight, each one POST                      body buffered whole, decrypt,
  timeout 120 s total per chunk, 3 retries (0.3/0.8/1.5 s)      write at offset, JSON ack
        │ TCP 127.0.0.1                                                ▲ TCP 127.0.0.1
connectivity.rs: loopback forwarder                           connectivity.rs: splice
  one TCP connection = one QUIC bi-stream + JSON header         tokio::io::copy both ways
        └──────────── iroh QUIC connection (one per peer) ────────────┘
              default transport config, Cubic, direct or relay
```

Facts read from the code:

| Area | What the code does | Where |
| --- | --- | --- |
| Chunking | 4 MiB chunks, window 8 (1–16), so 32 MiB in flight; each chunk is one HTTP request with a 120 s total deadline | `transfer.rs:48,52`, `transfer_tuning.rs:1` |
| Retry | A timed-out or broken chunk is resent whole, 3 times, after 0.3, 0.8 and 1.5 s | `transfer.rs:63`, `send_chunk_with_retry` |
| Failure | Any chunk that exhausts its retries fails the transfer; the receiver deletes its `.part`; nothing resumes | `finish_sender_after_chunk_error`, `fail_receiver_transfer` |
| Receiver | Chunks are written at `index × chunk size` and tracked in `received_chunks`; a duplicate is acknowledged without rewriting | `receive_file_chunk_handler` |
| Progress | The sender reports progress only when a chunk is acknowledged | `send_binary_chunks_pipelined` |
| QUIC | iroh's defaults: 1.25 MB stream receive window, 10 MB send window, Cubic, 30 s idle timeout, 5 s keep-alive | `iroh_carrier.rs:88` (no `transport_config`), `noq-proto` defaults |
| Carrier link | One connection per peer, redialled on the next TCP connection after it closes, 12 s connect bound | `connectivity.rs::link_to` |
| Liveness | Every 2 s, one new HTTP request through the same tunnel, 5 s bound; 4 consecutive failures invalidate the session | `bridge_lifecycle.rs:32-33` |
| Invalidation | The session is marked reconnecting and the room server is stopped and restarted, which cancels the Bridge's transfers | `invalidate_unreachable_sessions`, `stop_room_server` |
| Probes are serial | One stalled tunnel probe delays every other Bridge's check by up to 5 s | `invalidate_unreachable_sessions` |

Layering overhead that looked suspicious and what the measurements say about it is in finding 5.

### Gaps in the earlier evidence

1. The 1.1 MiB/s figure comes from **8 MiB** transfers (about 7 s). At 350 ms a QUIC connection is still ramping up for the first seconds, so a sample this short cannot give a steady rate (emulated: an 8 MiB sample on an unlimited 350 ms relay path reads 1.5 MiB/s where the steady rate is 3.0).
2. Both endpoints were on **one machine and one uplink**, so each byte crossed that uplink twice and the relay twice.
3. The run pinned to `aps1-1` reports the same 348 ms RTT as the `usw1-1` runs. Either the pin did not take effect or the two relays are equally far; the file cannot tell.
4. No record separates a relay rate limit, the home uplink and the software. The run's HTTP part (eight 4 MiB chunks) also read 1.0 MiB/s.
5. There is no two-machine run, no direct-path run across NATs, no run longer than 8 MiB on a relay, and no record of loss or congestion-window counters from the hotspot Test connection runs (Pastey logs them as `link_benchmark_path`, but the lines were not kept).
6. Every matrix cell in `tools/net-probe/README.md` (C1–C6, R1–R3, M1) is still empty.

## Phase 2: findings (all emulated)

### 1. Below about 270 KiB/s a transfer cannot succeed, by construction

Eight 4 MiB chunks share the link and each must finish within 120 s: 32 MiB / 120 s = 273 KiB/s. Below that, every chunk of the first window times out, is resent whole three times and times out again. The transfer fails after about 8 minutes having delivered nothing.

| Link (150 ms RTT) | Window × chunk | Result |
| --- | --- | --- |
| 384 KiB/s, relay | 8 × 4 MiB (default) | Completes, but no progress is visible for 94 s (all eight chunks finish together) |
| 256 KiB/s, direct | default | **Fails** at 483 s: 4 timeouts, 0 bytes delivered |
| 256 KiB/s, relay | default | **Fails** at 483 s, 0 bytes delivered |
| 128 KiB/s, relay | default | **Fails** at 483 s, 0 bytes delivered |
| 256 KiB/s, relay | 2 × 4 MiB | Completes at the link rate (0.22 MiB/s) |
| 256 KiB/s, relay | 8 × 512 KiB | Completes at the link rate |
| 256 KiB/s, relay | raw QUIC, 1 stream | Completes at the link rate (0.23 MiB/s) |

The changelog's hotspot figures (0.2–0.4 MB/s) sit on this threshold. That the failures users see are this mechanism is a **hypothesis**; a transfer log from such a run (`chunk_attempt` lines with `timeout`) would confirm or refute it.

### 2. On a loaded relay path the liveness probe fails and would end the session

With Cubic, a relay path gives the sender no loss signal (the relay leg is TCP), so the congestion window grows until the queues toward the relay hold seconds of data. Measured path RTT under load in the emulation: 2–3 s at 1.1 MiB/s, 4.9 s at 384 KiB/s, 13–17 s at 256 and 128 KiB/s (base RTT 150–350 ms). Liveness probes ride the same connection and time out at 5 s.

| Relay path, Pastey-shaped transfer | Probe failures (longest run) | Session would be invalidated |
| --- | --- | --- |
| 1.1 MiB/s, 350 ms | 0; probe p50 2.2 s, max 2.8 s | No |
| 384 KiB/s, default window | 8 (3) | No (one short of the limit of 4) |
| 256 KiB/s, default window | 65 (60) | Yes, at 85 s |
| 256 KiB/s, window 2 or 512 KiB chunks | 12–13 (2–3) | No |
| 128 KiB/s, window 2 or 512 KiB chunks | 38–44 (20–23) | Yes, at 32 s |

In the app, invalidation restarts the room server and cancels the transfer. The harness only records the moment, so its transfers ran on; in Pastey they would have ended there. On the direct path the same links showed no probe failures (the shaper's UDP queue is short and drops, which Cubic reacts to).

How much queueing a real relay path builds depends on the real relay and socket buffers. The mechanism is observed; its size on n0's relays is **not measured**. The October probe's control round trips (p50 366 ms, p99 1.2 s during an 8 MiB bulk) are consistent with it.

### 3. Random loss collapses throughput with the default congestion controller

Direct path, 100 ms RTT, unlimited link:

| Loss | Raw, 1 stream | Pastey-shaped | Loss-free reference |
| --- | --- | --- | --- |
| 0.1 % | 0.98 MiB/s | 0.66 MiB/s | 10 MiB/s (1 stream), about 30 (Pastey-shaped) |
| 1 % | 0.16 MiB/s, not finished in 300 s | 0.08 MiB/s, not finished in 300 s | |

This matches what a loss-based controller is expected to do (about 1.22 × packet size / (RTT × √loss)). Whether Pastey's real paths (phone hotspot, Wi‑Fi) have loss in this range is **not measured**; the counters Pastey already reads (`PathCounters.lost_packets`, `congestion_events`, `cwnd`) would show it.

### 4. BBRv3 removed both collapses in the emulation

`noq-proto` 1.3, which iroh 1.3 uses, ships a BBRv3 controller. Switching to it is a transport-config setting on the sending endpoint; nothing on the wire changes.

| Case | Cubic (default) | BBRv3 |
| --- | --- | --- |
| 1 % loss, direct 100 ms, Pastey-shaped 64 MiB | 0.08 MiB/s, not finished | 22.3 MiB/s, done in 2.9 s |
| 1 % loss, raw 1 stream | 0.16 MiB/s | 5.2 MiB/s |
| Relay 256 KiB/s, window 2: path RTT under load | 2.7 s | 0.34 s |
| Relay 256 KiB/s, window 2: probe failures | 12 (3 in a row) | 0 |
| Relay 1.1 MiB/s, 350 ms: path RTT, probe max | 2.1 s, 2.8 s | 0.39 s, 1.0 s |
| Unlimited 350 ms, 256 MiB (direct / relay) | 15.0 / 14.6 MiB/s | 14.9 / 15.7 MiB/s |
| Loopback, no shaping, raw | 153 MiB/s | 161 MiB/s |
| Relay 256 KiB/s, **default window 8** | Fails (finding 1) | Still fails: finding 1 is independent |

This is the strongest single result, and also the one that most needs real-network confirmation before it ships: BBR's behaviour depends on real bottleneck queues and competing traffic, which a shaper does not reproduce.

### 5. The HTTP/1.1-over-loopback bridge is not a throughput bottleneck

At 350 ms RTT, unlimited link, 256 MiB:

| Workload | Direct | Relay |
| --- | --- | --- |
| Raw QUIC, 1 stream | 2.8 MiB/s | 3.0 MiB/s |
| Raw QUIC, 8 streams | 17.4 MiB/s | 17.6 MiB/s |
| Raw QUIC, 1 stream, 8 MiB stream window | 15.2 MiB/s | 14.5 MiB/s |
| Pastey-shaped, window 1 | 2.65 MiB/s | 2.65 MiB/s |
| Pastey-shaped, window 8 (default) | 15.0 MiB/s | 14.6 MiB/s |
| Pastey-shaped, window 16 | 18.4 MiB/s | 18.0 MiB/s |

- A single QUIC stream is capped by the default 1.25 MB stream window at about 1.2 MiB per round trip: 19 MiB/s at 50 ms, 10 at 100 ms, 5 at 200 ms, 3 at 350 ms.
- Pastey's eight parallel requests are eight streams, so the Pastey shape already runs about five times faster than one raw stream at 350 ms, and within 15 % of eight raw streams.
- On a 1.1 MiB/s link, raw (1 or 8 streams) and Pastey-shaped all deliver 0.97–1.0 MiB/s: the link, not the layering, sets the rate.
- Direct and relay are equal at equal latency in the emulation. A real relay adds its own distance, rate limits and load, none of which is modelled.
- With no shaping, the Pastey shape moves 64 MiB at 100 ms in about 2 s; loopback copies, HTTP framing and whole-body buffering do not show at WAN rates.

There is no evidence here for replacing the forwarder with HTTP on QUIC or a custom framing.

### 6. Small files and first progress

At 350 ms, 1 MiB takes 3.0 s and 16 MiB takes 5.0 s on an unlimited link, the same as raw QUIC for 1 MiB: this is connection ramp-up, not Pastey overhead. Because progress is per acknowledged chunk, a default transfer shows nothing for the first 4–5 s at 350 ms, 32 s on a 1.1 MiB/s link and 94 s at 384 KiB/s.

### 7. Outages

Pastey-shaped 128 MiB at 100 ms and 2 MiB/s; the path goes silent 15 s in.

| Outage | Direct | Relay |
| --- | --- | --- |
| 5 s, stalled | Survives on the same connection; no probe failures | Survives; no probe failures |
| 5 s, relay connections closed | n/a | Survives; iroh reconnects to the relay; 1 probe failure |
| 20 s | Survives; 3 probe failures in a row (limit 4) | Transfer bytes survive, but 4 probes fail in a row: **session invalidated** 26 s after the outage began, 6 s after it ended |
| 40 s | QUIC connection idles out at 30 s; session invalidated at 27 s | Same; invalidated at 26 s |
| 40 s, raw QUIC stream | Fails: `connection lost: timed out` | |
| 20 s, raw QUIC stream | Survives | |

So the limits stack: liveness ends the session after about 26–28 s of silence (sooner on a relay path that is still draining its queue), QUIC ends the connection at 30 s, and after either one the transfer restarts from zero. A chunk's own retry budget after the connection is gone is 3 retries, each waiting up to 12 s for a redial.

Harness limitation: in direct mode, after a redial iroh reaches the peer's real loopback address and leaves the shaper, so the direct 40 s row's throughput after the outage is not valid (its path is reported as `–`). The invalidation and idle-timeout observations before the redial are.

## Root causes, by confidence

| # | Cause | Evidence |
| --- | --- | --- |
| 1 | Fixed 32 MiB in flight against a 120 s per-chunk deadline: links under about 270 KiB/s always fail, and whole chunks are resent | Observed (emulated), follows from constants in the code |
| 2 | Liveness probes share a tunnel whose delay grows without bound on a loaded relay path; failure cancels the transfer | Mechanism observed (emulated); magnitude on real relays not measured |
| 3 | No resumption: any failure discards everything received | Read from the code |
| 4 | Cubic under random loss | Observed (emulated); real loss rates not measured |
| 5 | The 1.1 MiB/s relay figure | **Unexplained.** Consistent with a rate cap of roughly 1.3–1.5 MiB/s somewhere (a relay limit, or one uplink carrying the bytes twice) read through an 8 MiB sample. Not consistent with the software alone, which carries 3 MiB/s on one stream and 15 on eight at that latency in emulation |
| – | HTTP/1.1 over loopback bridged into QUIC | Measured **not** to be a bottleneck |

## Phase 3: recommended minimal changes, ranked

| Rank | Change | Measured benefit (emulated) | Reliability | Complexity | Regression risk |
| --- | --- | --- | --- | --- | --- |
| 1 | **Bound in-flight bytes by the link.** On a tunnel endpoint start with window 2 and adjust the existing runtime window from chunk completion times (grow while a chunk takes under about 10 s, shrink above about 30 s). Sender-only; `runtime_window` already exists | 256 KiB/s: fails → completes. 128 KiB/s: fails → completes | Removes the deterministic failure; progress appears 4× sooner | Low: one function near `send_binary_chunks_pipelined` | Low. LAN and overlay keep window 8. A fast high-latency link starts slower for a few chunks |
| 2 | **Do not count a probe failure while the session is carrying authenticated traffic.** A chunk acknowledged, or an authenticated request accepted, from that peer session in the last few seconds proves more than a ping | Would have prevented every invalidation in finding 2 where chunks were still completing | Removes the transfer-killing false positive | Low–medium: a per-session "last authenticated traffic" timestamp read in `invalidate_unreachable_sessions` | Medium. Liveness never authorizes, so there is no authority risk, but a truly dead session must still end within about half a minute: only traffic *received and authenticated* may count |
| 3 | **BBRv3 for the carrier**, behind a setting first | 1 % loss: 0.08 → 22 MiB/s. Relay path RTT under load: 2–17 s → 0.3–0.4 s. No change on clean links | Removes the cause of finding 2 rather than its symptom | Low in code (one `transport_config` call; needs `noq-proto` as a direct dependency pinned to iroh's version, because iroh does not re-export the type) | Medium–high until measured on real paths: BBR is young in this library, and can be unfair to other traffic |
| 4 | **Resume an interrupted transfer** (proposal below) | Not measured (not built) | An interruption costs the bytes in flight instead of the whole file | Medium | Medium: lifetime and cleanup of partial files, Burn |
| 5 | Raise the QUIC stream receive window to 8 MiB | 1 stream at 350 ms: 3 → 15 MiB/s. Nothing for the default window 8 | None | Trivial | Low; up to 8 MiB more buffered per stream. Worth doing only together with rank 1, which starts with fewer streams |

Not recommended:

- Replacing the loopback forwarder with HTTP over QUIC streams or a custom protocol: no measured benefit (finding 5), high regression risk, and it would touch the room server every Bridge route uses.
- Raising `CHUNK_REQUEST_TIMEOUT` alone: it moves the threshold but keeps 32 MiB in flight, 90 s without progress and whole-chunk resends.
- Raising the tunnel liveness bound alone: delays of 13–17 s were observed; a bound that long no longer detects a dead peer in useful time.

### Regression and compatibility notes

- Ranks 1, 3 and 5 are local to the sending Host: no wire change, no Bridge, Room Control or task-lifecycle change, and an old Host on the other side behaves as today.
- Rank 2 changes when a session is declared gone, on one Host, with no wire change. The half-minute bound for a silent peer must be kept and tested (existing tests: `reconnect_policy_is_bounded_before_disconnected`, `a_lost_probe_does_not_make_a_connected_session_unavailable`).
- None of the ranks touches the Transport–Session contract in `connectivity.rs`: the carrier stays a byte pipe, pins and admission are unchanged, a lost carrier is still an ordinary transport failure, and nothing is queued or rerun. Pastey still decides where bytes go, who may send them and when a transfer is complete; Agents are not involved.
- LAN and overlay paths are unaffected if rank 1 keys on `is_tunnel_endpoint` and rank 3 applies only to the iroh endpoint.
- Windows is untested for all of this. iroh's `fast-apple-datapath` is macOS-only, so absolute numbers will differ.

## Implementation proposal (separate; not started)

Order: evidence first, then the two low-risk changes, then the two that need real-network proof.

**Step 0 — collect the missing evidence (no app change).**
- Two machines on two networks: `net-probe` cells C1, C2, C4 with `--bulk-mib 256`, both directions, plus `--relay-only` controls.
- The same against a self-hosted `iroh-relay` with no rate limit, to split "n0 relay limit" from "path".
- One Pastey transfer over the hotspot with transfer logging on, keeping `chunk_attempt`, `transfer_benchmark_summary` and `link_benchmark_path` lines (loss, cwnd, RTT).
- Exit: the 1.1 MiB/s figure is attributed, and real loss and RTT-under-load are known.

**Step 1 — adaptive in-flight window on tunnel endpoints (rank 1). Done; see the result below.** Sender-only, in `transfer.rs`. Acceptance: `wan-bench` group F passes at 128 and 256 KiB/s; LAN throughput unchanged in the existing release benchmark; existing transfer tests pass. The LAN path does not run the new code; the release LAN benchmark was not rerun.

### Step 1 result (emulated)

Implemented as `transfer_tuning.rs::TunnelWindow`, used by `send_binary_chunks_pipelined` only when the peer endpoint is a tunnel forwarder and no diagnostic window override is set. It lowers the configured window and never raises it; the receiver, the wire format and LAN or overlay transfers are unchanged. The harness mirrors it as `adaptive=1` (group K).

| Link | Fixed window 8 | Adaptive | In flight at the end |
| --- | --- | --- | --- |
| 384 KiB/s relay, 32 MiB | Completes in 94.5 s; nothing visible for 94 s; 3 probe failures in a row | 94.5 s; first progress at 23 s; 2 in a row | 2 |
| 256 KiB/s relay | **Fails**, 0 bytes | Completes in 143 s (link rate) | 1 |
| 256 KiB/s direct | **Fails**, 0 bytes | Completes in 133 s | 1 |
| 128 KiB/s relay | **Fails**, 0 bytes | Completes in 288 s, but 15 probe failures in a row: the session would still be invalidated at 32 s (finding 2) | 1 |
| 64 KiB/s, direct and relay | Fails | **Still fails**: two 4 MiB chunks need 128 s, over the 120 s deadline | – |
| 1.1 MiB/s, 350 ms, 64 MiB | 63–66 s; first progress at 32 s | 64–67 s; first progress at 15 s | 4–6 |
| Unlimited, 350 ms, 16 MiB | 5.0 s | 6.1–6.2 s | 6 |
| Unlimited, 350 ms, 64 MiB | 7.0 s | 9.7–10.0 s | 8 |
| Unlimited, 350 ms, 256 MiB | 17.1–17.5 s | 17.8–18.0 s | 8 |
| Unlimited, 50 ms, 256 MiB, direct | 2.7 s | 2.9 s | 8 |

What it costs: on a fast, high-latency link the first six chunks ramp from 2 to 8 in flight, about 1–3 s on files of tens of MiB. What it does not fix: the floor is now about 70 KiB/s instead of 270 (a timeout does not shrink the window, because only acknowledged chunks are observed); liveness on a slow relay path (Step 2); resumption (Step 4). The 50 ms relay pair in group K (fixed 8.0 s, adaptive 4.3 s) ran while a build was using the machine and is not a comparison.

Automated checks: four unit tests of the controller in `transfer_tuning.rs`; the `transfer` (104) and `connectivity` (13) Rust suites pass. Not exercised then: a real Pastey transfer through a real tunnel. Since done on an emulated network (see the production-path results below); still not on a real network.

**Step 2 — traffic counts as liveness (rank 2). Done; see the result below.** In `bridge_lifecycle.rs`. Acceptance: a peer that stops answering is still invalidated within the current bound; a loaded but working tunnel is not.

### Step 2 result (emulated)

What counts: only a request that passed `verify_peer_request` on this Host's room server (the Bridge's current peer key, a MAC under the session's key agreement, a fresh timestamp, an unused nonce). The peer's own liveness probes are such requests, so each side sees the other's every round. A chunk acknowledgement does not count: it is an unsigned HTTP response. A tunnel session whose probe failed is treated as alive for that round if such a request arrived within 8 s (`TUNNEL_TRAFFIC_PROOF_WINDOW`). LAN and overlay sessions keep the probe as the only proof. Resolving a session for use (`prove_current_peer`) is unchanged and still needs the probe's key proof.

Bound for a peer that is really gone: 8 s after its last signed request, then four rounds of 5 s + 2 s: 36 s at most (before: 28 s). A unit test holds this.

The harness has one probing side, so it cannot run the rule itself. Group L records what the rule depends on: when the sender's pings reach the receiver's room server on a loaded relay path.

| Relay path (adaptive window unless noted) | Probes timed out for the sender (longest run) | Pings that reached the receiver | Longest gap between arrivals |
| --- | --- | --- | --- |
| 1.1 MiB/s, 350 ms | 0 | 21 | 3.8 s |
| 384 KiB/s | 6 (2) | 19 | 8.6 s |
| 256 KiB/s | 5 (2) | 24 | 10.5 s |
| 128 KiB/s | 37 (10): invalidated at 32 s under the old rule | 40 | 19.0 s |
| 256 KiB/s, fixed window 8 | 67 (63): invalidated at 32 s under the old rule | 55 | 22.7 s |
| 128 KiB/s, direct | 0 | 113 | 2.4 s |

Reading it: the requests do keep arriving while their replies time out, which is the premise. The gaps are longer than the 8 s window, so single failed rounds still count; ending the session takes four in a row, which needs roughly 29 s or more without one arrival (8 s of credit, then three more rounds and the next probe's bound). The longest observed gap is 23 s. So, **by arithmetic on these runs, not by running the rule**, none of these sessions would now be invalidated, including the two that were before. The other direction (the receiver's probes reaching the sender) travels against the data and was not modelled; it should queue less.

Not done at the time: the rule was not run end to end with two Hosts on a loaded tunnel. It has since run in emulation (see below), not yet on a real network. Automated checks: three unit tests in `bridge_lifecycle.rs` (the decision, the bound, the per-Bridge-and-key record); the `bridge_lifecycle` (21), `transfer` (104), `connectivity` (13), `room_control` (58) and `bridge_auth` (6) Rust suites pass. If real paths show gaps beyond about 29 s, the remaining lever is the queue itself (rank 3, BBRv3), not a longer window.

### Steps 1 and 2 on the production path (2026-11-17, emulated network)

Unlike `wan-bench`, this runs the product's own code end to end. Two independent Hosts in one test process: the Room Control pair harness, with separate data directories, SQLite databases, device and transport keys and room servers. Each runs the production `IrohCarrier` (its handshake hook, links, streams and default QUIC configuration), its own tunnel forwarder, room server, Transfer sender or receiver, and its Bridge lifecycle loop (liveness, invalidation, reconnect) on the real 2 s schedule. Only the network is emulated (`connectivity/shaped_net.rs`, the `wan-bench` shaper). Direct: a UDP hop with delay, rate, loss and blackouts, with iroh pinned to it. Relay: an in-process iroh relay behind a TCP hop, both endpoints relay-only. The same test files ran against three builds: `before` (2214d79, neither step), `step1` (19621ac) and `step1+2` (77607a9). No product timeout or assertion was changed for this. Results: `tools/net-probe/results/2026-11-17-wan-pair-production-path.{jsonl,md}`.

Each scenario checks five things: the file is intact on the receiver, exactly once (SHA-256 over the receiver's whole data directory); no `.part` is left; no active transfer state is left on either side; progress events never go backwards; and when either Host's session left `Connected`.

| Scenario | before | step1 | step1+2 |
| --- | --- | --- | --- |
| Direct 256 KiB/s, 350 ms, 32 MiB | **Fails** after 489 s, 0 bytes (×2) | 147–149 s, first progress at 36 s (×2) | Same as step1 (×2) |
| Relay 384 KiB/s, 150 ms, 32 MiB | **Cancelled**: both sessions invalidated at 60–78 s (×2) | 106 s, first progress at 26 s, no invalidation (×2) | Same (×2) |
| Relay 128 KiB/s, 150 ms, 16 MiB | **Cancelled**: invalidated at 37 s (×2) | **Cancelled**: invalidated at 34–41 s (×2) | 154–161 s, no invalidation; first progress only at 78–86 s (×2) |
| Direct, 1% loss, 100 ms, 16 MiB | 106 s; first progress at 105 s | 121 s; 64 s | 98 s; 42 s |
| 15 s outage, direct 384 KiB/s | Completes, session kept | Completes, kept | Completes, kept |
| 10 s outage, loaded relay 384 KiB/s | Completes, session kept | Completes, kept | Completes, kept |
| 45 s outage, direct 384 KiB/s | Invalidated about 29 s into the silence; transfer cancelled, nothing left; reconnected 0.2 s after the network returned; a new 1 MiB file arrived once | Same | Same, invalidated about 31 s in (within the 36 s bound) |
| Direct 64 KiB/s, 16 MiB (below the floor) | Fails at 488 s, nothing left, session kept | Fails at 485 s, same | Fails at 485 s, same |
| Receiver Burns at 22 s | Sender returns 103 s after the Burn; nothing left; carrier pin and room server gone | Returns 12 s after | Returns 12 s after |
| Sender Burns at 22 s | Sender returns 105 s after the Burn; nothing left in the end | 15 s; receiver clean at the same moment | 15 s; same (0.05 s after the fix below) |
| Codex continuation beside a relay 384 KiB/s transfer, 5 s outage during Review | Review before any run; Accept is one-shot; runs once; Completed; file once | Same | Same |

Reading it:

- **Step 1 is necessary and sufficient for the slow direct link.** The 256 KiB/s case fails deterministically without it and completes at link rate with it.
- **On a loaded relay, liveness ended the session and so the transfer, before either step.** With the production lifecycle running, the before build's 384 KiB/s relay transfer was cancelled by invalidation (the session's room server is stopped, which cancels its transfers). `wan-bench` could not show this because it only counted probes. Step 1 alone fixes the 384 KiB/s case: less in flight, less queue. At 128 KiB/s the relay RTT still reached 15–19 s, and only Step 2 kept the session: there it is the deciding change.
- **Step 2 did not keep a dead session.** After a real 45 s silence both Hosts still invalidated, inside the documented 36 s bound, and reconnected at once when the network returned.
- **Exactly once held everywhere.** No scenario left two copies or a partial file. The Agent continuation ran once in every build, despite congestion and an outage during Review.
- **Burn ends in a clean state in all builds, but not promptly** (finding below).

New finding, not caused by either step (all builds): **Burn does not abort chunk uploads already in flight.** After the sender Burns, its in-flight chunk requests keep uploading. The receiver still decrypts, writes and acknowledges them (about 12 s after the Burn in the step1+2 run). Only then does the sender stop and the receiver delete the partial file. The same happens when the receiver Burns: its room server shuts down gracefully and finishes requests already accepted. The delay is bounded by the in-flight chunks: 103–105 s with the old window of eight 4 MiB chunks, 12–15 s with Step 1 at this rate, and in the worst case up to one chunk timeout (120 s). The test's own expectation, that the sender stops within 5 s of its Burn, fails in all three builds and was left as written.

**Fixed afterwards (same day): in-flight uploads end on cancel and Burn.** `send_chunk_with_retry` now races each attempt against the transfer's cancel token. A cancel, a Burn and a lost session (which cancels a Bridge's transfers) drop the request and close its connection; no change to the protocol or the receiver. A unit test (`cancelling_ends_a_chunk_upload_already_in_flight`) holds it, and fails without the change. All shaped scenarios were rerun on the fixed build (`step1+2+abort` in the results), and all 11 pass:

| Scenario | step1+2 | step1+2+abort |
| --- | --- | --- |
| Sender Burns at 22 s | Sender returns 15 s after the Burn; the receiver wrote and acknowledged two chunks meanwhile | Returns **0.05 s** after; the receiver completes no chunk (no `chunk_received` after the Burn) and is clean 17 s after |
| Receiver Burns at 22 s | Sender returns 12 s after | 13 s after (the sender is not told; its uploads end on their own) |
| Every other scenario | – | Same outcomes as step1+2 |

What remains, by design or for later: across the internet the local tunnel forwarder has already handed part of each chunk to QUIC. Those bytes still cross the link, and the receiver discards them as an incomplete request. At 256 KiB/s that took about 17 s before the receiver got the cancel and cleared its partial file. Ending them would need the forwarder to reset the QUIC stream, which it cannot tell apart from an ordinary close without a tunnel protocol change. When the receiver Burns, its room server still finishes reading requests it has accepted; it writes nothing, but the sender learns of the Burn only when they end.

Limits of this evidence:

- One machine and one process: no real NAT, relay distance, real loss pattern or competing traffic. Both Hosts share one scheduler, CPU and the process-wide log. It is not two app processes, the packaged app or the Tauri command layer. Transfers start from `send_room_file_to_bridge_peer_endpoint`, which `send_file_to_room` calls.
- After a redial on the direct path, iroh leaves the shaper for the real loopback path; the `wan-bench` limitation still applies. So the transfer after the 45 s outage (1 MiB in 0.38 s against a 384 KiB/s limit) was not shaped. The reconnect itself is valid; post-reconnect throughput is not evidence.
- The relay hop shares one rate bucket per direction across both Hosts' relay connections, a simplification of two separate uplinks.
- Each cell ran once, and the three deciding cells twice, with identical outcomes; no wider statistics.
- Not covered: Windows, sleep/wake, a network change mid-transfer (as opposed to a blackout), a relay that drops or rate-limits per connection, and Burn while a Native Agent workspace movement is in flight.

Run them with `cargo test --manifest-path src-tauri/Cargo.toml --bin pastey connectivity::wan_pair_tests -- --ignored --nocapture --test-threads 1`. They need local sockets, and take about 15 minutes plus 8 for the below-floor case. The quick harness check, `a_shaped_direct_path_carries_a_verified_transfer_between_two_hosts`, runs in the normal suite.

**Step 3 — BBRv3 behind a developer setting (rank 3), default off.** Turn it on by default only after Step 0's matrix is repeated with it on real paths and shows no regression on direct LAN-like paths.

**Step 4 — resumable transfers (rank 4), its own design review.** Sketch: on an *interrupted* transfer (not a cancel, an integrity failure or a Burn) the receiver keeps the `.part` and its `received_chunks` for a bounded time; a new `transfers/start` for the same sender Host, file identity, size and chunk size is answered with the chunk indexes already held; the sender still reads and hashes the whole file but sends only the missing chunks; the final digest check is unchanged. Additive and negotiated in the start request and response, so an older Host on either side falls back to a full send. Open questions for the review: how the file identity is bound to the authenticated session, where partial files live and when they are deleted, and how this meets Burn and writer-lock rules. This is the only item that changes the transfer protocol, which is why it is last and separate.

## Reproducing

```bash
cargo build --release --example wan_bench --manifest-path tools/net-probe/Cargo.toml
```

```bash
sh tools/net-probe/wan-bench.sh
```

```bash
python3 -I tools/net-probe/wan-bench-summarize.py tools/net-probe/results/wan-bench-emulated.jsonl
```

The full matrix takes about 35 minutes (the slow-link failures run out their timeouts). `WAN_GROUPS="A F"` runs a subset; one case is `target/release/examples/wan_bench mode=relay work=pastey rtt_ms=150 rate_kib=256 mib=32`. The harness copies Pastey's constants and its `splice`; if those change in `transfer.rs`, `connectivity.rs` or `bridge_lifecycle.rs`, update `examples/wan_bench.rs` to match.

What the harness does not model: real NATs and hole punching, a real relay's distance, load and rate limits, real loss patterns, competing traffic, Windows, disk and encryption cost (irrelevant at these rates: LAN transfers run at about 100 MB/s), and Pastey's start and finish requests.

# Stage 2 results

Evidence levels:
- **Physical cross-network:** two real devices on two different networks. *None yet.*
- **Real network, single machine:** both endpoints ran on one Mac, using the real internet relays. This shows relay reachability and relay-path cost from this network, but a "direct" path is a local path. It is not cross-NAT evidence.
- **Loopback:** an in-process test relay on 127.0.0.1. This proves the tool's logic only.
- **Build only:** compiled, not run.

## 2026-10-15, MacBook (macOS 26.5.2, arm64), this Mac's current network

### This network (real network)

- **NAT mapping:** the STUN probe (`docs/research/native-nat-traversal-poc/stun_probe.py`) saw one public IP and the same mapped port toward Google and Cloudflare STUN, so the mapping is endpoint independent. The port is not preserved. Filtering behaviour cannot be measured this way.
- **IPv6:** no global address.
- **Home relay:** iroh picked n0 US-West (`usw1-1`) in 7 of 7 default runs. Plain ICMP from here averaged 187 ms to US-West and 207 ms to Asia-Pacific (`aps1-1`), so US-West is the nearer relay from this network.

### Single machine through n0's public relays

Raw runs are in `results/2026-10-15-mac-single-machine.jsonl`.

| Configuration | Runs | Connect | Path | RTT | Bulk | Control during bulk (p50 / p99) | HTTP over QUIC |
|---|---|---|---|---|---|---|---|
| Direct allowed, 64 MiB | 2 | 2–3 ms | direct immediately | 0.7–1.1 ms | 98.4–99.2 MiB/s, digest ok, 0 relay bytes | 3.3–7.0 / 10.7–14.5 ms | 20 pings (p50 0.09 ms), 8 × 4 MiB at ~90 MiB/s, digests ok, peer id injected correctly, `ConnectInfo` route returns 500 |
| Relay only (US-West), DNS on, 8 MiB | 2 | 329–337 ms | relay, never direct | 344–346 ms | 1.1 MiB/s, digest ok | 364–367 / 1154–1252 ms | ok; ping p50 329–358 ms; chunks ~1.0 MiB/s |
| Relay only, `--no-dns` | 2 | 408–420 ms | relay | 375–402 ms | 1.1 MiB/s, digest ok | 352–368 / 1150–1252 ms | ok |
| Relay only, pinned Asia-Pacific relay | 1 | 346 ms | relay | 348 ms | 1.1 MiB/s, digest ok | 380 / 1252 ms | not run |
| Relay only, pinned US-West relay | 1 | 344 ms | relay | 347 ms | 1.1 MiB/s, digest ok | 382 / 1214 ms | not run |

How to read this:
- A relayed connection between two devices behind NAT works, keeps data intact, and carries the room-server HTTP shape unchanged.
- From this network it costs about 350 ms per round trip and about 1.1 MiB/s.
- The throughput was the same through two different relays with different RTTs, which suggests a per-connection rate limit on n0's free relays rather than a latency bound. That is inferred, not confirmed.
- During a bulk transfer, a small control message waits up to about 1.2 s at p99 on the relay path, against 11–15 ms on a local direct path.
- **Relay fallback is therefore usable for control and small items, but not for file transfer.** A direct path is what makes cross-network Transfer viable.

### Tool checks (loopback)

`cargo test` in this crate passed 6 of 6, and passed 3 runs in a row. The tests show:
- a direct loopback path carrying bulk, HTTP and a correct peer identity;
- a relay-only peer reported as relay and never direct;
- an unlisted key refused after the handshake, with code 403 and no stream served;
- three listed hosts served at once while a fourth, unlisted key is refused;
- a listener restarted on a new port, recovered by redialling the same key through the relay.

A separate end-to-end CLI check on this Mac ran `id`, `listen --allow` and `dial` by ticket as two processes. It reached direct with digest and HTTP intact, and a stranger key was rejected with 403.

### Windows

The probe cross-compiles for `x86_64-pc-windows-gnu` (24.3 MB PE32+). That is **build only**: it has not run on Windows.

### Not measured: the whole physical matrix

C1–C6, R1–R3 and M1 in `README.md` have not been run.

**Blocker:** this session runs on one Mac on one network. There is no Windows machine, second network, phone hotspot or second location it can operate. The `pastey-physical` tool server exposed no tools in this session.

The binaries, matrix, steps and summarizer are ready. `python3 -I summarize.py results/*.jsonl` reports every cell still untested.
- The `net_desc` text "home Wi-Fi" in the raw single-machine runs was my label at run time; the network type was not verified.

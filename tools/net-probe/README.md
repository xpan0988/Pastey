# net-probe — Stage 2 cross-NAT probe (research only)

`net-probe` measures whether two of your devices on different networks can reach each other with iroh 1.3, and how. It is a standalone crate with its own `[workspace]`. It is not a dependency of the Pastey app, not in its bundle, and it never touches a Pastey Bridge, key or database.

What one run measures:
- **Connection:** how long the QUIC handshake took, the first selected path (`relay` or `direct`), when a direct path was selected, the final path, the RTT, and every path change.
- **Bulk:** one stream of N MiB with a BLAKE3 digest. Small control echoes run concurrently on another stream, giving p50/p99, which approximates Room Control latency during a large Transfer. Also recorded: the share of time spent on direct, and the bytes sent on relay versus direct paths.
- **HTTP-over-QUIC:** an axum router shaped like Pastey's room server, served over one QUIC bi-stream with HTTP/1.1:
  - 20 pings and 8 × 4 MiB chunk POSTs, each checked by digest;
  - whether the router sees the authenticated peer through an injected identity instead of `ConnectInfo<SocketAddr>`;
  - what a route that still extracts `ConnectInfo` returns over QUIC (expected 500).
- **Watch:** one probe per second for S seconds. It records outages, whether each recovered on the same connection or needed a redial, sleep gaps, and path changes.
- **Membership gate:** the listener's `after_handshake` hook accepts only the probe keys you allow; any other key is closed with code 403.

Every run prints exactly one JSON line. Field definitions are in `src/lib.rs` (`RunReport`).

The probe keys (`*.key`) are throwaway keys for this tool only, separate from Pastey's device identity. Never reuse them.

## Build

macOS (Rust via rustup):

```bash
cargo build --release --manifest-path tools/net-probe/Cargo.toml
```

The binary is `tools/net-probe/target/release/net-probe`.

Windows, native in PowerShell (Rust via rustup, MSVC toolchain), from the repository root:

```powershell
cargo build --release --manifest-path tools\net-probe\Cargo.toml
```

The binary is `tools\net-probe\target\release\net-probe.exe`. A Mac can also cross-build it with `cargo build --release --target x86_64-pc-windows-gnu --manifest-path tools/net-probe/Cargo.toml` (MinGW required); copy `target/x86_64-pc-windows-gnu/release/net-probe.exe` across.

On the first `listen`, Windows Defender Firewall asks about network access. Record what you chose; allowing *private* only blocks the probe when Windows classifies the network as public.

Local self-checks (loopback with an in-process relay; they prove the tool, not real networks):

```bash
cargo test --manifest-path tools/net-probe/Cargo.toml
```

## One run

Run these on the two devices, called **L** (listener) and **D** (dialer). Collect `results/*.jsonl` from the dialer, and run each cell in both directions: Mac as L, then Windows as L.

1. Once per device, create a probe key and note its id. On Windows, use `net-probe.exe` and `--key win.key`.

   ```bash
   net-probe id --key mac.key
   ```

2. On L, allow D's id; L prints a ticket line:

   ```bash
   net-probe listen --key mac.key --allow <D_ID> --net "C2 L: Windows home Wi-Fi, Optus"
   ```

3. On D, send the ticket by chat or email; it contains only an id, the relay URL and addresses. Then run ten attempts with bulk and HTTP:

   ```bash
   for i in $(seq 1 10); do net-probe dial "<TICKET>" --key mac.key --cell C2 --bulk-mib 256 --http --net "C2 D: Mac on iPhone hotspot, Telstra" >> results/C2.jsonl; done
   ```

   On Windows:

   ```powershell
   1..10 | % { .\net-probe.exe dial "<TICKET>" --key win.key --cell C2 --bulk-mib 256 --http --net "C2 D: Windows ..." >> results\C2.jsonl }
   ```

4. For stability, keep one watch run going and do the cell's action during it:

   ```bash
   net-probe dial "<TICKET>" --key mac.key --cell R1 --watch 300 --net "R1 D: Mac Wi-Fi then hotspot" >> results/R1.jsonl
   ```

The `--net` text should name the network type, ISP or carrier, and anything unusual (VPN on, guest network). Do not paste public IP addresses. The probe does not record them itself.

Other options:
- `--relay none` uses the ticket's addresses only.
- `--relay https://your-relay` uses a self-hosted `iroh-relay`.
- `--no-dns` stops publishing to and looking up in n0 DNS (ticket only).
- `--relay-only` disables direct paths, as a control.
- `--no-portmap` disables UPnP/NAT-PMP/PCP.

## Matrix

| Cell | Setup | Runs | Expected | Pass |
|---|---|---|---|---|
| C1 | Mac and Windows on the same Wi-Fi | 10 dial, both directions | direct within 1 s | 10/10 direct, digests intact, HTTP ok; records the Windows firewall behaviour |
| C2 | Windows on home Wi-Fi, Mac on a phone hotspot (CGNAT) | 10 + 10 | direct after relay bootstrap | ≥ 8/10 reach direct within 10 s; 100% digests |
| C3 | Home ↔ university or corporate Wi-Fi | 10 + 10 | direct often blocked (UDP filtering, client isolation) | truthful: if not direct, `final_path` is `relay` and nothing is lost |
| C4 | Hotspot ↔ hotspot, two carriers | 10 + 10 | hardest common case | record the rate; no pass bar |
| C5 | Sydney ↔ mainland China | 10 + 10, plus `--relay-only` controls | n0 relays may be slow or blocked from China | record whether the relay is reachable at all (`connect.error`), and the direct rate |
| C6 | UDP blocked on the Mac (see below) | 5 | no direct; relay over HTTPS if reachable | `time_to_direct_ms` null, digests intact or a clean error |
| R1 | Watch 300 s; switch the Mac from Wi-Fi to hotspot at about 60 s and back at about 180 s | 1 per direction | outage, then recovery | each outage has `recovered_by`; none left open |
| R2 | Watch 600 s; sleep the Mac 2 min, wake it | 1 | `suspend_gaps_ms` recorded, then recovery | recovered within 30 s of wake |
| R3 | Watch 300 s; quit L at about 60 s, start it again with the same key and the **same ticket** | 1 | redial via relay URL | `recovered_by: "redial"` |
| M1 | One Windows L allowing two dialer keys; Mac on a hotspot and a second device on another network dial at the same time | 5 each | both served | all digests intact; a third, unlisted key is rejected (`connect.rejected: true`) |

UDP blocking for C6 on macOS. This needs administrator rights and must be undone afterwards:

```bash
echo "block drop out quick proto udp from any to any port != 53" | sudo pfctl -a com.apple/netprobe -f - && sudo pfctl -E
sudo pfctl -a com.apple/netprobe -F all
```

The second command removes the rule.

## Summarize

```bash
python3 -I tools/net-probe/summarize.py tools/net-probe/results/*.jsonl
```

This prints, per cell:
- connect and direct rate, median time to direct, and how many runs ended on the relay;
- median throughput, digest failures, the relay share of bulk bytes and control p99;
- HTTP successes, and watch outages with how each recovered.

It also lists every matrix cell that has no runs. Single-machine runs (`net-probe local`) are listed separately and are never cross-NAT evidence.

## Zero-cost infrastructure

Default bootstrap uses n0's free public relays, whose terms say "development and hobby use only". Keep runs modest: 256 MiB per run is plenty. The probe can point at a self-hosted relay (`--relay URL`) or use none at all (`--relay none`), and costs nothing else.

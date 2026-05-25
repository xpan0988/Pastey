# Transfer Hot Path Notes

These notes describe Pastey's current file-transfer hot path, developer transfer tuning, and how diagnostics relate to real transfers.

## Current Transfer Path

Binary-v1 is the normal high-performance transfer path. Each file is split into encrypted chunks, encoded as binary-v1 frames, sent over the LAN peer endpoint, acknowledged per chunk, and finalized after the receiver verifies the expected chunk count and size.

Binary-v1 senders use pipelined in-flight chunk uploads. Receivers accept chunks by `chunk_index` and write each chunk at its file offset, so chunks may complete out of order. The receiver tracks a per-transfer received bitmap; finalize still verifies completeness before marking the transfer complete.

The old stop-and-wait behavior is historical context only. It is still useful as a conceptual baseline for `window=1`, but normal binary-v1 transfers use a larger window.

SQLite room item status is not written per chunk. Status writes happen at terminal states such as sent, failed, cancelled, interrupted, burned, and finalize. Hot-path progress and non-error logs are sampled or throttled to avoid creating self-inflicted I/O overhead.

## Transfer Window Tuning

- Normal binary-v1 transfers default to `window=8`.
- There is no user-facing MB/s transfer limiter; normal transfers do not sleep for speed pacing.
- The old user-facing transfer-limit mapping, such as "10 MB/s -> window 1", "50 MB/s -> window 2", and "100 MB/s -> window 4", is obsolete and should not be reused.
- Developer benchmarking can force a window with `PASTEY_TRANSFER_WINDOW_SIZE`; values are clamped to `1..16`.

`window=1` means the sender uploads one encrypted chunk and waits for its ACK before starting the next chunk. Larger windows allow multiple encrypted binary-v1 chunks to be in flight at the same time, which keeps a fast LAN link busier while the receiver decrypts, writes, and acknowledges earlier chunks.

Increasing the window is expected to improve throughput only until something else becomes the bottleneck: link bandwidth, receiver CPU, disk writes, queueing, or OS/network buffers. Scaling is not guaranteed to be linear, and very large windows can increase memory usage, latency, and receiver backlog.

The normal Settings UI does not expose window size. Developer tools can expose a Transfer Window block with Default / Auto, `window=1`, `window=2`, `window=4`, `window=8`, `window=16`, and custom window options. Enable it with a debug build or `PASTEY_DEV_TOOLS=1`.

For release-build benchmarking, start Pastey with `PASTEY_TRANSFER_WINDOW_SIZE` set to one of `1`, `2`, `4`, `8`, or `16`. The environment override takes precedence over the dev Settings value. Invalid non-numeric values fall back to the dev Settings value or default; numeric values outside the supported range are clamped to `1..16`.

Manual launch examples:

```sh
PASTEY_TRANSFER_WINDOW_SIZE=8 open /Applications/pastey.app
```

```powershell
$env:PASTEY_TRANSFER_WINDOW_SIZE="8"; Start-Process "pastey.exe"
```

The transfer logs include `event=transfer_tuning` at transfer start with `effective_window_size`, `chunk_size`, `override_source`, and `transfer_protocol`. Successful binary-v1 transfers also emit `event=transfer_benchmark_summary` with sender and receiver timing summaries, average throughput, failed chunk count, duplicate chunk count, and finalize status.

## Real Transfer Benchmark Checklist

Use the same large file, same sender, same receiver, same network, and release builds only. Record one row per forced window:

- `window=1`
- `window=2`
- `window=4`
- `window=8`
- `window=16`

For each run, record:

- `average_MBps`
- receiver CPU
- sender CPU
- `decrypt_ms`
- `write_ms`
- `send_ack_ms`
- duplicate chunks
- failed chunks
- finalize success

Real file transfers are the only end-user transfer-speed measurement because they include network behavior, Pastey's protocol, file read/write, Inbox/finalize behavior, and UI lifecycle.

## Diagnostics Benchmarks

Device Diagnostics are lightweight, non-destructive, local-first, advisory, and developer-oriented. They do not upload payloads to cloud services, run disk stress tests, write benchmark payloads to Inbox, or write benchmark payloads to disk.

Diagnostics modes are different from real file transfer measurements:

- Loopback raw memory measures a same-device memory/socket baseline over localhost.
- Loopback Pastey pipeline measures same-device encrypted/framed pipeline overhead over localhost.
- Peer raw benchmark measures discard-only device-to-device LAN behavior between trusted room peers.
- Peer Pastey pipeline benchmark adds Pastey's encrypted/framed payload path for peer LAN diagnostics, while still discarding benchmark payloads.
- Real file transfer measures the full user path: network, protocol, file I/O, Inbox/finalize, and UI lifecycle.

Loopback tests stay on the same device. They do not measure Wi-Fi, Ethernet, router, ISP, school network, or internet speed. Peer benchmarks are the diagnostics path for device-to-device LAN behavior.

Diagnostics signals, including `DeviceProfile`, capability probing, CPU/GPU/runtime display, and `recommended_roles`, are advisory. `recommended_roles` are internal hints and are not user-facing permissions.

## Room and Inbox Lifecycle

Rooms are manual-burn lifecycle objects in-session. A room should not be destroyed only because a default expiry duration elapsed.

Burning a room stops active coordination, marks or interrupts active transfer state, removes transient room data, removes temporary payloads and `.part` state where applicable, and prevents new work from being added to that burned room.

Burn is not the same thing as deleting user-owned output. Inbox-saved received files and images are durable output and must not be silently deleted by Burn. Transient received items may be cleaned during Burn or recovery.

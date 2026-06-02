# Transfer Hot Path Notes

These notes describe Pastey's current file-transfer hot path, developer transfer tuning, and how diagnostics relate to real transfers.

## Current Transfer Path

Global Transfer Scheduler v1 is frontend orchestration only. Multi-file picker and drag/drop input is normalized into an in-memory queue, and the scheduler uses a weighted planner to start runnable queued file-like transfers. Before send start, queued file-like items resolve and cache display name, MIME type, size, modified time, and dedupe metadata. Metadata failure marks the queued item failed without starting a transfer. Each ordinary runnable plan still calls the existing `sendFileToRoom` wrapper and Rust `send_file_to_room` command for one file. Eligible tiny file-like queue items may be represented as a scheduler-only `MicroFlowGroup`; the current group runner dispatches each child serially through the same single-file path and records internal group status as queued, running, completed, completed with errors, cancelled, or interrupted. Queued sends pass the frontend queue item id as optional progress-correlation metadata so outgoing progress can attach to the intended queued item without relying only on room id, display name, and size. `sendFileToRoom` resolution or rejection remains the authoritative completion signal for each queued item.

Binary-v1 is the normal high-performance transfer path. Each file is split into encrypted chunks, encoded as binary-v1 frames, sent over the LAN peer endpoint, acknowledged per chunk, and finalized after the receiver verifies the expected chunk count and size.

Binary-v1 senders use pipelined in-flight chunk uploads. Receivers accept chunks by `chunk_index` and write each chunk at its file offset, so chunks may complete out of order. The receiver tracks a per-transfer received bitmap; finalize still verifies completeness before marking the transfer complete.

The scheduler can start multiple existing queued file-like transfers when the planner allocates runnable plans. A serial `MicroFlowGroup` consumes exactly one planner requested window while its children run one at a time. A failed child makes the internal group finish as completed with errors unless batch cancellation or room interruption stops the group first. Active outgoing binary-v1 sender transfers can update their future in-flight window after a planner-managed queue item reaches a terminal state. This is completion-only runtime window mutation; it is not retry/timeout adaptive control, speed-history tuning, archive bundling, folder recursion, a new chunk protocol, binary-v2, substream multiplexing, or backend-owned scheduling. Queue item correlation metadata, MicroFlowGroup ids/status, and requested sender windows are not part of the binary-v1 or JSON chunk protocols. Text sending remains immediate and does not enter the file queue. File type may affect labels, but transport behavior is opaque binary file transfer and does not branch on file type.

The old stop-and-wait behavior is historical context only. It is still useful as a conceptual baseline for `window=1`, but normal binary-v1 transfers use a larger window.

SQLite room item status is not written per chunk. Status writes happen at terminal states such as sent, failed, cancelled, interrupted, burned, and finalize. Hot-path progress and non-error logs are sampled or throttled to avoid creating self-inflicted I/O overhead.

## Transfer Window Tuning

- Normal binary-v1 transfers default to `window=8`.
- There is no user-facing MB/s transfer limiter; normal transfers do not sleep for speed pacing.
- The old user-facing transfer-limit mapping, such as "10 MB/s -> window 1", "50 MB/s -> window 2", and "100 MB/s -> window 4", is obsolete and should not be reused.
- Developer benchmarking can force a window with `PASTEY_TRANSFER_WINDOW_SIZE`; values are clamped to `1..16`.
- Planner-driven file-like sends pass an optional sender-side requested window. Non-planner sends omit it and keep the default unless an env or effective Developer Tools override is active.
- Serial MicroFlowGroup sends pass the group requested window, currently 1, to each child transfer. This is a planner request only and remains below env/dev overrides.
- For active outgoing binary-v1 planner-managed sends, a completion-only rebalance can update the sender's runtime window for future chunk scheduling. Existing in-flight chunks are not cancelled when the window decreases, and JSON fallback transfers return a structured no-op for runtime window updates.

`window=1` means the sender uploads one encrypted chunk and waits for its ACK before starting the next chunk. Larger windows allow multiple encrypted binary-v1 chunks to be in flight at the same time, which keeps a fast LAN link busier while the receiver decrypts, writes, and acknowledges earlier chunks.

Increasing the window is expected to improve throughput only until something else becomes the bottleneck: link bandwidth, receiver CPU, disk writes, queueing, or OS/network buffers. Scaling is not guaranteed to be linear, and very large windows can increase memory usage, latency, and receiver backlog.

The normal Settings UI does not expose window size. Developer Tools can expose a Transfer Window block with Default / Auto, `window=1`, `window=2`, `window=4`, `window=8`, `window=16`, and custom window options. Release builds keep Developer Tools hidden by default; enable the Settings toggle when release diagnostics are needed. `PASTEY_DEV_TOOLS=1` still force-enables the same tools for developer sessions.

For release-build benchmarking, start Pastey with `PASTEY_TRANSFER_WINDOW_SIZE` set to one of `1`, `2`, `4`, `8`, or `16`. The environment override takes precedence over the dev Settings value and any planner requested window. Effective Developer Tools transfer-window settings also override planner requested windows. Invalid non-numeric env values fall back to the dev Settings value, planner request, or default; numeric values outside the supported range are clamped to `1..16`.

Manual launch examples:

```sh
PASTEY_TRANSFER_WINDOW_SIZE=8 open /Applications/pastey.app
```

```powershell
$env:PASTEY_TRANSFER_WINDOW_SIZE="8"; Start-Process "pastey.exe"
```

The transfer logs include `event=transfer_tuning` at transfer start with `effective_window_size`, `chunk_size`, `override_source`, and `transfer_protocol`. Successful binary-v1 transfers also emit `event=transfer_benchmark_summary` with sender and receiver timing summaries, average throughput, failed chunk count, duplicate chunk count, and finalize status.

Frontend scheduler diagnostics are bridged into the normal app log with allowlisted prefixes. `[pastey:planner]` records launch-plan summaries, `[pastey:micro-group]` records planned, launched, running, child_running, child_terminal, stopped, and final group lifecycle events, and `[pastey:runtime-window]` records planner runtime-window tracking start, update attempts, and per-transfer window summaries. These lines use room ids, group ids, queue item ids, display names, sizes, counts, statuses, terminal reasons, requested/effective windows, override source, and transfer protocol when the frontend can infer it. They must not include absolute file paths.

MicroFlowGroup validation requires at least two eligible children in the same grouping key. A single sub-1 MiB file, or a batch whose small files are mostly over `maxChildSizeBytes = 1 MiB`, will not emit `[pastey:micro-group]` lifecycle lines. In that case, inspect the `[pastey:planner] event=launch_summary` fields `tiny_candidates`, `eligible_tiny_candidates`, `largest_eligible_micro_group_bucket`, `over_child_size_limit`, and `micro_group_skip_reason`.

Runtime-window summaries are emitted when the frontend observes a terminal queue state, including normal completion and frontend-known cancel/burn paths. If the app process exits while a transfer is active, frontend terminal cleanup cannot run; use the earlier `[pastey:runtime-window] event=tracking_started` and `event=update` lines as the durable pre-exit evidence.

Example validation searches, using the app log path for the current platform:

```sh
rg "\\[pastey:(planner|micro-group|runtime-window)\\]" /path/to/pastey.log
rg "\\[pastey:planner\\].*micro_group_skip_reason" /path/to/pastey.log
rg "\\[pastey:micro-group\\].*event=final" /path/to/pastey.log
rg "\\[pastey:runtime-window\\].*event=summary" /path/to/pastey.log
```

## Local Dev-Fast Transfer Testing

Normal Tauri dev uses Cargo's `dev` profile. That keeps the edit/run loop quick, but it under-represents transfer throughput because the Rust transfer hot path runs without optimization and still carries debug-profile overhead.

Use `npm run tauri:dev-fast` for local transfer-throughput testing. It keeps the Tauri dev workflow and Vite frontend server, but runs the Rust app with Pastey's optimized `dev-fast` Cargo profile. This mode is intended for realistic local transfer testing before scheduling work; it is not a production release replacement.

Packaged release builds remain the final production benchmark. Use release artifacts when validating end-user throughput before shipping.

## Real Transfer Benchmark Checklist

Use the same large file, same sender, same receiver, same network, and the same app mode. For local pre-release transfer testing, use `npm run tauri:dev-fast`; for final production validation, use packaged release builds. Record one row per forced window:

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

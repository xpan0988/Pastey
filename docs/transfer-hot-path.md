# Transfer Hot Path Notes

Investigation summary for the LAN throughput collapse:

- Sender chunk upload was stop-and-wait: read one chunk, encrypt it, encode it, POST it, wait for that chunk ACK, emit progress, then continue.
- Receiver chunk handling enforced strict `expected_chunk_index == chunk_index`, so concurrent or out-of-order binary chunk uploads were rejected.
- ACK generation is per chunk in `receive_file_chunk_handler`; sender ACK waiting was inside `send_chunk_with_retry`.
- Frontend progress was emitted per acknowledged sender chunk and per received receiver chunk.
- SQLite room item status is not written per chunk; status writes happen at terminal states such as sent, failed, cancelled, and finalize.
- Logging was in the per-chunk hot path on sender and receiver. Each log line goes through `logging::write_transfer_line`, which opens/appends the log file.
- Binary-v1 frame encode/decode lives in `src-tauri/src/chunk_frame.rs`; the binary sender still builds one per-chunk request body, but avoids JSON/base64.
- Receiver `.part` writes previously opened the part file in append mode for each chunk and flushed each chunk.

Implemented first incremental fix:

- Binary-v1 senders use a conservative window of 4 in-flight chunk uploads.
- Receiver accepts chunks by `chunk_index` and writes each chunk at its file offset, allowing pipelined chunks to complete out of order.
- Receiver tracks a per-transfer received bitmap; finalize still verifies the received chunk count and size.
- Hot-path progress and non-error logs are sampled/throttled to reduce self-inflicted I/O overhead.
- Sampled timing logs now report sender read/encrypt/send/ACK timing and receiver decode/decrypt/write/UI timing without paths, keys, or content.

Speed limit wiring:

- The Settings UI saves `speed_limit_mbps` through `update_config`.
- Rust persists it in `StoredConfig` and active transfers read `state.config`, so pacing changes are visible without app restart.
- Existing pacing applies to both binary-v1 and legacy JSON/base64 paths after ACK progress.
- Binary-v1 now also maps the configured MB/s limit to its transfer window at transfer start:
  - Unlimited: window 4
  - 100 MB/s and higher: window 4
  - 50 MB/s or custom up to 50: window 2
  - 10 MB/s or lower: window 1
- Benchmark runs can force a window with `PASTEY_TRANSFER_WINDOW_SIZE`; values are clamped to 1..16.

## Transfer Window Tuning

`window=1` is equivalent to the old stop-and-wait behavior: the sender uploads one encrypted chunk and waits for its ACK before starting the next chunk. Larger windows allow multiple encrypted binary-v1 chunks to be in flight at the same time, which keeps a fast LAN link busier while the receiver decrypts, writes, and acknowledges earlier chunks.

Increasing the window is expected to improve throughput only until something else becomes the bottleneck: link bandwidth, receiver CPU, disk writes, queueing, or OS/network buffers. Scaling is not guaranteed to be linear, and very large windows can increase memory usage, latency, and receiver backlog.

The normal Settings UI does not expose window size. For release-build benchmarking, start Pastey with `PASTEY_TRANSFER_WINDOW_SIZE` set to one of `1`, `2`, `4`, `8`, or `16`. Invalid values fall back to the speed-limit mapping; numeric values outside the supported range are clamped to `1..16`.

Manual launch examples:

```sh
PASTEY_TRANSFER_WINDOW_SIZE=8 open /Applications/pastey.app
```

```powershell
$env:PASTEY_TRANSFER_WINDOW_SIZE="8"; Start-Process "pastey.exe"
```

## Benchmark Checklist

Use the same large file, same sender, same receiver, same network, and release builds only. Record one row per forced window:

- `window=1`
- `window=2`
- `window=4`
- `window=8`
- `window=16`

For each run, record:

- average MB/s
- receiver CPU
- sender CPU
- `decrypt_ms`
- `write_ms`
- `send_ack_ms`
- duplicate chunks
- failed chunks
- finalize success

The transfer logs include `event=transfer_tuning` at transfer start with `configured_speed_limit_mbps`, `effective_window_size`, `chunk_size`, `override_source`, and `transfer_protocol`. Successful binary-v1 transfers also emit `event=transfer_benchmark_summary` with sender and receiver timing summaries.

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

- Binary-v1 senders use pipelined in-flight chunk uploads.
- Receiver accepts chunks by `chunk_index` and writes each chunk at its file offset, allowing pipelined chunks to complete out of order.
- Receiver tracks a per-transfer received bitmap; finalize still verifies the received chunk count and size.
- Hot-path progress and non-error logs are sampled/throttled to reduce self-inflicted I/O overhead.
- Sampled timing logs now report sender read/encrypt/send/ACK timing and receiver decode/decrypt/write/UI timing without paths, keys, or content.

Default transfer tuning:

- Normal binary-v1 transfers default to `window=8`.
- Release-build LAN testing found `window=8` to be the best observed stable default: `window=4` reached about 96-103 MB/s, `window=8` reached about 111 MB/s, and `window=16` averaged about 107 MB/s with higher ACK wait.
- There is no user-facing MB/s transfer limiter; normal transfers do not sleep for speed pacing.
- Developer benchmarking can still force a window with `PASTEY_TRANSFER_WINDOW_SIZE`; values are clamped to 1..16.

## Transfer Window Tuning

`window=1` is equivalent to the old stop-and-wait behavior: the sender uploads one encrypted chunk and waits for its ACK before starting the next chunk. Larger windows allow multiple encrypted binary-v1 chunks to be in flight at the same time, which keeps a fast LAN link busier while the receiver decrypts, writes, and acknowledges earlier chunks.

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

## Benchmark Checklist

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

The transfer logs include `event=transfer_tuning` at transfer start with `effective_window_size`, `chunk_size`, `override_source`, and `transfer_protocol`. Successful binary-v1 transfers also emit `event=transfer_benchmark_summary` with sender and receiver timing summaries.

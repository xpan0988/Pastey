# Layer 1 — Secure transfer substrate

Layer 1 owns the encrypted byte-oriented transfer path used when an Agent task needs resources or results to cross Hosts. Transfer supports execution placement when capability and resource locations differ; it does not define Pastey's product identity. This is the canonical documentation for binary-v1 mechanics and transfer lifecycle. Scheduling and capacity admission belong to [Layer 3](layer-3-orchestration.md); Bridge target resolution belongs to [Layer 4](layer-4-bridge.md); managed step continuation belongs to [Layer 5](layer-5-agent.md).

## Transport model

Pastey transfers text, files, and images through an encrypted Bridge session. File type affects metadata and UI presentation only: file-like transfer operates on encrypted bytes, not format-specific transfer adapters. The normal high-throughput path is `binary-v1`; JSON/base64 remains a compatibility fallback when binary-v1 negotiation is unavailable.

The implementation includes UDP LAN discovery, nearby join and code-join plumbing, payload encryption and key wrapping, local item persistence, the transfer runtime, and binary chunk framing. Primary implementation boundaries are `src-tauri/src/discovery.rs`, `crypto.rs`, `chunk_frame.rs`, `transfer.rs`, `storage.rs`, and their Tauri command callers.

## Binary-v1 lifecycle

For a file-like transfer, the sender divides the file into 4 MiB chunks, encrypts each one with ChaCha20-Poly1305 bound to its transfer id, index, total, size and final flag, and sends each chunk as one HTTP request to the resolved peer endpoint. Up to the transfer's window of chunks (8 by default) are in flight at once; the sender advances as acknowledgements arrive. The receiver authenticates each chunk, writes it at its position, and at finalization checks the expected chunk count, total bytes and the sender's signed BLAKE3 digest of the whole file before exposing a terminal outcome. A malformed or forged chunk request is refused on its own and does not cancel the transfer; a chunk that fails authentication is never accepted as payload bytes.

The sender may update a supported active binary-v1 window at runtime. Window policy is Layer 3 policy; Layer 1 provides the update primitive and does not create its own scheduler.

Layer 1 reports that an encrypted transfer safely landed. It does not read a Bridge Plan, decide which managed primitive follows, infer Transform, or create Layer 5 execution authority. A landed native workspace likewise only waits for the executing Host's executor Review.

## Routing and handoff boundaries

Layer 4 validates a current-session route before ordinary text or file-like delivery. A failed selected-peer route has no arbitrary legacy fallback. Ordinary-data selected-peers and broadcast are expanded according to Layer 4 and Layer 3 rules; each file-like target still uses the existing selected-peer transfer path.

The live durable Bridge Plan Search → Transfer workflow reuses this encrypted path only when its approved destination is the requesting device, after the receiver validates a requester-selected bounded result against its private candidate store. A selected-device Pastey Shared destination is instead copied locally by the selected Host. An explicit PipelinePrivate Transfer also reuses this transport, lands in an app-owned private area, and is registered through the shared safe physical-file identity path before Layer 5 may bind a later authored Transfer. `handoff_queued` is not transfer completion: it says only that the existing queue accepted a source. It does not say that bytes transferred, an endpoint accepted the transfer, or the transfer completed.

## Cancellation, recovery, and failure

Cancellation, disconnect, burn, and terminal guards keep interrupted work from becoming active again. Burn removes the Bridge session's local encrypted payloads, transient received files, partial files, Bridge items, and active receiver transfer state. Each Bridge payload key is separately wrapped by the existing installation secret and is removed at the authority cutoff, so deleted Bridge ciphertext is not recoverable through retained Pastey key state. User-saved Inbox output remains user-owned and is not removed by Burn. This is logical deletion plus cryptographic erasure of Pastey-managed Bridge recoverability; it is not a physical secure-erase guarantee for SQLite/WAL/journal or filesystem blocks.

Startup recovery clears stale active-transfer state and partial files. It does not recreate active work or durable routes. A route that expires or becomes unavailable fails according to the current-session route policy rather than rebinding to a later endpoint.

## Compatibility and limits

Transfer runs over whatever path the Bridge session uses: the LAN, an overlay address, or the optional internet carrier, which tunnels the same HTTP routes and may fall back to a relay ([Connectivity](layer-4-connectivity.md)). Over a relay it is slow, and an interrupted transfer fails rather than resuming. There is no WebRTC/TURN path, mobile release support, binary-v2 stream multiplexing, or second independent transfer stack.

Layer 1 sends one file per transfer and has no general archive or folder bundling for ordinary sends. Folders cross Hosts only through Layer 5: native workspace movement and its Return, and authored managed regular-file-set Transfer, each frame a bounded set of regular files (at most 1,024 entries) into one Host-private `pastey-rfs-transfer-v1` package (`src-tauri/src/regular_file_set_transfer.rs`). Layer 1 carries that package as one ordinary file, and the receiving Host validates and materializes it. Symlinks, special files, empty directories and executable modes are not represented ([Layer 5](layer-5-agent.md)).

For protocol and lifecycle names, see [reference.md](../reference.md). For automated and manual validation, see [development.md](../development.md).

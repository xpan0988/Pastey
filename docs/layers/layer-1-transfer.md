# Layer 1 — Secure LAN transport

Layer 1 owns the encrypted byte-oriented LAN transfer path. It is the canonical documentation for binary-v1 mechanics and transfer lifecycle. Scheduling and capacity admission belong to [Layer 3](layer-3-orchestration.md); Bridge target resolution belongs to [Layer 4](layer-4-bridge.md); managed step continuation belongs to [Layer 5](layer-5-agent.md).

## Transport model

Pastey transfers text, files, and images through an encrypted Bridge session. File type affects metadata and UI presentation only: file-like transfer operates on encrypted bytes, not format-specific transfer adapters. The normal high-throughput path is `binary-v1`; JSON/base64 remains a compatibility fallback when binary-v1 negotiation is unavailable.

The implementation includes UDP LAN discovery, nearby join and code-join plumbing, payload encryption and key wrapping, local item persistence, the transfer runtime, and binary chunk framing. Primary implementation boundaries are `src-tauri/src/discovery.rs`, `crypto.rs`, `chunk_frame.rs`, `transfer.rs`, `storage.rs`, and their Tauri command callers.

## Binary-v1 lifecycle

For a file-like transfer, the sender creates an encrypted payload, divides it into chunks, sends chunks to the resolved peer endpoint, and advances only through acknowledgements. The receiver authenticates encrypted chunks, checks expected chunk count and total bytes at finalization, then exposes a terminal outcome. Corrupted or malformed chunks fail rather than being accepted as payload bytes.

The sender may update a supported active binary-v1 window at runtime. Window policy is Layer 3 policy; Layer 1 provides the update primitive and does not create its own scheduler.

Layer 1 reports that an encrypted transfer safely landed. It does not read a Bridge Plan, decide which managed primitive follows, infer Transform, or create Layer 5 execution authority.

## Routing and handoff boundaries

Layer 4 validates a current-session route before ordinary text or file-like delivery. A failed selected-peer route has no arbitrary legacy fallback. Ordinary-data selected-peers and broadcast are expanded according to Layer 4 and Layer 3 rules; each file-like target still uses the existing selected-peer transfer path.

The live durable Bridge Plan Search → Transfer workflow reuses this encrypted path only when its approved destination is the requesting device, after the receiver validates a requester-selected bounded result against its private candidate store. A selected-device Pastey Shared destination is instead copied locally by the selected Host. An explicit PipelinePrivate Transfer also reuses this transport, lands in an app-owned private area, and is registered through the shared safe physical-file identity path before Layer 5 may bind a later authored Transfer. `handoff_queued` is not transfer completion: it says only that the existing queue accepted a source. It does not say that bytes transferred, an endpoint accepted the transfer, or the transfer completed.

## Cancellation, recovery, and failure

Cancellation, disconnect, burn, and terminal guards keep interrupted work from becoming active again. Burn removes the Bridge session's local encrypted payloads, transient received files, partial files, Bridge items, and active receiver transfer state. User-saved Inbox output remains user-owned and is not removed by Burn.

Startup recovery clears stale active-transfer state and partial files. It does not recreate active work or durable routes. A route that expires or becomes unavailable fails according to the current-session route policy rather than rebinding to a later endpoint.

## Compatibility and limits

The current stack is LAN-only. It has no cloud relay, WebRTC/TURN fallback, mobile release support, binary-v2 stream multiplexing, archive/folder bundling, whole-file hash exchange beyond authenticated chunks and finalize metadata, or second independent transfer stack.

For protocol and lifecycle names, see [reference.md](../reference.md). For automated and manual validation, see [development.md](../development.md).

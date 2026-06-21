# Transfer Architecture

This is the current source of truth for Pastey's Layer 1 secure LAN transfer stack. For scheduler and runtime-window policy, see [scheduler.md](scheduler.md). For validation commands and smoke-test boundaries, see [validation.md](validation.md). For the project-wide layer contract, see [../architecture/Project-specifications.md](../architecture/Project-specifications.md). For Bridge semantics, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For future multi-peer target semantics, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md).

## Current Stack

Pastey uses encrypted Bridge sessions and binary-v1 as its normal high-throughput, format-agnostic file transport. File type may affect display metadata, but the transfer path operates on encrypted bytes rather than per-format adapters.

Legacy implementation term: room. Existing code and storage paths still use room naming during the Bridge terminology migration.

Current production paths include:

- UDP discovery and nearby join: `src-tauri/src/discovery.rs`.
- Bridge lifecycle commands: `src-tauri/src/commands.rs`. Legacy implementation term: room lifecycle.
- Local Bridge item persistence and recovery: `src-tauri/src/storage.rs`. Legacy implementation term: room/item persistence.
- Payload encryption and key wrapping: `src-tauri/src/crypto.rs`.
- Transfer runtime: `src-tauri/src/transfer.rs`.
- Binary-v1 framing: `src-tauri/src/chunk_frame.rs`.
- Frontend send wrappers and queue entry points: `src/lib/tauri.ts`, `src/App.tsx`, and `src/pages/RoomPage.tsx`.

## Transfer Flow

Text send is selected-peer authoritative before it reaches Tauri. The frontend derives an exact selected-peer Bridge route, includes a text-only `bridgeRoute` payload in `send_text_to_room`, and Rust validates that route against the current single-peer room endpoint/key state before creating the outgoing item. After validation, text is converted to bytes, encrypted, persisted as a Bridge item, and sent through the existing Bridge transfer path.

Queued file/image dispatch is also selected-peer authoritative before it reaches Tauri. File picker, drag/drop, and pasted-image enqueue paths require a selected-peer route before entering the queue. The queue item remains room-scoped and current-session route metadata is not persisted; `App.tsx` re-derives the selected-peer route from live Room state at dispatch and passes a file-only `bridgeRoute` payload in `send_file_to_room`. Rust validates that route before creating the outgoing file item and starting binary transfer.

Future multi-device routing defaults should be explicit: text can default to broadcast only after a separate production design enables it, while files and images should default to selected-peer routing with optional selected-peers or broadcast support. File/image broadcast must be explicit and should show the target count. The current production paths are still primarily two-peer unless separate multi-peer runtime and validation evidence exists.

Files and images are treated as opaque file-like byte streams. Large files are split into encrypted chunks, sent over the LAN peer endpoint, acknowledged per chunk, and finalized after the receiver verifies expected chunk count and total bytes. JSON/base64 remains the compatibility fallback where binary-v1 negotiation is unavailable.

## Encryption And Identity Boundary

Payload bytes use ChaCha20-Poly1305 authenticated encryption. Runtime transport keys are wrapped through the Bridge session/key exchange path. Legacy implementation term: room session/key exchange. A corrupted encrypted chunk fails authentication.

These properties are transport and payload security properties. They do not establish durable authenticated device identity. A successful encrypted Bridge session is a current communication relationship, not reusable trust and not execution authority.

## Binary-v1 Runtime Windows

Binary-v1 supports sender-side runtime windows. The effective sender window follows this precedence:

```text
PASTEY_TRANSFER_WINDOW_SIZE env override
  -> effective Developer Tools transfer-window override
  -> planner requestedWindow
  -> default binary-v1 window 8
```

Layer 3 can update supported active outgoing binary-v1 sender windows without canceling or restarting the transfer. Real local outgoing Bridge control demand lowers the data target from `8` to `7`; idle restores `8` after the quiet period. The transfer protocol itself does not gain new fields for that scheduler policy.

## Lifecycle And Recovery

Bridge sessions can be created, joined, left, disconnected, and burned. Nearby accept or 8-digit code join creates accepted peer status only for the current Bridge session.

Burning a Bridge session removes the session's local encrypted payloads, transient received files, related partial files, Bridge items, and active receiver transfer state. User-saved Inbox output remains user-owned.

Startup recovery clears stale active transfer state and partial files so interrupted transfers do not revive as active work.

Bridge leave, disconnect, burn, and startup recovery do not create durable history, durable device identity, reusable trust, or execution authority.

## Non-Current Scope

The current transfer stack does not implement:

- durable authenticated device identity;
- whole-file hash exchange beyond authenticated chunks and finalize metadata;
- Linux/mobile release support;
- cloud relay, WebRTC, TURN, or internet transfer;
- binary-v2 stream multiplexing;
- archive/folder bundle transfer;
- backend-owned scheduler replacement.

Those topics should be scored only when production runtime and validation evidence exist.

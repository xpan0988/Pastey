# Transfer Architecture

This is the current source of truth for Pastey's Layer 1 secure LAN transfer stack. For scheduler and runtime-window policy, see [scheduler.md](scheduler.md). For validation commands and smoke-test boundaries, see [validation.md](validation.md). For the project-wide layer contract, see [../architecture/项目布局规范.md](../architecture/项目布局规范.md).

## Current Stack

Pastey uses encrypted room sessions and binary-v1 as its normal high-throughput, format-agnostic file transport. File type may affect display metadata, but the transfer path operates on encrypted bytes rather than per-format adapters.

Current production paths include:

- UDP discovery and nearby join: `src-tauri/src/discovery.rs`.
- Room lifecycle commands: `src-tauri/src/commands.rs`.
- Local room/item persistence and recovery: `src-tauri/src/storage.rs`.
- Payload encryption and key wrapping: `src-tauri/src/crypto.rs`.
- Transfer runtime: `src-tauri/src/transfer.rs`.
- Binary-v1 framing: `src-tauri/src/chunk_frame.rs`.
- Frontend send wrappers and queue entry points: `src/lib/tauri.ts`, `src/App.tsx`, and `src/pages/RoomPage.tsx`.

## Transfer Flow

Text is converted to bytes, encrypted, persisted as a room item, and sent through the room transfer path.

Files and images are treated as opaque file-like byte streams. Large files are split into encrypted chunks, sent over the LAN peer endpoint, acknowledged per chunk, and finalized after the receiver verifies expected chunk count and total bytes. JSON/base64 remains the compatibility fallback where binary-v1 negotiation is unavailable.

## Encryption And Identity Boundary

Payload bytes use ChaCha20-Poly1305 authenticated encryption. Runtime transport keys are wrapped through the room session/key exchange path. A corrupted encrypted chunk fails authentication.

These properties are transport and payload security properties. They do not establish durable authenticated device identity. A successful encrypted session is a current communication relationship, not reusable trust and not execution authority.

## Binary-v1 Runtime Windows

Binary-v1 supports sender-side runtime windows. The effective sender window follows this precedence:

```text
PASTEY_TRANSFER_WINDOW_SIZE env override
  -> effective Developer Tools transfer-window override
  -> planner requestedWindow
  -> default binary-v1 window 8
```

Layer 3 can update supported active outgoing binary-v1 sender windows without canceling or restarting the transfer. Real local outgoing room-control demand lowers the data target from `8` to `7`; idle restores `8` after the quiet period. The transfer protocol itself does not gain new fields for that scheduler policy.

## Lifecycle And Recovery

Rooms can be created, joined, left, and burned. Burning a room removes the room's local encrypted payloads, transient received files, related partial files, room items, and active receiver transfer state. User-saved Inbox output remains user-owned.

Startup recovery clears stale active transfer state and partial files so interrupted transfers do not revive as active work.

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

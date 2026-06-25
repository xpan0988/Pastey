# Transfer Architecture

This is the current source of truth for Pastey's Layer 1 secure LAN transfer stack. For scheduler and runtime-window policy, see [scheduler.md](scheduler.md). For validation commands and smoke-test boundaries, see [validation.md](validation.md). For the project-wide layer contract, see [../architecture/Project-specifications.md](../architecture/Project-specifications.md). For Bridge semantics, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For multi-peer target semantics, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md).

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

Text send is route-authoritative before it reaches Tauri. The Room UI builds an ordinary data route for selected peer, selected peers, or explicit broadcast, includes a text-only `bridgeRoute` payload in `send_text_to_room`, and Rust validates that route against the current-session `bridge_peers` endpoint table before creating the outgoing item. After validation, text is converted to bytes, encrypted, persisted as a Bridge item, and sent to the endpoint/key resolved for each target. The returned item can include `BridgeSendOperation` with one `BridgeDeliveryOutcome` per resolved peer and an aggregate status of completed, partial, or failed.

Queued file/image/pasted-image dispatch is also route-authoritative before it reaches Tauri. File picker, drag/drop, and pasted-image enqueue paths require a route before entering the queue. Selected-peers and explicit broadcast are resolved at enqueue time into per-target child queue items. Each child carries current-session in-memory route fields, target peer display data, and a shared `bridgeOperationId`; each child still dispatches through the existing single-file selected-peer transfer path. `App.tsx` passes the child selected-peer file `bridgeRoute` payload in `send_file_to_room`. Rust validates that route against `bridge_peers` before creating the outgoing file item and starting binary transfer to the selected peer endpoint/key.

The Tauri route payload shape expresses selected-peer, selected-peers, and explicit broadcast targets. The backend stores current-session peer endpoints in `bridge_peers` and mirrors the existing one-peer room endpoint into that table. Rust parses all target kinds and validates them against peer rows. Selected-peer delivery fails closed when the requested peer is unknown, stale, expired, disconnected, reconnecting, or otherwise unrouteable. Selected-peers delivery rejects malformed, duplicate, and unknown targets before delivery; known stale, expired, disconnected, reconnecting, or otherwise unrouteable targets become rejected per-target outcomes while routeable selected targets can still complete. Broadcast resolves only the current routeable accepted peer set at send time or enqueue time and fails closed when no peers are routeable. There is no fallback to an arbitrary legacy peer when route validation fails.

Broadcast is explicit and current-session scoped. It is not durable group history and does not retroactively include peers that join later or remove peers that leave later from the original operation record. File/image broadcast shows the current target count before enqueue and creates one child per resolved target.

Durable pairing is display/recognition metadata only. `bridge_durable_identities` can store a paired-device label, pairing fingerprint, pairing method, revocation state, and rotation state, while routeable endpoint host/port/key data remains only in current-session `bridge_peers` rows. Paired status can be shown in the Room target selector, but selected-peer, selected-peers, and broadcast delivery still resolve only current `connected` routeable rows. A paired identity alone cannot receive data, join a Bridge, revive old routes, or grant Agent Bridge consent.

Files and images are treated as opaque file-like byte streams. Large files are split into encrypted chunks, sent over the LAN peer endpoint, acknowledged per chunk, and finalized after the receiver verifies expected chunk count and total bytes. JSON/base64 remains the compatibility fallback where binary-v1 negotiation is unavailable.

## Encryption And Identity Boundary

Payload bytes use ChaCha20-Poly1305 authenticated encryption. Runtime transport keys are wrapped through the Bridge session/key exchange path. Legacy implementation term: room session/key exchange. A corrupted encrypted chunk fails authentication.

These properties are transport and payload security properties. Paired-device recognition, when present, is separate metadata. A successful encrypted Bridge session is a current communication relationship, not reusable trust and not execution authority.

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

Reconnect is represented through current-session liveness and route replacement. A peer with changed endpoint host, endpoint port, or transport public key receives a new current-session `peer_session_id`; the old row is marked stale, its endpoint/key fields are cleared, and old selected-peer queue children fail with route-expired instead of rebinding to the new peer row.

Burning a Bridge session removes the session's local encrypted payloads, transient received files, related partial files, Bridge items, and active receiver transfer state. User-saved Inbox output remains user-owned.

Startup recovery clears stale active transfer state and partial files so interrupted transfers do not revive as active work. Previously connected peer rows are marked expired and endpoint/key material is cleared.

Bridge leave, disconnect, burn, and startup recovery do not create durable history, durable device identity, reusable trust, or execution authority.

## Non-Current Scope

The current transfer stack does not implement:

- durable route recovery or auto-join from paired-device identity;
- whole-file hash exchange beyond authenticated chunks and finalize metadata;
- Linux/mobile release support;
- cloud relay, WebRTC, TURN, or internet transfer;
- binary-v2 stream multiplexing;
- archive/folder bundle transfer;
- backend-owned scheduler replacement.

Those topics should be scored only when production runtime and validation evidence exist.

# Bridge Control Transport

Bridge control transport is the Layer 4/Layer 5 control plane for Agent Bridge events. It is separate from Bridge text and file transfer. For project-wide layer boundaries, see [../architecture/Project-specifications.md](../architecture/Project-specifications.md). For Bridge membership and authority semantics, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For the target routing model, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md).

Legacy implementation term: room-control transport. Existing production code and tests still use `RoomControlEvent`, `room_control.rs`, and related names.

## Contract

Bridge control events are typed current-session values transported through the Bridge session and validated on receipt. They carry Agent Bridge previews, status events, execution requests, and execution results. They are not Bridge items, not durable workflow records, and not displayed as ordinary text.

Legacy implementation term: typed `RoomControlEvent` values transported through the room server.

Agent Bridge capability payloads also have a shared lifecycle envelope view, `pastey-agent-bridge-capability-envelope-v1`, derived from the existing typed preview/control payloads. It records capability id/version, selected-peer route policy, exact allow-once consent policy, source/target refs, expiry, payload hash, and bounded transport metadata. This shared envelope does not replace the per-capability schemas validated by `RoomControlEvent`.

Production paths:

- TypeScript event schema and validation: `src/lib/agentBridge/roomControlEvent.ts`.
- Local queue integration: `src/lib/agentBridge/controlQueue.ts`.
- Runtime demand reducer: `src/lib/agentBridge/controlWindowRuntime.ts`.
- Tauri bridge calls: `src/lib/tauri.ts`.
- Rust endpoint and inbox: `src-tauri/src/room_control.rs`.
- Control-key wrapping and domain separation: `src-tauri/src/crypto.rs`.

## Encryption And Domain Separation

Control events use the Bridge control transport path and are encrypted separately from ordinary payload storage. The Rust crypto layer wraps control material with explicit control-key wrapping and derives domain-separated transport keys.

This provides encrypted current-session delivery. It does not create durable authenticated device identity.

## Runtime Bounds

The Rust room-control runtime enforces bounded payload and state limits for the current implementation, including:

- bounded control request and response sizes;
- bounded event size;
- current-session event expiry;
- bounded inbox depth;
- bounded replay cache;
- rate and burst limits;
- event-kind allowlisting;
- unsafe-field rejection.

The current implementation is intentionally current-session. It does not provide durable Bridge history, durable workflow records, reusable trust, or reconnectable control replay.

## Replay, Expiry, And Inbox Semantics

Inbound events are validated before they affect the local control queue. Duplicate event references, expired events, invalid shapes, unsafe fields, and unsupported event kinds are rejected or recorded as terminal status according to the event path.

The inbox is a current-session receive buffer, not a durable event log. It is useful for Bridge-scoped workflow and audit correlation, but it is not durable Bridge history and must not be used as cross-session authority.

## Queue And Delivery Semantics

The TypeScript `ControlQueueState` models outbound and inbound status transitions for preview, acknowledgement, denial, invalid, expired, execution request, and execution result paths.

Outbound production sends require an exact selected-peer Bridge route before `sendRoomControlEvent` runs. The frontend assertion binds the event's `roomRef`, `sourceDeviceRef`, and `targetPeerRef` to the active current-session room-control session, then forwards a `pastey-bridge-control-route-v1` payload to Tauri.

The Rust backend treats that selected-peer route as authoritative. `src-tauri/src/room_control.rs` resolves the target endpoint and transport key from the current-session `bridge_peers` row, validates the route room/session and event source/target refs, and fails closed for unknown, stale, expired, disconnected, reconnecting, missing-endpoint, or route-mismatched peers. Inbound control delivery must match one unique connected current-session transport key; the observed TCP source IP is not an identity or authority signal. It does not fall back to arbitrary legacy room endpoint fields after route validation fails.

Control and capability transport remains single-target. `selected_peers` and `broadcast_bridge` route payloads are rejected for room-control and Agent Bridge capability events. Ordinary text/file/image/pasted-image multi-target delivery is separate and remains the only implemented fan-out path.

`runtime.hello_stdout` uses the same selected-peer control transport as the fixed Hello Peer template for diagnostics/tests. Its typed stdout result is a capability execution result payload, not ordinary Bridge text, and delivery of that result remains separate from consent or future authority. It is no longer user-facing product UI.

Pastey 1.9.1 product paths use this same selected-peer transport for Ask Bridge Search / Return. Search and candidate payload Return requests use the canonical room-control selected peer ref for the embedded request target and preview envelope target. Denial status events are terminal product lifecycle events, not failures that retry or continue to execution. Bridge detail uses one serialized session inbox pump for automatic polling, focus refresh, and the manual `Check for updates` fallback. A bounded processed-event registry routes Ask Bridge events by capability and exact preview correlation; status events are matched only on the device that registered the outbound preview. The registry retains outbound preview correlation through panel close/reopen until expiry. Active operations poll at about 1.6 seconds; idle or terminal Bridge detail has no periodic room-control interval and refreshes on entry, focus, or the fallback button. Queue refs are updated before pumping, and unchanged or already processed inbox data does not trigger product state updates or resend an already delivered event.

Delivery receipt means the transport accepted or exposed the event. It does not mean:

- the peer consented;
- the request is authorized;
- execution happened;
- the Bridge relationship is durable;
- durable device identity was created;
- the event can be replayed later as trust evidence.

## Runtime Capacity Reservation

Outgoing local control demand reserves scheduler capacity by lowering the data target from `8` to `7`. After the demand becomes quiet, the data target restores to `8` after the runtime quiet period.

This is a Layer 3 capacity policy for a Layer 4/5 control transport. It does not create a separate execution lane, and it does not grant capability authority.

Control events continue to default to selected-peer routing. They should not be broadcast to a Bridge unless a future design explicitly validates that event kind, threat model, replay behavior, rate limits, and UI disclosure.

See [../transfer/scheduler.md](../transfer/scheduler.md) for scheduler details and [../transfer/validation.md](../transfer/validation.md) for the automated contention harness.

## Validation

Relevant validation includes:

- TypeScript room-control event and queue tests.
- Rust `src-tauri/src/room_control.rs` route, event, inbox, and receipt tests.
- Rust control-key wrapping and transfer-window tests.
- `rtk node scripts/run-cl4-contention-smoke.mjs`, which exercises the production demand reducer, planner allocations, real Rust runtime-window update primitive, and room-control transport test suites.

The harness is deterministic lower-boundary evidence. It is not a full dual-instance GUI or two-device LAN Agent Bridge run.

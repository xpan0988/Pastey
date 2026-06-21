# Bridge Control Transport

Bridge control transport is the Layer 4/Layer 5 control plane for Agent Bridge events. It is separate from Bridge text and file transfer. For project-wide layer boundaries, see [../architecture/Project-specifications.md](../architecture/Project-specifications.md). For Bridge membership and authority semantics, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For the target routing model, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md).

Legacy implementation term: room-control transport. Existing production code and tests still use `RoomControlEvent`, `room_control.rs`, and related names.

## Contract

Bridge control events are typed current-session values transported through the Bridge session and validated on receipt. They carry Agent Bridge previews, status events, execution requests, and execution results. They are not Bridge items, not durable workflow records, and not displayed as ordinary text.

Legacy implementation term: typed `RoomControlEvent` values transported through the room server.

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

Outbound production sends now require an exact selected-peer Bridge route before `sendRoomControlEvent` runs. The frontend assertion binds the event's `roomRef`, `sourceDeviceRef`, and `targetPeerRef` to the active current-session room-control session. This does not change the Tauri payload shape or Rust endpoint resolution; the current backend transport still sends to the one stored peer endpoint/key for the room.

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

Control events should default to selected-peer routing in future multi-device Bridge support. They should not be broadcast to a Bridge unless a future design explicitly validates that event kind, threat model, replay behavior, rate limits, and UI disclosure.

See [../transfer/scheduler.md](../transfer/scheduler.md) for scheduler details and [../transfer/validation.md](../transfer/validation.md) for the automated contention harness.

## Validation

Relevant validation includes:

- TypeScript room-control event and queue tests.
- Rust `src-tauri/src/room_control.rs` tests.
- Rust control-key wrapping and transfer-window tests.
- `rtk node scripts/run-cl4-contention-smoke.mjs`, which exercises the production demand reducer, planner allocations, real Rust runtime-window update primitive, and room-control transport test suites.

The harness is deterministic lower-boundary evidence. It is not a full dual-instance GUI or two-device LAN Agent Bridge run.

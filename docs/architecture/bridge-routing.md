# Bridge Routing Semantics

This document defines Pastey's target routing model for future multi-peer Bridge sessions. It is architecture guidance and audit evidence only. It does not implement multi-device runtime behavior.

For the broader Bridge lifecycle and authority boundaries, see [bridge-semantics.md](bridge-semantics.md). Legacy implementation term: Room.

## Core Model

A Bridge is an ephemeral encrypted current-session transfer/control session. Routing answers a narrow question: when more than one current-session accepted peer is present, which peer or peers receive each Bridge item or Bridge control event?

Bridge routing does not create:

- durable history;
- durable peer identity;
- reusable trust;
- execution authority;
- automatic consent;
- reconnectable workflow state.

Delivery to a route proves only that the transport accepted or exposed the payload for the current session. It is never consent or proof of durable trust.

## Target Terms

`BridgeTarget` is the target expression for one send operation.

Suggested future shape:

```ts
type BridgeTarget =
  | { kind: "selected_peer"; peerSessionRef: string }
  | { kind: "selected_peers"; peerSessionRefs: string[] }
  | { kind: "broadcast_bridge"; bridgeSessionRef: string; expectedPeerCount: number };
```

`BridgeRoute` is the resolved current-session route used by transport after policy validation. It should be bound to the Bridge session and to the current accepted-peer set at send time.

Suggested future shape:

```ts
interface BridgeRoute {
  bridgeSessionRef: string;
  target: BridgeTarget;
  resolvedPeerSessionRefs: string[];
  routePolicy: BridgeBroadcastPolicy;
  createdAt: string;
}
```

`BridgePeerSelection` is UI or caller intent before route resolution. It should not be persisted as trust.

Suggested future shape:

```ts
type BridgePeerSelection =
  | { mode: "current_selected_peer"; peerSessionRef: string }
  | { mode: "current_selected_peers"; peerSessionRefs: string[] }
  | { mode: "current_bridge_broadcast" };
```

`BridgeBroadcastPolicy` defines whether broadcast is allowed for a content/event kind and what must be shown before send.

Suggested future shape:

```ts
interface BridgeBroadcastPolicy {
  allowed: boolean;
  requiresExplicitUserAction: boolean;
  displayTargetCount: boolean;
  allowedEventKinds?: string[];
}
```

These names are preparation vocabulary only. Do not wire them into production transfer behavior until the runtime has safe route resolution, validation, and tests.

## Source-Level Foundation

The current source foundation now represents the future routing model while preserving existing one-peer delivery behavior:

- `src/lib/bridgeRouting.ts` models explicit selected-peer, selected-peers, and broadcast targets plus content-kind policy checks.
- `src/lib/bridgePeers.ts` models current-session accepted peers, routeable-peer filtering, route-to-peer compatibility checks, broadcast resolution to current routeable peers, and default single-peer route derivation.
- `src/lib/bridgeRoomAdapter.ts` translates the current legacy single-peer Room shape into the source-level Bridge peer model and derives the same default selected-peer route only when exactly one routeable remote peer exists.
- `src/lib/bridgeRoutingRuntime.ts` derives production frontend routing state for the current legacy Room and makes text, file/image enqueue, pasted-image enqueue, queued file dispatch, and room-control/capability sends route-authoritative at the frontend boundary.
- `deriveDefaultBridgeRouteForCurrentSession` returns a selected-peer route only when exactly one routeable remote peer exists. Zero routeable peers returns no route, and multiple routeable peers require explicit selection.
- `deriveLegacyRoomDefaultBridgeRoute` is an adapter preparation point for a future send-path pass. It does not change send payloads, Tauri commands, endpoint selection, queue behavior, or Agent Bridge consent.
- `deriveBridgeRoutingStateForRoom` is now used by the production frontend Room view and data-send preparation to make the existing single-peer target explicit as local state.
- `assertCapabilityEventHasSelectedPeerRoute` is now used by the room-control send wrapper to bind outbound capability events to the active current-session room-control peer.

These helpers do not create multi-peer delivery, persist identity, create history, grant trust, or change Agent Bridge consent. Existing production delivery still uses the legacy single-peer room implementation. The frontend now requires an exact selected-peer route before text send, file/image enqueue, pasted-image enqueue, queued file dispatch, and outbound room-control/capability transport. Text send carries a text-only selected-peer route payload to Tauri, and queued file/image dispatch carries a file-only selected-peer route payload to Tauri. Rust validates those payloads against the current single-peer room endpoint/key state before creating the outgoing item. Room-control/capability sends additionally require the event `targetPeerRef` to match the active current-session peer. Room-control Tauri command payloads, backend endpoint selection, transfer queue item shape, and scheduler/window accounting remain unchanged.

## Peer State

A current-session accepted peer is a peer admitted into the current Bridge session through nearby accept, 8-digit code join, or an equivalent explicit session join path.

A session-verified peer is an accepted peer whose current Bridge session relationship is verified well enough for encrypted current-session delivery.

Neither term means durable trusted device, reusable trust, execution consent, or authority to run capabilities.

## Routing Modes

Selected peer means one explicitly selected current-session accepted peer.

Selected peers means an explicit selected subset of current-session accepted peers.

Broadcast to Bridge means all currently accepted peers in the Bridge session at route-resolution time.

Broadcast must remain a send-time current-session route, not a durable group definition. If peers join or leave later, that does not retroactively change the route.

## Content Routing Rules

### Text

Text may default to broadcast inside the Bridge when multi-peer support exists.

Future UI may support selected peer or selected peers for text. Text remains ordinary Bridge content. It is not a control event, not execution authority, and not durable history.

### File, Image, And Pasted Image

File, image, and pasted-image transfer should default to selected peer once multi-peer support exists.

Selected peers and broadcast may be supported later, but broadcast must be explicit and should show the target count before send. File routing must not imply durable trust, durable history, or reusable peer identity.

### Bridge Control Events

Bridge control events must default to selected peer.

Control events should not be broadcast unless a future design explicitly validates that event kind, threat model, replay behavior, rate limits, and UI disclosure. Delivery is not consent. Accepted Bridge membership is not execution authority.

### Agent Bridge Capability Events

Agent Bridge capability events must bind to one exact selected peer, Bridge session, request, and expiry window.

They must not use broadcast by default. Existing Hello Peer safety semantics remain unchanged: model output is advisory, host validation and PolicyGate are required, execution requests are host-built, and capability execution requires explicit per-request consent.

## Lifecycle Interaction

Leave removes the local peer from the current Bridge session. It invalidates future routes that include that local peer, but it does not create durable identity or durable workflow state.

Disconnect means current-session delivery to a peer is unavailable or interrupted. A selected-peer route should fail closed if the selected peer is unavailable. A selected-peers or broadcast route should have explicit per-peer delivery outcomes before it can be claimed as complete.

Burn clears local Bridge session state according to the implemented cleanup path. Burn should cancel or invalidate pending current-session routes for that local session and must not preserve durable routing history. Files already saved to Inbox remain user-owned output.

Replay boundaries are current-session only. A route, delivery receipt, control inbox entry, log entry, or consent record must not be reused as cross-session authority.

## Current Single-Peer Assumptions Audit

The current production runtime is still primarily two-peer and room-named. This audit is durable preparation evidence for future routing work; it is not a runtime change request.

Some single-peer assumptions are now represented by source-level helpers:

- route shape and broadcast policy: `src/lib/bridgeRouting.ts`;
- current-session peer membership and liveness: `src/lib/bridgePeers.ts`;
- legacy Room-to-Bridge adaptation for the current one-peer room state: `src/lib/bridgeRoomAdapter.ts`;
- production frontend local route derivation for the current one-peer room state: `src/lib/bridgeRoutingRuntime.ts`;
- behavior-preserving default selected-peer derivation for the exactly-one-routeable-peer case: `deriveDefaultBridgeRouteForCurrentSession`;
- route compatibility checks that keep control events and Agent Bridge capability events selected-peer only by policy.

The production runtime assumptions below still remain in force. Text send, file/image enqueue, pasted-image enqueue, queued file dispatch, and outbound room-control/capability transport are now route-authoritative at the frontend boundary and currently accept only the exact selected-peer route derived from one routeable remote peer or active room-control session. Text and queued file/image dispatch are also selected-peer validated at the Tauri/Rust boundary. Selected-peers sending, broadcast sending, file/image broadcast, control-event broadcast, and Agent Bridge capability broadcast are still not enabled in production.

| Area | Current evidence | Routing implication |
| --- | --- | --- |
| Data model | `src/lib/types.ts` and `src-tauri/src/models.rs` expose `RoomInfo` with one `peer_device_name`, one `peer_connected`, one `peer_burned_at`, and no accepted-peer collection. | Current UI state cannot represent selected peers or broadcast membership. |
| Room creation and 8-digit join | `src/lib/tauri.ts` exposes `createRoom(expiryMinutes)` and `joinRoom(code)`, while `src-tauri/src/commands.rs` creates one local room row and updates one peer endpoint after join. | Join establishes one current peer relationship, not a multi-peer membership set. |
| Nearby accept | `requestNearbyJoin` and `acceptNearbyJoin` in `src-tauri/src/commands.rs` return one `RoomInfo` and one peer response. | Nearby accept maps to one accepted peer for one current session. |
| Text send path | `src/pages/RoomPage.tsx` calls `sendTextToRoomWithBridgeRoute`; `src/lib/tauri.ts` forwards `roomId`, text, and a text-only selected-peer `bridgeRoute`; `src-tauri/src/commands.rs` validates that route before `transfer::send_room_item`. | Text is selected-peer authoritative at the frontend and Tauri/Rust command boundary, but delivery still resolves the one stored peer endpoint/key. |
| File/image/pasted-image path | `RoomPage.tsx` requires a selected-peer route before enqueue; `src/lib/transferScheduler.ts` stores `roomId` per queue item; `App.tsx` re-derives the selected-peer route before queued dispatch; `sendFileToRoom(roomId, path, options)` forwards a file-only selected-peer `bridgeRoute`; `src-tauri/src/commands.rs` validates that route before `transfer::send_room_file`. | File-like dispatch is selected-peer authoritative at the frontend and Tauri/Rust command boundary, but queue storage remains room-scoped and delivery still resolves the one stored peer endpoint/key. |
| Backend item/file delivery | `src-tauri/src/transfer.rs` resolves one `peer_host`, one `peer_port`, and one `peer_transport_public_key` from the room row in `send_room_item` and `send_room_file`. | Transport has one resolved receiver endpoint/key per room. |
| Bridge control transport | `RoomControlPanel.tsx` now requires `assertCapabilityEventHasSelectedPeerRoute(session, event)` before `sendRoomControlEvent(roomId, event)`. `src-tauri/src/room_control.rs` still sends to one stored peer endpoint/key and rejects inbound traffic unless it matches the stored peer host/key. | Control delivery is selected-peer authoritative in frontend state and event refs, but single-peer at the transport layer. |
| Agent Bridge consent binding | `src/lib/agentBridge/roomControlEvent.ts` and `peerConsent.ts` bind `roomRef`, `sourceDeviceRef`, and `targetPeerRef`; `RoomControlPanel.tsx` derives those refs from the one active room-control session and now rejects outbound capability transport unless the target matches that session. | Hello Peer has exact peer/session/request binding, but it is still bound to the one active peer, not a peer selection model. |
| Queue/planner | `src/lib/transferPlanner.ts` and `src/lib/transferScheduler.ts` group, dedupe, and schedule by `roomId`; MicroFlowGroup eligibility requires the same room. | Planner capacity is room-scoped, not route-scoped, and has no target-count accounting. |
| Room UI | `src/pages/RoomPage.tsx` displays one connection status, one peer device name, one send composer, and one Burn Room action. | UI assumes one peer and has no peer picker, target count, per-peer outcomes, or broadcast confirmation. |
| Agent Bridge UI | `src/components/agentBridge/RoomControlPanel.tsx` owns one active session context and logs one `peerSessionRef`. | Control UI is selected-session scoped but not multi-peer aware. |

The selected-peer productionization milestone does not change this table's backend delivery facts. `RoomPage` requires a selected-peer route before text send, file/image enqueue, and pasted-image enqueue; `App` requires a selected-peer route before queued file dispatch; `RoomControlPanel` requires a selected-peer route and matching event target before existing room-control operations. The text and file commands now carry selected-peer route payloads, and Rust rejects malformed, mismatched, unknown, disconnected, selected-peers, and broadcast route targets. Room-control commands still carry the same legacy arguments, and the backend still resolves one stored peer endpoint/key from the room.

Backend/Tauri selected-peer delivery remains deferred for room-control payloads. The current Rust room model stores one peer endpoint/key per room; text and file route validation map the current selected-peer payload to that single current endpoint/key and do not add fan-out. Queue route metadata remains deferred because dispatch re-derives the current selected-peer route from live Room state, and durable route storage would need queue route lifecycle and recovery semantics.

## Preparation Requirements Before Runtime Work

Before adding real multi-peer routing, Pastey needs explicit design and validation for:

- accepted-peer collection shape and lifecycle;
- current-session peer refs that are stable only within one Bridge session;
- route resolution from `BridgePeerSelection` to `BridgeRoute`;
- per-peer delivery outcomes for selected-peers and broadcast routes;
- target-count disclosure for file/image broadcast;
- control-event allowlist for any broadcast-capable event kind;
- Agent Bridge consent binding that stays exact to one selected peer/session/request;
- queue and planner accounting for target count, route failures, and partial delivery;
- lifecycle handling for leave, disconnect, burn, and startup recovery without durable history.

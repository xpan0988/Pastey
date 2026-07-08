# Bridge Routing Semantics

This document defines Pastey's target routing model for multi-peer Bridge sessions. Ordinary data delivery now supports selected peer, selected peers, and explicit broadcast targets. Bridge control and Agent Bridge capability delivery remain selected-peer only.

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

These names are routing vocabulary. They are current for ordinary data sends. Current-session reconnect liveness and durable paired-device recognition are implemented for ordinary routing display, while control/capability fan-out remains deferred.

## Source-Level Foundation

The current source foundation represents the routing model while preserving existing one-peer delivery behavior:

- `src/lib/bridgeRouting.ts` models explicit selected-peer, selected-peers, and broadcast targets plus content-kind policy checks.
- `src/lib/bridgePeers.ts` models current-session accepted peers, routeable-peer filtering, route-to-peer compatibility checks, broadcast resolution to current routeable peers, and default single-peer route derivation.
- `src/lib/bridgeRoomAdapter.ts` translates the current legacy single-peer Room shape into the source-level Bridge peer model and derives the same default selected-peer route only when exactly one routeable remote peer exists.
- `src/lib/bridgeRoutingRuntime.ts` derives production frontend routing state for the current legacy Room and makes text, file/image enqueue, pasted-image enqueue, queued file dispatch, and room-control/capability sends route-authoritative at the frontend boundary.
- `src/lib/bridgeIdentity.ts` defines the durable identity model as pairing metadata only. It is separate from current-session Bridge membership and cannot grant consent, reusable trust, execution authority, auto-join, or history.
- `src-tauri/src/storage.rs` owns a `bridge_peers` current-session endpoint table and mirrors the existing one-peer Room endpoint into that table for route validation and selected-peer delivery. It also owns `bridge_durable_identities` for paired-device recognition records. When a reconnect changes endpoint host, endpoint port, or transport public key, the old route row is marked stale and a new current-session `peer_session_id` is minted.
- `deriveDefaultBridgeRouteForCurrentSession` returns a selected-peer route only when exactly one routeable remote peer exists. Zero routeable peers returns no route, and multiple routeable peers require explicit selection.
- `deriveLegacyRoomDefaultBridgeRoute` is the compatibility adapter for deriving the current legacy Room's default selected-peer Bridge route when exactly one routeable peer exists.
- `deriveBridgeRoutingStateForRoom` is now used by the production frontend Room view and data-send preparation to make the existing single-peer target explicit as local state.
- `assertCapabilityEventHasSelectedPeerRoute` is now used by the room-control send wrapper to bind outbound capability events to the active current-session room-control peer.

These helpers do not create history, grant trust, or change Agent Bridge consent. The frontend requires an explicit ordinary-data route before text send, file/image enqueue, pasted-image enqueue, and queued file dispatch. Text send carries a text route payload to Tauri; queued file/image/pasted-image dispatch carries a file route payload to Tauri. Rust validates selected-peer, selected-peers, and explicit broadcast data routes against `bridge_peers` before creating outgoing items. Text selected-peers and broadcast routes fan out in the command and return a `BridgeSendOperation` with per-target `BridgeDeliveryOutcome` entries. File-like selected-peers and broadcast sends are expanded by the frontend into target-specific queue children; each child uses the existing selected-peer file transfer path, and the queue derives aggregate status from child terminal states.

Room-control/capability sends carry a control-route payload to Tauri and remain selected-peer only. The backend resolves the selected peer through `bridge_peers`, validates the selected route against the event `roomRef`, `sourceDeviceRef`, `targetPeerRef`, endpoint/key material, and current-session liveness, and rejects selected-peers or broadcast control/capability routes.

## Layer 4 Runtime Status

This table is the concise Layer 4 routing status. The project-wide layer/scoring contract remains [Project-specifications.md](Project-specifications.md); validation commands and matrix detail remain [../transfer/validation.md](../transfer/validation.md).

| Area | Production runtime | Automated validation | Status |
| --- | --- | --- | --- |
| Current-session peer table | `bridge_peers` stores endpoint host/port/key, liveness, join method, optional paired display id, and current-session `peer_session_id`. | Rust storage and command route tests. | Implemented |
| Ordinary data selected-peer | Text and file-like sends resolve the selected target through `bridge_peers`. | Rust route tests plus TypeScript routing runtime tests. | Implemented |
| Ordinary data selected-peers | Text returns per-target outcomes; file/image/pasted-image enqueue target-specific queue children. | Rust route tests, TypeScript routing runtime tests, transfer scheduler tests. | Implemented |
| Ordinary data broadcast | Broadcast resolves the current routeable accepted peer set at send/enqueue time. | Rust route tests plus TypeScript peer/routing tests. | Implemented |
| Per-target outcomes | Text fan-out returns `BridgeSendOperation` outcomes; file-like fan-out reports child terminal states. | Rust command tests and TypeScript outcome/queue tests. | Implemented |
| Queue children | File/image/pasted-image multi-target sends create current-session child queue items with target refs and one operation id. | Transfer scheduler execution tests. | Implemented |
| Reconnect invalidation | Endpoint/key changes create a new `peer_session_id`; old rows become stale/expired and unrouteable. | Rust storage/route tests and TypeScript stale-route tests. | Implemented |
| Durable pairing | Explicit pairing, revocation, and bounded `rotation_state` provide display/recognition metadata only. | Rust storage tests and TypeScript identity tests. | Implemented, no authority |
| Room-control selected-peer backend route | Control/capability transport uses selected-peer control route payloads and resolves endpoint/key through `bridge_peers`. | Rust room-control tests and TypeScript control-route tests. | Implemented |
| Control/capability fan-out | Selected-peers and broadcast for room-control/capability are rejected. | Rust room-control tests and TypeScript route-policy tests. | Deferred intentional non-goal |
| Full cryptographic paired-key rotation | Runtime records bounded rotation state, but full cryptographic rotation protocol is not implemented. | Storage tests cover bounded state only. | Deferred intentional non-goal |
| Two-machine/release validation | Manual/release smoke must still cover real LAN and packaged-app behavior. | Not automated by this matrix. | Pending manual/release validation |

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

Text supports selected peer, selected peers, and explicit broadcast to current routeable Bridge peers. Broadcast is resolved at send time and does not include peers that join later. Text remains ordinary Bridge content. It is not a control event, not execution authority, and not durable history.

### File, Image, And Pasted Image

File, image, and pasted-image transfer supports selected peer, selected peers, and explicit broadcast. Multi-target file-like sends resolve the target set at enqueue time and create target-specific queue children. Each child uses the existing single-file selected-peer transfer path, so planner capacity remains bounded by the existing global budget. Broadcast must be explicit and shows the target count before send. File routing must not imply durable trust, durable history, or reusable peer identity.

### Bridge Control Events

Bridge control events must default to selected peer.

Control events should not be broadcast unless a future design explicitly validates that event kind, threat model, replay behavior, rate limits, and UI disclosure. Delivery is not consent. Accepted Bridge membership is not execution authority.

### Agent Bridge Capability Events

Agent Bridge capability events must bind to one exact selected peer, Bridge session, request, and expiry window.

They must not use selected-peers or broadcast routes. Existing bounded Agent Bridge capability semantics remain unchanged: model output is advisory, host validation and PolicyGate are required, execution requests are host-built, and capability execution requires explicit per-request consent.

## Lifecycle Interaction

Leave removes the local peer from the current Bridge session. It invalidates future routes that include that local peer, but it does not create durable identity or durable workflow state.

Disconnect means current-session delivery to a peer is unavailable or interrupted. A selected-peer route fails closed if the selected peer is unavailable. Selected-peers validation rejects malformed, duplicate, and unknown targets before delivery starts; known stale, expired, disconnected, reconnecting, or otherwise unrouteable selected-peers targets become per-target rejected outcomes so routeable targets can still complete. Broadcast resolves only currently routeable peers and fails closed when there are none. Per-target outcomes distinguish completed, partial, failed, rejected, cancelled, and interrupted delivery at the operation or queue-child level.

Reconnect remains current-session scoped. Pastey uses the safer route contract: a reconnect with changed endpoint host, endpoint port, or transport public key replaces the current-session `peer_session_id` instead of reusing the old route binding. The previous row is marked `stale`, endpoint/key fields are cleared, and old selected-peer routes fail with `route_expired` rather than rebinding to the new endpoint. A reconnect that reports the same endpoint/key can update the existing row. The newly connected row is routeable only while its liveness is `connected` and endpoint/key fields are present.

Current liveness values are:

- `connected`: routeable when endpoint host, endpoint port, and transport public key are present;
- `reconnecting`: visible current-session state for a reconnect attempt, not routeable;
- `disconnected`: temporarily unavailable, not routeable;
- `left`: explicit leave or peer-burn, not routeable;
- `stale`: replaced by burn, reconnect replacement, or other route invalidation, not routeable;
- `expired`: process/startup recovery invalidated the previous runtime endpoint binding, not routeable.

Startup recovery marks previously connected peer rows `expired`, clears endpoint/key fields, and does not reconstruct durable Bridge history. Reconnect does not create routeability from durable identity, durable trust, reusable consent, or durable route recovery.

Durable paired-device association is display-only. Explicit pairing from a connected current-session peer creates or updates a `bridge_durable_identities` record with display label, pairing public-key fingerprint, pairing method, timestamps, revocation state, and rotation state, and stores the active `durable_identity_id` on the current `bridge_peers` row. A paired identity alone is never included in selected-peer, selected-peers, or broadcast routing. Broadcast still resolves only current `connected` routeable peer rows at send/enqueue time.

Revocation marks the durable identity revoked and clears active peer display association for that identity. Revocation does not delete user-owned Inbox output, rewrite ordinary delivery history, change liveness, or create routeability. Key rotation is represented by bounded `rotation_state` values such as `current` and `rotation_required`; a rotation-required identity remains recognition metadata only. Fingerprint/key mismatch does not silently preserve paired association or authority.

Burn clears local Bridge session state according to the implemented cleanup path. Burn should cancel or invalidate pending current-session routes for that local session and must not preserve durable routing history. Files already saved to Inbox remain user-owned output.

Replay boundaries are current-session only. A route, delivery receipt, control inbox entry, log entry, or consent record must not be reused as cross-session authority.

## Current Routing Assumptions Audit

The current production runtime still has legacy room-named surfaces and selected-session control transport. Ordinary data routing now supports selected peer, selected peers, and explicit broadcast.

The remaining selected-peer assumptions are represented by source-level helpers:

- route shape and broadcast policy: `src/lib/bridgeRouting.ts`;
- current-session peer membership and liveness: `src/lib/bridgePeers.ts`;
- legacy Room-to-Bridge adaptation for current Room state: `src/lib/bridgeRoomAdapter.ts`;
- production frontend local route derivation for current Room state: `src/lib/bridgeRoutingRuntime.ts`;
- behavior-preserving default selected-peer derivation for the exactly-one-routeable-peer case: `deriveDefaultBridgeRouteForCurrentSession`;
- route compatibility checks that keep control events and Agent Bridge capability events selected-peer only by policy.

The production runtime assumptions below still remain in force for authority boundaries. Text send, file/image enqueue, pasted-image enqueue, and queued file dispatch are route-authoritative at the frontend boundary and accept selected peer, selected peers, or explicit broadcast ordinary-data routes. Outbound room-control/capability transport accepts only the exact selected-peer route derived from the active room-control session. Text, file/image dispatch, and room-control/capability transport are validated at the Tauri/Rust boundary through the backend current-session endpoint table. Control-event broadcast and Agent Bridge capability broadcast are not enabled in production.

| Area | Current evidence | Routing implication |
| --- | --- | --- |
| Data model | `src-tauri/src/storage.rs` creates `bridge_peers` with `room_id`, current-session `peer_session_id`, endpoint host/port/key, liveness, join method, optional durable identity reference, and `updated_at`. It also creates `bridge_durable_identities` with display label, pairing fingerprint, pairing method, timestamps, revocation, and rotation state. `RoomInfo` includes a `peers` list derived from current-session rows plus safe paired display metadata while retaining legacy one-peer fields. Reconnect endpoint/key changes stale the old row and create a fresh current-session peer id. | Backend can represent current-session peer endpoints separately from durable identity, and stale routes do not silently bind to new endpoint/key material. Durable identity is recognition metadata only. |
| Room creation and 8-digit join | `src/lib/tauri.ts` exposes `createRoom(expiryMinutes)` and `joinRoom(code)`, while `src-tauri/src/commands.rs` creates one local room row and `update_room_peer` mirrors the connected endpoint into `bridge_peers`. | Join establishes one current peer endpoint row for now, not durable trust or automatic future membership. |
| Nearby accept | `requestNearbyJoin` and `acceptNearbyJoin` in `src-tauri/src/commands.rs` return one `RoomInfo` and one peer response. `update_room_peer` mirrors nearby/requested joins into `bridge_peers`; creator-side accept still receives the peer endpoint when the remote announces join. | Nearby accept maps to current-session accepted endpoint rows only. |
| Text send path | `src/pages/BridgeProductPages.tsx` calls `sendTextToRoomWithBridgeRoute`; `src/lib/tauri.ts` forwards `roomId`, text, and a text-only `bridgeRoute`; `src-tauri/src/commands.rs` validates that route against `bridge_peers` before delivery. | Selected-peer, selected-peers, and explicit broadcast text routes use table-resolved endpoint/key data. The returned item can include `BridgeSendOperation` with per-target outcomes and aggregate status. |
| File/image/pasted-image path | `BridgeProductPages.tsx` requires a route before enqueue and expands multi-target file-like sends into per-target queue children; `src/lib/transferScheduler.ts` stores in-memory child route/target fields; `App.tsx` dispatches each child with a selected-peer file `bridgeRoute`; `sendFileToRoom(roomId, path, options)` forwards the route to Rust; `src-tauri/src/commands.rs` validates that route against `bridge_peers` before `transfer::send_room_file_to_bridge_peer_endpoint`. | File-like selected-peers and broadcast are implemented as queue children over the existing single-file transfer path. Queue children are current-session runtime state, not durable route history. |
| Backend item/file delivery | `src-tauri/src/transfer.rs` has endpoint-specific selected-peer helpers and keeps the legacy one-endpoint helpers as compatibility fallback for no-route callers. | Selected-peer transport resolves receiver endpoint/key from `bridge_peers`. The text command performs command-level fan-out for multi-target ordinary text; file-like UI delivery fans out through target-specific queue children. |
| Bridge control transport | `RoomControlPanel.tsx` requires `assertCapabilityEventHasSelectedPeerRoute(session, event)` before `sendRoomControlEvent(roomId, event, bridgeRoute)`. `src/lib/tauri.ts` forwards a `pastey-bridge-control-route-v1` route. `src-tauri/src/room_control.rs` resolves the selected endpoint/key from `bridge_peers` and rejects stale, disconnected, expired, unknown, mismatched, selected-peers, and broadcast control routes. | Control delivery is selected-peer authoritative at both the frontend and backend boundaries. It remains single-target and current-session only. |
| Agent Bridge consent binding | `src/lib/agentBridge/roomControlEvent.ts` and `peerConsent.ts` bind `roomRef`, `sourceDeviceRef`, and `targetPeerRef`; `RoomControlPanel.tsx` derives those refs from the active room-control session, while the backend route must resolve to the same current-session peer row/key. | Fixed Hello Peer and Hello Stdout capabilities keep exact peer/session/request binding. Durable pairing display metadata, delivery receipts, logs, and Bridge membership do not create capability authority. |
| Queue/planner | `src/lib/transferPlanner.ts` and `src/lib/transferScheduler.ts` group, dedupe, and schedule by `roomId`; MicroFlowGroup eligibility requires the same room. Multi-target file-like sends create child queue items with target-aware dedupe keys and one shared `bridgeOperationId`. | Planner capacity remains room-scoped and globally bounded. Target count is represented by child items rather than independent unbounded transfer windows. |
| Bridge detail UI | `src/pages/BridgeProductPages.tsx` displays a Bridge target selector for ordinary data sends, routeable peer count, selected target count, explicit broadcast target-count disclosure, safe paired-device status/actions, queue child outcomes, and returned text/file aggregate outcomes where available. | UI supports selected peer, selected peers, and explicit broadcast for ordinary data only. Paired status is separate from current-session routeability. |
| Agent Bridge UI | `src/components/agentBridge/RoomControlPanel.tsx` owns one active session context and logs one `peerSessionRef`. | Control UI is selected-session scoped but not multi-peer aware. |

The endpoint-table route boundary applies to ordinary data and control-plane delivery. Bridge detail requires an ordinary data route before text send, file/image enqueue, and pasted-image enqueue; `App` requires a selected-peer child route before queued file dispatch; `RoomControlPanel` requires a selected-peer route and matching event target before room-control operations. The text, file, and room-control commands carry route payloads, and Rust rejects malformed, mismatched, unknown, duplicate, stale, unavailable, and unsupported route targets before delivery. Selected-peer stale or unavailable routes fail closed. Selected-peers ordinary data can record known stale or unavailable targets as rejected per-target outcomes. Room-control commands remain exact selected-peer only.

Selected-peers and broadcast production fan-out are implemented for ordinary data only. Route validation remains fail-closed for malformed, duplicate, unknown, and mismatched routes. Selected-peers ordinary data records known stale or unavailable targets as rejected per-target outcomes, while selected-peer ordinary data and all control/capability routes fail closed. Broadcast resolves the current routeable peer set at send time or enqueue time for ordinary data only. Queue route metadata is in-memory child dispatch state and is not durable route history. Durable route recovery remains deferred.

## Durable Paired-Device Runtime

Durable paired-device identity is implemented as recognition metadata, not as a routing or authority model.

The runtime stores paired identities in `bridge_durable_identities` and links active current-session peer rows through `bridge_peers.durable_identity_id`. The durable record stores display label, pairing public-key fingerprint, pairing method, timestamps, optional revocation time, and rotation state. Endpoint host, endpoint port, and transport public key remain in current-session `bridge_peers` rows only.

Pairing is explicit and starts from a connected current-session peer. Revocation marks the identity revoked and clears active peer display association. Rotation is currently represented by bounded state such as `rotation_required`; full cryptographic key rotation remains a bounded future expansion. None of these states grant routeability, auto-join, reusable consent, automatic approval, provider execution, tool runtime, or Agent Bridge capability authority.

When reconnect reports the same pairing fingerprint, the new current-session peer row may show the paired identity for display. If endpoint host, endpoint port, or transport public key changed, the `peer_session_id` replacement rule still applies and old routes still fail as expired. If the pairing fingerprint changes or the identity is revoked, the durable association is not silently preserved.

## Deferred Extensions

Remaining deferred work:

- durable route recovery across app restarts;
- control-event allowlist for any future broadcast-capable event kind;
- Agent Bridge consent binding beyond the current exact selected peer/session/request model;
- broader retry UX for partially failed multi-target file-like operations;
- lifecycle expansion beyond the current leave, disconnect, burn, cancellation, terminal queue guards, and startup recovery behavior.

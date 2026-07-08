# Bridge Semantics

This document defines the product-level semantics for Pastey Bridge sessions. It is the canonical terminology reference for Bridge lifecycle, peer acceptance, and authority boundaries. For the target route/selection model and current single-peer audit, see [bridge-routing.md](bridge-routing.md).

Legacy implementation term: Room. Existing code, storage, tests, and some file names may still use `Room`, `RoomItem`, `RoomControlEvent`, `room_id`, and room-control naming during the Bridge terminology migration. Those names describe the current implementation surface, not the final product concept.

## Core Definition

A Bridge is an ephemeral encrypted LAN session for routing transfers and control events between accepted peers.

A Bridge is:

- current-session scoped by default;
- a routing and communication relationship between accepted peers;
- suitable for encrypted local text, file, image, and bounded control-event delivery.

A Bridge is not:

- a chat room;
- durable history;
- reusable trust;
- durable device identity;
- execution authority;
- a long-term group.

## Primary Invariant

Bridge semantics do not escape the Bridge session.

Expanded:

- Bridge items are not durable history.
- Bridge control events are not durable workflow records.
- Bridge membership is not durable device identity.
- Bridge acceptance is not reusable trust.
- Bridge delivery is not consent.
- Bridge logs are not state.
- Nothing inside a Bridge becomes cross-session meaning unless explicitly promoted by a separate durable identity or explicit export feature.

## Peer Terms

An accepted Bridge peer is a peer that has been accepted into the current Bridge session through the current product join path, such as nearby accept or 8-digit code join.

A session-verified peer is a peer whose current Bridge session relationship has been verified well enough for encrypted current-session delivery. Session verification is scoped to this Bridge session only.

Accepted peer and session-verified peer do not mean:

- durable trusted device;
- reusable trust;
- execution authority;
- approval for Agent Bridge capability execution;
- permission to reuse consent across requests or sessions.

A paired device is a durable recognition record created by explicit pairing from a current-session peer. It can carry a display label, pairing public-key fingerprint, pairing method, timestamps, revocation state, and key-rotation state. It is not a routeable peer by itself.

Use "paired" or "known device" for this display/recognition state. Do not use "trusted" loosely for current Bridge membership or paired-device display.

## Bridge Items And Control Events

A Bridge item is a current-session transfer payload record for text, file, image, or pasted-image movement. It tracks payload metadata and lifecycle state for delivery and cleanup. It is not a durable message history and is not a control event.

Legacy implementation term: `RoomItem`.

A Bridge control event is a typed, bounded, current-session control-plane event used for Agent Bridge preview, status, execution request, and execution result paths. It is not ordinary text, not a Bridge item, and not a durable workflow record.

Legacy implementation term: `RoomControlEvent`.

Bridge control-event delivery proves only that the transport accepted or exposed the event in the current session. It does not prove consent, execution, trust, durable identity, or future replay authority.

## Routing Terms

Bridge routing distinguishes these modes:

- Send to selected peer: route one item or control event to one explicitly selected accepted peer.
- Send to selected peers: route one item or control event to an explicit selected subset of accepted peers.
- Broadcast to Bridge: route one item or event to all currently accepted peers in the Bridge session.

Current production behavior:

- Ordinary text, file, image, and pasted-image sends support selected peer, selected peers, and explicit broadcast.
- Multi-target file-like sends create current-session queue children.
- Bridge control events and Agent Bridge capability events remain selected-peer only.
- Control/capability selected-peers and broadcast are rejected unless a future design explicitly validates that event kind, threat model, replay behavior, rate limits, and UI disclosure.

Do not describe durable route recovery, durable trust, or control/capability fan-out as complete unless the runtime and validation evidence exist.

The owning route model terms are `BridgeTarget`, `BridgeRoute`, `BridgePeerSelection`, and `BridgeBroadcastPolicy`; see [bridge-routing.md](bridge-routing.md).

## Lifecycle Semantics

Leave means the local peer exits the current Bridge session. It does not delete user-owned Inbox output and does not create a durable trust or identity record.

Disconnect means current-session delivery is unavailable or interrupted. It is not proof that a durable peer identity changed, and it is not a durable workflow state.

Reconnect is current-session route replacement, not durable identity continuity. If endpoint host, endpoint port, or transport public key changes, Pastey treats the reconnected peer as a fresh current-session route with a new `peer_session_id`; the old peer row becomes stale and unrouteable. Reconnect never creates durable trust, reusable consent, automatic approval, or capability execution authority.

Durable pairing may associate the newly connected row with an existing paired identity when the pairing fingerprint still matches. That association is display metadata only. It does not let old selected-peer routes bind to the new session, does not revive queue children, and does not preserve Agent Bridge consent.

Revocation marks the durable paired identity revoked and clears active peer display association for that identity. It does not delete user-owned Inbox output, rewrite delivery history, change liveness, create routeability, or grant/revoke execution authority.

Key rotation is represented by a bounded `rotation_state`. A rotation-required paired identity remains display metadata only. Fingerprint/key mismatch does not silently preserve paired association or authority; endpoint transport key changes still follow the current-session reconnect rule above.

Burn means the local Bridge session state is cleared according to the implemented cleanup path. Current behavior removes local encrypted payloads, transient received files, partial files, Bridge items, and active receiver transfer state for that session. Files already saved to Inbox are user-owned output and are not deleted by Burn.

Startup recovery may clear stale active transfer state and partial files so interrupted current-session work does not revive as active work. It marks previous connected route rows expired and clears endpoint/key data. It does not reconstruct durable Bridge history.

## Agent Bridge Authority Boundary

Agent Bridge capability execution authority is separate from Bridge membership.

Nearby accept, 8-digit code join, accepted peer status, session verification, encrypted delivery, and Bridge membership never authorize capability execution. Capability execution still requires explicit per-request consent, host validation, PolicyGate review, bounded executor semantics, and replay/expiry checks.

Bridge membership can provide the current-session communication context for asking. It does not provide authority to act.

Durable pairing does not change that boundary. Paired status, reconnect status, delivery outcomes, and route existence are not consent, execution authority, durable trust, reusable approval, or permission to run tools.

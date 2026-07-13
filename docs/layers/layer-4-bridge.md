# Layer 4 — Multi-device Bridge sessions and peer identity

Bridge is Pastey's ephemeral, current-session device workspace. This document owns Bridge membership, peer identity boundaries, ordinary-data routing, and control transport. Legacy code and storage still use **Room** terminology; it refers to the current Bridge session, not a separate product model.

## Current-session peer model

An accepted peer is admitted through a nearby accept, 8-digit join, or equivalent explicit session join. A routeable peer additionally has a current connected endpoint and transport key in `bridge_peers`. Neither accepted nor routeable means durable trusted device, reusable trust, consent, or execution authority.

Bridge is not durable history. The peer table holds the current-session endpoint, liveness, join method, `peer_session_id`, and optional paired-device display reference. On reconnect with a changed host, port, or transport public key, Pastey creates a new `peer_session_id`, marks the old route stale, clears its endpoint/key material, and rejects old routes as expired. Old routes do not rebind. Startup recovery expires prior connected rows and clears runtime endpoint material.

Liveness values are `connected`, `reconnecting`, `disconnected`, `left`, `stale`, and `expired`. Only a connected peer with endpoint/key material is routeable.

## Paired-device display identity

Explicit pairing can retain label, fingerprint, pairing method, timestamps, revocation state, and bounded rotation state in `bridge_durable_identities`. This is display/recognition metadata only. It cannot receive data, auto-join, revive a route, grant consent, or grant capability authority. Full cryptographic paired-key rotation is not implemented.

## Ordinary-data routing

`selected peer` means one explicitly selected current-session accepted peer. `selected peers` means an explicit selected subset. `broadcast to Bridge` means all current routeable peers at resolution time; it is explicit, not a durable group, and later membership changes do not rewrite the operation.

Text supports all three ordinary-data modes and reports per-target outcomes. File, image, and pasted-image actions also support all three; selected-peers and broadcast resolve to target-specific queue children before dispatch. Malformed, duplicate, unknown, mismatched, stale, or unavailable routes fail under the current policy. A selected-peer route fails closed. There is no arbitrary legacy endpoint fallback after validation fails.

## Control transport

Bridge control events are encrypted, typed current-session values separate from ordinary Bridge items. The transport has bounded event/request/response size, expiry, inbox depth, replay cache, rate/burst limits, event-kind allowlisting, and unsafe-field rejection. The inbox is a current-session buffer, not durable workflow history.

Control and capability events remain exact selected-peer only. `selected_peers` and broadcast control routes are rejected. Layer 4 transports a capability request; it does not authorize it. A delivery receipt says only that transport accepted or exposed an event—not that the peer consented, execution happened, or a durable relationship exists.

The Bridge detail panel uses one serialized control-inbox pump. Active nonterminal operations refresh automatically; focus/entry refresh and **Check for updates** are fallbacks. Processed or unchanged events do not create duplicate product-state updates or resend delivery.

## Lifecycle boundaries

Leave, disconnect, burn, and startup recovery invalidate current-session work as applicable; they do not create durable history, durable route recovery, or authority. Burn removes local session state and active transient transfer state but not user-saved Inbox files. Replay, delivery receipts, control inbox entries, logs, and consent records cannot cross a Bridge session as authority.

## Current limitations

Durable route recovery, auto-join, control-event fan-out, and full cryptographic key rotation are not implemented. Multi-target fan-out is limited to ordinary data. Two-device/package validation remains a required manual/release check.

For exact vocabularies and schemas, see [reference.md](../reference.md). For Layer 5 consent and capabilities, see [Layer 5](layer-5-agent.md). For validation, see [development.md](../development.md).

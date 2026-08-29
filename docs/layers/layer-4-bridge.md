# Layer 4 — Multi-device Bridge sessions and peer identity

Bridge is Pastey's ephemeral, current-session device workspace. This document owns Bridge membership, peer identity boundaries, ordinary-data routing, and control transport. Legacy code and storage still use **Room** terminology; it refers to the current Bridge session, not a separate product model.

## Current-session peer model

An accepted peer is admitted through a nearby accept, 8-digit join, or equivalent explicit session join. A routeable peer additionally has a current connected endpoint and transport key in `bridge_peers`. Neither accepted nor routeable means a known, paired, consented, or execution-authorized device.

Bridge is not durable history. The peer table holds the current-session endpoint, liveness, join method, `peer_session_id`, optional logical `HostRef` association, and optional paired-device display reference. The HostRef is learned through an additive join-handshake field and is identity only: it does not make the row routeable, paired, admitted, approved, or capable. Older peers may omit it. On reconnect with a changed host, port, or transport public key, Pastey creates a new `peer_session_id`, marks the old route stale, clears its endpoint/key material, and rejects old routes as expired. Old routes do not rebind; the new current session must establish its own HostRef association. Startup recovery preserves an active logical Bridge and its member projection, marks every prior live peer session disconnected, clears runtime endpoint/key material, starts a new local Bridge server, and attempts bounded code-based rediscovery of the same Bridge identity. A successful rediscovery always creates a fresh exact peer session even if the host and port happen to match the old route.

Temporary route loss changes liveness/reachability but does not by itself remove current Bridge membership. Explicit leave/Burn is different: before local cleanup removes route keys, the departing Host creates a typed encrypted `bridge_membership.departure` event bound to the exact authenticated current session. The receiver deletes only that peer's current membership/route, revokes Bridge-scoped runtime authority bound to the departed session, and retains its own local Bridge. Paired-device display identity may remain separately. The event is a membership fact, never permission to Burn another Host.

Liveness values are `connected`, `reconnecting`, `disconnected`, `left`, `stale`, and `expired`. Only a connected peer with endpoint/key material is routeable.

The Host probes each exact connected peer on a two-second reconciliation cadence with a bounded transport-key check. Failure immediately removes that exact route from selection, changes the member to reconnecting, clears Bridge-scoped process authority, and rotates the local server session before recovery. After bounded failed attempts the truthful product state is disconnected; later retries may still establish a fresh session. A normal Quit signals the lifecycle, discovery, and Bridge servers to stop and relies on this remote liveness boundary. It does not Burn or delete the local logical Bridge.

## Product interaction boundary

**New Bridge** is a non-mutating choice screen. Nearby discovery and refresh only observe available devices; a remote human acceptance creates one new Bridge, manual code entry joins an existing Bridge, and **Create Bridge** is the only action on that screen that creates an empty Bridge/code. Opening the screen from the sidebar or empty workspace creates no storage record.

**Devices** inspects the selected Bridge's known members and current liveness. It has no add/find/join action because Pastey does not implement adding another Host to an existing Bridge. Nearby admission belongs to New Bridge.

**Developer Mode** replaces only the selected Bridge's central workspace. The Bridge selection, sidebar, current-Bridge context, member state, and Bridge lifecycle remain in place. A receiver observes authenticated pending terminal requests for its known Bridges without first creating receiver-side Developer Mode UI authority. Accept or Deny is still an explicit Host-local action: that action creates the receiver's short-lived UI session and either consumes the existing terminal admission path once or denies it without starting a PTY.

## Paired-device display identity

Explicit pairing can retain label, fingerprint, pairing method, timestamps, revocation state, and bounded rotation state in `bridge_durable_identities`. This is display/recognition metadata only. It cannot receive data, auto-join, revive a route, grant consent, or grant capability authority. Full cryptographic paired-key rotation is not implemented.

## Ordinary-data routing

`selected peer` means one explicitly selected current-session accepted peer. `selected peers` means an explicit selected subset. `broadcast to Bridge` means all current routeable peers at resolution time; it is explicit, not a durable group, and later membership changes do not rewrite the operation.

Text supports all three ordinary-data modes and reports per-target outcomes. File, image, and pasted-image actions also support all three; selected-peers and broadcast resolve to target-specific queue children before dispatch. Malformed, duplicate, unknown, mismatched, stale, or unavailable routes fail under the current policy. A selected-peer route fails closed. There is no arbitrary legacy endpoint fallback after validation fails.

## Control transport

Bridge control events are encrypted, typed current-session values separate from ordinary Bridge items. The transport has bounded event/request/response size, expiry, inbox depth, replay cache, rate/burst limits, event-kind allowlisting, and unsafe-field rejection. The inbox is a current-session buffer, not durable workflow history.

Bridge Plan control messages remain exact selected-peer only. `selected_peers` and broadcast control routes are rejected. Layer 4 transports a Plan message; it does not authorize it. A delivery receipt says only that transport accepted or exposed an event—not that approval, execution, or a durable relationship exists.

Bridge Plan v1 and v2 coexist explicitly. V1 retains its existing `bridge_plan.*` event kinds and requester/selected-session payload contract. V2 uses only `bridge_plan.v2.review_request` and `bridge_plan.v2.attempt_start`, carries its own exact protocol version and Plan participants, and uses a separate replay namespace. On inbound v2 delivery, Layer 4 resolves the authenticated peer's current `HostSessionBinding`; Core verifies that binding against the reviewed sender/target HostRefs and performs Host admission. Layer 4 does not derive participants, approve the Plan, choose a Host, insert Transfer, or create an execution grant.

Room Control delivers correlated current-session completion/control events to the Layer 5 Host coordinator. It does not inspect Plan topology, assume PipelinePrivate is followed by Transform, choose the next primitive, or create semantic step authority.

Developer Mode v0 reuses the same current-session peer resolution, authenticated encrypted Room Control envelope, event expiry, and replay boundary for a distinct `developer_terminal` message family. Terminal frames are delivered directly to the UI-independent terminal service, with separate bounded streaming rate/buffer state; they do not enter ordinary Room Control inbox or Bridge item history. Layer 4 transport still does not grant terminal consent: the remote human admission and `DeveloperTerminalGrant` are owned above this layer. See [architecture](../architecture.md#managed-workspace-and-developer-mode).

The Bridge detail panel uses one serialized control-inbox pump. Active nonterminal operations refresh automatically; focus/entry refresh and **Check for updates** are fallbacks. Processed or unchanged events do not create duplicate product-state updates or resend delivery.

## Lifecycle boundaries

Disconnect, explicit departure, Burn, and startup recovery invalidate current-session work as applicable; recovery creates no authority. Disconnect retains membership for a possible fresh-session reconnect. Explicit authenticated departure removes the departing peer from current membership but does not delete the survivor's Bridge. Local Burn first cuts local authority off, then removes that Host's routes, active server/transfer state, room-control inbox/replay/rate state, candidate/Plan workflow state, Bridge membership, items, and reusable room material. Bridge payload items use a per-Bridge wrapping key under the existing installation secret; Burn removes that key before fallible content/database cleanup, so retained Pastey key state cannot decrypt leftover Bridge ciphertext. It preserves user-saved final Inbox files and an independent paired-device display identity only. The durable Burn tombstone is opaque and non-sensitive; it is not product history. Replay, delivery receipts, control inbox entries, logs, and approval records cannot cross a Bridge session as authority.

## Current limitations

Durable paired-device auto-join, adding a Host to an existing Bridge, generic control-event fan-out, and full cryptographic key rotation are not implemented. Restart recovery is limited to rediscovering the same active logical Bridge from its persisted code/hash and Bridge id on the LAN; it does not treat identity, pairing, a prior endpoint, or navigation as admission. Burn provides logical deletion and cryptographic erasure through the per-Bridge payload-key lifecycle, but SQLite page/WAL/journal and filesystem block remanence are outside Pastey's physical guarantee. Phase 6.6 coordinates an authored v2 Plan by sending separate exact review/readiness/start/commit events to each current Host route; it does not turn Room Control into broadcast authority. Multi-target fan-out remains limited to ordinary data. Two-device/package validation remains a required manual/release check.

For exact vocabularies and schemas, see [reference.md](../reference.md). For Layer 5 semantic approval and step authority, see [Layer 5](layer-5-agent.md). For validation, see [development.md](../development.md).

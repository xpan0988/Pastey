# Room-Control Transport Design

## 1. Status and Scope

This document began as CL-3A transport design and feasibility. CL-3B now
implements the bounded preview-only transport, and CL-3C connects its delivery
receipts and non-destructive Rust inbox retrieval to the existing frontend
`ControlQueueState`. CL-4 implements scheduler reservation, CL-5 implements
exact one-time consent, and CL-6 carries one fixed bounded execution
request/result pair through the same typed transport.

CL-1 provides closed preview-only `RoomControlEvent` types, validation, expiry,
current-session replay helpers, and the pure capacity helper. CL-2 provides the
current-session inbound/outbound queue model. CL-3B provides real transport,
and CL-3C uses that same queue model for real transport events.

The smallest feasible next transport is a separate bounded room HTTP endpoint
that carries an encrypted typed control envelope. It must not reuse ordinary
room items, file-transfer frames, or MicroFlowGroup.

## 2. Current Room Communication Architecture

### Room Creation, Join, and Session State

| Concern | Current repository path | Current behavior |
| --- | --- | --- |
| Create room | `src/pages/RoomsPage.tsx`; `src/lib/tauri.ts::createRoom`; `src-tauri/src/commands.rs::create_room`; `storage::create_room`; `transfer::start_room_server` | Creates a stored room, starts its Axum room server, and advertises it through discovery. |
| Join by code | `src/pages/DevicesPage.tsx`; `src/lib/tauri.ts::joinRoom`; `commands::join_room`; `discovery::discover_room`; `storage::create_room`; `transfer::{start_room_server,announce_join}` | Discovers by room-code hash, creates local joined state, starts a local room server, exchanges ports/device names/transport public keys, and stores peer connection details. |
| Nearby join | `commands::{request_nearby_join,accept_nearby_join}`; `discovery.rs`; then the same `storage::create_room`, `start_room_server`, and `announce_join` path | Adds an approval-oriented discovery path before establishing the same room connection state. |
| Active room server | `src-tauri/src/main.rs::{AppState,ActiveRoomServer}`; `transfer::start_room_server` | Holds room ID, room-code hash, ephemeral transport secret, port, expiry, and shutdown handle in current-process memory. |
| Stored peer connection | `models::StoredRoom`; `storage::{create_room,update_room_peer,get_room_by_id}` | Stores peer host, peer port, peer device name, and peer transport public key. |
| Room lifecycle | `commands::{burn_room,leave_room}`; `transfer::{notify_room_burn_with_peer,notify_room_leave,remote_burn_handler,remote_leave_handler,stop_room_server}`; `storage::{burn_room,leave_room,mark_peer_burned,mark_peer_left}` | Burn and disconnect stop the server, clear peer connection material, and terminate transfers. |

The room server binds to `0.0.0.0` on a random port and uses plain `http://`
requests. It is not an HTTPS or authenticated-connection channel.

`join_handler` checks the path room ID and burned state, then records the
request source IP, advertised peer port, device name, and transport public key.
The current generic room routes do not apply a shared HTTP authorization token
or client certificate.

### Current Room Server Endpoints

`src-tauri/src/transfer.rs::start_room_server` currently registers:

```text
POST /rooms/:room_id/join
POST /rooms/:room_id/items
POST /rooms/:room_id/transfers/start
POST /rooms/:room_id/transfers/:transfer_id/chunks
POST /rooms/:room_id/transfers/:transfer_id/finish
POST /rooms/:room_id/transfers/:transfer_id/cancel
POST /rooms/:room_id/diagnostics/ping
POST /rooms/:room_id/diagnostics/benchmark/raw
POST /rooms/:room_id/diagnostics/benchmark/pipeline
POST /rooms/:room_id/burn
POST /rooms/:room_id/leave
```

The separate `POST /rooms/:room_id/control-events` endpoint carries only the
closed typed room-control event kinds. It remains separate from ordinary room
items and file transfer.

### Encryption and Key Handling

`src-tauri/src/crypto.rs` provides:

- ChaCha20-Poly1305 payload encryption through `encrypt_bytes` and
  `decrypt_bytes`;
- ephemeral X25519 room-server transport key pairs through
  `generate_transport_secret` and `transport_public_key`;
- X25519 shared-secret derivation and HKDF-SHA256 key derivation through
  `wrap_session_for_receiver`, `unwrap_session_from_sender`, and
  `derive_transport_key`;
- the existing HKDF salt `pastey:transport:v1` and info
  `payload-key-wrap`.

Outgoing text/file payload keys are random per-item keys stored locally wrapped
by the app master key. For network delivery, the sender wraps the payload key
to the stored peer transport public key. The receiver unwraps it with its
current in-memory `ActiveRoomServer.transport_secret`.

This provides payload confidentiality and integrity to the holder of the
target transport private key. It does not make the surrounding HTTP connection
authenticated, and the current receive handlers do not verify that the
request's supplied `sender_public_key` equals the stored joined peer key.

### Current Text Send Path

```text
RoomPage.tsx::handleSendText
  -> src/lib/tauri.ts::sendTextToRoom
  -> commands::send_text_to_room
  -> storage::create_outgoing_text_item
  -> transfer::send_room_item
  -> POST /rooms/:room_id/items
  -> transfer::receive_item_handler
  -> storage::persist_incoming_item
  -> normal room history as PayloadType::Text
```

Text sends immediately. They do not enter the frontend file transfer queue or
weighted planner.

### Current File Send Path

```text
RoomPage.tsx file picker / paste / drag input
  -> App.tsx::{enqueueRoomFiles,enqueueRoomTransferInputs}
  -> transferScheduler::enqueueTransferBatch
  -> transferScheduler::planRunnableTransferLaunches
  -> App.tsx::processTransferQueueItem
  -> src/lib/tauri.ts::sendFileToRoom
  -> commands::send_file_to_room
  -> storage::create_outgoing_file_item_with_metadata
  -> transfer::send_room_file
  -> POST /rooms/:room_id/transfers/start
  -> POST /rooms/:room_id/transfers/:transfer_id/chunks
  -> POST /rooms/:room_id/transfers/:transfer_id/finish
```

`transfer::send_room_file` prefers binary-v1 chunk frames and may use JSON-v1
fallback. Chunk requests receive `ChunkAckResponse`; finish success marks the
ordinary file room item sent.

### Current Incoming Item Receive Path

```text
POST /rooms/:room_id/items
  -> transfer::receive_item_handler
  -> path room-ID and room-availability checks
  -> unwrap supplied session key with local transport secret
  -> decode and decrypt payload
  -> write file payload to Inbox when applicable
  -> storage::persist_incoming_item
  -> ordinary incoming room item
```

Large file transfers instead use
`start_file_transfer_handler -> receive_file_chunk_handler ->
finish_file_transfer_handler`, with active receiver state in
`AppState.active_file_transfers` and final file metadata persisted as an
ordinary room item.

### Tauri, Queue, and Window Boundaries

- `src-tauri/src/main.rs` registers the Tauri command boundary.
- `src/lib/tauri.ts` is the frontend invoke wrapper.
- `src/lib/transferScheduler.ts` is the frontend in-memory file queue and
  MicroFlowGroup scheduler.
- `src/lib/transferPlanner.ts::DEFAULT_TRANSFER_PLANNER_POLICY` defines
  `globalWindowBudget = 8`.
- `src-tauri/src/transfer_tuning.rs::DEFAULT_BINARY_V1_WINDOW` defines the
  empirically based lower-level binary-v1 default of `8`.
- `transfer::send_binary_chunks_pipelined` uses the active binary-v1 runtime
  window and chunk acknowledgements.

## 3. Forbidden Reuse Paths

Preview-only `RoomControlEvent` must not use:

- `RoomPage.tsx::handleSendText`;
- `src/lib/tauri.ts::sendTextToRoom`;
- `commands::send_text_to_room`;
- `storage::create_outgoing_text_item`;
- ordinary text or file `RoomItem`;
- `transfer::send_room_item`;
- `commands::send_file_to_room` or `transfer::send_room_file`;
- binary-v1 or JSON-v1 file chunk frames;
- the frontend transfer queue, planner, or MicroFlowGroup.

Reusing those paths would:

- render control events as ordinary room history or file transfers;
- persist control payloads under user-content rules;
- blur data delivery, preview acknowledgement, consent, and authority;
- make ordinary text containing control-looking JSON ambiguous;
- create a future risk that user content is accidentally interpreted as an
  execution trigger;
- alter file-transfer accounting, progress, retries, window use, and
  MicroFlowGroup behavior;
- make a control event inherit inappropriate item/file lifecycle semantics.

Ordinary text that resembles a `RoomControlEvent` remains ordinary text and
must never be parsed as control traffic.

## 4. Transport Options

| Option | Complexity | Security boundary | Encryption/session reuse | Persistence | Binary-v1 compatibility | Scheduler integration | Auditability and extension |
| --- | --- | --- | --- | --- | --- | --- | --- |
| A. New bounded room HTTP endpoint, `POST /rooms/:room_id/control-events` | Lowest incremental complexity. Matches the existing Axum room-server shape. | Clear route, DTO, size cap, validation, and response boundary. Must add explicit encrypted-envelope and peer-session checks because HTTP itself is plain. | Can reuse current ephemeral X25519 room-server key context with a new domain-separated control key derivation. | Separate current-session memory only. | Fully compatible; does not touch chunk protocols. | CL-3B transport remains separate; CL-4 now accounts for real local outgoing demand at the frontend scheduler boundary. | Strongly inspectable with typed request/receipt and bounded sanitized errors. |
| B. New typed frame within an existing authenticated room connection | Not currently available as described: the repo uses independent HTTP requests, not one authenticated persistent room connection. | Could be clean only after a connection/session protocol exists. | Would require designing that connection and framing first. | Separate by design. | Risks coupling control work to current transfer framing or inventing a new connection protocol. | Could integrate later. | Good eventually, but substantially larger than CL-3B. |
| C. Dedicated control substream/session deferred to binary-v2 | Highest near-term complexity and depends on unimplemented binary-v2 design. | Potentially strongest protocol-native separation. | Could define purpose-built session keys and frames. | Separate by design. | Does not help binary-v1 peers without another path. | Natural later integration. | Extensible, but premature for preview-only transport. |

### Recommendation

Use **Option A** for CL-3B: one new bounded typed HTTP route adjacent to, but
separate from, existing item and transfer routes.

Option A does not require binary-v2 and does not change binary-v1. The route
must carry its own encrypted, session-bound control envelope; merely adding a
JSON route to the current plain HTTP server would not be sufficient.

## 5. Proposed CL-3B Transport Contract

### Endpoint and Media Types

```text
POST /rooms/:room_id/control-events
Content-Type: application/vnd.pastey.room-control-envelope+json
Accept: application/vnd.pastey.room-control-receipt+json
```

Initial allowed decrypted event kinds:

```text
capability_preview
capability_preview_ack
capability_preview_deny
capability_preview_invalid
capability_preview_expired
```

Every other kind is rejected.

### Bounded Encrypted Request Envelope

The exact Rust DTO should be closed-field and versioned. A feasible minimal
shape is:

```json
{
  "schemaVersion": "pastey-room-control-transport/v1",
  "senderPublicKey": "<base64 current room-session transport public key>",
  "wrappedEventKey": "<base64 domain-separated wrapped event key>",
  "keyWrapNonce": "<base64 nonce>",
  "eventNonce": "<base64 nonce>",
  "ciphertext": "<base64 encrypted RoomControlEvent JSON>"
}
```

Rules:

- Maximum decrypted `RoomControlEvent`: existing CL-1 cap, `64 KiB`.
- Maximum complete HTTP request body: `96 KiB`, enforced before JSON parsing.
- Maximum complete response body: `4 KiB`.
- Suggested connect timeout: `2 seconds`.
- Suggested total request timeout: `5 seconds`.
- Maximum event lifetime accepted by transport: `2 minutes`. CL-1 builders
  default to two minutes, but CL-3B must add an explicit maximum-lifetime check
  because the current validator checks expiry ordering, not a maximum TTL.
- Maximum shared accepted-but-not-finalized current-session control records per
  room: `64`, with no fixed inbound/outbound half split.
- Initial per-room receive limit: burst `8`, sustained `30 events/minute`.
- Current-session replay records: at most `256`, retained until the relevant
  event expiry or session end, whichever comes first.
- No automatic retry in CL-3B.
- Status events use the same endpoint and contract.
- The route path room ID must equal the active server room ID.
- Decrypted `event.roomRef` must equal the path room ID.
- `senderPublicKey` must exactly equal the stored current peer transport public
  key for the room.
- The request source IP should equal the stored current peer host. A changed
  peer address requires rejoin rather than silent rebinding.
- `sourceDeviceRef` and `targetPeerRef` must be current-session references
  derived from the joined peer and local transport public-key fingerprints.
  Current mock refs such as `local-device-preview` and `mock-peer-1` are not
  transport identities and cannot be accepted by CL-3B.
- The receiver validates expiry and current-session replay state before
  accepting the event.

### Transport Receipt

Successful acceptance returns `202 Accepted` with a small receipt encrypted
under the event key using a fresh receipt nonce:

```json
{
  "schemaVersion": "pastey-room-control-receipt-envelope/v1",
  "receiptNonce": "<base64 nonce>",
  "ciphertext": "<base64 encrypted receipt JSON>"
}
```

The decrypted receipt should contain only:

```json
{
  "schemaVersion": "pastey-room-control-receipt/v1",
  "eventId": "<accepted event ID>",
  "deliveryStatus": "accepted_for_inbound_control_processing",
  "receivedAt": "<ISO timestamp>"
}
```

This receipt means only that the peer transport validated and accepted the
event into bounded current-session control processing. It is not
`capability_preview_ack`, peer consent, or execution authorization.

`capability_preview_ack` is a separate `RoomControlEvent` sent later after the
receiver user explicitly allows that exact preview once. It is not transport
delivery, execution, completion, or reusable authority.

### Status Codes and Sanitized Errors

| Status | Meaning |
| --- | --- |
| `202 Accepted` | Current-session-key-bound envelope decrypted, validated, replay-checked, and accepted into bounded current-session control processing. |
| `400 Bad Request` | Malformed envelope, invalid encoding, decrypt/integrity failure, malformed event, unsafe fields, or invalid schema. Return a generic bounded error. |
| `403 Forbidden` | Sender key, source address, source session reference, or target session reference does not match the current joined room peer. |
| `404 Not Found` | Path room ID does not match this server or the room does not exist. |
| `409 Conflict` | Duplicate event, envelope, or embedded request ID in the current session. |
| `410 Gone` | Room is burned/unavailable or the event is expired. |
| `413 Payload Too Large` | Outer request or decrypted event exceeds its cap. |
| `415 Unsupported Media Type` | Wrong content type. |
| `429 Too Many Requests` | Bounded current-session control inbox or rate limit is exceeded. |
| `500 Internal Server Error` | Local processing failed without exposing internal detail. |

Errors must use
`Content-Type: application/vnd.pastey.room-control-error+json` and a dedicated
bounded control error DTO with a stable code and generic message. They must not
expose keys, ciphertext, raw event payload, paths, logs, parser internals, or
secrets.

## 6. Encryption, Authentication, and Identity Findings

### What Exists

- Room-server HTTP requests are plain HTTP, not an authenticated/encrypted
  connection.
- Item and file contents use payload-level ChaCha20-Poly1305.
- Per-item/file payload keys are wrapped using an X25519 shared secret derived
  from the sender's active room-server secret and the stored peer transport
  public key.
- Transport public keys are exchanged during join and stored on the peer room
  record.
- The active server transport secret is current-process/current-room-server
  state, not durable device identity.

### CL-3B Requirements

CL-3B should reuse the current ephemeral room transport key context, but not the
existing `derive_transport_key` output unchanged. Add a separate
domain-separated derivation, for example using the same HKDF salt with a new
info label such as `room-control-event-key-wrap-v1`. Existing item/file key
wrapping must remain unchanged.

Each request should:

1. generate a random event key;
2. encrypt the serialized validated `RoomControlEvent` with ChaCha20-Poly1305;
3. wrap the event key to the stored peer transport public key using the
   domain-separated control derivation;
4. include the sender's current room-server public key;
5. require the receiver to compare that key with the stored joined peer key
   before unwrapping;
6. validate current-session source/target fingerprints after decrypting.

The current join exchange is not a durable cryptographic device-identity
system. `join_handler` currently updates stored peer host/port/key state from a
request that reaches the room-ID route; it does not verify a durable peer
credential. CL-3B can establish binding to the currently recorded joined
session, not a claim about a long-term human or device identity. Before CL-3B,
the security review must decide whether that current join trust is acceptable
for preview-only events or whether join-state authentication must be
strengthened first.

These boundaries remain distinct:

| Boundary | Meaning |
| --- | --- |
| Transport confidentiality/integrity | Only the intended current room-server key holder should decrypt or alter the control envelope undetected. |
| Current room/session binding | The sender key, source address, room ID, and session-derived refs match the currently recorded joined peer state. This inherits the current join trust boundary. |
| Event validation | The decrypted event is a bounded, allowed, preview-only `RoomControlEvent`. |
| User consent | A user explicitly approves a later capability request. Not provided by transport. |
| Execution authorization | A future peer PolicyGate/executor decision. Not provided by room trust, delivery, or preview acknowledgement. |

Trusted room membership permits bounded communication. It does not imply
execution authorization.

## 7. Persistence and Visibility Policy

Preview-only room-control events must:

- not become ordinary room items;
- not appear as chat messages;
- not be stored in normal room history;
- not enter transfer history or progress accounting;
- not be copied into file queue, MicroFlowGroup, Inbox, or payload storage;
- remain current-session only in CL-3B/CL-3C.

A bounded in-memory current-session record may retain:

- event ID;
- request ID;
- envelope ID;
- source session ref;
- target session ref;
- event kind;
- received/enqueued timestamp;
- final preview status.

Raw decrypted event payloads should be retained only while needed for visible
current-session preview processing. No hidden long-term replay or behavior
history should be introduced. A persistent visible audit record is a separate
future product/security decision.

The active Room Agent Bridge panel shows transport acceptance, rejection
category, queue state, and final preview state without raw keys, ciphertext,
secrets, or payload dumps. Advanced transport details remain collapsed.
Redacted structured lifecycle entries in `pastey.log` mirror these transitions
for audit only and are never used to restore queue, consent, or transport
state.

## 8. Future Inbound Processing Path

Full inbound processing should be:

```text
POST /rooms/:room_id/control-events
  -> room-control route body-size and content-type guard
  -> active room/path/source-address/current peer-key checks
  -> decode envelope and unwrap domain-separated event key
  -> decrypt bounded event bytes
  -> parse closed RoomControlEvent shape
  -> validateRoomControlEvent-equivalent Rust validation
  -> expected room/source/target session checks
  -> expiry and current-session replay checks
  -> bounded current-session transport inbox
  -> CL-3C enqueue inbound ControlQueueItem
  -> encrypted bounded transport receipt
```

Likely repository ownership:

| Step | Likely future location |
| --- | --- |
| Route registration | `src-tauri/src/transfer.rs::start_room_server`, adding only the new route |
| DTO, cap, encryption-envelope decode, status mapping, inbound current-session inbox/replay state | New `src-tauri/src/room_control.rs` |
| Domain-separated key-wrap helpers | New helpers in `src-tauri/src/crypto.rs`, without changing existing item/file helpers |
| Current-session backend state | New bounded state owned by `AppState` in `src-tauri/src/main.rs` |
| Rust event validator | `src-tauri/src/room_control.rs`, mirrored against the closed CL-1 contract and cross-checked by fixtures/tests |
| Frontend queue adaptation | CL-3C changes around `src/lib/agentBridge/controlQueue.ts` and Developer Tools |

The Rust receive boundary must validate independently. A network handler cannot
trust that a remote sender ran the TypeScript validator.

The Rust receive/transport boundary never executes a capability. A later
explicit frontend queue-processing step may pass a validated CL-6 request to
the fixed host-owned in-process function; no runtime launch or
stdout/stderr/exit-code output exists.

## 9. Future Outbound Processing Path

Full outbound processing should be:

```text
confirmed local preview event
  -> validate and enqueue outbound ControlQueueItem
  -> select by CL-2 control priority/FIFO policy
  -> verify current room/session peer binding
  -> encrypt and send typed control envelope
  -> verify/decrypt bounded transport receipt
  -> CL-3C local transport-delivery state update
  -> wait for a separate inbound preview ack/deny/invalid/expired event
```

CL-3B should expose one narrowly typed outbound bridge, not a generic HTTP or
arbitrary event sender. A likely future boundary is:

- `src/lib/tauri.ts::sendRoomControlEvent`;
- a new narrow `commands::send_room_control_event`;
- `room_control::send_room_control_event`.

The Rust boundary must revalidate the event, room/session binding, expiry, and
allowed kind before network send.

Transport delivery success must not mark the preview
`acknowledged_preview_only`. Peer acknowledgement remains a separate inbound
event. CL-3B should not retry automatically; a timeout or disconnect becomes a
visible bounded transport failure.

## 10. Relationship to the Unified Eight-Window Model

- `transfer_tuning.rs::DEFAULT_BINARY_V1_WINDOW = 8` is the empirically tested
  lower-level outgoing runtime capacity basis.
- `transferPlanner.ts::globalWindowBudget = 8` is the frontend scheduler
  abstraction of that same capacity.
- They are not independent pools.
- The proposed control transport is not a ninth lane.
- CL-4 now reserves from the unified sender budget before an outbound control
  send proceeds.
- Real queued, selected, or sending outbound control work exposes data `7` /
  control `1`; inbound-only review state does not reserve.
- With no eligible control backlog, the model remains data `8` / control `0`.
- Control events never enter MicroFlowGroup.

Logical control-lane accounting is a scheduler/resource policy. Physical HTTP
request concurrency is an implementation detail. Inbound and outbound network
directions may use different sockets at the operating-system level while still
sharing one logical control-lane budget. The control lane must not be described
as a literal physical wire.

## 11. Threat and Failure Analysis

| Case | Fail-closed behavior |
| --- | --- |
| Malformed JSON or closed-field violation | Reject before event processing with bounded `400`; do not enqueue. |
| Oversized outer request or decrypted event | Reject with `413`; do not parse beyond the cap or reserve control backlog. |
| Unknown event kind | Reject with bounded `400`; do not forward or reinterpret. |
| Unsafe fields such as command, shell, code, path, stdout, stderr, or exitCode | Reject through the closed event validator; do not enqueue. |
| Room/path mismatch | Return `404` or bounded invalid-room response; do not reveal other room state. |
| Source spoofing | Require request source address, supplied sender public key, stored peer public key, and decrypted source session ref to match current joined state. Reject with `403`. |
| Unauthorized join-state replacement | Current join handling is not a durable authenticated identity proof. CL-3B must either accept this explicitly as a preview-only risk with visible rejoin state or strengthen join authentication before enabling transport. Never treat current peer-key state as execution authority. |
| Target mismatch | Require the decrypted target session ref to match the local active room-server fingerprint. Reject with `403`. |
| Expired event | Reject with `410`; do not enqueue or retry. |
| Duplicate/replayed event, envelope, or request ID | Reject with `409`; do not re-enqueue or re-run UI transitions. |
| Replay after reconnect | New active room-server keys produce new session refs; old ciphertext/key wraps and old refs must fail. Short event expiry remains required. |
| Peer disconnect or burned room | Reject/abort, clear bounded current-session control state, and surface a generic terminal transport status. |
| Timeout | Mark local transport delivery failed/unknown; do not assume peer receipt and do not automatically retry. |
| Transport delivery succeeds, then peer denies | Keep delivery and peer denial as separate states; denial is final and does not retry/escalate. |
| Peer sends `capability_preview_ack` | Treat it as exact one-time receiver consent for the matching preview. Never treat it as execution, completion, durable trust, or reusable authority. |
| Ordinary text contains control-looking JSON | Preserve as ordinary user text. Never parse text-room items as control events. |
| Malicious peer inside a trusted room | Enforce schema, caps, rate/inbox bounds, expiry, replay checks, visible UI, exact consent consumption, and fixed-capability validation. Trusted-room membership does not bypass validation or consent. |
| Forged/plain HTTP success response | Require the bounded successful receipt to prove possession of the event key; otherwise delivery is not confirmed. |

## 12. Implemented CL-3B Scope

The transport currently implements:

- the five preview/status kinds plus the closed CL-6
  `capability_execute_request` and `capability_execution_result` kinds;
- one `POST /rooms/:room_id/control-events` route;
- one narrowly typed outbound bridge;
- a `96 KiB` outer request cap, existing `64 KiB` decrypted event cap, and
  `4 KiB` response cap;
- domain-separated encrypted control envelopes and encrypted success receipts;
- explicit current-session peer key, source address, room, source-ref, and
  target-ref validation;
- a bounded current-session backend inbox/replay cache;
- a maximum two-minute event lifetime, shared `64`-record current-session
  inbox cap, `256` replay-record cap, and initial burst/sustained rate limit;
- no ordinary room-item or transfer persistence;
- no transport-owned execution authority;
- no automatic retry;
- active Room visibility with sanitized state only;
- focused Rust/TypeScript contract, crypto-envelope, replay, expiry, cap, and
  failure tests.

Implemented files/functions:

| File / function | Minimal CL-3B change |
| --- | --- |
| `src-tauri/src/room_control.rs` | Implements transport DTOs, constants, sender/receiver helpers, Rust event validation, current-session inbox/replay/rate bounds, encrypted receipt/error mapping, and focused tests. |
| `src-tauri/src/crypto.rs` | Adds control-specific `room-control-event-key-wrap-v1` domain-separated key wrapping while preserving existing item/file helpers. |
| `src-tauri/src/transfer.rs::start_room_server` | Registers only the new route. Existing item/file/chunk handlers remain separate. Room-server stop and join transition clear current-session control state. |
| `src-tauri/src/main.rs::{AppState,invoke_handler}` | Owns bounded current-session transport state and registers narrow control commands. |
| `src-tauri/src/commands.rs` | Adds typed send, session-context, and received-inbox commands that delegate to `room_control`; no generic endpoint caller. |
| `src/lib/tauri.ts` | Adds validated typed send/session/inbox wrappers. |
| `src/lib/agentBridge/roomControlTransport.ts` | Rebinds a validated preview envelope to current-session refs, exposes bounded send observability, and processes one selected outbound queue item through transport. |
| `src/lib/agentBridge/controlQueue.ts` and `controlWindowRuntime.ts` | Own the current-session queue and classify only nonterminal outbound transport work as runtime capacity demand. |
| `src/components/AiSlotPreview.tsx` | Exposes the active Room's session-level queue/inbox controls independently of outbound advisory generation. The bounded execution action/result UI remains outside chat. |
| CL-3B tests | Focused Rust transport/crypto/validation/bounds tests and TypeScript session-binding tests. |

Files and behavior that CL-3B should not modify:

- `transfer::send_room_item`, `receive_item_handler`, and ordinary room-item
  storage;
- `transfer::send_room_file`, binary-v1/JSON-v1 chunk encode/decode, ACK,
  finish, retry, and runtime-window hot paths;
- `src/lib/transferScheduler.ts`, `src/lib/transferPlanner.ts`, `src/App.tsx`
  file dispatch, and MicroFlowGroup;
- room lifecycle semantics;
- generic peer execution, MCP, or model-provider behavior.

## 13. Staging

### CL-3A: Transport Design and Feasibility

Complete in this document. No runtime behavior changed.

### CL-3B: Minimal Preview-Only Room-Control Transport

Implemented. The bounded typed endpoint, encrypted session-bound envelope,
encrypted delivery receipt, current-session backend inbox/replay/rate bounds,
narrow outbound bridge, and active Room visibility exist. No scheduler
reservation or execution exists.

### CL-3C: Delivery/Status Integration With Local Control Queues

Implemented. Outbound events enqueue before one-item priority selection and
transport send; receipts or sanitized failures update the same queue item.
Non-destructive Rust inbox refresh validates and deduplicates events into the
inbound queue. Transport delivery remains separate from preview
acknowledgement, consent, and execution.

### CL-4: Real Scheduler Reservation From the Unified Eight-Window Budget

Implemented. Real eligible local outgoing control demand exposes data `7` /
control `1`; idle restores data `8` / control `0` after a `750 ms` quiet
period. Existing active binary-v1 senders hot-adjust without restart. Inbound
review backlog alone does not reserve, and MicroFlowGroup semantics do not
change.

### CL-5: Peer PolicyGate and Explicit Consent

Implemented. Processing an inbound `capability_preview` runs a receiver-side
PolicyGate for the exact fixed Hello Peer capability/message and exposes only
explicit Allow once or Deny. The decision is event/envelope/request/session
bound, current-session-only, and expiring. Allow once queues
`capability_preview_ack`; deny queues `capability_preview_deny`; one later
explicit queue-processing action sends it. An ack means the receiver allowed
that exact preview once. It is not transport delivery, execution, completion,
or durable trust.

### CL-6: Bounded Hello Peer Executor

Implemented. A matched allow-once ack permits one explicit sender action to
queue an exact execution request. The receiver revalidates the local consent
record, consumes it once, calls only the fixed in-process Hello Peer template,
and queues a bounded typed result for a later explicit send. No shell, process,
file, network, generic runtime, reusable trust, arbitrary output, or automatic
retry exists.

## 14. Feasibility Conclusion

A minimal preview-only room-control transport appears feasible with the current
architecture. The safest smallest insertion is a separate bounded Axum room
route plus a narrow outbound Tauri bridge.

The transport design does not require binary-v2, changes to binary-v1 format,
or changes to MicroFlowGroup. CL-4 separately adds sender-side scheduler
reservation without changing this route or envelope. The transport requires a
new
domain-separated encrypted control envelope and explicit current-session
peer-key/source/target binding because the existing room HTTP routes are not a
generic authenticated/encrypted channel. The current join-state trust boundary
also requires an explicit security decision before CL-3B is enabled.

CL-3B now provides real preview-only transport delivery bound to the currently
recorded joined session; it does not claim durable authenticated device
identity. CL-3C connects that transport to the existing local queue and uses
its priority/FIFO policy. CL-4 activates sender-side reservation for outgoing
transport demand. CL-5 adds one-time peer consent. CL-6 adds only the fixed
bounded Hello Peer execution/result stage and does not broaden transport
authority.

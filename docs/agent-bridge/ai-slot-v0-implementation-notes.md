# AI Slot v0 / Phase E1 Implementation Notes

AI Slot Phase E1 is an internal advisory, local-confirmation, request-preview,
and capability-envelope preview frontend slice. It demonstrates the following
safe pipeline inside Developer Tools for both mock and experimental cloud
routes:

```text
MockProvider or CloudOpenAICompatibleProvider
  -> AiGenerateResult
  -> AiActionPlan
  -> validateAiActionPlan
  -> evaluateAiPolicy
  -> createPendingAiAction
  -> local confirm, cancel, or expire
  -> buildHelloPeerRequestFromPendingAction
  -> validateHelloPeerRequest
  -> local outbound request preview
  -> buildCapabilityRequestPreviewEnvelope
  -> validateCapabilityRequestPreviewEnvelope
  -> local inbound-preview simulation
  -> acknowledge or deny preview only
  -> Developer Tools preview
```

The implementation lives under `src/lib/ai/`, with a preview component at
`src/components/AiSlotPreview.tsx`. The preview uses a synthetic current-session
context. `MockProvider` returns a deterministic safe
`request_peer_hello_demo` plan. `CloudOpenAICompatibleProvider` calls a
configured OpenAI-compatible chat-completions endpoint and parses JSON output
without repair.

An accepted Hello Peer proposal becomes a visible `PendingAiAction` with a
short expiry, local pending ID, canonical payload, and deterministic payload
hash. The hash binds the local preview confirmation to the displayed payload;
it is not a transport-security primitive. Confirming changes status only to
`confirmed_local_only`.

A confirmed local action can now become a canonical `HelloPeerRequest` with a
request ID, nonce, short expiry, pending-payload hash, request-payload hash, and
`transportStatus: "preview_only"`. This is a local outbound preview only. No
request is sent and no peer receives it. The identifiers, expiry, and hashes
prepare future replay defenses but do not provide transport security or replay
protection.

Phase E1 adds a `CapabilityRequestPreviewEnvelope`, current-session duplicate
detection, and local inbound preview/acknowledge/deny state. Actual room
transport is blocked in this build. The existing `sendTextToRoom` path creates,
encrypts, persists, and transfers an ordinary user text room item through
`send_text_to_room` and `transfer::send_room_item`; it is not a safe
capability-preview transport and was not reused.

Control Lane CL-1 adds a separate frontend/library-only `RoomControlEvent`
foundation under `src/lib/agentBridge/`. It can wrap a validated
`CapabilityRequestPreviewEnvelope` in a typed `capability_preview` event, build
bounded preview acknowledge/deny/invalid/expired status events, validate exact
preview-only shapes, and detect duplicate event/envelope/request IDs in
current-session memory. Its pure `computeControlLaneBudget` helper returns data
`8` / control `0` without backlog and data `7` / control `1` with backlog. The
helper is not wired into the scheduler, and no event is sent or received.

Control Lane CL-2 adds a current-session-only local queue simulation under
`src/lib/agentBridge/controlQueue.ts` and exposes it in the Developer Tools
preview. It simulates separate outbound/inbound queues, deny-first priority,
FIFO tie-breaking, duplicate/replay rejection, expiry, strict local terminal
transitions, next-item selection, and hypothetical budget impact. With local
backlog the pure calculation is data `7` / control `1`; without backlog it is
data `8` / control `0`. It is not wired into the scheduler and sends or
receives nothing.

Control Lane CL-3B adds real preview-only room-control transport delivery
through a separate bounded encrypted room route. Developer Tools can bind a
preview to an active current room session, send it, show the encrypted
transport delivery receipt, and refresh the bounded received control inbox.
The receipt means accepted for the peer's local inbox only. It is not preview
acknowledgement, peer consent, or execution authority. Received transport
events are not yet injected into CL-2 queues; that remains CL-3C.

CL-3B.2 adds send observability only. Every transport-send click now shows an
immediate sending state followed by one visible accepted or sanitized rejected
result. Duplicate/replay, expiry, validation, session, peer, inbox, rate-limit,
oversize, malformed-receipt, and generic transport failures no longer fail
silently in Developer Tools. Repeated clicks resend the same currently built
event ID so replay behavior can be inspected. There is no automatic retry,
CL-3C queue integration, consent, execution, or scheduler reservation.

## AI Slot Phase E1 Status

- Mock advisory loop: implemented.
- Cloud provider advisory preview: implemented.
- Validator and deny-first `PolicyGate`: implemented and shared by both routes.
- Local pending-action confirmation, cancellation, and expiry: implemented.
- Local `HelloPeerRequest` builder, validator, and outbound preview: implemented.
- Capability preview envelope, validator, duplicate cache, and local inbound
  preview simulation: implemented.
- CL-1 type-only `RoomControlEvent` builders, validator, current-session
  duplicate helper, and pure control-lane budget feasibility helper:
  implemented.
- CL-2 current-session local control queue simulation, priority selection,
  expiry/replay handling, local status transitions, and hypothetical budget
  display: implemented.
- CL-3B preview-only room-control send/receive and bounded Rust inbox:
  implemented.
- CL-3B.2 latest room-control send result and sanitized rejection visibility:
  implemented.
- CL-3C transport delivery/status integration with CL-2 queues: not
  implemented.
- Scheduler reservation: not implemented.
- Provider output: untrusted.
- Execution: not implemented.
- Peer request transport: not implemented.
- Peer executor: not implemented.
- Production API-key storage: not implemented.
- Local GGUF and MCP integration: not implemented.

The current boundary is deliberately narrow:

- mock provider remains the default;
- cloud base URL, model, and optional API key are Developer Tools runtime-memory
  inputs only;
- cloud calls use a strict whitelisted synthetic context;
- no real room-state context;
- no capability transport;
- preview-only room-control send/receive exists, but scheduler reservation does
  not;
- no persistence or runtime dispatch from the local control queue simulation;
- no sent peer request, peer receive path, or peer execution;
- local confirmation only, with no action dispatch;
- no runtime behavior outside the Developer Tools preview.

Model output is treated as an untrusted proposal. The manual validator checks
the action-plan shape and rejects unsafe fields. The local `PolicyGate`
separately checks the exact Hello Peer message, capability, visible/trusted mock
peer, required confirmation flag, and constrained executor settings.

The preview is informational only:

- **Advisory only - no action is executed.**
- **Local confirmation only - no action is executed.**
- **Transport delivery is not peer consent.**
- **Scheduler reservation is not active.**
- **Inbound preview only - this cannot execute.**
- **Acknowledging preview is not execution consent.**
- **Local control queue simulation only - no room event is sent.**
- **Current scheduler behavior is unchanged.**
- **No peer executor exists in Phase E1.**
- **No stdout, stderr, exit code, or runtime output exists in Phase E1.**
- **Peer consent is still required before any future execution.**
- **Trusted room is not execution authorization.**
- **Cloud context is redacted and current-session only.**
- **Provider output is untrusted and must pass validation and PolicyGate.**
- **No raw shell, file access, peer filesystem search, or hidden transfer is
  available.**

AI Slot v0 does not modify room lifecycle, transfers, scheduler,
MicroFlowGroup, Device Diagnostics, binary-v1, or binary-v2 behavior. Pastey
remains AI-ready, not AI-dependent.

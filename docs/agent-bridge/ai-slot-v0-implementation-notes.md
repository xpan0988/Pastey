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

A confirmed local action can become a canonical `HelloPeerRequest` with a
request ID, nonce, short expiry, pending-payload hash, request-payload hash, and
`transportStatus: "preview_only"`. It remains preview-only, but the validated
envelope can be queued and delivered through the separate room-control
transport. The identifiers, expiry, hashes, transport replay cache, and CL-5
decision cache provide current-session binding and replay defenses; they do not
provide durable device identity or execution authority.

Phase E1 adds a `CapabilityRequestPreviewEnvelope`, current-session duplicate
detection, and local inbound preview simulation. The separate CL-3 room-control
transport carries real preview events; the existing `sendTextToRoom` path still
creates, encrypts, persists, and transfers ordinary user text and was not
reused.

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
events are integrated into the existing CL-2 queue by CL-3C.

CL-3B.2 adds send observability only. Every transport-send click now shows an
immediate sending state followed by one visible accepted or sanitized rejected
result. Duplicate/replay, expiry, validation, session, peer, inbox, rate-limit,
oversize, malformed-receipt, and generic transport failures no longer fail
silently in Developer Tools. Repeated clicks resend the same currently built
event ID so replay behavior can be inspected. There is no automatic retry,
consent, execution, or scheduler reservation.

CL-3C connects real transport to the existing local `ControlQueueState`.
Outbound previews enqueue before one explicit priority-based process action;
transport receipts and failures update the same queue item without becoming
peer acknowledgement. Non-destructive Rust inbox refresh validates and
deduplicates real inbound events into the inbound queue. The receiver queue and
inbox controls require only an active room session, not an outbound advisory.
CL-4 uses real local **outgoing** control demand from this queue to activate
sender-side runtime reservation. Queued, selected, or sending outbound control
work changes the data target to `7`; terminal outbound or inbound-only review
state does not reserve. Idle restores `8` after a `750 ms` quiet period.

CL-5 adds a receiver-side PolicyGate and exact one-time decision state in
`src/lib/agentBridge/peerConsent.ts`. Processing an inbound preview checks the
current room/session/source/target, exact fixed capability and message, expiry,
unsafe fields, and prior decisions. The receiver may explicitly Allow once or
Deny. The resulting ack/deny event enters the existing outbound queue and is
sent only by a later explicit Process next action. Allow once is bound to one
event/envelope/request and expires; it is not remembered trust and executes
nothing.

CL-6 adds `src/lib/agentBridge/helloPeerExecution.ts` and two closed typed
control-event kinds: `capability_execute_request` and
`capability_execution_result`. A sender can build an execution request only
after processing a matched allow-once ack with its exact bounded consent grant,
and only through the explicit **Request Hello Peer execution** action. The
receiver revalidates room/session/source/target, request hash, capability,
message, expiry, replay state, and the exact local `PeerConsentRecord`, then
consumes the consent before calling the zero-argument in-process
`executeHelloPeerTemplate`. The only successful output is `hello peer!`.
Results are bounded typed control events and require a later explicit Process
next action. There is no automatic request, response retry, generic runtime, or
reusable trust.

Agent Bridge configuration and workflow now have separate owners. Settings
mounts `AgentBridgeSettings` and retains only the enabled state, provider
selection, runtime-memory cloud base URL/model/API key, lifecycle log level,
log clearing, and a concise safety summary. The active Room mounts
`AiSlotPreview` with its exact `RoomInfo`; `RoomControlPanel` uses that room's
current session and peer context directly rather than listing or selecting
rooms independently.

The Room panel uses progressive disclosure. Its default view prioritizes one
stage-aware advisory action and the real room-control queue/inbox workflow.
Full identifiers, validation details, canonical payloads, complete queue
lists, replay-test controls, local simulation tools, and detailed safety notes
remain under collapsed diagnostics. Room/session/peer identity changes clear
the mounted current-session workflow state.

`src/lib/agentBridge/logging.ts` writes allowlisted, redacted lifecycle events
through the existing frontend diagnostics bridge as one
`[pastey:agent-bridge]` JSON object per line. References are shortened, raw
payloads and secrets are excluded, and the existing bounded `pastey.log`
rotation remains authoritative. These records are audit mirrors only: they
are never read to reconstruct state and are not authority, consent, or trust.
This UI and logging refactor does not change provider, validation, transport,
queue, scheduler, security, or execution behavior.

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
- CL-3C transport delivery/status integration with the existing control queue:
  implemented.
- CL-4 sender-side scheduler reservation and active binary-v1 hot adjustment:
  implemented.
- CL-5 receiver PolicyGate, explicit one-time consent, and ack/deny queue
  integration: implemented.
- Provider output: untrusted.
- Fixed bounded Hello Peer execution: implemented.
- Preview-only peer request transport: implemented.
- Generic peer executor: not implemented.
- Production API-key storage: not implemented.
- Local GGUF and MCP integration: not implemented.

The current boundary is deliberately narrow:

- mock provider remains the default;
- cloud base URL, model, and optional API key are Settings runtime-memory
  inputs only;
- cloud calls use a strict whitelisted synthetic context;
- no real room-state context;
- preview-only room-control send/receive and outgoing-demand scheduler
  reservation exist;
- no persistence or generic execution from the local control queue;
- preview, explicit one-time consent, one fixed Hello Peer execution request,
  and one bounded result exist;
- local confirmation only, with no action dispatch;
- current-session Agent Bridge workflow is owned by the active Room panel;
- structured lifecycle logs are redacted audit mirrors only and never runtime
  state or authorization evidence.

Model output is treated as an untrusted proposal. The manual validator checks
the action-plan shape and rejects unsafe fields. The local `PolicyGate`
separately checks the exact Hello Peer message, capability, visible/trusted mock
peer, required confirmation flag, and constrained executor settings.

The preview is informational only:

- **Advisory/provider output cannot execute or construct an execution request.**
- **Local preview confirmation alone does not execute.**
- **Transport delivery is not peer consent.**
- **Scheduler reservation is active only for real local outgoing control
  demand.**
- **Inbound preview only - this cannot execute.**
- **Acknowledging preview records exact one-time receiver consent, but is not
  execution or proof of execution.**
- **Inbound-only preview review does not reserve an outgoing data window.**
- **No transfer is cancelled or restarted for CL-4 window adjustment.**
- **Only one fixed host-owned Hello Peer executor exists; there is no generic
  peer executor.**
- **The only successful output is bounded `hello peer!`; no stdout, stderr,
  exit code, logs, stack trace, or attachment exists.**
- **Unexpired exact one-time peer consent is consumed once before execution.**
- **Trusted room is not execution authorization.**
- **Cloud context is redacted and current-session only.**
- **Provider output is untrusted and must pass validation and PolicyGate.**
- **No raw shell, file access, peer filesystem search, or hidden transfer is
  available.**

AI Slot v0 does not modify room lifecycle, text/file semantics, MicroFlowGroup
semantics, Device Diagnostics, binary-v1 format, or binary-v2 behavior. CL-4
does apply deterministic sender-side planner/runtime-window policy for real
outgoing control demand. Pastey remains AI-ready, not AI-dependent.

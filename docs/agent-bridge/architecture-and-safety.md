# Agent Bridge Architecture And Safety

Agent Bridge is the Layer 5 narrow capability path for model-assisted planning, host validation, explicit consent, bounded execution, result return, and redacted audit. For the project-wide layer contract, see [../architecture/Project-specifications.md](../architecture/Project-specifications.md). For Bridge membership and authority boundaries, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For routing semantics, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md).

The current product reality is a fixed Hello Peer vertical slice. It proves the safety shape, not a general agent workspace.

## Current Flow

1. The Bridge UI builds a redacted current-session context snapshot. Legacy implementation term: Room UI.
2. A provider returns an advisory action plan.
3. The host validates the plan with an allowlist and unsafe-field scan.
4. The local PolicyGate decides whether the plan may enter pending confirmation.
5. The local user explicitly confirms sending a Hello Peer preview.
6. The preview becomes a typed capability preview envelope.
7. The envelope is sent through the encrypted Bridge control transport. Legacy implementation term: room-control transport.
8. The receiver validates replay, expiry, queue bounds, and PolicyGate rules.
9. The receiver can choose Allow once or Deny.
10. A matched allow-once decision permits exactly one execution request for the same capability, request, peer, and Bridge/session binding.
11. The receiver consumes the consent once, runs the fixed in-process Hello Peer executor, and returns a typed result.
12. Both sides write redacted lifecycle audit logs.

## Implemented Production Paths

- Provider and AI types: `src/lib/ai/types.ts`.
- Mock provider: `src/lib/ai/mockProvider.ts`.
- OpenAI-compatible cloud provider: `src/lib/ai/cloudOpenAICompatibleProvider.ts`.
- Redacted context snapshots: `src/lib/ai/contextSnapshot.ts`.
- Action-plan validation: `src/lib/ai/actionPlanValidator.ts`.
- Local PolicyGate: `src/lib/ai/policyGate.ts`.
- Pending local confirmation: `src/lib/ai/pendingAction.ts`.
- Hello Peer request construction: `src/lib/ai/helloPeerRequest.ts`.
- Capability preview envelope: `src/lib/ai/capabilityPreviewEnvelope.ts`.
- Bridge control events: `src/lib/agentBridge/roomControlEvent.ts`. Legacy implementation term: `RoomControlEvent`.
- Control queue state: `src/lib/agentBridge/controlQueue.ts`.
- Receiver consent: `src/lib/agentBridge/peerConsent.ts`.
- Fixed executor: `src/lib/agentBridge/helloPeerExecution.ts`.
- Redacted logging: `src/lib/agentBridge/logging.ts`.
- Bridge-scoped UI: `src/components/agentBridge/RoomControlPanel.tsx` and `src/components/AiSlotPreview.tsx`.
- Runtime room-control endpoint: `src-tauri/src/room_control.rs`.

## Safety Invariants

- Model output is advisory data, never executable instruction.
- Host validation is required before a plan can become a preview.
- PolicyGate is deny-first.
- Local confirmation only authorizes sending a preview to a peer.
- Transport delivery is not receiver consent.
- Receiver Allow once applies to one exact bounded request.
- Consent is consumed once and is not reusable trust.
- Accepted Bridge peer status is not durable trust or execution authority.
- Nearby accept, 8-digit code join, and session verification never authorize capability execution.
- Capability events must bind to an exact selected peer/session/request and must not use broadcast by default.
- Logs mirror lifecycle but are not queue state, consent state, execution state, or authority.

## Trust Boundaries

The model may propose a plan. The host validates the plan. The sender decides whether to ask a peer. The receiver decides whether to allow one execution. The executor is host-owned and fixed.

The current implementation does not allow provider-crafted execution requests. Execution requests are built by the host after a matched receiver acknowledgement. The receiver revalidates the binding before execution.

## Logging And Audit

Agent Bridge lifecycle logs use the `[pastey:agent-bridge]` prefix and a single structured redacted JSON object. They may include transition names, shortened references, capability names, provider kind, and bounded error codes.

They must not contain plaintext user content, API keys, raw control payloads, reusable consent tokens, file contents, or model prompts that would reconstruct sensitive state.

Logs are useful for debugging and audit review. They are never the source of truth for authorization, replay protection, queue state, or execution state.

## Current Non-Goals

The current Agent Bridge implementation does not provide:

- arbitrary shell, process, file, or network execution;
- a generic tool runtime;
- multi-step autonomous task graphs;
- a general capability registry;
- reusable trust;
- durable device identity;
- MCP integration;
- local LLM scheduling;
- file/tool capabilities beyond Hello Peer;
- cross-Bridge or cross-device automatic delegation.

Those features require new Layer 4 identity/routing work and new Layer 5 capability contracts before they can be treated as implemented.

## Current Completion Status

Against the canonical Layer 5 definition, Agent Bridge is a narrow capability slice with strong safety boundaries. The implemented scope is mature enough to demonstrate model proposal, host validation, consent, bounded execution, result return, and audit. The full Agent-assisted device workspace remains incomplete until Pastey has broader capabilities, explicit durable peer identity integration where needed, and real multi-step orchestration.

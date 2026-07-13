# Agent Bridge Architecture And Safety

Agent Bridge is the Layer 5 narrow capability path for model-assisted planning, host validation, explicit consent, bounded execution, result return, and redacted audit. For the project-wide layer contract, see [../architecture/Project-specifications.md](../architecture/Project-specifications.md). For template and manifest implementation status, see [capability-templates.md](capability-templates.md). For naming rules covering capability IDs, schema versions, provider actions, executors, and future capabilities, see [../architecture/naming-conventions.md](../architecture/naming-conventions.md). For Bridge membership and authority boundaries, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For routing semantics, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md).

The current product reality is Pastey 1.9.1 with Ask Bridge natural-v1: Search, Search -> Return, and bounded Transform contracts. Ask Bridge is the single natural-language Layer 5 entry. Natural-v1 reduces provider/model output to `Search`, `Transform`, and `Return`; concrete capabilities are implementation details behind those primitives. Only `Transform { transformKind: "selected_artifact_output" }` is supported. Other Transform kinds remain `unsupported_future`. Production has no verified sandbox and returns bounded `sandbox_unavailable` before receiver-host lease acquisition; it does not launch a runtime or fallback process. Hello Stdout / `runtime.hello_stdout` is diagnostic/test-only and no longer user-facing product UI.

## Natural-V1 Safety Model

Provider instructions guide model behavior; they do not enforce safety. The natural-v1 JSON schema is the provider output contract. The host validator, PolicyGate, sender confirmation, receiver Allow once/Deny, second receiver consent for Return, and bounded capability executors are the enforcement and authority boundaries.

Model/provider output can never grant consent, claim that execution already happened, select a candidate by itself, author capability IDs, peer/session bindings, candidate bindings, result contracts, shell/command/arguments/stdin/cwd/env/runtime/compiler/interpreter/network/path/content fields, choose selected-peers or broadcast control routing, or bypass the selected-file second payload consent. Pastey host code constructs `artifact.transform_selected` only after manual selection.

The product timeline and lifecycle logs show Pastey events, not chain-of-thought, reasoning traces, hidden prompts, provider scratchpads, or fake model reasoning.

## Current Flow

1. Bridge detail builds a redacted current-session context snapshot.
2. A provider returns an advisory action plan.
3. The host validates the plan with an allowlist and unsafe-field scan.
4. The local PolicyGate decides whether the plan may enter pending confirmation.
5. The local user explicitly confirms sending a fixed capability preview.
6. The preview becomes a typed capability preview envelope.
7. The envelope is sent through the encrypted selected-peer Bridge control transport. Legacy implementation term: room-control transport.
8. The receiver validates replay, expiry, queue bounds, and PolicyGate rules.
9. The receiver can choose Allow once or Deny.
10. A matched allow-once decision permits exactly one execution request for the same capability, request, peer, and Bridge/session binding.
11. The receiver consumes the consent once, runs the fixed host-owned executor, and returns a typed result.
12. The sender sees a typed product result or queue-handoff status, and both sides write redacted lifecycle audit logs.

For `filesystem.find_file_candidates`, the receiver returns redacted metadata candidates and records a receiver-local, in-memory, TTL-bounded candidate store. For `transfer.request_candidate_payload`, the flow validates and consumes a separate exact Allow once, resolves the selected candidate locally against that store, and queues the resolved local file source through the existing transfer queue. Pastey does not send files automatically after discovery, create a new data plane, or expose real receiver paths to the sender or provider.

The deterministic candidate workflow coordinates those existing capabilities only. It lets a natural-language Ask Bridge intent start with advisory Search planning, but the AI remains advisory, the host remains authoritative, the user must confirm the plan before any peer request is sent, the user must select the candidate, and the receiver must still Allow once for payload Return. Search consent does not authorize payload transfer. `handoff_queued` means accepted into the existing transfer queue, not transfer completion.

## Implemented Production Paths

- Provider and AI types: `src/lib/ai/types.ts`.
- Mock provider: `src/lib/ai/mockProvider.ts`.
- OpenAI-compatible cloud provider: `src/lib/ai/cloudOpenAICompatibleProvider.ts`.
- Static capability registry: `src/lib/ai/capabilityRegistry.ts`.
- Redacted context snapshots: `src/lib/ai/contextSnapshot.ts`.
- Action-plan validation: `src/lib/ai/actionPlanValidator.ts`.
- Local PolicyGate: `src/lib/ai/policyGate.ts`.
- Pending local confirmation: `src/lib/ai/pendingAction.ts`.
- File-candidate advisory and request construction: `src/lib/ai/fileCandidateAdvisory.ts` and `src/lib/ai/fileCandidateRequest.ts`.
- Candidate-payload advisory and request construction: `src/lib/ai/candidatePayloadAdvisory.ts` and `src/lib/ai/candidatePayloadRequest.ts`.
- Hello Peer request construction: `src/lib/ai/helloPeerRequest.ts`.
- Hello Stdout request construction: `src/lib/ai/helloStdoutRequest.ts`.
- Capability preview envelope: `src/lib/ai/capabilityPreviewEnvelope.ts`.
- Static capability manifests and additive template helpers: `src/lib/agentBridge/capabilityManifest.ts` and `src/lib/agentBridge/capabilityTemplateHelpers.ts`.
- Bridge control events: `src/lib/agentBridge/roomControlEvent.ts`. Legacy implementation term: `RoomControlEvent`.
- Control queue state: `src/lib/agentBridge/controlQueue.ts`.
- Receiver consent: `src/lib/agentBridge/peerConsent.ts`.
- Fixed/bounded executors and scaffolds: `src/lib/agentBridge/helloPeerExecution.ts`, `src/lib/agentBridge/helloStdoutExecution.ts`, `src/lib/agentBridge/fileCandidateExecution.ts`, `src/lib/agentBridge/candidatePayloadExecution.ts`, `src-tauri/src/hello_stdout.rs`, and `src-tauri/src/file_candidates.rs`.
- Redacted logging: `src/lib/agentBridge/logging.ts`.
- Bridge-first product UI: `src/pages/BridgeProductPages.tsx`, `src/lib/agentBridge/helloStdoutProductFlow.ts`, and `src/components/OperationTimeline.tsx`.
- Developer/control UI: `src/components/agentBridge/RoomControlPanel.tsx` and `src/components/AiSlotPreview.tsx`.
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
- Capability events must bind to an exact selected peer/session/request. Backend transport resolves that selected peer through the current-session `bridge_peers` row and rejects selected-peers or broadcast routes.
- Durable paired-device metadata, delivery receipts, logs, and Bridge membership do not create capability authority.
- Logs mirror lifecycle but are not queue state, consent state, execution state, or authority.

## Trust Boundaries

The model may propose a plan. The host validates the plan. The sender decides whether to ask a peer. The receiver decides whether to allow one execution. The executor is host-owned and fixed.

The capability registry is a static contract table for known bounded capabilities. It centralizes capability ids, versions, provider action kinds, route/consent policy, schema names, forbidden provider fields, executor kind, audit policy, and UI labels. It does not load plugins, accept provider-supplied capability entries, or dispatch arbitrary runtime names. Static manifests and template helpers are now additive around these explicit entries; they must not become an open-ended tool surface or replace host validation.

The current implementation does not allow provider-crafted execution requests. Execution requests are built by the host after a matched receiver acknowledgement. The receiver revalidates the binding before execution.

`runtime.hello_stdout` is not a shell, process, Python, Node, or general command runtime. It is a single Rust host helper that returns typed stdout metadata for the fixed output `hello peer`. It remains available for diagnostics/tests and template coverage only; it is not exposed as user-facing Ask Bridge product UI. The wrapper uses the static manifest and exact binding helpers without changing its public contract or Rust executor behavior.

`filesystem.find_file_candidates` is not remote file access. It is a bounded metadata candidate-discovery capability. Model output may propose a filename hint and safe limits, but it does not authorize filesystem traversal by itself. Traversal happens only on the receiver after selected-peer routing, local sender confirmation, receiver Allow once, exact consent binding, and execution-request validation. It is now template-wrapped for manifest-backed constants, exact capability/request-hash binding, expiry checks, and forbidden public-field checks. Safe scopes, query bounds, filename and extension filters, depth and candidate limits, hidden-file and symlink behavior, candidate id opacity, receiver-local candidate storage, and Rust command validation remain capability-specific. The executor does not read file contents, return absolute paths, search hidden files, search the whole device, start automatic transfer, or create reusable access to a peer.

`transfer.request_candidate_payload` is not transfer completion. It is a second-consent handoff path for one selected metadata candidate from a prior discovery result. Search consent does not authorize transfer. Payload handoff requires second consent. Model output may propose the opaque candidate id and display metadata, but candidate ids are not paths and not transfer authority. It is now template-wrapped for manifest-backed constants, exact capability/request-hash binding, expiry checks, and forbidden public-field checks. Source discovery binding, receiver-local candidate lookup, changed/deleted/expired candidate handling, queue handoff, queue metadata, result statuses, and scheduler interaction remain capability-specific. The path validates selected-peer routing and exact Allow once, consumes that consent once, resolves the candidate only through the receiver-local in-memory store, and returns `handoff_queued` only after the existing transfer queue accepts the payload source. `handoff_queued` still reports `transferredBytes: 0`; transfer progress and completion remain owned by the existing transfer pipeline.

`artifact.transform_selected` is a separate selected-candidate operation, not payload transfer and not a generic executor. An authenticated received Transform preview is the only source from which Rust may create a pending consent prompt; the receiver UI submits only that prompt ID and `allow_once` or `deny`, and Rust loads every binding field from the pending record before creating the ledger record. No caller-supplied grant can create approval. Rust owns execution-admission consent, the receiver-local lease, the durable post-start operation journal, and completed-result validation/sanitation; TypeScript consent is a validation/UI mirror only. The journal persists only opaque operation/request binding, timestamps, consent-consumed state, and the closed states `reserved`, `revalidated`, `started`, `completed`, `failed`, `timed_out`, `rejected`, or `execution_state_unknown`; it never persists a local path, digest, source bytes, raw output, or sanitation markers. Request hash is correlation data, while Rust directly compares the bound room/session, peers, capability, request, candidate, and result-contract fields. A pre-start failure or abort releases the lease, removes the active reservation, and allows the same still-valid exact consent to retry; a distinct request requires a new confirmation and Allow once. On recovery, unstarted reservations are discarded with their in-memory lease, while started operations become `execution_state_unknown` unless finalized. At receiver-host-private executor start acknowledgement Rust permanently consumes the consent. A future verified sandbox implements Rust's private adapter seam: it acknowledges start, keeps raw output inside Rust, then invokes the private start, sanitation, finalization, result-construction, and transport path. TypeScript receives only bounded authoritative outcomes; it has no raw-result, start, sanitizer, or Transform-result send command. Rust sanitizes, finalizes, constructs, and sends Transform results itself; generic room-control send rejects all caller-created Transform execution results. Before a completed result can become room-control or UI state, Rust enforces 16 KiB UTF-8 byte limits per stream and rejects exact receiver-local path, file-URL, digest, lease, and operation markers. A sanitation rejection records terminal `rejected`, returns no raw output, and cannot replay. Terminal replay returns only the recorded bounded category without re-execution. The current production coordinator uses an unavailable Rust adapter and returns `sandbox_unavailable` before it creates a pending prompt, reservation, operation, lease, identity record, start transition, or result event. A verified sandbox backend remains separate future work.

`candidatePayloadWorkflow` is not automation authority. It is a deterministic host-owned coordinator for the existing discovery and candidate-payload capabilities. It records metadata-safe workflow state, requires explicit user candidate selection, builds only the existing `transfer.request_candidate_payload` preview, and preserves both receiver consent decisions. It does not add a capability id, expose paths or contents, auto-send, implement trusted-session behavior, create a generic executor, or treat provider instructions as enforcement.

## Product Closure Status

The current Bridge detail product UI exposes Ask Bridge natural-v1 as the single Layer 5 entry:

- Search: Ask Bridge runs `filesystem.find_file_candidates` on exactly one selected peer after sender confirmation and receiver Allow once, then shows redacted candidates only.
- Search -> Return: after candidates return, the user manually selects one candidate, then Ask Bridge sends `transfer.request_candidate_payload` through a second receiver Allow once before queue handoff.
- Search -> Transform -> Return: only `selected_artifact_output` is supported; it requests `artifact.transform_selected` after manual selection and returns a bounded typed result. Production rejects it as `sandbox_unavailable` until a verified sandbox exists; other Transform kinds remain `unsupported_future`.
- Hello Stdout / `runtime.hello_stdout`: diagnostic/test-only fixed host runtime coverage, not user-facing product UI.

The shared operation timeline shown in Bridge detail visualizes Pastey lifecycle events only. It does not display model chain-of-thought, hidden prompts, provider scratchpads, raw internal prompts, reasoning traces, or fake reasoning.

Pastey 1.9.1 also consolidates the product smoke fixes around those flows: Ask Bridge uses the canonical room-control selected peer ref for embedded requests and preview envelopes, Deny is a terminal lifecycle state, active Bridge detail operations auto-refresh while `Check for updates` remains a fallback, remote platform labels are display metadata only, and long sent/received text remains fully accessible through detail or copy actions. Preview truncation is UI-only.

## Workspace Capability Roadmap

The current Layer 5 direction is Agent-assisted device workspace: helping a user ask a selected peer for bounded help, such as finding candidate files, without turning Pastey into a remote shell or file browser.

The first workspace capability is `filesystem.find_file_candidates`. Its current flow is:

1. Provider proposes `request_peer_file_candidates` as advisory JSON only.
2. Host validation rejects unsafe fields, whole-device search, contents, absolute paths, selected-peers/broadcast, and auto-transfer.
3. Local user confirms whether to send a selected-peer preview.
4. Receiver explicitly allows once or denies.
5. Receiver-side search returns bounded redacted metadata candidates only.
6. Receiver records a TTL-bounded in-memory candidate store for the returned candidates.
7. A selected candidate payload request requires a second capability and separate consent decision.
8. The current candidate-payload path resolves the selected candidate locally and queues it through the existing transfer scheduler.

Current implementation covers steps 1 through 8 and adds a deterministic workflow that can coordinate those steps after a user manually selects a candidate. It implements receiver-local candidate storage, metadata-only resolution, and queue handoff into the existing transfer pipeline. It does not implement auto-send after discovery, trusted-session runtime behavior, a new data plane, a universal executor, or broad natural-language automation.

## Logging And Audit

Agent Bridge lifecycle logs use the `[pastey:agent-bridge]` prefix and a single structured redacted JSON object. They may include transition names, shortened references, capability names, provider kind, and bounded error codes.

They must not contain plaintext user content, API keys, raw control payloads, reusable consent tokens, file contents, or model prompts that would reconstruct sensitive state.

Logs are useful for debugging and audit review. They are never the source of truth for authorization, replay protection, queue state, or execution state.

## Current Non-Goals

The current Agent Bridge implementation does not provide:

- arbitrary shell, process, file, or network execution;
- an open-ended tool runtime;
- multi-step autonomous task graphs;
- model/provider consent or execution claims;
- a model judge or sub-agent that can approve execution;
- dynamic capability/plugin registration;
- reusable trust;
- durable device identity as trust, routeability, consent, auto-join, or execution authority;
- MCP integration;
- local LLM scheduling;
- executable file/tool capabilities beyond the fixed Hello Peer, Hello Stdout, bounded file-candidate metadata search path, and candidate-payload queue handoff path;
- automatic file transfer after discovery;
- cross-Bridge or cross-device automatic delegation.

Those features require new authority-bearing Layer 4 identity/routing semantics and new Layer 5 capability contracts before they can be treated as implemented. The current durable paired-device runtime is display/recognition metadata only.

## Current Completion Status

Against the canonical Layer 5 definition, Agent Bridge is a narrow capability slice with strong safety boundaries. The implemented scope is mature enough to demonstrate model proposal, host validation, consent, bounded execution, metadata-only workspace discovery, typed result return, second-consent queue handoff, and audit through Bridge-first product UI. The full Agent-assisted device workspace remains incomplete until Pastey has broader two-device smoke validation, transfer-completion smoke for the handoff path, broader capabilities, explicit durable peer identity integration where needed, global Activity detail surfaces, and real multi-step orchestration.

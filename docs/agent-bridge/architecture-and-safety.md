# Agent Bridge Architecture And Safety

Agent Bridge is the Layer 5 narrow capability path for model-assisted planning, host validation, explicit consent, bounded execution, result return, and redacted audit. For the project-wide layer contract, see [../architecture/Project-specifications.md](../architecture/Project-specifications.md). For Bridge membership and authority boundaries, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For routing semantics, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md).

The current product reality is a narrow bounded-capability vertical slice plus the first read-only workspace capability. The Hello capabilities prove fixed execution; the file-candidate capability proves receiver-consented metadata discovery without file transfer authority.

## Current Flow

1. The Bridge UI builds a redacted current-session context snapshot. Legacy implementation term: Room UI.
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
12. Both sides write redacted lifecycle audit logs.

For `filesystem.find_file_candidates/v1`, the flow stops after returning redacted metadata candidates. Pastey does not hand off a selected candidate to transfer, send files automatically, or expose real receiver paths to the sender or provider.

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
- Hello Peer request construction: `src/lib/ai/helloPeerRequest.ts`.
- Hello Stdout request construction: `src/lib/ai/helloStdoutRequest.ts`.
- Capability preview envelope: `src/lib/ai/capabilityPreviewEnvelope.ts`.
- Bridge control events: `src/lib/agentBridge/roomControlEvent.ts`. Legacy implementation term: `RoomControlEvent`.
- Control queue state: `src/lib/agentBridge/controlQueue.ts`.
- Receiver consent: `src/lib/agentBridge/peerConsent.ts`.
- Fixed/bounded executors: `src/lib/agentBridge/helloPeerExecution.ts`, `src/lib/agentBridge/helloStdoutExecution.ts`, `src/lib/agentBridge/fileCandidateExecution.ts`, `src-tauri/src/hello_stdout.rs`, and `src-tauri/src/file_candidates.rs`.
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
- Capability events must bind to an exact selected peer/session/request. Backend transport resolves that selected peer through the current-session `bridge_peers` row and rejects selected-peers or broadcast routes.
- Durable paired-device metadata, delivery receipts, logs, and Bridge membership do not create capability authority.
- Logs mirror lifecycle but are not queue state, consent state, execution state, or authority.

## Trust Boundaries

The model may propose a plan. The host validates the plan. The sender decides whether to ask a peer. The receiver decides whether to allow one execution. The executor is host-owned and fixed.

The capability registry is a static contract table for known bounded capabilities. It centralizes capability ids, versions, provider action kinds, route/consent policy, schema names, forbidden provider fields, executor kind, audit policy, and UI labels. It does not load plugins, accept provider-defined capability entries, or dispatch arbitrary runtime names.

The current implementation does not allow provider-crafted execution requests. Execution requests are built by the host after a matched receiver acknowledgement. The receiver revalidates the binding before execution.

`runtime.hello_stdout/v1` is not a shell, process, Python, Node, or generic command runtime. It is a single Rust host helper that returns typed stdout metadata for the fixed output `hello peer`.

`filesystem.find_file_candidates/v1` is not remote file access. It is a bounded metadata candidate-discovery capability. Model output may propose a filename hint and safe limits, but it does not authorize filesystem traversal by itself. Traversal happens only on the receiver after selected-peer routing, local sender confirmation, receiver Allow once, exact consent binding, and execution-request validation. The executor does not read file contents, return absolute paths, search hidden files, search full disk, start automatic transfer, or create reusable access to a peer.

## Workspace Capability Roadmap

The next Layer 5 direction is Agent-assisted device workspace: helping a user ask a selected peer for bounded help, such as finding candidate files, without turning Pastey into a remote shell or file browser.

The first workspace capability is `filesystem.find_file_candidates/v1`. Its current flow is:

1. Provider proposes `request_peer_file_candidates` as advisory JSON only.
2. Host validation rejects unsafe fields, full-disk search, contents, absolute paths, selected-peers/broadcast, and auto-transfer.
3. Local user confirms whether to send a selected-peer preview.
4. Receiver explicitly allows once or denies.
5. Receiver-side search returns bounded redacted metadata candidates only.
6. Any file transfer remains a separate future capability and separate consent decision.

Current implementation covers steps 1 through 5. It does not implement candidate selection or approved transfer handoff.

## Logging And Audit

Agent Bridge lifecycle logs use the `[pastey:agent-bridge]` prefix and a single structured redacted JSON object. They may include transition names, shortened references, capability names, provider kind, and bounded error codes.

They must not contain plaintext user content, API keys, raw control payloads, reusable consent tokens, file contents, or model prompts that would reconstruct sensitive state.

Logs are useful for debugging and audit review. They are never the source of truth for authorization, replay protection, queue state, or execution state.

## Current Non-Goals

The current Agent Bridge implementation does not provide:

- arbitrary shell, process, file, or network execution;
- a generic tool runtime;
- multi-step autonomous task graphs;
- dynamic capability/plugin registration;
- reusable trust;
- durable device identity as trust, routeability, consent, auto-join, or execution authority;
- MCP integration;
- local LLM scheduling;
- executable file/tool capabilities beyond the fixed Hello Peer, Hello Stdout, and bounded file-candidate metadata search paths;
- automatic file transfer handoff;
- cross-Bridge or cross-device automatic delegation.

Those features require new authority-bearing Layer 4 identity/routing semantics and new Layer 5 capability contracts before they can be treated as implemented. The current durable paired-device runtime is display/recognition metadata only.

## Current Completion Status

Against the canonical Layer 5 definition, Agent Bridge is a narrow capability slice with strong safety boundaries. The implemented scope is mature enough to demonstrate model proposal, host validation, consent, bounded execution, metadata-only workspace discovery, result return, and audit. The full Agent-assisted device workspace remains incomplete until Pastey has approved transfer handoff, broader capabilities, explicit durable peer identity integration where needed, and real multi-step orchestration.

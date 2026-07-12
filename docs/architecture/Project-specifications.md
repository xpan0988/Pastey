# Pastey Project Layout Specification / 项目布局规范

This document is the canonical definition of Pastey's project layout and layer boundaries. When older planning documents or diagrams conflict with it, this document takes precedence.

It is both the architecture contract and the completion-scoring standard for Pastey. Source code is the primary evidence. Tests, automated harnesses, current documentation, runtime paths, and recorded manual smoke results are supporting evidence.

For architecture, schema, protocol, Agent Bridge capability, provider action, executor, template kind, and future capability naming rules, see [naming-conventions.md](naming-conventions.md). For Agent Bridge capability templates, manifests, autonomy profiles, approval policy, migration phases, and implementation status, see [../agent-bridge/capability-templates.md](../agent-bridge/capability-templates.md).

Pastey 1.9.1 implements bounded natural-v1 Transform contracts alongside the fixed Search and Search -> Return product paths. Natural-v1 reduces provider/model output to `Search`, `Transform`, and `Return`. Search and Search -> Return use `filesystem.find_file_candidates` plus second-consent `transfer.request_candidate_payload` queue handoff. The sole supported Transform is `selected_artifact_output`, mapped by host code to `artifact.transform_selected` after manual selection. Rust derives a pending consent prompt only from an authenticated received preview, owns the exact Transform ledger/journal/result sanitizer, and alone constructs and sends Transform results; TypeScript is a UI/validation mirror. Production execution remains unavailable: its Transform coordinator returns `sandbox_unavailable` before prompt creation, operation creation, candidate mutation, or start acknowledgement and has no direct-process fallback. A future verified sandbox must use receiver-local lease identity revalidation and the existing Rust journal/finalizer; result sanitation already enforces bounded output and receiver-private marker rejection. Other Transform kinds remain `unsupported_future`. `runtime.hello_stdout` is diagnostic/test-only.

Version suffixes such as `v1` are reserved for substantial product/module or protocol milestones, such as `binary-v1` and natural-v1. Do not add `v1` suffixes to helper modules, provider wrappers, provider instruction packs, safety cleanups, documentation notes, or implementation details.

## Purpose And Authority

Use this document to decide:

- which layer owns a feature;
- which layer must not silently absorb a responsibility;
- what counts as completion for each layer;
- where subsystem documentation belongs;
- how future architecture reports must be scored.

Non-canonical documents should link here instead of restating the five-layer model. If a subsystem document needs a local summary, keep it short and preserve the boundaries below.

## Boundary Invariants

- Device facts != scheduler command.
- Encrypted session != durable device identity.
- Accepted Bridge peer != durable trusted device.
- Bridge membership != execution authority.
- Transport delivery != consent.
- Consent != reusable trust.
- Broadcast routing != control or execution authority.
- Model output != executable instruction.
- Logs != runtime state or authorization.
- Bridge semantics do not escape the Bridge session.

These are product constraints, not style preferences. Ambiguity should fail closed.

## Five-Layer Definitions

| Layer | Canonical definition |
| --- | --- |
| Layer 1 - Secure LAN transport | Moves data securely and reliably over the LAN. |
| Layer 2 - Device intelligence | Observes and describes current-session device, capability, liveness, local endpoint/provider availability, and benchmark facts. It does not rank peers, recommend devices, or produce scheduler preferences. |
| Layer 3 - Smart orchestration | Plans and schedules data/control work, allocates runtime capacity, manages MicroFlowGroup behavior, and performs hot runtime-window adjustment. |
| Layer 4 - Multi-device Bridge sessions and peer identity | Owns current-session Bridge membership, selected-peer routing, selected-peers routing, broadcast routing, current-session provenance, replay boundaries, reconnect semantics, and current-session control-plane delivery. Future optional durable identity is a separate concern. Bridge membership never equals execution authority or reusable trust. |
| Layer 5 - Agent-assisted device workspace | Owns model-assisted planning, host validation, PolicyGate, explicit consent, bounded capability execution, result orchestration, and audit. The model proposes; the host validates; the user authorizes; a bounded executor acts. |

## Layer Responsibilities

### Layer 1 - Secure LAN Transport

Responsibilities:

- UDP LAN discovery and nearby join.
- Bridge session creation, join, leave, burn, and startup recovery plumbing. Legacy implementation term: room.
- Encrypted text/file/image movement over Bridge session endpoints.
- Binary-v1 chunk framing, acknowledgement, finalize, cancellation, and receiver integrity checks.
- Payload encryption, session/control key wrapping, and transfer error handling.
- High-throughput sender windows and runtime window updates exposed to Layer 3.

Non-responsibilities:

- Durable authenticated device identity.
- Execution authority.
- Long-term trust decisions.
- Model planning, consent, or capability semantics.

Completion criteria:

- Reliable encrypted transfer across supported desktop platforms.
- Corruption and malformed frame rejection.
- Clear lifecycle recovery for cancel, burn, disconnect, and app restart.
- Documented protocol limits and release-build validation.
- Security review and broad two-device validation before any `100%` claim.

### Layer 2 - Device Intelligence

Responsibilities:

- Observe the current device and current-session link conditions.
- Describe capability, liveness, endpoint availability, and benchmark facts in an explainable way.
- Keep device intelligence local-first and current-session unless a future durable profile is explicitly designed.

Non-responsibilities:

- Directly commanding the scheduler.
- Producing planner hints, scheduler preferences, peer rankings, best-device suggestions, or user coaching.
- Granting trust, authority, or execution permission.
- Hidden long-term profiling.
- Replacing Layer 3 planning decisions.

Completion criteria:

- `DeviceProfile`, `DeviceCapabilities`, and `LinkBenchmark` facts exist in production runtime.
- Current-session observations are visible or explainable to the user.
- Layer 2 data remains factual and is not consumed as a scheduler command.

### Layer 3 - Smart Orchestration

Responsibilities:

- Plan queued file-like transfers and active transfer window allocations.
- Schedule runnable work fairly within current capacity.
- Manage fixed and dynamic MicroFlowGroup behavior.
- Reserve runtime capacity for control work and perform hot window adjustment.
- Provide transfer/control orchestration diagnostics and validation harnesses.

Non-responsibilities:

- Durable peer or Bridge identity.
- Peer trust or execution authorization.
- Model output validation.
- Transport cryptography.

Completion criteria:

- The production scheduler and planner enforce capacity bounds.
- Runtime-window changes affect real Rust binary-v1 sender paths.
- Control-lane demand and transfer contention are validated by automated harnesses.
- MicroFlowGroup lifecycle and accounting are tested.
- Broader adaptive policies are scored only when implemented, not inferred from plans.

### Layer 4 - Multi-Device Bridge Sessions And Peer Identity

Responsibilities:

- Own current-session Bridge membership, accepted-peer status, session verification, and future optional durable identity boundaries as a separate concern.
- Own selected-peer routing, selected-peers routing, and broadcast routing semantics.
- Route Bridge items and Bridge control events to the correct current-session accepted peer or peers.
- Define current-session provenance, replay boundaries, reconnect, leave, burn, disconnect, and control-plane semantics.
- Keep accepted/session-verified peers separate from durable trusted devices.
- Keep Bridge membership separate from execution authority.

Non-responsibilities:

- Running capabilities.
- Reusing consent as trust.
- Treating encrypted payload receipt as identity proof.
- Treating logs as Bridge state.
- Treating Bridge items or control events as durable history by default.

Completion criteria:

- Bridge/session binding and event provenance are explicit and test-covered.
- Replay and routing boundaries exist for data and control events, including per-peer outcomes where selected-peers or broadcast routing exists.
- Durable identity continuity, if introduced, is an explicit system separate from current Bridge acceptance.
- Reconnect semantics are implemented before full-layer completion.
- Multi-peer behavior is validated beyond the current two-peer/session-scoped foundation.

### Layer 5 - Agent-Assisted Device Workspace

Responsibilities:

- Model-assisted planning through a bounded provider abstraction.
- Redacted context construction and host-side action-plan validation.
- Deny-first PolicyGate behavior.
- Explicit local and receiver-side consent.
- Bounded capability execution and typed result return.
- Read-only workspace capabilities that preserve advisory model output, explicit receiver consent, and bounded host-owned execution.
- Deterministic capability workflows that chain existing capabilities only after host validation and explicit user decisions.
- Future reusable capability templates and manifests that reduce duplicated lifecycle code while preserving explicit capability validators, route binding, consent binding, scope limits, and redacted result contracts.
- Ask Bridge natural-v1 product closure that exposes Search, Search -> Return, and the single host-built selected-artifact Transform contract without adding a growing task taxonomy.
- Natural-v1 provider instruction pack and static risk scanner support, with provider instructions treated as guidance and validator/consent/executor boundaries treated as enforcement.
- Audit logging that mirrors lifecycle without becoming state or authority.

Non-responsibilities:

- Letting a model directly execute instructions.
- Treating provider output as trusted code.
- Open-ended shell/process/file/network execution without a new capability contract.
- Auto-selecting candidates or auto-sending files from model output.
- Treating provider instructions, docs, Markdown, or a model judge as execution authority.
- Claiming multi-provider adapter support before adapter-specific request envelopes and shared natural-v1 validation tests exist.
- Reusable trust from one consent decision.
- Durable peer identity; that belongs to Layer 4.

Completion criteria:

- Provider support, redaction, validation, policy, consent, execution, result, and audit are all wired through a real Bridge control path. Legacy implementation term: room-control path.
- Each capability has a fixed schema, host-owned executor, explicit consent binding, replay protection, and tests.
- Advisory-only workspace capability scaffolds may exist before execution. They do not count as implemented receiver capabilities until preview, receiver consent, execution/result, and validation are complete.
- Pastey 1.9.1 implements Layer 5 Search and Search -> Return product paths through Ask Bridge natural-v1. Selected-artifact Transform has bounded `artifact.transform_selected` consent, lease, and result contracts, but production execution returns `sandbox_unavailable` until a verified sandbox exists. Request file remains a Search / Return plan. Hello Stdout / `runtime.hello_stdout` remains diagnostic/test-only.
- Broader workspace completion requires more than the fixed Hello Peer/Hello Stdout slices, minimal static registry, file-candidate metadata search, candidate-payload queue handoff, and Bridge-first product panels: broad capability coverage, multi-step orchestration, local model scheduling or equivalent local-provider story, release/two-device validation, global Activity detail surfaces, and explicit durable peer identity integration if a capability depends on durable trust.

## Inter-Layer Dependencies

| From | Depends on | Rule |
| --- | --- | --- |
| Layer 2 | Layer 1 | Benchmarks and peer observations may use transport paths, but Layer 2 outputs remain factual. |
| Layer 3 | Layer 1 | Scheduler capacity maps onto real binary-v1 runtime sender windows. |
| Layer 3 | Layer 2 | Layer 3 may observe factual Layer 2 measurements, but scheduler policy is owned by Layer 3 and does not obey Layer 2 recommendations. |
| Layer 4 | Layer 1 | Bridge sessions, route delivery, and control events require encrypted session transport but must not call that durable identity. |
| Layer 5 | Layer 4 | Capability transport and consent are Bridge-scoped and must bind to an exact selected peer/session/request. Accepted Bridge membership alone never authorizes execution. |
| Layer 5 | Layer 3 | Agent/control work may reserve runtime capacity; execution authority still comes from Layer 5 consent. |

## Current Repository Implementation Status

| Layer | Production runtime evidence | Test / harness evidence | Manual or documented evidence | Implemented category |
| --- | --- | --- | --- | --- |
| Layer 1 | `src-tauri/src/discovery.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/storage.rs`, `src-tauri/src/crypto.rs`, `src-tauri/src/transfer.rs`, `src-tauri/src/chunk_frame.rs` | Rust transport, storage, discovery, binary frame, encryption, finalize, runtime-window tests | `docs/transfer/validation.md` records same-machine smoke boundaries and two-machine requirements | Production runtime |
| Layer 2 | `src-tauri/src/diagnostics.rs`, `src-tauri/src/device_profile.rs`, `src-tauri/src/capability_probe.rs`, `src-tauri/src/link_benchmark.rs`, `src/pages/SettingsPage.tsx` | Device profile, capability probe, diagnostics DTO, and benchmark tests | Developer Tools current-session diagnostics | Production runtime, Developer Tools visible |
| Layer 3 | `src/lib/transferPlanner.ts`, `src/lib/transferScheduler.ts`, `src/App.tsx`, `src/lib/agentBridge/controlWindowRuntime.ts`, `src-tauri/src/transfer.rs` | Planner/scheduler/MicroFlowGroup tests plus `scripts/run-cl4-contention-smoke.mjs` | Transfer validation guide and logged smoke interpretation | Production runtime with automated harness |
| Layer 4 | `src-tauri/src/storage.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/room_control.rs`, `src/lib/bridgeRoutingRuntime.ts`, `src/lib/agentBridge/roomControlEvent.ts`, `src/lib/agentBridge/controlQueue.ts` | Bridge route, storage, room-control Rust tests, routing/runtime TypeScript tests, control-event validation tests, `scripts/run-layer4-validation-matrix.mjs` | Session-scoped Bridge routing/control flow documented in Bridge, Agent Bridge, and transfer docs. Legacy implementation term: room. | Production runtime with automated matrix; manual/release smoke pending |
| Layer 5 | `src/lib/ai/*`, `src/lib/agentBridge/capabilityManifest.ts`, `src/lib/agentBridge/capabilityTemplateHelpers.ts`, `src/lib/agentBridge/peerConsent.ts`, `src/lib/agentBridge/helloStdoutProductFlow.ts`, `src/lib/agentBridge/helloPeerExecution.ts`, `src/lib/agentBridge/helloStdoutExecution.ts`, `src/lib/agentBridge/fileCandidateExecution.ts`, `src/lib/agentBridge/candidatePayloadExecution.ts`, `src/lib/agentBridge/candidatePayloadWorkflow.ts`, `src-tauri/src/hello_stdout.rs`, `src-tauri/src/file_candidates.rs`, `src/lib/agentBridge/logging.ts`, `src/pages/BridgeProductPages.tsx`, `src/components/OperationTimeline.tsx`, `src/components/agentBridge/RoomControlPanel.tsx`, `src/lib/transferScheduler.ts` | Provider, natural-v1 plan validation, context, static registry, static manifests, template helper checks, validator, PolicyGate, preview, consent, execution, deterministic workflow tests, product source-shape tests, logging, room-control tests, Hello diagnostic tests, file-candidate advisory/executor tests, candidate-payload consent/handoff tests, and transfer scheduler contention tests | Bridge-first Ask Bridge natural-language UI for Search / Return, developer/control UI, diagnostic/test-only Hello Stdout, and redacted lifecycle logs. Manual dual-device smoke remains pending. Legacy implementation term: room-scoped. | Production runtime, narrow product closure with static registry, template-wrapped common checks, Search + Return through metadata discovery and second-consent queue handoff |

## Current Completion Assessment

These scores are against the canonical definitions in this document, not against outdated planning labels. `Canonical completion` measures the whole intended layer. `Implemented-scope maturity` measures how complete and validated the currently shipped scope is.

| Layer | Canonical completion | Implemented-scope maturity | Status label | Confidence | Main remaining gaps |
| --- | ---: | ---: | --- | --- | --- |
| Layer 1 - Secure LAN transport | 86% | 90% | Mature operational core | High | Durable identity, broader release/two-device validation, independent security review, whole-file hash contract |
| Layer 2 - Device intelligence | 74% | 84% | Factual diagnostics and capability facts implemented | High | Broader device matrix, peer benchmark UI, local provider availability facts if surfaced later |
| Layer 3 - Smart orchestration | 84% | 88% | Operational orchestration core | High | Deficit/history-aware adaptation, broader end-to-end contention validation, future capability-routing policy |
| Layer 4 - Multi-device Bridge sessions and peer identity | 72% | 86% | Session-scoped Bridge routing/control core | High | Full cryptographic paired-key rotation, durable route recovery if explicitly designed, broader release/two-device validation, independent security review |
| Layer 5 - Agent-assisted device workspace | 68% | 88% | Ask Bridge natural-v1 product paths for Search and Search -> Return; bounded Transform contracts with unavailable production execution | High | Manual/two-device validation, transfer-completion smoke, verified sandbox backend, global Activity detail drawer, broad capability coverage, multi-step orchestration, local LLM scheduling, MCP/tool integration, durable peer-identity dependency if needed |

No layer is `100%`. The largest gap between full vision and implementation remains Layer 5: Pastey 1.9.1 proves Ask Bridge natural-v1 Search / Return through metadata-only discovery plus second-consent candidate-payload handoff. Search consent does not authorize payload transfer, and `handoff_queued` remains queue acceptance rather than transfer completion. Pastey still lacks broad manual/two-device validation, a global Activity detail drawer, bounded transform runtime, broad capability coverage, durable identity/trust integration where explicitly needed, and multi-step workspace orchestration.

## Completion Scoring Scale

| Score | Meaning |
| --- | --- |
| `0%` | Not started |
| `1-25%` | Design/research only |
| `26-50%` | Partial foundation or local simulation |
| `51-75%` | Working implementation with important missing production boundaries |
| `76-90%` | Operational core with limited scope or incomplete validation |
| `91-99%` | Functionally complete but missing release hardening or broad real-world validation |
| `100%` | Production-complete against the canonical layer definition |

Future completion reports must cite production code, tests, scripts/harnesses, current documentation, real runtime paths, and documented manual smoke results. They must classify evidence as production runtime, Developer Tools only, local simulation, automated harness only, documentation/design only, or stale/deprecated. Do not give `100%` without release hardening, broad real-world validation, and the full layer scope.

## Canonical Terminology

| Term | Meaning |
| --- | --- |
| Bridge | Product-level term for an ephemeral encrypted LAN session that routes transfers and control events between accepted peers. It is current-session scoped by default and is not chat, durable history, reusable trust, durable device identity, execution authority, or a long-term group. |
| Room | Legacy implementation term for Bridge. Current code/storage/tests may still use Room naming during migration. |
| Accepted Bridge peer | Peer accepted into the current Bridge session through nearby accept, 8-digit code join, or equivalent current-session join path. Acceptance is not reusable trust or execution authority. |
| Session-verified peer | Peer verified for encrypted current-session delivery inside the current Bridge session. Session verification is not durable peer identity. |
| Durable trusted device | Reserved future term for an explicit durable identity/trust system. Do not use it for current Bridge membership. |
| Bridge item | Text/file/image transfer payload metadata and lifecycle state inside a Bridge. It is not durable history and not a control event. Legacy implementation term: Room item. |
| Bridge control event | Typed bounded control-plane event for Agent Bridge preview, status, execution request, and result. It is not ordinary Bridge text, not a Bridge item, and not a durable workflow record. Legacy implementation term: RoomControlEvent. |
| BridgeTarget | Target expression for one send operation: selected peer, selected peers, or broadcast to Bridge. It is current-session only. |
| BridgeRoute | Resolved current-session delivery route bound to a Bridge session and accepted-peer set at send time. It is not durable history or reusable trust evidence. |
| BridgePeerSelection | UI or caller selection intent before route resolution. It must be revalidated against current-session membership. |
| BridgeBroadcastPolicy | Policy deciding whether a content/event kind may broadcast and what explicit disclosure is required. Control/capability broadcast requires separate design and validation. |
| Control lane | Scheduler/runtime reservation behavior for local outgoing Bridge control demand. It reserves capacity; it does not grant authority. Legacy implementation term: room-control demand. |
| Capability | A fixed host-owned operation with schema, preview, consent, executor, result, and audit requirements. |
| PolicyGate | Deny-first host/receiver decision point that validates whether a capability request may proceed to user consent. |
| Execution consent | One explicit user authorization for one exact bounded capability execution request. Consent is consumed and is not reusable trust. |
| Audit log | Redacted lifecycle mirror for debugging and review. It is not runtime state, authorization, or reconstructable payload data. |
| Selected-peer routing | Current-session routing mode that sends one ordinary data item or one control event to one explicitly selected accepted peer. |
| Selected-peers routing | Current-session routing mode that sends ordinary data to an explicit selected subset of accepted peers. Control/capability selected-peers remains intentionally unsupported. |
| Broadcast routing | Current-session routing mode that sends ordinary data to all currently routeable accepted peers at send/enqueue time. Control/capability broadcast remains intentionally unsupported. |

## Documentation Ownership Map

| Topic | Canonical document |
| --- | --- |
| Five-layer definitions, layer boundaries, scoring | `docs/architecture/Project-specifications.md` |
| Bridge semantics, peer terminology, lifecycle, authority boundary | `docs/architecture/bridge-semantics.md` |
| Bridge routing model and Layer 4 runtime status | `docs/architecture/bridge-routing.md` |
| Secure transport architecture | `docs/transfer/architecture.md` |
| Scheduler, runtime windows, MicroFlowGroup | `docs/transfer/scheduler.md` |
| Transfer validation and harnesses | `docs/transfer/validation.md` |
| Dynamic MicroFlowGroup capacity research reference | `docs/transfer/dynamic-microflowgroup-window-capacity-design.pdf` |
| Agent Bridge architecture and safety | `docs/agent-bridge/architecture-and-safety.md` |
| Bridge control transport | `docs/agent-bridge/room-control-transport.md` |
| Capability consent and execution contracts | `docs/agent-bridge/capability-contracts.md` |
| Provider configuration | `docs/agent-bridge/provider-configuration.md` |
| Release operations | `docs/operations/release-workflow.md` |

Duplicated definitions in non-canonical documents should be replaced with links to the owning document.

## Classifying Future Features

Use the highest layer that owns the product responsibility:

- New frame formats, encryption, discovery, transfer recovery, or sender window primitives belong to Layer 1.
- New current-session observations, benchmarks, capability summaries, liveness facts, or endpoint/provider availability facts belong to Layer 2.
- New queue, capacity, fairness, MicroFlowGroup, or runtime-window policies belong to Layer 3.
- New current-session accepted-peer collections, BridgeTarget/BridgeRoute behavior, selected-peer routing, selected-peers routing, broadcast routing, current-session provenance, replay boundaries, reconnect, explicit export, or cross-session record semantics belong to Layer 4. Future durable peer identity also belongs to Layer 4, but only as a separate explicit durable identity system.
- New model planning, validation, PolicyGate, consent, capability execution, result orchestration, or audit behavior belongs to Layer 5.

When a feature crosses layers, document both the owner and the dependency. For example, a future "choose best peer for execution" feature would need Layer 2 factual observations, Layer 3 planning policy, Layer 4 identity/routing, and Layer 5 consent/execution, but Layer 2 would still not produce the recommendation and Layer 5 would still own execution authority.

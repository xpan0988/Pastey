# Pastey Project Layout Specification / 项目布局规范

This document is the canonical definition of Pastey's project layout and layer boundaries. When older planning documents or diagrams conflict with it, this document takes precedence.

It is both the architecture contract and the completion-scoring standard for Pastey. Source code is the primary evidence. Tests, automated harnesses, current documentation, runtime paths, and recorded manual smoke results are supporting evidence.

## Purpose And Authority

Use this document to decide:

- which layer owns a feature;
- which layer must not silently absorb a responsibility;
- what counts as completion for each layer;
- where subsystem documentation belongs;
- how future architecture reports must be scored.

Non-canonical documents should link here instead of restating the five-layer model. If a subsystem document needs a local summary, keep it short and preserve the boundaries below.

## Boundary Invariants

- Device recommendation != scheduler command.
- Encrypted session != durable device identity.
- Trusted room != execution authority.
- Transport delivery != consent.
- Consent != reusable trust.
- Model output != executable instruction.
- Logs != runtime state or authorization.

These are product constraints, not style preferences. Ambiguity should fail closed.

## Five-Layer Definitions

| Layer | Canonical definition |
| --- | --- |
| Layer 1 - Secure LAN transport | Moves data securely and reliably over the LAN. |
| Layer 2 - Device intelligence | Observes, describes, and recommends based on current-session device and link conditions. It is advisory-first and is not required to take automatic control of Layer 3. |
| Layer 3 - Smart orchestration | Plans and schedules data/control work, allocates runtime capacity, manages MicroFlowGroup behavior, and performs hot runtime-window adjustment. |
| Layer 4 - Multi-device trusted rooms | Owns device/room relationships, membership, durable identity continuity, provenance, routing, replay boundaries, reconnect semantics, and the trusted control plane. Trusted room membership never equals execution authority. |
| Layer 5 - Agent-assisted device workspace | Owns model-assisted planning, host validation, PolicyGate, explicit consent, bounded capability execution, result orchestration, and audit. The model proposes; the host validates; the user authorizes; a bounded executor acts. |

## Layer Responsibilities

### Layer 1 - Secure LAN Transport

Responsibilities:

- UDP LAN discovery and nearby join.
- Room creation, join, leave, burn, and startup recovery plumbing.
- Encrypted text/file/image movement over room endpoints.
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
- Describe capability and benchmark facts in an explainable way.
- Recommend advisory roles or planning hints.
- Keep device intelligence local-first and current-session unless a future durable profile is explicitly designed.

Non-responsibilities:

- Directly commanding the scheduler.
- Granting trust, authority, or execution permission.
- Hidden long-term profiling.
- Replacing Layer 3 planning decisions.

Completion criteria:

- `DeviceProfile`, `DeviceCapabilities`, `LinkBenchmark`, and recommended-role hints exist in production runtime.
- Current-session observations are visible or explainable to the user.
- Recommendations are advisory and do not imply authority.
- If Layer 3 consumes hints later, that consumption is explicit, bounded, and test-covered.

### Layer 3 - Smart Orchestration

Responsibilities:

- Plan queued file-like transfers and active transfer window allocations.
- Schedule runnable work fairly within current capacity.
- Manage fixed and dynamic MicroFlowGroup behavior.
- Reserve runtime capacity for control work and perform hot window adjustment.
- Provide transfer/control orchestration diagnostics and validation harnesses.

Non-responsibilities:

- Durable room identity.
- Peer trust or execution authorization.
- Model output validation.
- Transport cryptography.

Completion criteria:

- The production scheduler and planner enforce capacity bounds.
- Runtime-window changes affect real Rust binary-v1 sender paths.
- Control-lane demand and transfer contention are validated by automated harnesses.
- MicroFlowGroup lifecycle and accounting are tested.
- Broader adaptive policies are scored only when implemented, not inferred from plans.

### Layer 4 - Multi-Device Trusted Rooms

Responsibilities:

- Own room/device relationships, membership, and current or durable identity continuity.
- Route room items and room-control events to the correct peer/session.
- Define replay, provenance, reconnect, and trusted-control-plane semantics.
- Keep trust separate from execution authority.

Non-responsibilities:

- Running capabilities.
- Reusing consent as trust.
- Treating encrypted payload receipt as identity proof.
- Treating logs as room state.

Completion criteria:

- Room/session binding and event provenance are explicit and test-covered.
- Replay and routing boundaries exist for data and control events.
- Durable identity continuity and reconnect semantics are implemented before full-layer completion.
- Multi-peer behavior is validated beyond the current two-peer/session-scoped foundation.

### Layer 5 - Agent-Assisted Device Workspace

Responsibilities:

- Model-assisted planning through a bounded provider abstraction.
- Redacted context construction and host-side action-plan validation.
- Deny-first PolicyGate behavior.
- Explicit local and receiver-side consent.
- Bounded capability execution and typed result return.
- Audit logging that mirrors lifecycle without becoming state or authority.

Non-responsibilities:

- Letting a model directly execute instructions.
- Treating provider output as trusted code.
- Generic shell/process/file/network execution without a new capability contract.
- Reusable trust from one consent decision.
- Durable identity; that belongs to Layer 4.

Completion criteria:

- Provider support, redaction, validation, policy, consent, execution, result, and audit are all wired through a real room-control path.
- Each capability has a fixed schema, host-owned executor, explicit consent binding, replay protection, and tests.
- Broader workspace completion requires more than the fixed Hello Peer slice: capability registry, multiple capabilities, multi-step/task orchestration, local model scheduling or equivalent local-provider story, and durable room identity integration.

## Inter-Layer Dependencies

| From | Depends on | Rule |
| --- | --- | --- |
| Layer 2 | Layer 1 | Benchmarks and peer observations may use transport paths, but recommendations remain advisory. |
| Layer 3 | Layer 1 | Scheduler capacity maps onto real binary-v1 runtime sender windows. |
| Layer 3 | Layer 2 | Layer 3 may use Layer 2 hints only through explicit planning policy; current completion does not require automatic control. |
| Layer 4 | Layer 1 | Rooms and control events require encrypted session transport but must not call that durable identity. |
| Layer 5 | Layer 4 | Capability transport and consent are room-scoped, but room membership alone never authorizes execution. |
| Layer 5 | Layer 3 | Agent/control work may reserve runtime capacity; execution authority still comes from Layer 5 consent. |

## Current Repository Implementation Status

| Layer | Production runtime evidence | Test / harness evidence | Manual or documented evidence | Implemented category |
| --- | --- | --- | --- | --- |
| Layer 1 | `src-tauri/src/discovery.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/storage.rs`, `src-tauri/src/crypto.rs`, `src-tauri/src/transfer.rs`, `src-tauri/src/chunk_frame.rs` | Rust transport, storage, discovery, binary frame, encryption, finalize, runtime-window tests | `docs/transfer/validation.md` records same-machine smoke boundaries and two-machine requirements | Production runtime |
| Layer 2 | `src-tauri/src/diagnostics.rs`, `src-tauri/src/device_profile.rs`, `src-tauri/src/capability_probe.rs`, `src-tauri/src/link_benchmark.rs`, `src/pages/SettingsPage.tsx` | Device profile, capability probe, diagnostics DTO, and benchmark tests | Developer Tools current-session diagnostics | Production runtime, Developer Tools visible |
| Layer 3 | `src/lib/transferPlanner.ts`, `src/lib/transferScheduler.ts`, `src/App.tsx`, `src/lib/agentBridge/controlWindowRuntime.ts`, `src-tauri/src/transfer.rs` | Planner/scheduler/MicroFlowGroup tests plus `scripts/run-cl4-contention-smoke.mjs` | Transfer validation guide and logged smoke interpretation | Production runtime with automated harness |
| Layer 4 | `src-tauri/src/storage.rs`, `src-tauri/src/room_control.rs`, `src/lib/agentBridge/roomControlEvent.ts`, `src/lib/agentBridge/controlQueue.ts` | Room lifecycle/storage tests, room-control Rust tests, control-event validation tests | Session-scoped room/control flow documented in Agent Bridge and transfer docs | Production runtime, session-scoped |
| Layer 5 | `src/lib/ai/*`, `src/lib/agentBridge/peerConsent.ts`, `src/lib/agentBridge/helloPeerExecution.ts`, `src/lib/agentBridge/logging.ts`, `src/components/agentBridge/RoomControlPanel.tsx` | Provider, context, validator, PolicyGate, preview, consent, execution, logging, and room-control tests | Room-scoped UI and redacted lifecycle logs | Production runtime, narrow capability slice |

## Current Completion Assessment

These scores are against the canonical definitions in this document, not against outdated planning labels. `Canonical completion` measures the whole intended layer. `Implemented-scope maturity` measures how complete and validated the currently shipped scope is.

| Layer | Canonical completion | Implemented-scope maturity | Status label | Confidence | Main remaining gaps |
| --- | ---: | ---: | --- | --- | --- |
| Layer 1 - Secure LAN transport | 86% | 90% | Mature operational core | High | Durable identity, broader release/two-device validation, independent security review, whole-file hash contract |
| Layer 2 - Device intelligence | 76% | 82% | Advisory diagnostics implemented; recommendation UX partial | High | User-visible recommended roles, peer benchmark UI, explicit planner-hint contract if consumed later, broader device matrix |
| Layer 3 - Smart orchestration | 84% | 88% | Operational orchestration core | High | Deficit/history-aware adaptation, broader end-to-end contention validation, future capability-routing policy |
| Layer 4 - Multi-device trusted rooms | 55% | 72% | Session-scoped trusted-room/control foundation | High | Durable identity continuity, reconnect semantics, multi-peer routing, generic RoomEvent provenance/history |
| Layer 5 - Agent-assisted device workspace | 52% | 78% | Narrow capability slice implemented | High | Capability registry, second capability, multi-step task orchestration, local LLM scheduling, MCP/tool integration, durable trust integration |

No layer is `100%`. The largest gap between full vision and implementation remains Layer 5: the Hello Peer path proves the safety shape, not the whole workspace vision.

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
| Room | A user-visible transfer/control relationship. Current runtime is primarily two-peer and session-scoped; future durable room identity belongs to Layer 4. |
| Room item | Text/file/image transfer payload metadata and lifecycle state. It is not a control event. |
| RoomControlEvent | Typed bounded control-plane event for Agent Bridge preview, status, execution request, and result. It is not ordinary room text. |
| Control lane | Scheduler/runtime reservation behavior for local outgoing room-control demand. It reserves capacity; it does not grant authority. |
| Capability | A fixed host-owned operation with schema, preview, consent, executor, result, and audit requirements. |
| PolicyGate | Deny-first host/receiver decision point that validates whether a capability request may proceed to user consent. |
| Consent | One explicit user authorization for one exact bounded request. Consent is consumed and is not reusable trust. |
| Audit log | Redacted lifecycle mirror for debugging and review. It is not runtime state, authorization, or reconstructable payload data. |

## Documentation Ownership Map

| Topic | Canonical document |
| --- | --- |
| Five-layer definitions, layer boundaries, scoring | `docs/architecture/项目布局规范.md` |
| Secure transport architecture | `docs/transfer/architecture.md` |
| Scheduler, runtime windows, MicroFlowGroup | `docs/transfer/scheduler.md` |
| Transfer validation and harnesses | `docs/transfer/validation.md` |
| Dynamic MicroFlowGroup capacity research reference | `docs/transfer/dynamic-microflowgroup-window-capacity-design.pdf` |
| Agent Bridge architecture and safety | `docs/agent-bridge/architecture-and-safety.md` |
| Room-control transport | `docs/agent-bridge/room-control-transport.md` |
| Capability consent and execution contracts | `docs/agent-bridge/capability-contracts.md` |
| Provider configuration | `docs/agent-bridge/provider-configuration.md` |
| Release operations | `docs/operations/release-workflow.md` |

Duplicated definitions in non-canonical documents should be replaced with links to the owning document.

## Classifying Future Features

Use the highest layer that owns the product responsibility:

- New frame formats, encryption, discovery, transfer recovery, or sender window primitives belong to Layer 1.
- New current-session observations, benchmarks, capability summaries, or advisory recommendations belong to Layer 2.
- New queue, capacity, fairness, MicroFlowGroup, or runtime-window policies belong to Layer 3.
- New durable peer identity, room membership, provenance, routing, replay, reconnect, or room history semantics belong to Layer 4.
- New model planning, validation, PolicyGate, consent, capability execution, result orchestration, or audit behavior belongs to Layer 5.

When a feature crosses layers, document both the owner and the dependency. For example, a future "choose best peer for execution" feature would need Layer 2 observations, Layer 3 planning, Layer 4 identity/routing, and Layer 5 consent/execution, but Layer 5 would still own execution authority.

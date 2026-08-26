# Pastey reference

This document owns stable cross-layer terminology, identifiers, and compact source pointers. Version 1.9.2 is the previous frozen semantic baseline; version 1.9.3 adds the completed Phase 1–5 foundations without changing it. Version 2.0.0 is reserved for the planned Phase 6 Agent product milestone. Source types and validators remain authoritative.

## Canonical primitives

| Primitive | Meaning | Current execution state |
| --- | --- | --- |
| Search | Find/select an object on an explicit Host. | Implemented |
| Transform | Authorize reviewed modification intent for the selected object on its current explicit Host. | Plan framework only |
| Transfer | Move the current object revision to an explicit device or landing. | Implemented |
| Execute | Authorize reviewed execution intent for the exact current revision on an explicit Host. | Plan framework only |

Only Transfer changes location. Transform conceptually advances the same logical object revision. Execute consumes the exact current revision. No Transform step means no mutation authority; no Execute step means no execution authority.

## Stable identifiers and source map

| Boundary | Current value / source |
| --- | --- |
| Natural proposal | `ask-bridge-natural-v1` — `src/lib/ai/naturalV1Plan.ts` |
| Provider vocabulary | `Search`, `Transform`, `Transfer`, `Execute` |
| Plan schema/hash v1 | `bridge-plan-v1` / `bridge-plan-revision-hash-v1:*` — `src-tauri/src/bridge_plan.rs` |
| Plan protocol v1 | `pastey-bridge-plan-protocol-v1` — `src-tauri/src/bridge_plan/protocol.rs` |
| Plan schema/hash v2 | `bridge-plan-v2` / `bridge-plan-revision-hash-v2:*` — `src-tauri/src/bridge_plan_v2.rs` |
| Plan protocol v2 | `pastey-bridge-plan-protocol-v2`; `bridge_plan.v2.review_request` / `bridge_plan.v2.attempt_start` — `src-tauri/src/bridge_plan_v2.rs` |
| Candidate binding | `BridgePlanCandidateStore` — `src-tauri/src/file_candidates.rs` |
| Safe private identity | `SourceIdentity` — `src-tauri/src/safe_file_identity.rs` |
| Logical dependency | v1 `LogicalObjectRevision`; v2 `ManagedObjectRevisionV2` / `PlanRootV2` — Plan modules |
| Capability transport | `pastey-peer-capabilities-v2` — `src-tauri/src/peer_capabilities.rs` |
| Shared transfer capacity | `TransferCapacityCoordinator` — `src-tauri/src/transfer_orchestration.rs` |
| Ordinary route | `BridgeRoute` — `src/lib/bridgeRouting.ts` |
| Control route | `pastey-bridge-control-route-v1` — `src-tauri/src/room_control.rs` |
| Logical Host identity | `HostRef`, `PlanParticipantRef`, `HostSessionBinding` — `src-tauri/src/host_identity.rs` |
| Developer Terminal protocol | `developer_terminal` / `pastey-developer-terminal-v0` — `src-tauri/src/developer_terminal.rs` |
| Developer terminal endpoint identity | `DeveloperHostRef`, `DeveloperTerminalBinding` — `src-tauri/src/host_runtime.rs` |
| Developer Terminal authority | private `DeveloperTerminalGrant` — `src-tauri/src/developer_terminal.rs` |
| Managed effect authority | `AuthorityContextV1`, `EffectEnvelopeV1`, `EffectAuthorityStateV1` — `src-tauri/src/effect_authority.rs` |
| Managed resources | `ManagedResourceResolverV1` — `src-tauri/src/managed_resources.rs` |
| Managed process/network backends | `ExecutionWorldServiceV1`, `NetworkBrokerServiceV1` — `src-tauri/src/execution_world.rs`, `src-tauri/src/network_broker.rs` |
| Core managed result seam | one-use v2 claim/evidence/result validation — `src-tauri/src/managed_execution.rs` |

There are currently no concrete Transform or Execute capability identifiers.

Developer Terminal identifiers are a separate human-only authority domain. They are not Layer 5 capability identifiers, Plan primitives, or runtime availability facts.

## Framework schemas

Conceptually, Transform carries:

```text
target
executionDevice
input logical revision
output logical revision
modificationIntent
```

It preserves location and declares the next same-object revision. The intent is bounded reviewed text, not a patch, command, path, or implementation selection.

Conceptually, Execute carries:

```text
target
executionDevice
target logical revision
executionIntent
```

It does not select a runtime, executable, shell, cwd, environment, network policy, or process. The 1.9.3 Core substrate can derive exact managed authority and validate evidence, but product attempts containing either primitive still fail closed until the planned Phase 6 Agent path exists.

## Authority, routing, and visibility

One requester Review & Run binds the complete immutable Plan. Candidate selection chooses data only. Rust continues currently executable Search and Transfer steps automatically. Transform/Execute framework presence creates no generic persistent modification or process authority.

Capability projections contain `0..N` bounded observations, never routing or authority. An empty projection is valid and creates no fallback fact, topology change, approval, or Host choice. Local and peer observations remain independent, and the current projection truthfully contains no implemented Transform/Execute facts.

Private paths, safe-open handles, source fingerprints, digests, PipelinePrivate roots, ObjectRefs, grants, and continuation state remain Rust-private. The renderer receives reviewed semantics, redacted candidate metadata, and safe activity/result summaries.

## Continuation and resource ownership

Layer 5 derives next-step eligibility only from immutable Plan/attempt state and atomically claims the next authored Transfer. Layer 3 admits bounded transport capacity for both ordinary and managed Transfers. Layer 4 supplies current-session route/control delivery; Layer 1 moves bytes. No lower layer infers that PipelinePrivate is followed by Transform.

## Managed object acquisition

Search means finding. Object acquisition/binding is the Host-owned boundary that validates a physical artifact and associates it with a managed logical object/revision. V1 retains `selected_file` and Search-first composition. V2 generic `PlanRootV2` identifies an exact logical revision and participant location without embedding a physical path, ObjectRef, or private binding. Inbox, drag/drop, local-selection, and generated-artifact roots do not add a fifth primitive and do not themselves grant Transform authority.

## Validation map

| Boundary | Primary source | Focused validation |
| --- | --- | --- |
| Composer/object flow | `src/lib/bridgePlanComposer.ts` | `tests/naturalV1Plan.test.ts` |
| Natural proposals | `src/lib/ai/naturalV1Plan.ts`, `providerInstructionPack.ts` | `scripts/run-natural-v1-tests.mjs` |
| Plan lifecycle/schema v1/v2 | `src-tauri/src/bridge_plan.rs`, `bridge_plan_v2.rs` | Rust Bridge Plan v1/v2 tests |
| Search/Transfer protocol | `src-tauri/src/bridge_plan/protocol.rs`, `commands.rs`, `transfer.rs` | Rust protocol/transfer tests |
| V2 review/admission protocol | `src-tauri/src/bridge_plan_v2.rs`, `host_admission.rs`, `room_control.rs` | Rust v2 identity/topology/replay/restart/Burn tests |
| Safe selection and identity | `src-tauri/src/file_candidates.rs`, `safe_file_identity.rs` | Rust candidate/identity tests |
| Capability facts | `src-tauri/src/peer_capabilities.rs` | Rust projection tests |
| Burn/restart | `src-tauri/src/bridge_plan.rs`, `object_refs.rs`, `room_control.rs` | Rust lifecycle/cleanup tests |
| Developer Terminal | `src-tauri/src/developer_terminal.rs`, `host_runtime.rs`, `room_control.rs` | Rust terminal authority/protocol/PTY tests and Windows cross-compile |

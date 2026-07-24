# Pastey reference

This document owns stable cross-layer terminology, identifier conventions, and compact source pointers. It does not replace source types or validators.

## Naming rules

| Subject | Rule |
| --- | --- |
| Large protocol or product generation | Use `name-vN`, for example `binary-v1` and `ask-bridge-natural-v1`. |
| `schemaVersion` | Use lowercase kebab-case with a `-vN` suffix for durable public records. |
| Transform intent | Use a stable bounded identifier such as `extract_readable_text_v1`. |
| Provider action kind | Advisory only; it never creates host authority. |
| Executor kind | Host-owned implementation selection; it is not provider input. |

Names must not imply stronger authority than exists. Do not add a `v1` suffix to implementation helpers, instruction packs, or documentation-only cleanup. Do not describe a queue handoff as delivery or an accepted peer as trusted execution authority.

Names must identify the owning Plan boundary and must not create authority outside a reviewed Bridge Plan.

## Core identifiers

| Class | Current values / source |
| --- | --- |
| Natural plan | `ask-bridge-natural-v1` — `src/lib/ai/naturalV1Plan.ts` |
| Provider action kinds | `Search`, `Transform`, and `Transfer` advisory plan vocabulary — `src/lib/ai/naturalV1Plan.ts` |
| Plan protocol | `pastey-bridge-plan-protocol-v1` — `src-tauri/src/bridge_plan/protocol.rs` |
| Private candidate binding | `BridgePlanCandidateStore` — `src-tauri/src/file_candidates.rs` |
| Object identity | `pastey-object-ref-v1`, opaque `object-ref-<uuid>` — Rust-private ephemeral object store in `object_refs.rs` |
| Transform intent | `extract_readable_text_v1` — `src-tauri/src/transform_registry.rs` |
| Ordinary Bridge route | `BridgeRoute` — `src/lib/bridgeRouting.ts` |
| Control route | `pastey-bridge-control-route-v1` — `src/lib/tauri.ts` and `src-tauri/src/room_control.rs` |

## Bridge route vocabulary

`selected_peer` is one exact current-session peer. `selected_peers` is an explicit ordinary-data subset. `broadcast_bridge` is explicit ordinary-data fan-out to the current routeable peer set. Bridge Plan control accepts only the reviewed selected-peer route.

`peer_session_id` identifies the current endpoint/key binding. An endpoint/key change creates a new ID; old routes expire rather than rebinding. `bridge_peers` is current-session route state. `bridge_durable_identities` is paired-device display metadata and is not routeability or authority.

## Room-control event vocabulary

Room-control carries only `bridge_plan.*` review, attempt, progress, result, and failure messages for Layer 5. It is encrypted current-session control transport—not ordinary Bridge items and not a second authority model. A delivery receipt never establishes Plan approval or receiver review.

## Approval and candidate vocabulary

Complete-plan approval and receiver review bind the Bridge, peers, revision, and expiry. One-use execution grants are process-local and terminally consumed; denial or an unsupported intent creates no execution authority.

A **candidate** is bounded, redacted discovery metadata. A live Search → Transfer Plan may return this metadata to let the requester choose one result; the selected Host validates that opaque selection against its private Bridge Plan candidate store before transfer. An **ObjectRef** is an opaque receiver-owned ephemeral identity bound to one Bridge, owner, kind, and finite TTL. It is not a path, approval, authority token, worker ID, or durable handle. `handoff_queued` means the Layer 3 queue accepted a source; it is neither byte-transfer success nor completion.

## Transform vocabulary

Natural-v1 carries only the bounded intent `extract readable text`. Host registry entry `extract_readable_text_v1` implements the supported text/plain, text/markdown, application/json, and text/csv to text/plain transition. In a Bridge Plan, the completed result remains an opaque selected-device-local output; a later approved Transfer may consume it, but raw process output is not a product result.

Unsupported Transform intents fail closed before staging or worker mutation and create an unapproved alternative Plan revision. The fixed readable-text worker accepts only an approved Bridge Plan Transform step. Raw worker output is Rust-only; only safe Plan result metadata crosses the product boundary.

Common bounded error/status vocabulary includes `route_expired`, `candidate_not_found`, `candidate_expired`, `candidate_changed`, `handoff_failed`, `rejected`, and `unsupported_future`. The exact Plan validator or Rust command is authoritative for which values are accepted on a particular path.

## Visibility and serialization

Provider input, sender-visible results, logs, and ordinary control events must not contain receiver absolute paths, file contents, source bytes, raw Transform output, lease identifiers, staging paths, consent secrets, or reusable authority tokens. Logs are redacted audit mirrors, not state or authorization.

## Authoritative source pointers

| Subject | Primary source | Validation |
| --- | --- | --- |
| Natural-v1 validation | `src/lib/ai/naturalV1Plan.ts`, `providerRiskScanner.ts` | `tests/aiSlot.test.ts` |
| Bridge Plan lifecycle and protocol | `src-tauri/src/bridge_plan.rs`, `bridge_plan/protocol.rs` | Rust Bridge Plan tests |
| Control event validation | `src-tauri/src/room_control.rs` | Rust room-control tests |
| Bridge routes | `src/lib/bridgeRouting.ts`, `src-tauri/src/room_control.rs` | bridge route and Rust tests |
| Candidate safety | `src-tauri/src/file_candidates.rs` | Rust Bridge Plan tests |
| Object lifecycle and Transfer | `src-tauri/src/object_refs.rs`, `commands.rs` | Rust Bridge Plan tests |
| Transform authority | `src-tauri/src/file_candidates.rs`, `commands.rs`, `transform_registry.rs`, `transform_sandbox/` | Transform and Rust tests |

See the layer documents for architecture and [development.md](development.md) for commands.

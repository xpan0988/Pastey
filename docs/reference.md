# Pastey reference

This document owns stable cross-layer terminology, identifier conventions, and compact source pointers. It does not replace source types or validators.

## Naming rules

| Subject | Rule |
| --- | --- |
| Large protocol or product generation | Use `name-vN`, for example `binary-v1` and `ask-bridge-natural-v1`. |
| `schemaVersion` | Use lowercase kebab-case with a `-vN` suffix, for example `transfer-request-candidate-payload-result-v1`. |
| Capability ID | Use a stable dotted ID without an embedded version, for example `filesystem.find_file_candidates`. |
| Registry version | Version the entry separately, currently `v1` where applicable. |
| Provider action kind | Advisory only; it never creates host authority. |
| Executor kind | Host-owned implementation selection; it is not provider input. |
| Template kind | Shared lifecycle classification, not a permission. |

Names must not imply stronger authority than exists. Do not add a `v1` suffix to implementation helpers, instruction packs, or documentation-only cleanup. Do not describe a queue handoff as delivery or an accepted peer as trusted execution authority.

Namespace prefixes identify the owning domain: `filesystem.` is a bounded receiver-local workspace capability, `transfer.` enters the existing transfer pipeline, `runtime.` is fixed host diagnostic coverage, and `artifact.` is a bounded Transform contract. A namespace does not broaden authority.

## Core identifiers

| Class | Current values / source |
| --- | --- |
| Natural plan | `ask-bridge-natural-v1` — `src/lib/ai/naturalV1Plan.ts` |
| Provider action kinds | `request_peer_file_candidates`, `request_peer_candidate_payload`, and diagnostic Hello actions — `src/lib/ai/capabilityRegistry.ts` |
| Capability IDs | `runtime.execute_hello_template`, `runtime.hello_stdout`, `filesystem.find_file_candidates`, `transfer.request_candidate_payload`, `artifact.transform_selected` — registry/manifest and validators |
| Candidate search schemas | `filesystem-find-file-candidates-*-v1` — `src/lib/ai/fileCandidateRequest.ts` |
| Candidate payload schemas | `transfer-request-candidate-payload-*-v1` — `src/lib/ai/candidatePayloadRequest.ts` |
| Capability envelope | `pastey-agent-bridge-capability-envelope-v1` — `src/lib/ai/capabilityPreviewEnvelope.ts` |
| Ordinary Bridge route | `BridgeRoute` — `src/lib/bridgeRouting.ts` |
| Control route | `pastey-bridge-control-route-v1` — `src/lib/tauri.ts` and `src-tauri/src/room_control.rs` |

## Bridge route vocabulary

`selected_peer` is one exact current-session peer. `selected_peers` is an explicit ordinary-data subset. `broadcast_bridge` is explicit ordinary-data fan-out to the current routeable peer set. Control and capability transport accept only selected-peer routes.

`peer_session_id` identifies the current endpoint/key binding. An endpoint/key change creates a new ID; old routes expire rather than rebinding. `bridge_peers` is current-session route state. `bridge_durable_identities` is paired-device display metadata and is not routeability or authority.

## Room-control event vocabulary

Room-control events carry typed capability previews, acknowledgements, denials, invalid/expired status, execution requests, and execution results. They are encrypted current-session control transport—not ordinary Bridge items and not durable workflow records. The control queue may show pending, delivered, denied, invalid, expired, executed, or terminal result state, but a delivery receipt never establishes consent.

## Consent and candidate vocabulary

**Allow once** is an exact, one-time receiver approval bound to the capability, session, peers, request, payload hash, and expiry. **Deny** is terminal for that request. Discovery consent is not payload-transfer consent.

A **candidate** is redacted discovery metadata. Its opaque ID is not a path, file handle, or authority token. A **candidate lease** is Rust-private Transform admission state. `handoff_queued` means the Layer 3 queue accepted a source; it is neither byte-transfer success nor completion.

## Transform vocabulary

`selected_artifact_output` is the one natural-v1 Transform kind. `artifact.transform_selected` is its host-built bounded capability. Relevant lifecycle categories include `reserved`, `revalidated`, `started`, `completed`, `failed`, `timed_out`, `rejected`, and `execution_state_unknown`.

`sandbox_unavailable` is the production result while `UnavailableTransformSandboxAdapter` is active. It is returned before staging, lease, journal, or execution mutation. Raw executor output is Rust-only; only sanitized bounded results may cross the authoritative result boundary.

Common bounded error/status vocabulary includes `route_expired`, `candidate_not_found`, `candidate_expired`, `candidate_changed`, `handoff_failed`, `rejected`, `unsupported_future`, and `sandbox_unavailable`. The exact public validator or Rust command is authoritative for which values are accepted on a particular path.

## Visibility and serialization

Provider input, sender-visible results, logs, and ordinary control events must not contain receiver absolute paths, file contents, source bytes, raw Transform output, lease identifiers, staging paths, consent secrets, or reusable authority tokens. Logs are redacted audit mirrors, not state or authorization.

## Authoritative source pointers

| Subject | Primary source | Validation |
| --- | --- | --- |
| Natural-v1 validation | `src/lib/ai/naturalV1Plan.ts`, `providerRiskScanner.ts` | `tests/aiSlot.test.ts` |
| Capability registry/manifests | `src/lib/ai/capabilityRegistry.ts`, `src/lib/agentBridge/capabilityManifest.ts` | `tests/capabilityManifest.test.ts` |
| Control event schemas | `src/lib/agentBridge/roomControlEvent.ts` | room-control tests |
| Bridge routes | `src/lib/bridgeRouting.ts`, `src-tauri/src/room_control.rs` | bridge route and Rust tests |
| Candidate safety | `src/lib/ai/*Candidate*.ts`, `src-tauri/src/file_candidates.rs` | candidate tests |
| Transform authority | `src-tauri/src/file_candidates.rs`, `commands.rs`, `transform_sandbox/` | Transform and Rust tests |

See the layer documents for architecture and [development.md](development.md) for commands.

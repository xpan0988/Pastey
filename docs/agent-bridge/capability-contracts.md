# Capability Contracts

This document owns the current Layer 5 capability contract for Pastey Agent Bridge. For the broader safety architecture, see [architecture-and-safety.md](architecture-and-safety.md). For transport details, see [room-control-transport.md](room-control-transport.md). For Bridge membership and peer terminology, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For target routing semantics, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md).

## Implemented Capabilities

The implemented capabilities are fixed, host-owned demonstration capabilities:

- `runtime.execute_hello_template`, the legacy fixed Hello Peer template;
- `runtime.hello_stdout/v1`, a fixed Hello Stdout capability backed by a Rust host helper.

They use:

- advisory provider output;
- host-side request construction;
- typed preview envelope;
- receiver PolicyGate;
- explicit Allow once or Deny;
- exact one-time consent binding;
- host-built execution request;
- fixed host-owned executors;
- typed execution result.

They do not execute model-authored code, shell commands, file operations, network calls, arbitrary arguments, arbitrary environment variables, arbitrary file paths, or arbitrary tool calls.

`runtime.execute_hello_template` returns the exact fixed Hello Peer result `hello peer!`.

`runtime.hello_stdout/v1` asks the receiver to run a host-owned Rust helper that returns typed stdout metadata. Its successful result must contain:

- `capability: runtime.hello_stdout/v1`;
- `runtimeKind: rust_host_helper`;
- `stdout: hello peer`;
- empty `stderr`;
- `exitCode: 0`;
- bounded `durationMs`;
- `timedOut: false`;
- bounded truncation flags.

## Static Capability Registry

The implemented registry is static and host-owned. It lives in `src/lib/ai/capabilityRegistry.ts` and currently contains only the two capabilities listed above. It is not plugin loading, not provider-configurable, and not a generic executor table.

Each registry entry defines:

- capability id and version;
- provider action kind;
- preview, consent grant, execution request, and result schema names;
- selected-peer route policy;
- exact allow-once consent policy;
- executor kind;
- provider-forbidden fields, including command/script/path/env/network fields and result-only stdout/stderr/exit fields;
- audit redaction policy;
- UI labels.

The registry is used to keep provider validation, PolicyGate, pending action hashing, preview dispatch, room-control event dispatch, consent binding, and UI labels aligned. It does not replace capability-specific schemas or validators. Unknown capability ids, unknown versions, and unknown schema names reject fail-closed.

The shared lifecycle envelope schema is `pastey-agent-bridge-capability-envelope/v1`. It is a compatibility view over the existing typed preview/control payloads and includes capability id/version, request id, room/source/target refs, selected-peer route policy, exact allow-once consent policy, created/expiry times, payload hash, typed payload, and bounded room-control transport metadata. Existing payload schemas remain capability-specific.

## Preview Contract

The preview request is built from a validated pending action and converted into a capability preview envelope. The envelope is bounded, typed, current-session, and tied to the active Bridge/peer context.

Legacy implementation term: active room/peer context.

Capability preview, acknowledgement, denial, execution request, and execution result events must bind to one exact selected peer/session/request. The event `roomRef`, `sourceDeviceRef`, and `targetPeerRef` must match the active room-control session, and outbound transport must carry a selected-peer control route that resolves through the current-session `bridge_peers` row for that peer.

Selected-peers and broadcast capability routes are not implemented and are rejected. Durable paired-device display metadata, accepted Bridge membership, logs, delivery receipts, and prior delivery outcomes do not satisfy capability target binding.

Production evidence:

- `src/lib/ai/helloPeerRequest.ts`
- `src/lib/ai/helloStdoutRequest.ts`
- `src/lib/ai/capabilityPreviewEnvelope.ts`
- `src/lib/agentBridge/roomControlEvent.ts`
- `src-tauri/src/room_control.rs`

## Consent Contract

Receiver consent is explicit and exact. Allow once binds to the previewed capability, request reference, sender/receiver context, and expiry window. Deny rejects the request and sends a typed status path.

Production evidence:

- `src/lib/agentBridge/peerConsent.ts`
- `src/components/agentBridge/RoomControlPanel.tsx`
- `src/lib/agentBridge/controlQueue.ts`

Consent is not reusable trust. Accepted Bridge peer status, session verification, and a future successful delivery must not substitute for consent. A capability must not inherit authority from consent granted to another capability.

## Execution Contract

The sender can queue an execution request only after a matched allow-once acknowledgement. The receiver revalidates the consent binding and consumes it once before execution.

The current executors are:

- `runtime.execute_hello_template`, which returns exactly the fixed Hello Peer result;
- `runtime.hello_stdout/v1`, which calls the Tauri `execute_hello_stdout_capability` command and returns typed stdout/stderr/exit metadata from a host-owned Rust helper.

Neither executor accepts command text, script text, runtime arguments, file paths, environment variables, network targets, shell interpolation, or model-authored execution material.

Production evidence:

- `src/lib/agentBridge/helloPeerExecution.ts`
- `src/lib/agentBridge/helloStdoutExecution.ts`
- `src-tauri/src/hello_stdout.rs`
- `src/lib/agentBridge/roomControlEvent.ts`
- `src-tauri/src/room_control.rs`

## Result Contract

Execution result events return typed success or bounded error data through the same Bridge control transport. Results are tied to the execution request and are not generic Bridge messages.

## Requirements For Future Capabilities

Every new capability must define:

- capability name and version;
- static registry entry;
- preview schema;
- unsafe-field rejection rules;
- PolicyGate criteria;
- consent binding and expiry;
- execution request schema;
- host-owned bounded executor;
- result schema;
- replay and duplicate behavior;
- queue and transport bounds;
- target route requirements, including why broadcast is disallowed or explicitly validated;
- redacted audit fields;
- tests across validator, consent, executor, Bridge control event, and UI state.

New capabilities must also state which layer owns each dependency. A capability that needs durable peer trust cannot be complete until the relevant Layer 4 durable identity semantics exist. Bridge membership alone never grants execution authority.

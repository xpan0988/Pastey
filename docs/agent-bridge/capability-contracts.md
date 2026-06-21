# Capability Contracts

This document owns the current Layer 5 capability contract for Pastey Agent Bridge. For the broader safety architecture, see [architecture-and-safety.md](architecture-and-safety.md). For transport details, see [room-control-transport.md](room-control-transport.md). For Bridge membership and peer terminology, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For target routing semantics, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md).

## Implemented Capability

The only implemented capability is the fixed Hello Peer capability.

It uses:

- advisory provider output;
- host-side request construction;
- typed preview envelope;
- receiver PolicyGate;
- explicit Allow once or Deny;
- exact one-time consent binding;
- host-built execution request;
- fixed in-process executor;
- typed execution result.

It does not execute model-authored code, shell commands, file operations, network calls, or arbitrary tool calls.

## Preview Contract

The preview request is built from a validated pending action and converted into a capability preview envelope. The envelope is bounded, typed, current-session, and tied to the active Bridge/peer context.

Legacy implementation term: active room/peer context.

Capability preview, execution request, and execution result events must bind to an exact selected peer/session/request. Broadcast is not the default route for capability events.

Production evidence:

- `src/lib/ai/helloPeerRequest.ts`
- `src/lib/ai/capabilityPreviewEnvelope.ts`
- `src/lib/agentBridge/roomControlEvent.ts`
- `src-tauri/src/room_control.rs`

## Consent Contract

Receiver consent is explicit and exact. Allow once binds to the previewed capability, request reference, sender/receiver context, and expiry window. Deny rejects the request and sends a typed status path.

Production evidence:

- `src/lib/agentBridge/peerConsent.ts`
- `src/components/agentBridge/RoomControlPanel.tsx`
- `src/lib/agentBridge/controlQueue.ts`

Consent is not reusable trust. Accepted Bridge peer status, session verification, and a future successful delivery must not substitute for consent. A future capability must not inherit authority from Hello Peer consent.

## Execution Contract

The sender can queue an execution request only after a matched allow-once acknowledgement. The receiver revalidates the consent binding and consumes it once before execution.

The current executor is `runtime.execute_hello_template` and returns exactly the fixed Hello Peer result. It has no shell, process, file, network, or generic runtime access.

Production evidence:

- `src/lib/agentBridge/helloPeerExecution.ts`
- `src/lib/agentBridge/roomControlEvent.ts`
- `src-tauri/src/room_control.rs`

## Result Contract

Execution result events return typed success or bounded error data through the same Bridge control transport. Results are tied to the execution request and are not generic Bridge messages.

## Requirements For Future Capabilities

Every new capability must define:

- capability name and version;
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

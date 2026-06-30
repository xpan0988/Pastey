# Capability Contracts

This document owns the current Layer 5 capability contract for Pastey Agent Bridge. For the broader safety architecture, see [architecture-and-safety.md](architecture-and-safety.md). For transport details, see [room-control-transport.md](room-control-transport.md). For Bridge membership and peer terminology, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md). For target routing semantics, see [../architecture/bridge-routing.md](../architecture/bridge-routing.md). For capability IDs, schema versions, provider action kinds, executor kinds, and future capability naming, see [../architecture/naming-conventions.md](../architecture/naming-conventions.md).

## Implemented Capabilities

The implemented capabilities are fixed, host-owned bounded capabilities:

- `runtime.execute_hello_template`, the legacy fixed Hello Peer template;
- `runtime.hello_stdout`, a fixed Hello Stdout capability backed by a Rust host helper;
- `filesystem.find_file_candidates`, a receiver-side metadata-only file-candidate search over approved local scope labels.

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

They do not execute model-authored code, shell commands, arbitrary process operations, network calls, arbitrary arguments, arbitrary environment variables, arbitrary file paths, arbitrary tool calls, or automatic file transfers. The file-candidate capability performs a bounded receiver-owned filesystem metadata traversal only after explicit receiver Allow once.

`runtime.execute_hello_template` returns the exact fixed Hello Peer result `hello peer!`.

`runtime.hello_stdout` asks the receiver to run a host-owned Rust helper that returns typed stdout metadata. Its successful result must contain:

- `capability: runtime.hello_stdout`;
- `runtimeKind: rust_host_helper`;
- `stdout: hello peer`;
- empty `stderr`;
- `exitCode: 0`;
- bounded `durationMs`;
- `timedOut: false`;
- bounded truncation flags.

## Static Capability Registry

The registry is static and host-owned. It lives in `src/lib/ai/capabilityRegistry.ts` and currently contains the implemented capabilities listed above. It is not plugin loading, not provider-configurable, and not a generic executor table.

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

The shared lifecycle envelope schema is `pastey-agent-bridge-capability-envelope-v1`. It is a compatibility view over the existing typed preview/control payloads and includes capability id/version, request id, room/source/target refs, selected-peer route policy, exact allow-once consent policy, created/expiry times, payload hash, typed payload, and bounded room-control transport metadata. Existing payload schemas remain capability-specific.

## Workspace Capability: `filesystem.find_file_candidates`

`filesystem.find_file_candidates` is the first implemented read-only workspace capability. It lets a sender ask one selected peer to search approved receiver-local scope labels for filename/metadata matches. It returns a bounded list of redacted candidate metadata only.

The user intent is: help find a file named `xxx` on another device and send it back. The implemented capability covers candidate discovery only. A later candidate-payload transfer requires a separate approved capability, likely named `transfer.request_candidate_payload`, and must require separate explicit consent. Search consent does not authorize payload transfer.

The advisory action kind is `request_peer_file_candidates`. Provider output must use this shape:

```json
{
  "schemaVersion": "ai-action-plan-v1",
  "kind": "request_peer_file_candidates",
  "title": "Find file candidates on the selected peer",
  "explanation": "Search the selected peer for filename or metadata matches and return a bounded candidate list. No file contents will be read and no file will be sent automatically.",
  "confidence": "medium",
  "requiresUserConfirmation": true,
  "references": [{ "kind": "peer", "ref": "selected-peer-ref" }],
  "proposedInput": {
    "capability": "filesystem.find_file_candidates",
    "targetPeerRef": "selected-peer-ref",
    "query": {
      "rawUserRequest": "help me find a file named xxx and send it to me",
      "filenameHint": "xxx",
      "extensions": [],
      "searchMode": "filename_metadata_only"
    },
    "scopePolicy": {
      "allowedScopes": ["downloads", "desktop", "documents", "pastey_shared"],
      "allowFullDisk": false,
      "includeFileContents": false,
      "includeAbsolutePaths": false,
      "includeHiddenFiles": false
    },
    "limits": {
      "maxCandidates": 10,
      "maxSearchMs": 5000,
      "maxDepth": 6
    },
    "safety": {
      "returnRedactedPaths": true,
      "noAutoTransfer": true,
      "requireReceiverConsent": true,
      "selectedPeerOnly": true
    }
  }
}
```

Current host validation requires:

- `schemaVersion: ai-action-plan-v1`;
- `kind: request_peer_file_candidates`;
- `requiresUserConfirmation: true`;
- `capability: filesystem.find_file_candidates`;
- one selected `targetPeerRef`;
- `searchMode: filename_metadata_only`;
- `allowedScopes` drawn only from `downloads`, `desktop`, `documents`, and `pastey_shared`;
- `allowFullDisk`, `includeFileContents`, `includeAbsolutePaths`, and `includeHiddenFiles` all false;
- `maxCandidates` from `1` to `20`;
- `maxSearchMs` from `500` to `10000`;
- `maxDepth` from `1` to `8`;
- `returnRedactedPaths`, `noAutoTransfer`, `requireReceiverConsent`, and `selectedPeerOnly` all true.

Unsafe provider fields reject fail-closed, including command/script/code fields, absolute paths, cwd/env/runtime arguments, network targets, stdout/stderr/exit fields, file contents, selected-peers/broadcast intent, hidden transfer requests, scheduler or MicroFlowGroup mutation, and durable-trust or trusted-executor claims.

The host-built preview and receiver result contracts preserve these additional boundaries:

- receiver Allow once is required before any search;
- results contain metadata candidates only, not file contents;
- paths are redacted labels, not absolute paths;
- candidate ids are current-request scoped and not reusable authority;
- delivery of candidates is not consent to transfer;
- selected-peers and broadcast remain unsupported;
- durable pairing display metadata does not authorize search or transfer.

The capability-specific schemas are:

- preview request: `filesystem-find-file-candidates-request-v1`;
- consent grant: `filesystem-find-file-candidates-consent-grant-v1`;
- execution request: `filesystem-find-file-candidates-execution-request-v1`;
- execution result: `filesystem-find-file-candidates-result-v1`.

The execution request is host-built after a matched allow-once acknowledgement. It carries the validated query, scope labels, limits, selected-peer route policy, consent policy, and payload hash. It does not carry real paths, shell commands, scripts, environment variables, runtime arguments, file contents, or transfer instructions.

The receiver-side executor is a Rust/Tauri host-owned executor with `executorKind: filesystem_find_candidates_host`. The receiver host maps only these safe labels to local directories when available:

- `downloads`;
- `desktop`;
- `documents`;
- `pastey_shared`.

Unavailable scopes are skipped and reported in `scopesSkipped`. `pastey_shared` is skipped when the app-owned shared directory does not exist. Hidden files and hidden directories are skipped. Symlinks are skipped. Traversal stays under the resolved scope root and skips entries whose canonicalization fails. Matching is filename-only with exact, case-insensitive, and substring matching; extension hints may narrow results. Directories are not returned as candidates.

The result contains:

- `schemaVersion: filesystem-find-file-candidates-result-v1`;
- `capability: filesystem.find_file_candidates`;
- `requestId`;
- `status`;
- `queryEcho`;
- redacted metadata candidates with opaque `candidateId`, `displayName`, `redactedLocation`, `extension`, `mimeFamily`, `sizeBytes`, `modifiedAt`, `matchReason`, and `confidence`;
- omitted/truncation metadata;
- bounded `durationMs`;
- bounded `errorCode`.

Candidate IDs are display/result identifiers only. They are not paths, durable file handles, transfer grants, or reusable authorization tokens. The current implementation does not persist a candidate-to-path store and does not pass candidates into the transfer queue.

## Preview Contract

The preview request is built from a validated pending action and converted into a capability preview envelope. The envelope is bounded, typed, current-session, and tied to the active Bridge/peer context.

Legacy implementation term: active room/peer context.

Capability preview, acknowledgement, denial, execution request, and execution result events must bind to one exact selected peer/session/request. The event `roomRef`, `sourceDeviceRef`, and `targetPeerRef` must match the active room-control session, and outbound transport must carry a selected-peer control route that resolves through the current-session `bridge_peers` row for that peer.

Selected-peers and broadcast capability routes are not implemented and are rejected. Durable paired-device display metadata, accepted Bridge membership, logs, delivery receipts, and prior delivery outcomes do not satisfy capability target binding.

Production evidence:

- `src/lib/ai/helloPeerRequest.ts`
- `src/lib/ai/helloStdoutRequest.ts`
- `src/lib/ai/fileCandidateRequest.ts`
- `src/lib/ai/fileCandidateAdvisory.ts`
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
- `runtime.hello_stdout`, which calls the Tauri `execute_hello_stdout_capability` command and returns typed stdout/stderr/exit metadata from a host-owned Rust helper.
- `filesystem.find_file_candidates`, which calls the Tauri `execute_file_candidate_search_capability` command and returns bounded redacted file metadata candidates from a host-owned Rust executor.

No executor accepts command text, script text, runtime arguments, arbitrary file paths, environment variables, network targets, shell interpolation, or model-authored execution material. The file-candidate executor accepts only validated safe scope labels and bounded metadata-search limits; it never reads file contents and never starts a transfer.

Production evidence:

- `src/lib/agentBridge/helloPeerExecution.ts`
- `src/lib/agentBridge/helloStdoutExecution.ts`
- `src/lib/agentBridge/fileCandidateExecution.ts`
- `src-tauri/src/hello_stdout.rs`
- `src-tauri/src/file_candidates.rs`
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

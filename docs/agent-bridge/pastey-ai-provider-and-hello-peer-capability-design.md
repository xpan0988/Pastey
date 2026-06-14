# Pastey AI Provider and Hello Peer Capability Design

## 1. Status and Scope

This is the design source for the AI Slot / Agent Bridge boundary. AI Slot
Phase E1 now implements mock and experimental OpenAI-compatible cloud advisory
provider calls, shared validation and policy gating, Settings-owned
runtime-memory provider configuration, and an active Room workflow for local
pending confirmation, outbound `HelloPeerRequest`, capability envelope, and
inbound review. Control Lane CL-1 additionally implements
a type-only `RoomControlEvent` wrapper, validation/current-session duplicate
helpers, and a pure capacity-split helper. CL-3 implements the separate
preview-only room-control transport and queue integration, CL-4 implements
sender-side scheduler reservation, CL-5 implements receiver PolicyGate review
with exact one-time consent, and CL-6 implements one fixed bounded Hello Peer
executor. The existing text path remains an ordinary persisted/encrypted
room-item transfer and is not reused. No generic peer executor, model-owned
action execution, remembered trust, or generic capability framework exists.

The target is the future Pastey AI Slot / Agent Bridge boundary. The design uses
the cross-system findings in
[AI Agent Provider and Tooling Survey](../research/ai-agent-provider-and-tooling-survey.md)
as prior research without duplicating that survey.

Pastey is AI-ready, not AI-dependent. Rooms, secure LAN transfer, the file
queue, scheduler, MicroFlowGroup, Device Diagnostics, Inbox, and all existing
user flows must remain complete and useful without a model, API key, provider
account, or AI feature.

This design does not require:

- binary-v2 for the first Hello Peer demo;
- direct local GGUF integration;
- a llama.cpp fork;
- a bundled inference runtime;
- a new transfer protocol or modification to existing transfer semantics.

The first capability bridge described here is deliberately narrow. It is a
design for a fixed Hello Peer demonstration, not a general peer execution
system.

The implemented advisory and bounded Hello Peer slice is recorded in
[AI Slot v0 / Phase E1 Implementation Notes](ai-slot-v0-implementation-notes.md).
CL-6 is deliberately limited to one host-owned zero-argument function and does
not implement the broader future capability bridge described here.

## 2. Design Goal

The practical AI Slot goal is:

```text
Pastey should be able to call a configured model provider,
send only a redacted current-session context,
receive a structured advisory action plan,
validate and gate it locally,
ask for user confirmation,
and only then translate it into a Pastey-owned action path.
```

The Pastey 2.0 vertical slice target is:

```text
AI proposes a restricted Hello Peer capability request.
Local user confirms.
The request is sent through a trusted room to a peer.
Peer validates and confirms.
Peer runs a restricted hello template using an available runtime.
Peer returns the bounded result: hello peer!
```

This target is not remote shell, arbitrary code execution, or file access. The
model does not choose a command, executable, script, path, or peer-side
implementation. The peer maps one declared capability to an internal fixed
template and retains the authority to deny it.

The existing secure LAN transfer and room paths remain the product foundation.
This document does not decide the final capability-message transport or add that
transport to current room behavior.

## 3. Common Industry Pattern Mapped to Pastey

| Industry Pattern | Pastey Mapping |
| --- | --- |
| Host / agent runtime | Pastey App |
| Context controls | `AiContextSnapshot` + `AiContextPolicy` |
| Provider adapter | `AiProvider` / `CloudApiProvider` / `LocalProvider` |
| Untrusted model plan | `AiActionPlan` |
| Schema validation | `AiActionPlan` validator |
| Policy gate | Pastey `PolicyGate` |
| Approval | Local user confirmation, currently `confirmed_local_only` |
| Executor | One fixed host-owned CL-6 Hello Peer function; no generic runtime |
| Result/audit | `CapabilityResult` + visible status/audit summary |

The model provider only generates text or structured output. It does not own:

- room create, join, leave, burn, or active-room state;
- peer trust or peer consent;
- text or file sending;
- queue, scheduler, requested-window, runtime-window, or MicroFlowGroup behavior;
- transfer protocol, ACK, finalize, cancel, burn, or Inbox behavior;
- Device Diagnostics or benchmark execution;
- local or peer capability permissions.

Model output is an untrusted proposal. Schema validation is not authorization.
Provider adapters normalize API transport, but Pastey owns validation, policy,
confirmation, action translation, execution boundaries, and result handling.

## 4. AI Entry Paths

Both possible entry paths must converge into the same Pastey-owned action-plan
and policy pipeline.

### 4.1 Built-in Pastey AI Path

This is the primary path for the current design:

```text
User
  -> Pastey UI
  -> AiContextSnapshot
  -> AiProvider
  -> AiGenerateResult
  -> AiActionPlan
  -> validation
  -> PolicyGate
  -> local pending confirmation
  -> validated HelloPeerRequest outbound preview
  -> validated CapabilityRequestPreviewEnvelope
  -> local inbound-preview simulation
  -> no room dispatch in Phase E1
```

The built-in path owns context minimization before provider invocation. The
provider receives only a policy-approved snapshot and returns untrusted output.

### 4.2 External Agent Path

This is a future, optional integration path:

```text
Claude Code / Codex / other agent
  -> future Pastey MCP or local tool server
  -> advisory-only Pastey tool
  -> AiActionPlan or equivalent proposal
  -> Pastey validation + PolicyGate + confirmation
```

External tools must not bypass the Pastey `PolicyGate`. Exposing a Pastey MCP
tool is not authorization. The external agent does not gain direct access to
Tauri commands, room internals, transfer functions, peer execution, or
credentials.

An external-agent proposal and a built-in provider proposal should be
indistinguishable at the policy boundary. Both are untrusted inputs that require
the same validation, reference binding, policy checks, and confirmation.

## 5. Provider Model

The following TypeScript-style shapes define the provider design boundary. AI
Slot Phase D preserves the Phase C `MockProvider` and
`CloudOpenAICompatibleProvider` in `src/lib/ai/`. Provider-specific OpenAI,
Anthropic, Gemini, gateway, local, and GGUF-backed adapters remain
unimplemented.

```ts
type AiProviderKind =
  | "mock"
  | "cloud_openai_compatible"
  | "openai"
  | "anthropic"
  | "gemini"
  | "gateway"
  | "local_openai_compatible";

type AiApiShape =
  | "openai_responses"
  | "openai_chat_completions"
  | "anthropic_messages"
  | "gemini_generate_content"
  | "openai_compatible_chat";

interface AiProviderConfig {
  providerId: string;
  displayName: string;
  kind: AiProviderKind;
  apiShape: AiApiShape;
  baseUrl?: string;
  model: string;
  apiKeyRef?: string;
  timeoutMs: number;
  maxOutputTokens: number;
  enabled: boolean;
}

interface AiProvider {
  readonly config: AiProviderConfig;
  generate(request: AiGenerateRequest): Promise<AiGenerateResult>;
}

interface CloudApiProvider extends AiProvider {
  readonly config: AiProviderConfig & {
    kind:
      | "cloud_openai_compatible"
      | "openai"
      | "anthropic"
      | "gemini"
      | "gateway";
  };
}

interface LocalProvider extends AiProvider {
  readonly config: AiProviderConfig & {
    kind: "local_openai_compatible";
  };
}

interface AiGenerateRequest {
  requestId: string;
  providerId: string;
  context: AiContextSnapshot;
  contextPolicy: AiContextPolicy;
  allowedActionKinds: AiActionKind[];
  outputSchema: "ai-action-plan/v1";
  userRequest: string;
}

interface AiGenerateResult {
  requestId: string;
  providerId: string;
  model: string;
  rawText?: string;
  parsedPlan?: AiActionPlan;
  usage?: {
    inputTokens?: number;
    outputTokens?: number;
  };
  error?: {
    code: string;
    message: string;
  };
}
```

### Provider Priority

```text
Phase 0: MockProvider
Phase 1: CloudOpenAICompatibleProvider or OpenAIProvider
Phase 2: AnthropicProvider / GeminiProvider
Phase 3: GatewayProvider
Phase 4: LocalOpenAICompatibleProvider as optional/experimental
```

Cloud API remains the intended primary production-quality AI route in this
design, but the current cloud route is an experimental Developer Tools preview,
not production-ready. Local GGUF-backed providers are optional and
unimplemented. A first local provider
should connect to an OpenAI-compatible local endpoint where possible, such as a
separately operated local inference server. Pastey should not directly load
GGUF files or fork llama.cpp at this stage.

Provider adapters may normalize:

- authentication handoff through a credential reference;
- API request and response envelopes;
- model identifiers;
- structured-output or tool-call formats;
- streaming events;
- usage and provider errors;
- timeouts and cancellation.

Provider adapters must not:

- authorize an action;
- execute a Pastey action;
- collect state directly from room, transfer, scheduler, diagnostics, or Tauri
  modules;
- convert a provider response into permission;
- silently weaken the requested schema or safety behavior.

## 6. Credential and Provider Safety

Provider credentials and provider output belong to different trust concerns.
Paying for or officially operating a provider does not make its generated
output authoritative.

- API keys must never be committed.
- API keys must not appear in logs, model context, crash reports, action plans,
  capability requests, or peer results.
- Production credentials should use the OS keychain or an equivalent secure
  credential store.
- Development-only environment variables may be allowed, but they must be
  clearly marked as development-only and must not be logged.
- `apiKeyRef` identifies a secure credential entry; it is not the secret value.
- Phase C does not implement `apiKeyRef` resolution or production credential
  storage. Its Developer Tools API-key input remains in runtime memory only.
- Provider and gateway privacy, retention, routing, and data handling must be
  visible to users.
- Gateway providers are not automatically trusted.
- A model response remains untrusted even when it comes from a paid or official
  provider.
- Provider errors, unavailable structured output, ambiguous responses, and
  timeouts must fail closed for consequential actions.
- Disabling or removing every AI provider must leave core Pastey behavior
  unaffected.

## 7. Context Boundary

`AiContextSnapshot` is an on-demand, current-session, purpose-built summary. It
must not serialize existing Pastey state objects directly.

Allowed summarized context may include:

- current room summary;
- peer summary;
- selected peer capability summary;
- transfer queue summary;
- scheduler summary;
- MicroFlowGroup mode summary;
- Device Diagnostics summary;
- latest bounded error/status summary.

```ts
interface AiContextSnapshot {
  schemaVersion: "ai-context-snapshot/v1";
  room?: RedactedRoomSummary;
  peers?: RedactedPeerSummary[];
  transferQueue?: RedactedTransferQueueSummary;
  scheduler?: RedactedSchedulerSummary;
  diagnostics?: RedactedDiagnosticsSummary;
  latestStatus?: RedactedStatusSummary;
  allowedActions: AiActionKind[];
}
```

Context must exclude by default:

- room keys and transport keys;
- auth tokens and API keys;
- raw logs and full transfer history;
- file contents;
- peer filesystem trees;
- arbitrary absolute paths and private local directories;
- raw Tauri command names;
- persistent behavioral history.

Room codes must also remain outside model context because they are join
credentials rather than explanatory metadata.

Cloud context should be stricter than local-model context. Cloud snapshots
should omit stable identifiers, coarsen names and metrics where possible, and
include only the sections required for the visible request. Even a local model
should not receive secrets, file contents, paths, or private logs
unnecessarily. Local routing is not an authority grant.

Snapshots should expire with the request or current UI session. References in a
plan must resolve to still-visible current-session objects before the plan can
proceed.

## 8. Action Plan Schema

The action plan is an advisory boundary between untrusted model output and
Pastey policy.

```ts
type AiActionKind =
  | "explain_status"
  | "summarize_room_state"
  | "summarize_diagnostics"
  | "explain_transfer_failure"
  | "suggest_retry"
  | "suggest_benchmark"
  | "suggest_transfer"
  | "draft_text_message"
  | "explain_microflowgroup_mode"
  | "request_peer_hello_demo";

interface AiActionPlan {
  schemaVersion: "ai-action-plan/v1";
  kind: AiActionKind;
  title: string;
  explanation: string;
  confidence: "low" | "medium" | "high";
  requiresUserConfirmation: boolean;
  references?: AiActionReference[];
  proposedInput?: Record<string, unknown>;
}
```

All action plans are advisory until validated, gated, and confirmed.
`request_peer_hello_demo` always requires local user confirmation.

The model may not include:

- raw shell command strings;
- arbitrary code or scripts;
- filesystem paths;
- peer filesystem search or file-read requests;
- hidden transfer instructions;
- scheduler, transfer-window, runtime-window, or MicroFlowGroup mutations;
- room create, join, leave, or burn operations in phase one.

Plans should use narrow opaque references to current visible state. A provider
must not invent Tauri commands, Rust functions, room IDs, peer IDs, transfer
IDs, or other internal references and expect Pastey to execute them.

## 9. PolicyGate

The local Pastey `PolicyGate` is separate from both schema validation and
confirmation.

Schema validation checks that a plan has the expected version, fields, types,
and bounded values. The `PolicyGate` checks whether the proposed action,
references, target, provider, context, and current session are permitted and
safe. User confirmation is not a substitute for sandboxing.

The `PolicyGate` must reject:

- an unknown schema version;
- an unknown action kind;
- missing confirmation for a consequential action;
- a raw shell string or command-like payload;
- a path-like payload where paths are not allowed;
- arbitrary code or script payloads;
- peer filesystem search;
- local or peer file-read requests;
- hidden transfer;
- scheduler, window, or MicroFlowGroup mutation;
- room key, transport key, token, API key, or secret exposure;
- unsupported provider-introduced fields;
- references to invisible, expired, mismatched, or stale objects;
- a target peer that is not in the current trusted room;
- a provider result that cannot be validated without guessing.

Confirmation must show the specific action, target peer, proposed input,
relevant risks, and constraints. It must not be a vague approval for future
unknown actions.

Pastey should deny by default when the action, target, context, provider
behavior, or policy result is uncertain.

## 10. Hello Peer Capability Demo

The minimum prototype safety decisions are defined in
[Hello Peer Safety Boundary](hello-peer-safety-boundary.md).

The complete conceptual flow is:

```text
1. User asks Pastey to have a peer output "hello peer!".
2. Pastey builds redacted context showing the trusted room and peer runtime
   capability summary.
3. Pastey calls the configured provider.
4. Provider returns AiActionPlan(kind="request_peer_hello_demo").
5. Pastey validates the action-plan schema.
6. Pastey PolicyGate checks the action, target, references, and constraints.
7. Local user confirms the specific request.
8. Pastey sends a capability request through the trusted room.
9. Peer validates the request and room/source.
10. Peer user confirms, or a separately designed demo-only grant applies.
11. Peer consumes the exact one-time consent and calls the fixed in-process
    Hello Peer template.
12. Peer returns a bounded structured result.
13. Local device displays the bounded result status outside chat.
```

This is not raw shell and not arbitrary code. The model does not provide the
command. The current device does not force a peer command. The peer maps a
declared capability plus an available runtime into an internal fixed template.
The peer can deny the request or fail closed at every validation boundary.

The first Hello Peer demo has no peer filesystem search, no user-file read, no
network request from the executor, no hidden transfer, and no binary-v2
requirement.

The phrase `hello peer!` is the only permitted message in the first template.
Making the message arbitrary would expand the capability and require a separate
review.

## 11. Capability Request and Result Shapes

Phase E0 implements the preview `HelloPeerRequest`. CL-6 adds the separate
host-owned execution request/result shapes below; the model cannot construct
them.

```ts
interface HelloPeerRequest {
  schemaVersion: "pastey-capability-request/v1";
  requestId: string;
  nonce: string;
  createdAt: string;
  expiresAt: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: "runtime.execute_hello_template";
  runtimePreference: Array<
    "python" | "node" | "sh" | "powershell" | "rust" | "unknown"
  >;
  input: {
    message: "hello peer!";
  };
  constraints: {
    templateOnly: true;
    noRawShell: true;
    filesystem: "none" | "temp-only";
    network: false;
    timeoutMs: number;
    maxStdoutBytes: number;
  };
  pendingPayloadHash: string;
  requestPayloadHash: string;
  transportStatus: "preview_only";
}

interface HelloPeerExecutionResult {
  schemaVersion: "pastey-hello-peer-execution-result/v1";
  executionId: string;
  requestId: string;
  consentId: string;
  status: "succeeded" | "rejected" | "expired" | "already_consumed" | "failed";
  output?: "hello peer!";
  errorCode?: string;
  createdAt: string;
}
```

The E0 request ID, nonce, expiry, and hashes are local preview-binding fields.
They prepare replay defenses but do not provide transport integrity by
themselves. CL-6 binds execution to an exact consent ID, preview event,
envelope, request/hash, room/session, source/target, capability, and message.
The result permits only fixed `hello peer!` success output or a bounded error
code. It has no stdout/stderr stream, exit code, runtime name, log, stack trace,
attachment, or authority to start another action.

## 12. Peer Runtime Capability

The peer capability declaration is also conceptual:

```ts
interface PeerRuntimeCapability {
  capability: "runtime.execute_hello_template";
  availableRuntimes: Array<"python" | "node" | "sh" | "powershell" | "rust">;
  requiresUserConfirmation: true;
  supportsRawShell: false;
  supportsArbitraryCode: false;
  filesystemAccess: "none" | "temp-only";
  networkAccess: false;
}
```

The declaration must be current-session and scoped to a current trusted room.
It should not imply broad device profiling or become hidden long-term history.
Runtime availability is informational until the peer validates the request and
confirms execution.

The peer must be able to deny. An advertised runtime is not an execution grant,
and a local model recommendation cannot override peer policy.

Current Pastey `DeviceCapabilities` can report a bounded runtime-probe summary,
but it is not currently exchanged as a peer capability grant and must not be
treated as one.

## 13. Peer PolicyGate and Restricted Executor

The peer has an independent `PolicyGate`. Local approval cannot replace peer
validation or peer consent. CL-5 implements the receiver PolicyGate and
explicit one-time consent for the exact fixed Hello Peer preview. CL-6
implements only the restricted executor described below.

The peer `PolicyGate` rejects:

- an unknown capability or schema version;
- an untrusted room or source;
- a target mismatch;
- an unavailable runtime;
- a raw shell request;
- arbitrary code;
- file reads or writes outside the declared constraints;
- a network request;
- an invalid timeout or output cap;
- missing peer confirmation;
- a stale, duplicate, or replayed request ID;
- a malformed or unsupported request.

The implemented restricted executor:

- executes a fixed internal template only;
- is a zero-argument in-process host function;
- returns exactly `hello peer!`;
- enforces a one-second timeout check and 64-byte output cap;
- returns a bounded typed result;
- does not expose a shell;
- does not search the filesystem;
- does not read user files;
- does not use network access;
- does not start a process, capture stdout/stderr, or persist hidden execution
  history.

## 14. Existing Pastey Mapping

The table below maps the design to current implementation found in the
repository. It does not grant AI or capability code access to these surfaces.
Source code remains authoritative.

| Area | Current files, functions, and types found | Design relationship |
| --- | --- | --- |
| Room create | `src/pages/RoomsPage.tsx::handleCreateRoom`; `src/lib/tauri.ts::createRoom`; `src-tauri/src/commands.rs::create_room`; `storage::create_room`; `transfer::start_room_server` | Existing room lifecycle remains Pastey-owned and outside AI authority. |
| Room join | `src/pages/DevicesPage.tsx::handleJoinRoom`; `src/lib/tauri.ts::joinRoom`; `commands::join_room`; `discovery::discover_room`; `transfer::announce_join`; `storage::update_room_peer` | Future capability requests must target a separately validated current trusted-room peer. AI must not join rooms. |
| Nearby join approval | `src/lib/tauri.ts::{requestNearbyJoin,acceptNearbyJoin,rejectNearbyJoin,pendingJoinRequests}`; matching commands and `discovery.rs` state | Shows an existing approval-oriented peer interaction pattern, but it is not a capability-execution permission. |
| Active room and peer state | `src/App.tsx` state `rooms`, `currentRoom`, `roomItems`; `src/lib/types.ts::{RoomInfo,NearbyDevice}`; Rust `models::{RoomInfo,StoredRoom}`; `AppState::{active_servers,nearby_devices}`; `ActiveRoomServer` | A future context builder may derive a redacted current-room/peer summary. Stored host, port, room code, transport keys, and device IDs stay excluded. |
| Room leave/burn | `commands::leave_room`; `transfer::{notify_room_leave,cancel_room_transfers,stop_room_server}`; `storage::leave_room`; `RoomPage.tsx::handleBurnRoom`; `App.tsx::handleBurnRoom`; `src/lib/tauri.ts::burnRoom`; `commands::burn_room`; `storage::burn_room` | Leave is internal legacy cleanup and burn is the product terminal action. Neither is an AI action in this design. |
| Text send | `RoomPage.tsx::handleSendText`; `src/lib/tauri.ts::sendTextToRoom`; `commands::send_text_to_room`; `storage::create_outgoing_text_item`; `transfer::send_room_item` | Confirmed future text drafts must use the existing composer/send path. Current text items are user text, not a capability-request protocol. |
| File queue entry | `RoomPage.tsx::{handlePickFile,handleComposerPaste}` plus webview drop handling; `App.tsx::{enqueueRoomFiles,enqueueRoomTransferInputs}`; `transferScheduler::enqueueTransferBatch` | Future AI may summarize path-free queue state or suggest a visible transfer. No hidden queue insertion is allowed. |
| File send | `App.tsx::processTransferQueueItem`; `src/lib/tauri.ts::sendFileToRoom`; `commands::send_file_to_room`; `storage::create_outgoing_file_item_with_metadata`; `transfer::send_room_file` | Existing confirmed file actions continue through this authoritative single-file path. Hello Peer is not a disguised file transfer. |
| Transfer queue | `src/lib/transferScheduler.ts::{TransferQueueInput,TransferQueueItem,TransferQueueBatch,TransferSchedulerState,RoomTransferQueueView,TransferQueueSummary,enqueueTransferBatch,selectRoomTransferQueue}` | A future snapshot may derive aggregate path-free state. Raw queue items contain absolute paths and must not enter provider context. |
| Planner and requested window | `src/lib/transferPlanner.ts::{TransferPlannerTask,TransferPlannerPolicy,TransferPlannerResult,planWeightedTransfers,DEFAULT_TRANSFER_PLANNER_POLICY}`; `transferScheduler::{planRunnableTransferLaunches,planActiveTransferWindowRebalances}`; `SendFileOptions.requestedWindow`; `updateTransferWindow`; Rust `update_transfer_window` and transfer-tuning helpers | Advisory summaries only. AI and capability requests must not change planner policy or windows. |
| MicroFlowGroup | `transferPlanner::{MicroFlowGroupMode,MicroFlowGroupPlan}`; `transferScheduler::{MicroFlowGroupRuntimeState,MicroFlowGroupStatus,markMicroFlowGroupQueued,markMicroFlowGroupRunning,recordMicroFlowGroupChildTerminal,completeMicroFlowGroupFromChildren,finishMicroFlowGroup}`; `App.tsx::processMicroFlowGroup` | Scheduler/resource abstraction only. It is not a capability group, protocol object, remote-execution object, or permission grant. |
| Transfer status/errors | `src/lib/types.ts::{FileTransferProgressEvent,TransferStatus,RoomItemStatus}`; `src/lib/transferState.ts::{mergeTransferEvent,isTerminalTransferStatus}`; queue terminal helpers and `RoomPage.tsx` rendering | A future snapshot may include a bounded current visible status/error summary. AI must not mutate status or terminal behavior. |
| Device Diagnostics | Rust `src-tauri/src/diagnostics.rs::{DeviceProfile,DeviceCapabilities,RuntimeCapability,LinkBenchmarkResult}`; `device_profile::local_device_profile_with_mode`; `capability_probe::probe_device_capabilities_with_mode`; commands `get_device_profile`, `get_device_capabilities`; frontend mirrors in `src/lib/types.ts` | A future context builder may derive redacted summaries. Current diagnostic data is advisory, not permission. |
| Runtime probing | `src-tauri/src/capability_probe.rs` fixed runtime probe list and `probe_runtime`; full probe includes Python, Node, Cargo, PowerShell, zsh, and bash where applicable; quick mode skips runtime commands | Potential informational input for future peer capability declaration. It does not currently authorize or execute Hello Peer. |
| Link benchmarks | `src/lib/tauri.ts::{runLoopbackBenchmark,runPeerLinkBenchmark,getLastBenchmarkResults}`; commands `run_loopback_benchmark`, `run_peer_link_benchmark`, `get_last_benchmark_results`; `link_benchmark::{run_loopback_benchmark,run_peer_link_benchmark}`; `AppState::latest_benchmark_results` | Latest results are current-session summaries. Benchmarks remain separate from capability execution and require visible user action. |
| Settings and Developer Tools | `src/pages/SettingsPage.tsx`; `AgentBridgeSettings.tsx`; `src/lib/agentBridge/config.ts`; `AppConfig` fields; `commands::update_config` | Settings retains only Agent Bridge enablement, runtime-memory provider configuration/API key, lifecycle log level, log clearing, and safety summary. It does not require an active room or mount workflow controls. |
| Frontend diagnostics bridge | `App.tsx::emitPasteyDiagnostic`; `src/lib/agentBridge/logging.ts`; `src/lib/tauri.ts::logFrontendDiagnostic`; `commands::{log_frontend_diagnostic,normalize_frontend_diagnostic_line}`; Rust `logging.rs` | Existing bridge is bounded to known prefixes and rejects path-like values. Agent Bridge writes allowlisted structured JSON with shortened references through `[pastey:agent-bridge]`; logs are redacted audit mirrors only, never runtime state or authorization evidence. |
| Existing agent-bridge docs | `docs/agent-bridge/{ai-slot.md,current-capability-map.md,provider-model.md,context-boundary.md,action-plan-schema.md,non-goals.md,ai-slot-v0-implementation-notes.md}` | Current documentation staging area for the implemented mock slice and future design boundaries. |
| Active Room Agent Bridge workflow | `RoomPage.tsx`; `src/components/AiSlotPreview.tsx`; `RoomControlPanel.tsx`; `src/lib/ai/{types.ts,contextSnapshot.ts,mockProvider.ts,cloudOpenAICompatibleProvider.ts,actionPlanValidator.ts,policyGate.ts,pendingAction.ts,helloPeerRequest.ts,capabilityPreviewEnvelope.ts,index.ts}`; `tests/aiSlot.test.ts`; `scripts/run-ai-slot-tests.mjs` | The exact active room/session/peer owns advisory, explicit confirmation, validated preview, queue, consent review, and bounded Hello Peer execution presentation. No route has generic execution, ordinary room-item dispatch, or reusable trust authority. |
| Control Lane CL-1 type foundation | `src/lib/agentBridge/{roomControlEvent.ts,index.ts}`; `tests/roomControlEvent.test.ts`; `scripts/run-room-control-event-tests.mjs` | Implements typed preview-only `RoomControlEvent` wrappers, builders, deny-first validation, current-session duplicate detection, and pure `computeControlLaneBudget`. It has no room-control transport, Tauri invoke, send/receive, scheduler wiring, transfer behavior, or execution authority. |
| Capability request/result room transport | `src/lib/agentBridge/{roomControlEvent,controlQueue,roomControlTransport}.ts`; `src-tauri/src/room_control.rs`; `RoomControlPanel.tsx` | Closed typed preview/status/execution request/result events use the separate bounded room-control path. Ordinary text/file paths are not reinterpreted. |
| Peer policy gate/restricted executor | `src/lib/agentBridge/{peerConsent,helloPeerExecution}.ts`; `RoomControlPanel.tsx`; `tests/{peerConsent,helloPeerExecution}.test.ts` | Exact one-time receiver consent is revalidated and consumed before one fixed in-process Hello Peer function. No generic runtime or reusable trust. |
| Pastey MCP/local tool server | Not found in current source search. | Future optional external-agent path only. |

Additional detailed mappings are maintained in
[Current Capability Map](current-capability-map.md). The canonical transfer
boundaries remain documented in
[Transfer Architecture](../transfer/architecture.md) and
[Transfer Scheduler](../transfer/scheduler.md).

## 15. Implementation Phases

These phases describe a possible design sequence. They do not authorize
implementation or establish a final product roadmap.

### Phase A: Documentation and Skeleton Types Only

- finalize design documents;
- define conceptual type names and ownership boundaries;
- make no runtime behavior changes.

### Phase B: MockProvider Advisory Loop - Current v0

- uses a deterministic mock provider returning a safe Hello Peer advisory plan;
- tests local schema validation and policy decisions without network access;
- performs no peer execution.

### Phase C: Cloud Provider Advisory Loop - Current

- calls a configured OpenAI-compatible chat-completions endpoint;
- sends a strict whitelisted synthetic context only;
- accepts JSON action-plan output only and does not repair invalid JSON;
- shows validation and PolicyGate results in the active Room only;
- keeps API-key input in runtime memory and has no production credential store.

### Phase D: Local Confirmation UI - Current

- shows the exact pending action, target, input, constraints, expiry, and
  payload hash;
- requires specific local approval and records `confirmed_local_only`,
  `cancelled`, or `expired`;
- permits no hidden execution, dispatch, room message, or peer request;
- prepares local request binding but does not implement transport replay
  prevention or peer consent.

### Phase E0: Hello Peer Request Builder and Outbound Preview - Current

- converts only `confirmed_local_only` into a canonical `HelloPeerRequest`;
- validates exact fixed capability, message, constraints, expiry, and hashes;
- renders `transportStatus: "preview_only"` in the active Room;
- sends no request and provides no peer receive path, peer consent, replay
  cache, or execution.

### Phase E1: Capability Request Transport Preview - Current, Transport Blocked

- builds and validates `CapabilityRequestPreviewEnvelope`;
- checks duplicate envelope and request IDs in current-session memory only;
- renders a local inbound-preview simulation with acknowledge/deny preview
  states;
- does not send through `sendTextToRoom`, because ordinary user text is not a
  safe capability-preview transport;
- has no peer receive path, execution consent, executor, stdout, stderr, or
  exit code.

### Control Lane CL-1: Type-Only Room Control Events - Current

- wraps a validated `CapabilityRequestPreviewEnvelope` in a typed preview-only
  `RoomControlEvent`;
- defines bounded acknowledge, deny, invalid, and expired preview status
  events;
- validates exact shapes, room/source/target bindings, expiry, embedded
  preview envelopes, and forbidden execution-like fields;
- detects duplicate event, envelope, and request IDs in current-session memory
  only;
- provides pure `computeControlLaneBudget` feasibility output: data `8` /
  control `0` without backlog, data `7` / control `1` with backlog;
- is not wired into the scheduler and sends or receives no room-control event;
- adds no transport, persistence, peer execution, or transfer behavior.

### Phase E: Hello Peer Transport/Request Prototype - Future

- send a capability request over a trusted room or a deliberately designed
  current-room messaging path;
- retain a no binary-v2 requirement for the first demo;
- expose no raw shell.

This phase must decide and document the transport boundary explicitly. It must
not silently reinterpret existing user text or file-transfer payloads as
commands.

### Phase F: Peer Policy and Restricted Executor

- CL-5 peer validation and explicit one-time Allow once/Deny: implemented;
- fixed hello-template execution: not implemented;
- bounded structured execution result: not implemented.

### Phase G: Hardening

- verify fail-closed behavior;
- add visible logging/audit surfaces;
- make provider privacy and context disclosure visible;
- review permissions and replay controls;
- define cross-platform executor constraints.

## 16. Risks and Open Questions

### Risks

- Cloud providers may retain, route, or expose context differently.
- Gateway providers add another trust and privacy boundary.
- Prompt injection may enter through user requests, room text, tool metadata, or
  future external-agent output.
- Peer capability declarations may become stale.
- Frequent or vague prompts may cause approval fatigue.
- Local confirmation may be mistaken for peer consent.
- Executor isolation differs across operating systems.
- Results and audit logs may disclose peer paths, commands, environment, or
  private status.
- A trusted room may not be a sufficient threat model for capability execution.
- Future MCP tool exposure may be confused with authorization.
- Product pressure may expand the fixed demo into raw shell too early.

### Open Questions

- Which providers are acceptable first?
- Where should API keys and provider configuration be stored?
- What exact cloud context is allowed for each action kind?
- Should peer confirmation be required for every Hello Peer request?
- Can a demo-only grant exist, and how long may it last?
- What OS-level executor isolation is required on each supported platform?
- How should current-session peer capabilities be discovered, signed, refreshed,
  and expired?
- Is binary-v2 useful later for explicit control frames, despite no binary-v2
  requirement for the first demo?
- What replay protection and request-ID lifetime are required?
- What visible audit trail is compatible with Pastey's low-trace and
  no-hidden-history principle?
- How should provider errors, capability denials, and peer result errors be
  shown without leaking private details?
- What exact transport boundary can carry capability requests without changing
  or confusing existing text and file semantics?

## 17. Non-goals

- no runtime code in this task;
- no AI execution;
- no raw shell;
- no arbitrary code execution;
- no peer filesystem search;
- no file-content reading;
- no automatic transfer;
- no hidden transfer;
- no scheduler or MicroFlowGroup mutation;
- no binary-v2 dependency for the first demo;
- no local GGUF direct loading;
- no llama.cpp fork;
- no bundled inference runtime;
- no persistent AI memory;
- no long-term device profiling;
- no new room, transfer, ACK, finalize, cancel, burn, or Inbox semantics;
- no claim that the current room text or file path is already a capability
  transport;
- no claim that current runtime probing is a peer execution grant.

## 18. Acceptance Criteria for the Design Document

This document is complete when it clearly explains:

- how paid API providers could fit behind a Pastey-owned adapter;
- how provider output becomes only an advisory `AiActionPlan`;
- how schema validation, policy, and confirmation separate model output from
  execution;
- how the Hello Peer demo could validate a narrow capability-bridge concept;
- how peer-side validation and a restricted executor prevent remote-shell
  semantics;
- how the design maps to actual current Pastey files, functions, and types;
- which required AI, capability, transport, peer-policy, and executor surfaces
  are not currently implemented;
- what remains future work.

The acceptance boundary remains Pastey's core principle: AI-ready, not
AI-dependent. Removing the entire AI Slot or every configured provider must not
reduce the completeness, security, or performance of existing Pastey features.

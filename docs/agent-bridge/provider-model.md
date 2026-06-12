# AI Provider Model

This document records the provider design boundary. AI Slot Phase E1 preserves
the Phase C
`MockProvider` and an experimental `CloudOpenAICompatibleProvider` under
`src/lib/ai/`, then permits accepted plans to enter a local-only pending
confirmation state. Anthropic, Gemini, gateway-specific, local, GGUF, and
bundled runtime providers remain future work.

## Provider Routes

### Local GGUF-Backed Models

Pastey should not directly depend on GGUF as a file format in the first AI
phase. GGUF models are normally driven by an inference runtime or server, such
as:

- a local llama.cpp server;
- Ollama's local API;
- LM Studio's local server;
- a future bundled sidecar, which is deferred.

The first local adapter should target a local HTTP provider. Prefer an
OpenAI-compatible local API where the selected runtime supports it. Runtime-
specific adapters may translate the common request/result shapes, but the rest
of Pastey should not know whether the model originated from a GGUF file.

Local does not mean trusted. Local provider output still requires schema
validation and the policy gate, and local context remains minimized.

### Cloud API Models

The implemented `CloudOpenAICompatibleProvider` uses an OpenAI-compatible
chat-completions HTTP endpoint behind the same `AiProvider` interface. Cloud
context is rebuilt through a strict whitelist before the request.

Pastey must not upload secrets, room keys, auth tokens, full logs, absolute
paths, file contents, full transfer history, or peer filesystem state by
default. Cloud output remains untrusted and passes through the same
`validateAiActionPlan` and `evaluateAiPolicy` path as mock output.

The Developer Tools preview accepts base URL, model, and an optional API key in
runtime memory only. It does not persist provider configuration or credentials.
Production key storage is not implemented.

### Mock Provider

The deterministic `MockProvider` remains the default preview route. It allows
context redaction, schema validation, policy gating, and advisory UI to be
tested without a model runtime, network, API key, or provider-specific
behavior.

## Unified Shapes

```ts
type AiProviderKind =
  | "mock"
  | "cloud_openai_compatible"
  | "openai"
  | "anthropic"
  | "gemini"
  | "gateway"
  | "local_openai_compatible";

interface AiProvider {
  readonly config: AiProviderConfig;
  generate(request: AiGenerateRequest): Promise<AiGenerateResult>;
}

interface CloudOpenAICompatibleProviderConfig extends AiProviderConfig {
  kind: "cloud_openai_compatible";
  apiShape: "openai_compatible_chat";
  baseUrl: string;
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

interface AiContextSnapshot {
  schemaVersion: "ai-context-snapshot/v1";
  capturedAt: number;
  room?: AiRoomSummary;
  peer?: AiPeerSummary;
  transferQueue?: AiTransferQueueSummary;
  selectedFiles?: AiSelectedFileSummary[];
  scheduler?: AiSchedulerSummary;
  diagnostics?: AiDiagnosticsSummary;
  latestBenchmark?: AiBenchmarkSummary;
  currentStatus?: AiStatusSummary;
}

interface AiContextPolicy {
  providerKind: AiProviderKind;
  allowedSections: Array<keyof AiContextSnapshot>;
  maxSelectedFiles: number;
  maxErrorCharacters: number;
  allowDeviceIdentifiers: false;
  allowAbsolutePaths: false;
  allowFileContents: false;
  allowRawLogs: false;
  allowSecrets: false;
  allowPersistentHistory: false;
}

interface AiActionPlan {
  schemaVersion: "ai-action-plan/v1";
  kind:
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
  title: string;
  explanation: string;
  confidence?: "low" | "medium" | "high";
  requiresUserConfirmation: boolean;
  references?: AiActionReference[];
  proposedInput?: Record<string, unknown>;
}

interface AiPolicyGate {
  evaluate(input: {
    plan: AiActionPlan;
    contextPolicy: AiContextPolicy;
    currentSession: AiPolicySessionSummary;
  }): AiPolicyDecision;
}

type AiPolicyDecision =
  | { allowed: true; advisoryOnly: true; requiresUserConfirmation: boolean }
  | { allowed: false; reason: string };
```

The current preview uses a synthetic `AiContextSnapshot` and rebuilds a
cloud-safe copy through `buildCloudSafeAiContextSnapshot`. It does not reuse
`RoomInfo`, `RoomItem`, `TransferSchedulerState`, `DeviceProfile`, or
`LinkBenchmarkResult` directly.

## Adapter Responsibilities

Every provider adapter should:

1. accept only an already-redacted `AiContextSnapshot`;
2. request only the allowed action-plan schema;
3. enforce timeouts and cancellation;
4. return provider output as untrusted `unknown`;
5. avoid direct imports from room, transfer, scheduler, or Tauri command
   modules;
6. avoid storing prompts, responses, or context unless a future explicit
   retention policy is designed.

The schema validator and `AiPolicyGate` must live outside provider adapters so
all local, cloud, and mock providers receive the same enforcement.

## Implementation Boundaries

The current implementation keeps separate modules for:

- provider interfaces and adapters;
- context snapshot construction and redaction;
- action-plan schema parsing;
- policy-gate evaluation;
- local pending-action confirmation UI;
- local `HelloPeerRequest` build/validation and outbound preview.

Local confirmation has no action-dispatch authority. Provider adapters do not
create pending actions directly and cannot bypass validation or the
`PolicyGate`. They also do not build or send capability requests. The Phase E0
request preview is produced locally after confirmation and has
`transportStatus: "preview_only"`. Phase E1 envelope and inbound-preview state
are also local-only and remain outside provider authority.

It should not place provider calls inside `App.tsx`, `RoomPage.tsx`,
`transferScheduler.ts`, `transferPlanner.ts`, or Rust transfer hot-path code.
Those modules may supply minimized summaries or receive an explicitly confirmed
existing UI action, but they must not become AI-dependent.

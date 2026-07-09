export type AiProviderKind =
  | "mock"
  | "cloud_openai_compatible"
  | "openai"
  | "anthropic"
  | "gemini"
  | "gateway"
  | "local_openai_compatible";

export type AiApiShape =
  | "openai_responses"
  | "openai_chat_completions"
  | "anthropic_messages"
  | "gemini_generate_content"
  | "openai_compatible_chat";

export type AiActionKind =
  | "explain_status"
  | "summarize_room_state"
  | "summarize_diagnostics"
  | "explain_transfer_failure"
  | "suggest_retry"
  | "suggest_benchmark"
  | "suggest_transfer"
  | "draft_text_message"
  | "explain_microflowgroup_mode"
  | "request_peer_hello_demo"
  | "request_peer_hello_stdout_demo"
  | "request_peer_file_candidates"
  | "request_peer_candidate_payload";

export type AiConfidence = "low" | "medium" | "high";

export interface AiProviderConfig {
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

export interface CloudOpenAICompatibleProviderConfig extends AiProviderConfig {
  kind: "cloud_openai_compatible";
  apiShape: "openai_compatible_chat";
  baseUrl: string;
}

export interface AiProvider {
  readonly config: AiProviderConfig;
  generate(request: AiGenerateRequest): Promise<AiGenerateResult>;
}

export interface AiContextPolicy {
  allowCloudContext: boolean;
  includeRawLogs: false;
  includeFileContents: false;
  includeAbsolutePaths: false;
  includeSecrets: false;
}

export interface AiContextSnapshot {
  schemaVersion: "ai-context-snapshot-v1";
  generatedAt: string;
  room?: {
    hasActiveRoom: boolean;
    trustedRoom: boolean;
    peerCount: number;
  };
  peers?: Array<{
    peerRef: string;
    visible: boolean;
    trusted: boolean;
    capabilities?: string[];
  }>;
  scheduler?: {
    microFlowGroupMode?: "fixed" | "dynamic" | "unknown";
  };
  diagnostics?: {
    available: boolean;
    summary?: string;
  };
  latestStatus?: {
    level: "info" | "warning" | "error";
    message: string;
  };
  allowedActions: AiActionKind[];
}

export interface AiActionReference {
  kind: "room" | "peer" | "transfer" | "diagnostic" | "scheduler";
  ref: string;
}

export interface AiActionPlan {
  schemaVersion: "ai-action-plan-v1";
  kind: AiActionKind;
  title: string;
  explanation: string;
  confidence: AiConfidence;
  requiresUserConfirmation: boolean;
  references?: AiActionReference[];
  proposedInput?: Record<string, unknown>;
}

export interface AiGenerateRequest {
  requestId: string;
  providerId: string;
  context: AiContextSnapshot;
  contextPolicy: AiContextPolicy;
  allowedActionKinds: AiActionKind[];
  outputSchema: "ai-action-plan-v1" | "ask-bridge-natural-v1";
  userRequest: string;
}

export interface AiGenerateResult {
  requestId: string;
  providerId: string;
  model: string;
  rawText?: string;
  parsedPlan?: unknown;
  usage?: {
    inputTokens?: number;
    outputTokens?: number;
  };
  error?: {
    code: string;
    message: string;
  };
}

export type ValidationResult<T> =
  | { valid: true; value: T; errors: [] }
  | { valid: false; errors: string[] };

export interface AiPolicyResult {
  status: "accepted" | "rejected";
  requiresUserConfirmation: boolean;
  reasons: string[];
  warnings: string[];
}

export type PendingAiActionStatus =
  | "pending"
  | "confirmed_local_only"
  | "cancelled"
  | "expired";

export interface PendingAiActionCanonicalPayload {
  schemaVersion: "ai-action-plan-v1";
  kind: AiActionKind;
  targetPeerRef: string;
  capability: string;
  message: string;
  constraints: Record<string, unknown>;
  query?: Record<string, unknown>;
  scopePolicy?: Record<string, unknown>;
  limits?: Record<string, unknown>;
  safety?: Record<string, unknown>;
  candidate?: Record<string, unknown>;
  references: AiActionReference[];
  pendingId: string;
  expiresAt: string;
}

export interface PendingAiAction {
  pendingId: string;
  actionPlan: AiActionPlan;
  policyResult: AiPolicyResult;
  createdAt: string;
  expiresAt: string;
  canonicalPayload: PendingAiActionCanonicalPayload;
  payloadHash: string;
  status: PendingAiActionStatus;
}

import { buildCloudSafeAiContextSnapshot } from "./contextSnapshot";
import { NATURAL_V1_PROVIDER_INSTRUCTIONS } from "./providerInstructionPack";
import type {
  AiGenerateRequest,
  AiGenerateResult,
  AiProvider,
  CloudOpenAICompatibleProviderConfig
} from "./types";

export type FetchLike = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

interface CloudProviderOptions {
  apiKey?: string;
  fetchImpl?: FetchLike;
}

interface OpenAICompatibleResponse {
  choices?: Array<{
    message?: {
      content?: unknown;
    };
  }>;
  usage?: {
    prompt_tokens?: unknown;
    completion_tokens?: unknown;
  };
}

export interface OpenAICompatibleChatRequestBody {
  model: string;
  messages: Array<{
    role: "system" | "user";
    content: string;
  }>;
  response_format: {
    type: "json_object";
  };
  temperature: 0;
  max_tokens: number;
}

const ADVISORY_SYSTEM_PROMPT = `You are the advisory-only Pastey AI Slot.
Return only one JSON object conforming to ai-action-plan-v1.
The object may contain only schemaVersion, kind, title, explanation, confidence, requiresUserConfirmation, references, and proposedInput.
schemaVersion must be ai-action-plan-v1. confidence must be low, medium, or high.
Model output is an untrusted proposal and never grants execution permission.
Allowed action kinds are supplied by the host.
Do not include shell commands, arbitrary code, scripts, paths, secrets, raw logs, file contents, peer filesystem search, hidden transfer, scheduler mutation, or MicroFlowGroup mutation.
For legacy Hello Peer, only propose request_peer_hello_demo with capability runtime.execute_hello_template, exact message hello peer!, requiresUserConfirmation true, and constraints templateOnly true, noRawShell true, filesystem none, network false, finite timeoutMs, and finite maxStdoutBytes.
For Hello Stdout, only propose request_peer_hello_stdout_demo with capability runtime.hello_stdout, exact message hello peer, requiresUserConfirmation true, and constraints templateOnly true, noRawShell true, filesystem none, network false, timeoutMs 1000, maxStdoutBytes 64, and maxStderrBytes 256.
For file candidate discovery, only propose request_peer_file_candidates with capability filesystem.find_file_candidates, one targetPeerRef, searchMode filename_metadata_only, allowedScopes limited to downloads desktop documents pastey_shared, allowFullDisk false, includeFileContents false, includeAbsolutePaths false, includeHiddenFiles false, noAutoTransfer true, requireReceiverConsent true, selectedPeerOnly true, maxCandidates 1-20, maxSearchMs 500-10000, and maxDepth 1-8.
For candidate payload requests, only propose request_peer_candidate_payload with capability transfer.request_candidate_payload, one targetPeerRef, sourceCapability filesystem.find_file_candidates, sourceRequestId, opaque candidateId, candidateDisplayName, candidateKind filesystem_file, and optional display metadata only. Do not include paths, contents, transfer queue ids, handoff ids, auto-send, selected-peers, or broadcast.
Do not include fields outside the action-plan schema.`;

export class CloudOpenAICompatibleProvider implements AiProvider {
  readonly config: CloudOpenAICompatibleProviderConfig;
  private readonly apiKey?: string;
  private readonly fetchImpl: FetchLike;

  constructor(config: CloudOpenAICompatibleProviderConfig, options: CloudProviderOptions = {}) {
    this.config = config;
    this.apiKey = options.apiKey?.trim() || undefined;
    this.fetchImpl = options.fetchImpl ?? fetch;
  }

  async generate(request: AiGenerateRequest): Promise<AiGenerateResult> {
    if (!request.contextPolicy.allowCloudContext) {
      return this.errorResult(request, "cloud_context_not_allowed", "Cloud context policy does not allow this request.");
    }

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.config.timeoutMs);
    try {
      const response = await this.fetchImpl(resolveChatCompletionsUrl(this.config.baseUrl), {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(this.apiKey ? { Authorization: `Bearer ${this.apiKey}` } : {})
        },
        body: JSON.stringify(buildOpenAICompatibleChatRequest(this.config, request)),
        signal: controller.signal
      });

      if (!response.ok) {
        return this.errorResult(request, "provider_http_error", `Provider request failed with HTTP ${response.status}.`);
      }

      const responseBody = await response.json() as OpenAICompatibleResponse;
      const content = responseBody.choices?.[0]?.message?.content;
      if (typeof content !== "string" || content.trim().length === 0) {
        return this.errorResult(request, "provider_response_invalid", "Provider response did not contain text content.");
      }

      try {
        return {
          requestId: request.requestId,
          providerId: this.config.providerId,
          model: this.config.model,
          rawText: content,
          parsedPlan: JSON.parse(content),
          usage: {
            inputTokens: finiteNumber(responseBody.usage?.prompt_tokens),
            outputTokens: finiteNumber(responseBody.usage?.completion_tokens)
          }
        };
      } catch {
        return {
          requestId: request.requestId,
          providerId: this.config.providerId,
          model: this.config.model,
          rawText: content,
          error: {
            code: "provider_json_parse_failed",
            message: "Provider output was not valid JSON. No repair or action was attempted."
          }
        };
      }
    } catch (error) {
      const message = error instanceof Error && error.name === "AbortError"
        ? "Provider request timed out."
        : "Provider request failed before a valid advisory response was received.";
      return this.errorResult(request, "provider_request_failed", message);
    } finally {
      clearTimeout(timeout);
    }
  }

  private errorResult(request: AiGenerateRequest, code: string, message: string): AiGenerateResult {
    return {
      requestId: request.requestId,
      providerId: this.config.providerId,
      model: this.config.model,
      error: { code, message }
    };
  }
}

export function buildOpenAICompatibleChatRequest(
  config: CloudOpenAICompatibleProviderConfig,
  request: AiGenerateRequest
): OpenAICompatibleChatRequestBody {
  const cloudContext = buildCloudSafeAiContextSnapshot(request.context);
  return {
    model: config.model,
    messages: [
      {
        role: "system",
        content: request.outputSchema === "ask-bridge-natural-v1"
          ? NATURAL_V1_PROVIDER_INSTRUCTIONS
          : ADVISORY_SYSTEM_PROMPT
      },
      {
        role: "user",
        content: JSON.stringify({
          userRequest: request.userRequest,
          contextPolicy: {
            allowCloudContext: request.contextPolicy.allowCloudContext,
            includeRawLogs: false,
            includeFileContents: false,
            includeAbsolutePaths: false,
            includeSecrets: false
          },
          context: cloudContext,
          allowedActionKinds: [...request.allowedActionKinds],
          outputSchema: request.outputSchema
        })
      }
    ],
    response_format: {
      type: "json_object"
    },
    temperature: 0,
    max_tokens: config.maxOutputTokens
  };
}

export function resolveChatCompletionsUrl(baseUrl: string): string {
  const trimmed = baseUrl.trim().replace(/\/+$/, "");
  return trimmed.endsWith("/chat/completions") ? trimmed : `${trimmed}/chat/completions`;
}

function finiteNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

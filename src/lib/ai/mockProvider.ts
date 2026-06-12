import type { AiActionPlan, AiGenerateRequest, AiGenerateResult, AiProvider, AiProviderConfig } from "./types";

export const MOCK_AI_PROVIDER_CONFIG: AiProviderConfig = {
  providerId: "pastey-mock-provider",
  displayName: "Pastey MockProvider",
  kind: "mock",
  apiShape: "openai_compatible_chat",
  model: "pastey-safe-advisory-v0",
  timeoutMs: 1_000,
  maxOutputTokens: 512,
  enabled: true
};

export function buildMockHelloPeerPlan(): AiActionPlan {
  return {
    schemaVersion: "ai-action-plan/v1",
    kind: "request_peer_hello_demo",
    title: "Ask peer to run Hello Peer demo",
    explanation: "The peer advertises a restricted hello-template capability. This plan asks Pastey to request that peer to output exactly 'hello peer!' through a fixed template after local and peer confirmation.",
    confidence: "high",
    requiresUserConfirmation: true,
    references: [
      { kind: "peer", ref: "mock-peer-1" }
    ],
    proposedInput: {
      targetPeerRef: "mock-peer-1",
      capability: "runtime.execute_hello_template",
      message: "hello peer!",
      constraints: {
        templateOnly: true,
        noRawShell: true,
        filesystem: "none",
        network: false,
        timeoutMs: 3_000,
        maxStdoutBytes: 4_096
      }
    }
  };
}

export class MockProvider implements AiProvider {
  readonly config = MOCK_AI_PROVIDER_CONFIG;

  async generate(request: AiGenerateRequest): Promise<AiGenerateResult> {
    return {
      requestId: request.requestId,
      providerId: this.config.providerId,
      model: this.config.model,
      rawText: "Mock advisory plan generated locally. No model or network call occurred.",
      parsedPlan: buildMockHelloPeerPlan(),
      usage: {
        inputTokens: 0,
        outputTokens: 0
      }
    };
  }
}

export const mockProvider = new MockProvider();

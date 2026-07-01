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
    schemaVersion: "ai-action-plan-v1",
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

export function buildMockHelloStdoutPlan(): AiActionPlan {
  return {
    schemaVersion: "ai-action-plan-v1",
    kind: "request_peer_hello_stdout_demo",
    title: "Ask peer to run Hello Stdout demo",
    explanation: "The peer advertises a restricted hello-stdout capability. This plan asks Pastey to request that peer to run a host-owned Rust helper after local and peer confirmation.",
    confidence: "high",
    requiresUserConfirmation: true,
    references: [
      { kind: "peer", ref: "mock-peer-1" }
    ],
    proposedInput: {
      targetPeerRef: "mock-peer-1",
      capability: "runtime.hello_stdout",
      message: "hello peer",
      constraints: {
        templateOnly: true,
        noRawShell: true,
        filesystem: "none",
        network: false,
        timeoutMs: 1_000,
        maxStdoutBytes: 64,
        maxStderrBytes: 256
      }
    }
  };
}

export function buildMockFileCandidatePlan(): AiActionPlan {
  return {
    schemaVersion: "ai-action-plan-v1",
    kind: "request_peer_file_candidates",
    title: "Find file candidates on the selected peer",
    explanation: "Search the selected peer for filename or metadata matches and return a bounded candidate list. No file contents will be read and no file will be sent automatically.",
    confidence: "medium",
    requiresUserConfirmation: true,
    references: [
      { kind: "peer", ref: "mock-peer-1" }
    ],
    proposedInput: {
      capability: "filesystem.find_file_candidates",
      targetPeerRef: "mock-peer-1",
      query: {
        rawUserRequest: "help me find a file named report and send it to me",
        filenameHint: "report",
        extensions: [],
        searchMode: "filename_metadata_only"
      },
      scopePolicy: {
        allowedScopes: ["downloads", "desktop", "documents", "pastey_shared"],
        allowFullDisk: false,
        includeFileContents: false,
        includeAbsolutePaths: false,
        includeHiddenFiles: false
      },
      limits: {
        maxCandidates: 10,
        maxSearchMs: 5_000,
        maxDepth: 6
      },
      safety: {
        returnRedactedPaths: true,
        noAutoTransfer: true,
        requireReceiverConsent: true,
        selectedPeerOnly: true
      }
    }
  };
}

export function buildMockCandidatePayloadPlan(): AiActionPlan {
  return {
    schemaVersion: "ai-action-plan-v1",
    kind: "request_peer_candidate_payload",
    title: "Request selected candidate payload",
    explanation: "Ask the selected peer for a second explicit consent decision for one previously discovered file candidate. This does not send bytes or queue a transfer.",
    confidence: "medium",
    requiresUserConfirmation: true,
    references: [
      { kind: "peer", ref: "mock-peer-1" },
      { kind: "transfer", ref: "file-candidate-request" }
    ],
    proposedInput: {
      capability: "transfer.request_candidate_payload",
      targetPeerRef: "mock-peer-1",
      sourceCapability: "filesystem.find_file_candidates",
      sourceRequestId: "file-candidate-request",
      candidateId: "file-candidate-request-opaque-1",
      candidateDisplayName: "exact-target.pdf",
      candidateKind: "filesystem_file",
      redactedLocation: "Pastey Shared/exact-target.pdf",
      sizeBytes: 21,
      modifiedAt: "2026-06-29T00:00:20.000Z",
      mimeFamily: "document",
      extension: "pdf"
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
      parsedPlan: buildMockHelloStdoutPlan(),
      usage: {
        inputTokens: 0,
        outputTokens: 0
      }
    };
  }
}

export const mockProvider = new MockProvider();

import type { AiContextPolicy, AiContextSnapshot } from "./types";

export const MOCK_AI_CONTEXT_POLICY: AiContextPolicy = {
  allowCloudContext: false,
  includeRawLogs: false,
  includeFileContents: false,
  includeAbsolutePaths: false,
  includeSecrets: false
};

export const CLOUD_STRICT_AI_CONTEXT_POLICY: AiContextPolicy = {
  allowCloudContext: true,
  includeRawLogs: false,
  includeFileContents: false,
  includeAbsolutePaths: false,
  includeSecrets: false
};

export function buildMockAiContextSnapshot(): AiContextSnapshot {
  return {
    schemaVersion: "ai-context-snapshot/v1",
    generatedAt: new Date().toISOString(),
    room: {
      hasActiveRoom: true,
      trustedRoom: true,
      peerCount: 1
    },
    peers: [{
      peerRef: "mock-peer-1",
      visible: true,
      trusted: true,
      capabilities: ["runtime.execute_hello_template"]
    }],
    scheduler: {
      microFlowGroupMode: "unknown"
    },
    diagnostics: {
      available: false,
      summary: "Mock advisory preview only."
    },
    latestStatus: {
      level: "info",
      message: "AI Slot v0 mock context is ready."
    },
    allowedActions: ["request_peer_hello_demo"]
  };
}

export function buildCloudSafeAiContextSnapshot(snapshot: AiContextSnapshot): AiContextSnapshot {
  return {
    schemaVersion: "ai-context-snapshot/v1",
    generatedAt: snapshot.generatedAt,
    room: snapshot.room ? {
      hasActiveRoom: snapshot.room.hasActiveRoom,
      trustedRoom: snapshot.room.trustedRoom,
      peerCount: snapshot.room.peerCount
    } : undefined,
    peers: snapshot.peers?.map((peer) => ({
      peerRef: peer.peerRef,
      visible: peer.visible,
      trusted: peer.trusted,
      capabilities: peer.capabilities ? [...peer.capabilities] : undefined
    })),
    scheduler: snapshot.scheduler ? {
      microFlowGroupMode: snapshot.scheduler.microFlowGroupMode
    } : undefined,
    diagnostics: snapshot.diagnostics ? {
      available: snapshot.diagnostics.available,
      summary: snapshot.diagnostics.summary
    } : undefined,
    latestStatus: snapshot.latestStatus ? {
      level: snapshot.latestStatus.level,
      message: snapshot.latestStatus.message
    } : undefined,
    allowedActions: [...snapshot.allowedActions]
  };
}

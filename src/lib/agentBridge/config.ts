import { useSyncExternalStore } from "react";

/** This renderer-side shape is retained only for non-authoritative compatibility
 * helpers. Production provider selection is Host-owned and is not read here. */
export type AgentBridgeProviderKind = "cloud";
export type AgentBridgeLogLevel = "off" | "errors" | "standard" | "verbose";

export interface AgentBridgeRuntimeConfig {
  enabled: boolean;
  providerKind: AgentBridgeProviderKind;
  cloudBaseUrl: string;
  cloudModel: string;
  cloudApiKey: string;
  logLevel: AgentBridgeLogLevel;
}

let config: AgentBridgeRuntimeConfig = {
  enabled: false,
  providerKind: "cloud",
  cloudBaseUrl: "https://api.openai.com/v1",
  cloudModel: "",
  cloudApiKey: "",
  logLevel: "standard",
};
const listeners = new Set<() => void>();

export function getAgentBridgeRuntimeConfig(): AgentBridgeRuntimeConfig {
  return config;
}

export function updateAgentBridgeRuntimeConfig(
  update: Partial<AgentBridgeRuntimeConfig>,
): void {
  config = { ...config, ...update };
  for (const listener of listeners) listener();
}

export function subscribeAgentBridgeRuntimeConfig(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useAgentBridgeRuntimeConfig(): AgentBridgeRuntimeConfig {
  return useSyncExternalStore(
    subscribeAgentBridgeRuntimeConfig,
    getAgentBridgeRuntimeConfig,
  );
}

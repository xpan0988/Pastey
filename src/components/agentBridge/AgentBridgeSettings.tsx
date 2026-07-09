import { useState } from "react";
import {
  updateAgentBridgeRuntimeConfig,
  useAgentBridgeRuntimeConfig,
} from "../../lib/agentBridge/config";
import { checkAskBridgeNaturalV1ProviderHealth as runNaturalV1ProviderHealthCheck } from "../../lib/ai";

export function AgentBridgeSettings() {
  const config = useAgentBridgeRuntimeConfig();
  const [healthMessage, setHealthMessage] = useState<string | null>(null);
  const [checkingHealth, setCheckingHealth] = useState(false);

  async function checkProviderHealth() {
    setCheckingHealth(true);
    setHealthMessage(null);
    try {
      const result = await runNaturalV1ProviderHealthCheck({
        providerId: "pastey-cloud-openai-compatible-natural-v1-health",
        displayName: "CloudOpenAICompatibleProvider",
        kind: "cloud_openai_compatible",
        apiShape: "openai_compatible_chat",
        baseUrl: config.cloudBaseUrl,
        model: config.cloudModel,
        apiKeyRef: config.cloudApiKey ? "runtime-memory-only" : undefined,
        timeoutMs: 30_000,
        maxOutputTokens: 512,
        enabled: config.providerKind === "cloud",
      }, {
        apiKey: config.cloudApiKey,
      });
      setHealthMessage(result.ok
        ? result.message
        : `${result.message}${result.errors.length > 0 ? ` ${result.errors.join(" ")}` : ""}`);
    } finally {
      setCheckingHealth(false);
    }
  }

  return (
    <div className="settings-row diagnostics-panel-row" data-testid="agent-bridge-settings">
      <span className="settings-icon wrench" aria-hidden="true" />
      <div className="diagnostics-panel">
        <div className="diagnostics-panel-header">
          <div>
            <strong>Agent Bridge</strong>
            <p className="muted">Global runtime-memory configuration only. Bridge workflow lives in the active Bridge.</p>
          </div>
          <label>
            Enabled
            <input
              type="checkbox"
              checked={config.enabled}
              onChange={(event) => updateAgentBridgeRuntimeConfig({ enabled: event.target.checked })}
            />
          </label>
        </div>
        <div className="ai-slot-provider-controls">
          <label>
            Provider
            <select
              value={config.providerKind}
              onChange={(event) => updateAgentBridgeRuntimeConfig({
                providerKind: event.target.value as "mock" | "cloud",
              })}
            >
              <option value="mock">MockProvider</option>
              <option value="cloud">CloudOpenAICompatibleProvider</option>
            </select>
          </label>
          {config.providerKind === "cloud" ? (
            <>
              <label>Base URL<input value={config.cloudBaseUrl} onChange={(event) => updateAgentBridgeRuntimeConfig({ cloudBaseUrl: event.target.value })} /></label>
              <label>Model<input value={config.cloudModel} onChange={(event) => updateAgentBridgeRuntimeConfig({ cloudModel: event.target.value })} /></label>
              <label>API key (runtime memory only)<input type="password" autoComplete="off" value={config.cloudApiKey} onChange={(event) => updateAgentBridgeRuntimeConfig({ cloudApiKey: event.target.value })} /></label>
              <button type="button" className="secondary-button" disabled={checkingHealth} onClick={() => void checkProviderHealth()}>
                {checkingHealth ? "Checking..." : "Check provider"}
              </button>
            </>
          ) : null}
          <label>
            Lifecycle logging
            <select
              value={config.logLevel}
              onChange={(event) => updateAgentBridgeRuntimeConfig({
                logLevel: event.target.value as typeof config.logLevel,
              })}
            >
              <option value="off">Off</option>
              <option value="errors">Errors</option>
              <option value="standard">Standard</option>
              <option value="verbose">Verbose diagnostic</option>
            </select>
          </label>
        </div>
        {healthMessage ? <p className="muted">{healthMessage}</p> : null}
        <p className="muted">Logs are bounded, structured, redacted audit mirrors only. They are never state, consent, authority, or trust.</p>
      </div>
    </div>
  );
}

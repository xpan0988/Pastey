import {
  updateAgentBridgeRuntimeConfig,
  useAgentBridgeRuntimeConfig,
} from "../../lib/agentBridge/config";

export function AgentBridgeSettings() {
  const config = useAgentBridgeRuntimeConfig();
  return (
    <div className="settings-row diagnostics-panel-row" data-testid="agent-bridge-settings">
      <span className="settings-icon wrench" aria-hidden="true" />
      <div className="diagnostics-panel">
        <div className="diagnostics-panel-header">
          <div>
            <strong>Agent Bridge</strong>
            <p className="muted">Global runtime-memory configuration only. Room workflow lives in the active Room.</p>
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
        <p className="muted">Logs are bounded, structured, redacted audit mirrors only. They are never state, consent, authority, or trust.</p>
      </div>
    </div>
  );
}

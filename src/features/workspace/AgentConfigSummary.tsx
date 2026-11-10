import type { NativeAgentConfig } from "../../lib/tauri";
import { effortLabel, presentAgentConfig } from "../../lib/nativeAgentConfig";

/** A native Agent's model and reasoning effort, as the Agent reported it,
 * with whose configuration it is (`label`) and on which device. Display
 * only: nothing here sets or changes the native configuration. */
export function AgentConfigSummary({ config, agentName, hostName, label, unreported, newer, strip }: {
  config: NativeAgentConfig | null | undefined;
  agentName: string;
  hostName?: string;
  /** Whose configuration this is: a native default or one session's. */
  label: string;
  /** Why nothing is shown when `config` is absent. */
  unreported?: string;
  /** A native default that differs from the session shown. */
  newer?: NativeAgentConfig | null;
  /** A single line under a conversation's header. */
  strip?: boolean;
}) {
  const shown = presentAgentConfig(config, unreported);
  return <div className={`agent-config${strip ? " agent-config-strip" : ""}`} role="group" aria-label={`${agentName} model configuration`} title={shown.detail}>
    <span className="agent-config-head">{agentName}{hostName ? ` · ${hostName}` : ""}<span className="subtle"> · {label}</span></span>
    <span className="agent-config-values">
      <span>Model: <strong className={config?.model ? "mono" : "subtle"}>{shown.model}</strong></span>
      <span>Reasoning: <strong className={config?.reasoningEffort ? undefined : "subtle"}>{shown.reasoning}</strong></span>
    </span>
    {!strip && !shown.complete ? <span className="agent-config-note">{shown.detail}</span> : null}
    {newer ? <span className="agent-config-note">New conversations now start with {newer.model ?? "an unreported model"} · {effortLabel(newer.reasoningEffort)} (native default). This session keeps its own.</span> : null}
  </div>;
}

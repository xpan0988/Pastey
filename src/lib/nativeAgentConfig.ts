/** How a native Agent's model configuration is said. The values are only
 * what the native Agent reported (see `NativeAgentConfig`): Pastey shows
 * them and never interprets, completes or overrides them. A native default
 * and a session's own record are different facts and are never merged. */
import type { NativeAgentConfig, NativeAgentConfigOrigin } from "./tauri";

const EFFORTS: Record<string, string> = {
  none: "None",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra high",
  max: "Max",
};

const ORIGINS: Record<NativeAgentConfigOrigin, string> = {
  user: "the user's own configuration",
  project: "this folder's project configuration",
  managed: "managed configuration",
  session_flags: "session flags",
  packaged_default: "the installed package's defaults",
  other: "another configuration layer",
};

/** A reported effort level in words; an unrecognized one as reported. */
export function effortLabel(effort: string | null | undefined): string {
  if (!effort) return "Unknown";
  return EFFORTS[effort] ?? effort;
}

export interface AgentConfigPresentation {
  model: string;
  reasoning: string;
  /** Both the model and the effort were reported. */
  complete: boolean;
  /** What the values are, where they came from, and why any is unknown. */
  detail: string;
}

/** The two lines of a configuration, with Unknown where nothing was
 * reported (or nothing could be asked: `config` absent). */
export function presentAgentConfig(config: NativeAgentConfig | null | undefined, unreported = "Not reported here."): AgentConfigPresentation {
  if (!config) return { model: "Unknown", reasoning: "Unknown", complete: false, detail: unreported };
  const parts: string[] = [];
  parts.push(config.scope === "session" ? `Recorded by this session · ${config.source}` : `Native default · ${config.source}`);
  if (config.modelProvider) parts.push(`Provider: ${config.modelProvider}`);
  if (config.model && config.modelOrigin) parts.push(`Model from ${ORIGINS[config.modelOrigin] ?? config.modelOrigin}`);
  if (config.reasoningEffort && config.reasoningEffortOrigin) parts.push(`Reasoning from ${ORIGINS[config.reasoningEffortOrigin] ?? config.reasoningEffortOrigin}`);
  if (config.note) parts.push(config.note);
  return {
    model: config.model || "Unknown",
    reasoning: effortLabel(config.reasoningEffort),
    complete: !!config.model && !!config.reasoningEffort,
    detail: `${parts.map((part) => part.replace(/\.$/, "")).join(". ")}.`,
  };
}

/** What a conversation's configuration line shows: the session's own
 * record once it has one, else the native default, labelled as such. When
 * both are known and differ, the default is named separately (`newer`):
 * new conversations would start differently, this one did not change. */
export function conversationConfig(sessionConfig: NativeAgentConfig | null | undefined, nativeDefault: NativeAgentConfig | null | undefined): {
  shown: NativeAgentConfig | null;
  label: string;
  newer: NativeAgentConfig | null;
} {
  const recorded = sessionConfig && (sessionConfig.model || sessionConfig.reasoningEffort) ? sessionConfig : null;
  if (!recorded) return { shown: nativeDefault ?? null, label: "Native default · not yet confirmed for this conversation", newer: null };
  const differs = !!nativeDefault && (
    (!!nativeDefault.model && nativeDefault.model !== recorded.model)
    || (!!nativeDefault.reasoningEffort && nativeDefault.reasoningEffort !== recorded.reasoningEffort));
  return { shown: recorded, label: "This session", newer: differs ? nativeDefault! : null };
}

/** A short single-line form, e.g. for a settings row. */
export function agentConfigLine(config: NativeAgentConfig | null | undefined): string {
  const shown = presentAgentConfig(config);
  return `Model: ${shown.model} · Reasoning: ${shown.reasoning}`;
}

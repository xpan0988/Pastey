import { useCallback, useEffect, useMemo, useState } from "react";
import { chosenSessionOptions, validModelChoice } from "../../lib/nativeSessions";
import { effortLabel } from "../../lib/nativeAgentConfig";
import {
  readNativeModelCatalogs,
  readRemoteNativeModelCatalogs,
  type NativeAgentConfig,
  type NativeModelCatalog,
  type NativeModelOption,
  type NativeSessionOptions,
  type NativeSessionProvider,
} from "../../lib/tauri";
import { Badge, Icon } from "./ui";

/** The Agent's own model and effort the user chooses for the next turn, in
 * this view only: never stored, cleared once a turn is sent with them.
 * Empty means the Agent's own configuration decides. `custom` is a model
 * typed by hand rather than picked from the Agent's list. */
export function useAgentTurnOptions() {
  const [model, setModelState] = useState("");
  const [effort, setEffort] = useState("");
  const [custom, setCustom] = useState(false);
  const invalid = !!model.trim() && !validModelChoice(model);
  const options: NativeSessionOptions | null = useMemo(() => chosenSessionOptions(model, effort), [model, effort]);
  const setModel = useCallback((next: string, typed = false) => { setModelState(next); setCustom(typed); }, []);
  const clear = useCallback(() => { setModelState(""); setEffort(""); setCustom(false); }, []);
  return { model, effort, custom, setModel, setEffort, invalid, options, clear };
}

export type AgentTurnOptions = ReturnType<typeof useAgentTurnOptions>;

/** Where the Agent applies a choice, as the help text names it. */
export type AgentTurnOptionsTarget = "session" | "conversation";

/** The device whose Agent would run the turn, and so is asked for its
 * models: never the device that controls it. */
export type ModelCatalogHost = { kind: "local" } | { kind: "remote"; roomId: string; hostRef: string; hostName: string };

type CatalogState = { catalog: NativeModelCatalog | null; loading: boolean; error: string | null };

/** Recent answers, per device and Agent, so reopening a view does not ask
 * again. In memory only, never persisted. */
const recent = new Map<string, { at: number; catalog: NativeModelCatalog }>();
const RECENT_MS = 5 * 60 * 1000;

/** The models `agent` lists on the device that would run the turn, asked
 * of that device itself. Read only; nothing is stored or set. */
export function useModelCatalog(agent: NativeSessionProvider, host: ModelCatalogHost, enabled: boolean) {
  const key = host.kind === "local" ? `local:${agent}` : `${host.roomId}:${host.hostRef}:${agent}`;
  const [state, setState] = useState<CatalogState>(() => ({ catalog: recent.get(key)?.catalog ?? null, loading: false, error: null }));
  const roomId = host.kind === "remote" ? host.roomId : null;
  const hostRef = host.kind === "remote" ? host.hostRef : null;
  const hostName = host.kind === "remote" ? host.hostName : "this device";

  const load = useCallback(async (refresh: boolean) => {
    const cached = recent.get(key);
    if (!refresh && cached && Date.now() - cached.at < RECENT_MS) { setState({ catalog: cached.catalog, loading: false, error: null }); return; }
    setState((current) => ({ ...current, loading: true, error: null }));
    try {
      let catalog: NativeModelCatalog | null;
      if (roomId && hostRef) {
        const response = await readRemoteNativeModelCatalogs(roomId, hostRef, agent, refresh);
        if (response.outcome !== "ok") {
          setState({ catalog: null, loading: false, error: response.outcome === "not_authorized"
            ? `${hostName} lists its models only to a device it lets ask for agent runs.`
            : response.outcome === "busy" ? `${hostName} is answering another request. Try again in a moment.`
              : `${hostName} did not list its models.` });
          return;
        }
        catalog = response.models?.find((entry) => entry.agent === agent) ?? null;
      } else {
        catalog = (await readNativeModelCatalogs(agent, refresh)).find((entry) => entry.agent === agent) ?? null;
      }
      if (catalog && catalog.state !== "unavailable") recent.set(key, { at: Date.now(), catalog });
      setState({ catalog, loading: false, error: catalog ? null : `${hostName} did not list its models.` });
    } catch (error) {
      // An older Pastey there does not know this question.
      setState({ catalog: null, loading: false, error: error instanceof Error ? error.message : String(error) });
    }
  }, [agent, hostName, hostRef, key, roomId]);

  useEffect(() => {
    if (!enabled) return;
    void load(false);
  }, [enabled, load]);

  return { ...state, refresh: () => load(true) };
}

export type ModelCatalogView = ReturnType<typeof useModelCatalog>;

/** The entry of `catalog` that is `model` (by id, or by the model an alias
 * stands for). */
export function catalogEntry(catalog: NativeModelCatalog | null, model: string | null | undefined): NativeModelOption | null {
  if (!catalog || !model) return null;
  return catalog.models.find((entry) => entry.id === model || entry.resolvesTo === model) ?? null;
}

export interface EffortChoices {
  /** The efforts that may be chosen. */
  efforts: { id: string; description?: string | null }[];
  /** Why the list is what it is, when that needs saying. */
  note: string | null;
}

/** The efforts to offer for the next turn: those the model it would run
 * with reports, or, when that model reports none, what the Agent lists
 * generally, said as such. `current` is the model the session runs with
 * now (or would start with), for a turn that keeps it. */
export function effortChoices(catalog: NativeModelCatalog | null, chosenModel: string, current: string | null | undefined, agentName: string): EffortChoices {
  const entry = catalogEntry(catalog, chosenModel || current);
  if (entry?.efforts) {
    return entry.efforts.length
      ? { efforts: entry.efforts, note: null }
      : { efforts: [], note: `${agentName} reports no effort setting for ${entry.displayName}.` };
  }
  if (!catalog) return { efforts: [], note: null };
  const general = catalog.efforts.length ? catalog.efforts
    : [...new Set(catalog.models.flatMap((model) => (model.efforts ?? []).map((effort) => effort.id)))];
  const of = chosenModel ? "this model" : "the model it runs with now";
  return {
    efforts: general.map((id) => ({ id })),
    note: general.length ? `${agentName} does not report which efforts ${of} supports; it decides when the turn starts.` : null,
  };
}

function matches(model: NativeModelOption, query: string): boolean {
  const text = query.trim().toLowerCase();
  if (!text) return true;
  return [model.displayName, model.id, model.resolvesTo ?? "", model.description ?? ""].some((value) => value.toLowerCase().includes(text));
}

/** What a choice keeps, in the Agent's own measured behaviour. */
function persistenceLine(agent: NativeSessionProvider, agentName: string, where: string): string {
  return agent === "codex"
    ? `${agentName} keeps a chosen model and effort for ${where}'s later turns.`
    : `${agentName} keeps a chosen model for ${where}'s later turns; an effort applies to this turn only.`;
}

/** One line for what the session last ran with, as it recorded it. */
function confirmedLine(confirmed: NativeAgentConfig | null | undefined, catalog: NativeModelCatalog | null): string {
  if (!confirmed?.model && !confirmed?.reasoningEffort) return "Not confirmed yet: nothing has run here, or the session did not report it.";
  const entry = catalogEntry(catalog, confirmed.model);
  const named = entry && entry.displayName.toLowerCase() !== confirmed.model?.toLowerCase();
  const model = confirmed.model ? named ? `${entry.displayName} (${confirmed.model})` : entry?.displayName ?? confirmed.model : "an unreported model";
  return `${model} · ${effortLabel(confirmed.reasoningEffort)} effort`;
}

/** The one model-and-effort control every view that sends a turn to Codex or
 * Claude Code shows above its composer: a new or current conversation and a
 * discovered session, here or on another device. Models are picked from
 * what the Agent on the executing device lists, searchable by name; what a
 * turn actually ran with is the session's own record (`confirmed`), shown
 * apart from the choice for the next turn. */
export function AgentTurnOptionsChoice({ agent, provider, hostName, choice, catalog, confirmed, current, disabled, target }: {
  agent: NativeSessionProvider;
  provider: string;
  hostName: string;
  choice: AgentTurnOptions;
  catalog: ModelCatalogView;
  /** What the session recorded for its last turn, if known. */
  confirmed?: NativeAgentConfig | null;
  /** The model the next turn keeps when none is chosen (the session's own,
   * or the native default for a new conversation), if known. */
  current?: string | null;
  disabled: boolean;
  target: AgentTurnOptionsTarget;
}) {
  const { model, effort, invalid, custom } = choice;
  const list = catalog.catalog;
  const [query, setQuery] = useState("");
  const [typed, setTyped] = useState(custom ? model : "");
  const chosenEntry = custom ? null : catalogEntry(list, model);
  const where = target === "session" ? "this session" : "this conversation";
  const efforts = effortChoices(list, model.trim(), current, provider);
  const shown = (list?.models ?? []).filter((entry) => matches(entry, query));
  const customAllowed = !!list?.customModels || (!list && agent === "claude_code");
  const typedInvalid = custom && invalid;
  const unlistedCodex = agent === "codex" && !!model && !chosenEntry;

  // An effort the newly chosen model does not offer is not kept.
  useEffect(() => {
    if (effort && efforts.efforts.length && !efforts.efforts.some((option) => option.id === effort)) choice.setEffort("");
    if (effort && !efforts.efforts.length && efforts.note) choice.setEffort("");
  }, [model, list]); // eslint-disable-line react-hooks/exhaustive-deps

  const chosen = !!model.trim() || !!effort;
  const modelName = chosenEntry?.displayName ?? model.trim();
  const summary = chosen
    ? `Next turn: ${[model.trim() ? modelName : `${provider}'s model`, effort ? `${effortLabel(effort)} effort` : null].filter(Boolean).join(" · ")}`
    : `Next turn: ${provider}'s native configuration`;
  const name = `model-${agent}-${target}`;

  return <details className="disclosure session-options model-picker" open={chosen || undefined}>
    <summary><Icon name="chevron" small />{summary}</summary>
    <div className="disclosure-body model-picker-body">
      <p className="xsmall model-confirmed"><span className="subtle">Last confirmed turn:</span> {confirmedLine(confirmed, list)}</p>
      <div className="row model-search">
        <input className="input" type="search" value={query} disabled={disabled} onChange={(event) => setQuery(event.target.value)} placeholder={`Search ${provider} models on ${hostName}`} aria-label={`Search ${provider} models`} spellCheck={false} autoComplete="off" />
        <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label="List again" title={`Ask ${provider} on ${hostName} again`} disabled={catalog.loading} onClick={() => void catalog.refresh()}><Icon name="refresh" small /></button>
      </div>
      <div className="model-options" role="radiogroup" aria-label={`${provider} model for the next turn`}>
        <label className={`model-option${!model ? " selected" : ""}`}>
          <input type="radio" name={name} checked={!model} disabled={disabled} onChange={() => { choice.setModel(""); setTyped(""); }} />
          <span>
            <strong>Use native configuration</strong>
            <small>{target === "session" ? `No change: ${provider} keeps the model this session runs with.` : `${provider} decides: a new conversation starts with its configured default; this one keeps its own.`}</small>
          </span>
        </label>
        {shown.map((entry) => <label key={entry.id} className={`model-option${!custom && model === entry.id ? " selected" : ""}`}>
          <input type="radio" name={name} checked={!custom && model === entry.id} disabled={disabled} onChange={() => { choice.setModel(entry.id); setTyped(""); }} />
          <span>
            <strong>{entry.displayName}</strong>{entry.displayName !== entry.id ? <code className="model-id">{entry.id}</code> : null}
            {entry.isDefault ? <> <Badge>{provider}'s default</Badge></> : null}
            {entry.description ? <small>{entry.description}</small> : null}
            {entry.resolvesTo ? <small>Now stands for <code>{entry.resolvesTo}</code></small> : null}
          </span>
        </label>)}
        {list && !shown.length && query ? <p className="xsmall subtle model-empty">No listed model matches “{query}”.</p> : null}
      </div>
      {customAllowed ? <details className="disclosure model-custom" open={custom || undefined}>
        <summary><Icon name="chevron" small />Another model ID (advanced)</summary>
        <div className="disclosure-body">
          <input className="input input-mono" value={typed} disabled={disabled} onChange={(event) => { setTyped(event.target.value); choice.setModel(event.target.value, true); }} placeholder="a model name or alias as Claude Code knows it" aria-label={`${provider} model ID for the next turn`} spellCheck={false} autoComplete="off" />
          <span className="field-hint">{typedInvalid ? "Type a model name or alias as Claude Code knows it (for example sonnet, or a full model name)." : `${provider} itself refuses a model it does not know, and the session keeps its last valid one.`}</span>
        </div>
      </details> : null}
      <label className="row model-effort">
        <span className="small">Effort</span>
        <select className="select" value={effort} disabled={disabled || (!efforts.efforts.length && !!efforts.note)} onChange={(event) => choice.setEffort(event.target.value)} aria-label={`${provider} reasoning effort for the next turn`}>
          <option value="">Use native configuration</option>
          {efforts.efforts.map((option) => <option key={option.id} value={option.id} title={option.description ?? undefined}>{effortLabel(option.id)}{option.description ? ` — ${option.description}` : ""}</option>)}
        </select>
      </label>
      {efforts.note ? <p className="xsmall subtle">{efforts.note}</p> : null}
      <p className="xsmall subtle">
        {catalog.loading ? `Asking ${provider} on ${hostName} for its models…`
          : catalog.error ? `${catalog.error} You can still use the native configuration.`
            : list ? `${list.state === "fallback" ? list.note ?? "" : `Listed by ${provider} on ${hostName} (${list.source}).`} A listed model is what ${provider} offers, not a promise that your account can use it.${list.state !== "fallback" && list.note ? ` ${list.note}` : ""}`
              : null}
      </p>
      {unlistedCodex ? <p className="xsmall warn-text">{provider} on {hostName} does not list this model, so it would refuse the turn before anything is sent.</p> : null}
      <p className="xsmall subtle">Applied by {provider} on {hostName} to {where}. {persistenceLine(agent, provider, where)} Pastey keeps neither and changes no settings.</p>
    </div>
  </details>;
}

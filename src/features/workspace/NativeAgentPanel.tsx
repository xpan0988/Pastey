import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import type { BridgePeerSession } from "../../lib/bridgePeers";
import { codexRunReadiness, looksLikeFullPath, type CodexWorkspaceSource } from "../../lib/nativeAgentReadiness";
import type { NativeAgentCapability } from "../../lib/tauri";
import {
  nativeAgentMovementBlocksNewRun,
  nativeAgentTargetAvailability,
  useRemoteNativeCapabilities,
  type NativeAgentTargetAvailability,
  type useNativeAgentTask,
} from "./AgentTaskLifecycle";
import type { PeerShares } from "./BridgeWorkspaces";
import { shareSummary } from "../../lib/workspaceShares";
import { Checklist, Dot, Field, Icon, type Tone } from "./ui";

export type NativeAgentController = ReturnType<typeof useNativeAgentTask>;

const WORKSPACE_MEMORY_KEY = "pastey:codex-workspaces-v1";

/** The last folder used per target, so a path typed for another device does
 * not have to be typed again. Kept only in this device's renderer storage. */
function rememberedWorkspace(key: string): string {
  try { return (JSON.parse(window.localStorage.getItem(WORKSPACE_MEMORY_KEY) ?? "{}") as Record<string, string>)[key] ?? ""; }
  catch { return ""; }
}

function rememberWorkspace(key: string, path: string) {
  try {
    const current = JSON.parse(window.localStorage.getItem(WORKSPACE_MEMORY_KEY) ?? "{}") as Record<string, string>;
    window.localStorage.setItem(WORKSPACE_MEMORY_KEY, JSON.stringify({ ...current, [key]: path }));
  } catch { /* storage is a convenience only */ }
}

/** Run Codex on this device or on one device of this Bridge. The device is
 * chosen by its durable HostRef, so a reconnect (a new peer session) keeps
 * the choice instead of silently falling back to this device. */
export function NativeAgentPanel({ roomId, agent, peers, members, target, shares, onOpenDetails }: {
  roomId: string;
  agent: NativeAgentController;
  /** Routeable now. */
  peers: readonly BridgePeerSession[];
  /** Every member, connected or not, for names. */
  members: readonly BridgePeerSession[];
  /** "local" or the chosen device's HostRef, chosen by the Agents workspace. */
  target: string;
  /** What the chosen device shares with this Bridge, as it last said. */
  shares?: PeerShares;
  onOpenDetails?: () => void;
}) {
  const codex = agent.capabilities.find((capability) => capability.agentId === "agent.coding.codex");
  const [source, setSource] = useState<CodexWorkspaceSource>("shared");
  const [sharedRef, setSharedRef] = useState("");
  const targetHostRef = target === "local" ? null : target;
  const targetMember = targetHostRef ? members.find((peer) => peer.hostRef === targetHostRef) ?? null : null;
  const remote = targetHostRef ? peers.find((peer) => peer.hostRef === targetHostRef) ?? null : null;
  const targetName = targetMember?.displayName ?? remote?.displayName ?? "the other device";
  const remoteCapabilities = useRemoteNativeCapabilities(roomId, remote ? targetHostRef : null);
  const availability: NativeAgentTargetAvailability = targetHostRef
    ? nativeAgentTargetAvailability("agent.coding.codex", remoteCapabilities.observation)
    : localAvailability(codex);
  const effectiveSource: CodexWorkspaceSource = targetHostRef ? source : "send_from_here";
  const memoryKey = `${targetHostRef ?? "local"}:${effectiveSource}`;
  const taskActive = agent.status?.state === "running" || agent.status?.state === "queued";
  const movementBlocksNewRun = nativeAgentMovementBlocksNewRun(agent.movement);

  useEffect(() => {
    if (effectiveSource === "shared") setSharedRef(rememberedWorkspace(memoryKey));
    else agent.setWorkspace(rememberedWorkspace(memoryKey));
  }, [memoryKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const knowledge = shares?.knowledge ?? { state: "unknown" as const };
  const sharedList = knowledge.state === "known" ? knowledge.workspaces : [];
  // A remembered or chosen folder counts only while that device still lists
  // it; otherwise the first folder it shares for agent requests.
  const sharedWorkspace = sharedList.find((share) => share.scopeRef === sharedRef)
    ?? sharedList.find((share) => share.agentRequests)
    ?? sharedList[0]
    ?? null;

  const readiness = codexRunReadiness({
    target: targetHostRef ? { kind: "remote", name: targetName, connected: !!remote?.hostRef } : { kind: "local" },
    availability,
    workspaceSource: effectiveSource,
    workspace: agent.workspace,
    taskText: agent.taskText,
    busy: agent.busy,
    movementBlocksNewRun,
    shares: knowledge,
    sharedWorkspace,
  });
  const blocker = taskActive ? "Codex is working on the current task. Stop it or wait for it to finish." : readiness.blocker;
  const locked = taskActive || agent.busy;

  async function chooseFolder() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") agent.setWorkspace(chosen);
  }

  function run() {
    if (!readiness.ready || taskActive) return;
    if (!remote || !targetHostRef) { rememberWorkspace(memoryKey, agent.workspace.trim()); void agent.start(); return; }
    if (readiness.action === "review_movement") { rememberWorkspace(memoryKey, agent.workspace.trim()); void agent.proposeRemoteMovement(roomId, targetHostRef); return; }
    if (effectiveSource === "shared" && sharedWorkspace) {
      rememberWorkspace(memoryKey, sharedWorkspace.scopeRef);
      void agent.startRemote(roomId, remote.peerSessionId, targetHostRef, { scopeRef: sharedWorkspace.scopeRef, label: sharedWorkspace.label });
      return;
    }
    rememberWorkspace(memoryKey, agent.workspace.trim());
    void agent.startRemote(roomId, remote.peerSessionId, targetHostRef);
  }

  return <div className="stack-lg">
    <section className="card" aria-label="Run with Codex">
      <div className="card-body form-grid">
        <div className="availability" role="status">
          <Dot tone={availabilityTone(availability, targetHostRef !== null && !remote)} pulse={availability.state === "checking"} />
          <span className="spacer">{targetHostRef && !remote ? `${targetName} is not connected right now.` : availabilityCopy(availability, targetHostRef ? targetName : null, effectiveSource === "send_from_here")}</span>
          {remote ? <button type="button" className="btn btn-link small" disabled={availability.state === "checking"} onClick={() => void remoteCapabilities.refresh()}>Check again</button>
            : !targetHostRef && availability.state !== "available" ? <button type="button" className="btn btn-link small" onClick={() => void agent.refreshCapabilities()}>Check again</button> : null}
        </div>

        {targetHostRef ? <Field label="Where is the folder?">
          <div className="choices" role="radiogroup" aria-label="Where is the folder?">
            <label className={`choice${source === "shared" ? " selected" : ""}`}>
              <input type="radio" name="codex-source" checked={source === "shared"} onChange={() => setSource("shared")} disabled={locked || movementBlocksNewRun} />
              <span><strong>Shared by {targetName}</strong><small>Pick a folder {targetName} shares with this Bridge, by name. Codex works there in place; no files move.</small></span>
            </label>
            <label className={`choice${source === "on_target" ? " selected" : ""}`}>
              <input type="radio" name="codex-source" checked={source === "on_target"} onChange={() => setSource("on_target")} disabled={locked || movementBlocksNewRun} />
              <span><strong>Another folder on {targetName}</strong><small>Type its full path there. Codex works in place; no files move.</small></span>
            </label>
            <label className={`choice${source === "send_from_here" ? " selected" : ""}`}>
              <input type="radio" name="codex-source" checked={source === "send_from_here"} onChange={() => setSource("send_from_here")} disabled={locked || movementBlocksNewRun} />
              <span><strong>On this device</strong><small>Pastey sends a copy after you review it. Codex works on the copy and the result comes back to be applied here.</small></span>
            </label>
          </div>
        </Field> : null}

        {effectiveSource === "shared" ? <Field label={`Folder shared by ${targetName}`} htmlFor="codex-shared-workspace" hint={sharedWorkspace ? `${shareSummary(sharedWorkspace)}. ${targetName} finds the folder itself; this device never sees its path.` : undefined}>
          {sharedList.length ? <select id="codex-shared-workspace" className="select" value={sharedWorkspace?.scopeRef ?? ""} onChange={(event) => setSharedRef(event.target.value)} disabled={locked} aria-label="Shared workspace">
            {sharedList.map((share) => <option key={share.scopeRef} value={share.scopeRef}>{share.label}{share.agentRequests ? "" : " (agent requests off)"}</option>)}
          </select> : <p className="small muted">{knowledge.state === "unknown" ? shares?.message ?? `Asking ${targetName} which folders it shares…` : `${targetName} shares no folder with this Bridge yet.`}</p>}
          <div className="row">{onOpenDetails ? <button type="button" className="btn btn-link small" onClick={onOpenDetails}>Workspaces in Bridge details</button> : null}{shares ? <button type="button" className="btn btn-link small" disabled={shares.loading} onClick={() => void shares.refresh()}>{shares.loading ? "Asking…" : "Check again"}</button> : null}</div>
        </Field> : effectiveSource === "on_target" ? <Field label={`Folder path on ${targetName}`} htmlFor="codex-workspace" hint={agent.workspace.trim() && !looksLikeFullPath(agent.workspace) ? `This does not look like a full path. ${targetName} looks for exactly this folder.` : `Type the full path as it appears on ${targetName}, for example C:\\Users\\you\\Projects\\app. Pastey cannot browse that device.`}>
          <input id="codex-workspace" className="input input-mono" value={agent.workspace} onChange={(event) => agent.setWorkspace(event.target.value)} placeholder="C:\Users\you\Projects\app" aria-label="Codex workspace" disabled={locked} spellCheck={false} autoComplete="off" />
        </Field> : <Field label="Folder on this device" htmlFor="codex-workspace">
          <div className="row">
            <input id="codex-workspace" className="input input-mono" value={agent.workspace} onChange={(event) => agent.setWorkspace(event.target.value)} placeholder="Choose a folder…" aria-label="Codex workspace" disabled={locked} spellCheck={false} autoComplete="off" />
            <button type="button" className="btn" disabled={locked} onClick={() => void chooseFolder()}><Icon name="folder" />Choose…</button>
          </div>
        </Field>}

        <Field label="Task" htmlFor="codex-task">
          <textarea id="codex-task" className="textarea" value={agent.taskText} onChange={(event) => agent.setTaskText(event.target.value)} placeholder="What should Codex do?" aria-label="Codex task" disabled={locked} />
        </Field>

        {effectiveSource !== "send_from_here" || !targetHostRef ? <label className="check">
          <input type="checkbox" checked={agent.newConversation} onChange={(event) => agent.setNewConversation(event.target.checked)} disabled={locked} />
          <span>Start a new conversation<small>Otherwise Codex continues the conversation Pastey keeps for you in this folder, if there is one (listed under Continue a session as “Started by Pastey”). To pick up a conversation from the Codex app or a terminal, use Continue a session instead.</small></span>
        </label> : null}

        {blocker && !taskActive ? <Checklist items={readiness.requirements} /> : null}
      </div>
      <footer className="card-footer">
        <span className="blocker spacer small muted" role="status">{blocker ?? readiness.next}</span>
        <button type="button" className="btn btn-primary" disabled={!readiness.ready || taskActive} title={blocker ?? undefined} onClick={run}>{readiness.actionLabel}</button>
      </footer>
    </section>
  </div>;
}

function localAvailability(codex: NativeAgentCapability | undefined): NativeAgentTargetAvailability {
  return { state: codex?.state ?? "unavailable", reason: codex?.reason ?? null, detail: codex?.detail ?? null, backend: codex?.backend ?? null };
}

function availabilityTone(availability: NativeAgentTargetAvailability, disconnected: boolean): Tone {
  if (disconnected) return "neutral";
  switch (availability.state) {
    case "available": return "ok";
    case "checking": return "neutral";
    case "unknown": return "warn";
    default: return "bad";
  }
}

/** What to tell the user about an unusable Codex, by the layer that failed. */
export function codexUnavailableCopy(reason: string | null | undefined, where: string): string {
  switch (reason) {
    case "native_agent_not_installed":
      return `Codex is not installed on ${where}, or Pastey there cannot find the codex command. On ${where}, check that “codex --version” works in a new terminal; if Codex was installed after Pastey started, quit Pastey there and open it again.`;
    case "native_agent_launch_failed":
      return `Pastey on ${where} found Codex but could not start it. Reinstall or update Codex on ${where}, then check again.`;
    case "native_agent_probe_failed":
      return `Codex on ${where} exits with an error when Pastey checks it (“codex --version”). Run that in a new terminal on ${where} to see why; a Codex installed with npm also needs Node.js on PATH.`;
    case "native_agent_probe_timeout":
      return `Codex on ${where} did not answer in time. Check again; its first start can be slow.`;
    case "native_agent_incompatible":
      return `Codex is detected on ${where}, but its required native interface is incompatible. Update Codex on ${where}.`;
    default:
      return `Codex is unavailable on ${where}.`;
  }
}

export function availabilityCopy(availability: NativeAgentTargetAvailability, remoteName: string | null, sendFromHere: boolean): string {
  const where = remoteName ?? "this device";
  switch (availability.state) {
    case "checking": return `Checking Codex on ${where}…`;
    case "unknown": return `Pastey could not check Codex on ${where}: ${availability.message}`;
    case "incompatible":
    case "unavailable": {
      const reason = availability.reason ?? (availability.state === "incompatible" ? "native_agent_incompatible" : null);
      const copy = codexUnavailableCopy(reason, where);
      return availability.detail ? `${copy} (${availability.detail})` : copy;
    }
    case "available":
      if (!remoteName) {
        return availability.backend === "desktop"
          ? "Codex from the ChatGPT desktop app is ready to use its native session. No separate Codex CLI is needed."
          : "Codex is ready to use its native session.";
      }
      return sendFromHere
        ? `Codex is available on ${where}. Pastey will review sending your folder there and returning its result.`
        : `Codex is available on ${where}. It runs in a folder there after ${where} accepts; no files move.`;
  }
}

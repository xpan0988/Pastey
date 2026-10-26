import { open } from "@tauri-apps/plugin-dialog";
import { useEffect } from "react";
import type { BridgePeerSession } from "../../lib/bridgePeers";
import { codexRunReadiness, looksLikeFullPath, type CodexWorkspaceSource } from "../../lib/nativeAgentReadiness";
import {
  nativeAgentMovementBlocksNewRun,
  type ConversationPlace,
  type NativeAgentTargetAvailability,
  type useNativeAgentTask,
} from "./AgentTaskLifecycle";
import { PastTurnView, TaskTurn } from "./AgentTaskTracker";
import type { PeerShares } from "./BridgeWorkspaces";
import { Composer, ConversationHeader, ConversationThread } from "./Conversation";
import { Checklist, Dot, Icon, type Tone } from "./ui";

export type NativeAgentController = ReturnType<typeof useNativeAgentTask>;

const WORKSPACE_MEMORY_KEY = "pastey:codex-workspaces-v1";

/** The last folder used per target, so a path typed for another device does
 * not have to be typed again. Kept only in this device's renderer storage. */
export function rememberedWorkspace(key: string): string {
  try { return (JSON.parse(window.localStorage.getItem(WORKSPACE_MEMORY_KEY) ?? "{}") as Record<string, string>)[key] ?? ""; }
  catch { return ""; }
}

export function rememberWorkspace(key: string, path: string) {
  try {
    const current = JSON.parse(window.localStorage.getItem(WORKSPACE_MEMORY_KEY) ?? "{}") as Record<string, string>;
    window.localStorage.setItem(WORKSPACE_MEMORY_KEY, JSON.stringify({ ...current, [key]: path }));
  } catch { /* storage is a convenience only */ }
}

/** Where a new conversation can run, as the navigator offers it. */
export type PlaceChoice =
  | { kind: "shared"; scopeRef: string }
  | { kind: "path" }
  | { kind: "send" }
  | { kind: "local"; path: string };

export function folderLabel(path: string): string {
  const parts = path.trim().split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path.trim();
}

/** A new conversation with Codex in one place on the chosen device, and its
 * turns. The first message always begins a new Codex conversation: it never
 * continues another one in the same folder (that is Continue a session,
 * which names the session). Later messages in this same view continue the
 * conversation this view began. The device is chosen by its durable HostRef,
 * so a reconnect keeps it instead of falling back to this device. */
export function NewConversation({ roomId, agent, peers, members, target, choice, shares, availability, onRecheck, current, onNewDraft, onBack }: {
  roomId: string;
  agent: NativeAgentController;
  /** Routeable now. */
  peers: readonly BridgePeerSession[];
  /** Every member, connected or not, for names. */
  members: readonly BridgePeerSession[];
  /** "local" or the chosen device's HostRef. */
  target: string;
  /** Where a new conversation would run. */
  choice: PlaceChoice | null;
  shares?: PeerShares;
  availability: NativeAgentTargetAvailability;
  onRecheck: () => void;
  /** Show the conversation this Bridge follows, rather than a new draft. */
  current: boolean;
  onNewDraft: () => void;
  onBack?: () => void;
}) {
  const targetHostRef = target === "local" ? null : target;
  const remote = targetHostRef ? peers.find((peer) => peer.hostRef === targetHostRef) ?? null : null;
  const targetName = targetHostRef ? members.find((peer) => peer.hostRef === targetHostRef)?.displayName ?? remote?.displayName ?? "the other device" : "this device";
  const conversationPlace = current && agent.origin?.kind === "new" ? agent.origin.place : null;
  const knowledge = shares?.knowledge ?? { state: "unknown" as const };
  const sharedList = knowledge.state === "known" ? knowledge.workspaces : [];
  const sharedScope = conversationPlace?.kind === "shared" ? conversationPlace.scopeRef : choice?.kind === "shared" ? choice.scopeRef : null;
  const sharedWorkspace = sharedScope ? sharedList.find((share) => share.scopeRef === sharedScope) ?? null : null;
  const kind = conversationPlace?.kind ?? choice?.kind ?? (targetHostRef ? "shared" : "local");
  const source: CodexWorkspaceSource = !targetHostRef ? "send_from_here" : kind === "path" ? "on_target" : kind === "send" ? "send_from_here" : "shared";
  const memoryKey = `${targetHostRef ?? "local"}:${source}`;
  const taskActive = agent.status?.state === "running" || agent.status?.state === "queued";
  const movementBlocksNewRun = nativeAgentMovementBlocksNewRun(agent.movement);
  const fixedPath = conversationPlace && conversationPlace.kind !== "shared" ? conversationPlace.path : choice?.kind === "local" ? choice.path : null;

  // The folder a message runs in: fixed for a local folder and for a
  // conversation already begun; typed or chosen otherwise, remembered.
  useEffect(() => {
    if (fixedPath !== null) agent.setWorkspace(fixedPath);
    else if (kind === "path" || kind === "send") agent.setWorkspace(rememberedWorkspace(memoryKey));
  }, [fixedPath, kind, memoryKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const place: ConversationPlace | null = conversationPlace ?? (
    kind === "shared" ? sharedWorkspace ? { kind: "shared", scopeRef: sharedWorkspace.scopeRef, label: sharedWorkspace.label } : null
      : agent.workspace.trim() ? { kind: kind as "path" | "send" | "local", path: agent.workspace.trim(), label: folderLabel(agent.workspace) } : null
  );
  const label = place?.label ?? (kind === "path" ? "a folder by path" : kind === "send" ? "a folder from this device" : "a folder");

  // Later messages continue this conversation, once it has a turn that ran.
  const conversationRan = current && agent.origin?.kind === "new"
    && (agent.status?.state === "completed" || agent.pastTurns.some((turn) => turn.status.state === "completed"));
  const followUp = conversationRan && kind !== "send";

  const readiness = codexRunReadiness({
    target: targetHostRef ? { kind: "remote", name: targetName, connected: !!remote?.hostRef } : { kind: "local" },
    availability,
    workspaceSource: source,
    workspace: agent.workspace,
    taskText: agent.taskText,
    busy: agent.busy,
    movementBlocksNewRun,
    shares: knowledge,
    sharedWorkspace,
  });
  const blocker = taskActive ? `${current ? "Codex is working on this" : "Codex is working on another task in this Bridge"}. Wait for it, or stop it.` : readiness.blocker;
  const locked = taskActive || agent.busy;

  async function chooseFolder() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") agent.setWorkspace(chosen);
  }

  function run() {
    if (!readiness.ready || taskActive || !place) return;
    if (place.kind !== "shared") rememberWorkspace(memoryKey, agent.workspace.trim());
    if (!remote || !targetHostRef) { void agent.start(place, followUp); return; }
    if (readiness.action === "review_movement") { void agent.proposeRemoteMovement(roomId, targetHostRef, place); return; }
    void agent.startRemote(roomId, remote.peerSessionId, targetHostRef, place, followUp);
  }

  const showTurns = current && (agent.origin?.kind === "new" || (!agent.origin && (agent.status || agent.movement)));
  const firstPrompt = agent.pastTurns[0]?.prompt ?? agent.prompt;
  const above = conversationPlace ? null
    : kind === "path" ? <div className="field">
      <input className="input input-mono" value={agent.workspace} onChange={(event) => agent.setWorkspace(event.target.value)} placeholder="C:\Users\you\Projects\app" aria-label="Codex workspace" disabled={locked} spellCheck={false} autoComplete="off" />
      <span className="field-hint">{agent.workspace.trim() && !looksLikeFullPath(agent.workspace) ? `This does not look like a full path. ${targetName} looks for exactly this folder.` : `The full path of the folder on ${targetName}. Pastey cannot browse that device.`}</span>
    </div>
      : kind === "send" || (kind === "local" && !fixedPath) ? <div className="row">
        <input className="input input-mono" value={agent.workspace} onChange={(event) => agent.setWorkspace(event.target.value)} placeholder="Choose a folder on this device…" aria-label="Codex workspace" disabled={locked} spellCheck={false} autoComplete="off" />
        <button type="button" className="btn btn-sm" disabled={locked} onClick={() => void chooseFolder()}><Icon name="folder" />Choose…</button>
      </div> : null;

  return <section className="conversation" aria-label="Run with Codex">
    <ConversationHeader
      onBack={onBack}
      title={showTurns && firstPrompt ? truncate(firstPrompt, 70) : "New conversation"}
      subtitle={<>Codex · {label} · {targetHostRef ? targetName : "this device"}</>}
      side={showTurns ? <button type="button" className="btn btn-sm" disabled={taskActive} onClick={onNewDraft}><Icon name="plus" small />New</button> : null}
    />
    <ConversationThread follow={`${agent.pastTurns.length}:${agent.status?.state}:${agent.status?.code}:${agent.movement?.state}`}>
      {showTurns ? <>
        {agent.pastTurns.map((turn) => <PastTurnView key={turn.taskId} turn={turn} hostName={agent.taskHostRef ? targetName : "this device"} />)}
        <TaskTurn roomId={roomId} agent={agent} peers={peers} members={members} />
      </> : <div className="conversation-intro">
        <h3>Start a new conversation</h3>
        <p>{kind === "send"
          ? `Pastey shows you exactly what it sends to ${targetName}, and sends nothing until you approve. Codex works on the copy there, and the result comes back to be applied here.`
          : `Codex starts fresh in ${label}${targetHostRef ? ` on ${targetName}` : ""}. It does not see your other conversations there; to pick one of those up, open it from the list.`}
          {targetHostRef && kind !== "send" ? ` ${targetName}'s user accepts each request, unless they allowed this device to run there without asking. No files move.` : ""}</p>
        <div className="availability" role="status">
          <Dot tone={availabilityTone(availability, targetHostRef !== null && !remote)} pulse={availability.state === "checking"} />
          <span className="spacer">{targetHostRef && !remote ? `${targetName} is not connected right now.` : availabilityCopy(availability, targetHostRef ? targetName : null, kind === "send")}</span>
          {availability.state !== "available" || targetHostRef ? <button type="button" className="btn btn-link small" disabled={availability.state === "checking"} onClick={onRecheck}>Check again</button> : null}
        </div>
        {readiness.blocker && readiness.requirements.some((requirement) => !requirement.met && requirement.id !== "task") ? <Checklist items={readiness.requirements} /> : null}
      </div>}
    </ConversationThread>
    <Composer
      label="Message Codex"
      value={agent.taskText}
      onChange={agent.setTaskText}
      onSend={run}
      canSend={readiness.ready && !taskActive && !!place}
      disabled={locked}
      sending={agent.busy}
      placeholder={followUp ? "Reply to continue this conversation…" : "What should Codex do?"}
      above={above}
      bar={<><Icon name="agent" small />Codex · {label}{targetHostRef ? ` · ${targetName}` : ""}</>}
      hint={blocker && (agent.taskText.trim() || taskActive || readiness.requirements.some((requirement) => !requirement.met && requirement.id !== "task")) ? blocker
        : followUp ? `Continues this conversation in ${label}. ${targetHostRef ? `${targetName} asks its user to accept unless it gave this device Full Access.` : ""}`
          : showTurns && current ? `Starts a new conversation in ${label}.` : readiness.next}
      hintTone={blocker && (agent.taskText.trim() || taskActive) ? "warn" : undefined}
    />
  </section>;
}

function truncate(text: string, limit: number): string {
  const line = text.trim().split("\n")[0];
  return line.length > limit ? `${line.slice(0, limit - 1)}…` : line;
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

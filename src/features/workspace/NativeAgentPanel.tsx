import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import type { BridgePeerSession } from "../../lib/bridgePeers";
import { codexRunReadiness, looksLikeFullPath, type CodexWorkspaceSource } from "../../lib/nativeAgentReadiness";
import { nativeDecisionsAwaitUser } from "../../lib/nativeDecisions";
import type { NativeAgentCapability, NativeAgentTaskStatus } from "../../lib/tauri";
import {
  nativeAgentAwaitingNativeDecision,
  nativeAgentConsequenceAbandonmentRequired,
  nativeAgentConversationLabel,
  nativeAgentConversationUnavailable,
  nativeAgentDisplayName,
  nativeAgentLifecycleLabel,
  nativeAgentMovementBlocksNewRun,
  nativeAgentRemoteReconciliationRequired,
  nativeAgentResponse,
  nativeAgentTargetAvailability,
  StatusBadge,
  useRemoteNativeCapabilities,
  type LifecycleTone,
  type NativeAgentTargetAvailability,
  type useNativeAgentTask,
} from "./AgentTaskLifecycle";
import { NativeDecisionPanel } from "./NativeDecisionPanel";
import { Checklist, Dot, Field, Icon, Notice, type Tone } from "./ui";

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
export function NativeAgentPanel({ roomId, agent, peers, members }: {
  roomId: string;
  agent: NativeAgentController;
  /** Routeable now. */
  peers: readonly BridgePeerSession[];
  /** Every member, connected or not, for names. */
  members: readonly BridgePeerSession[];
}) {
  const codex = agent.capabilities.find((capability) => capability.agentId === "agent.coding.codex");
  const [target, setTarget] = useState<string>("local");
  const [source, setSource] = useState<CodexWorkspaceSource>("on_target");
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

  useEffect(() => { agent.setWorkspace(rememberedWorkspace(memoryKey)); }, [memoryKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const readiness = codexRunReadiness({
    target: targetHostRef ? { kind: "remote", name: targetName, connected: !!remote?.hostRef } : { kind: "local" },
    availability,
    workspaceSource: effectiveSource,
    workspace: agent.workspace,
    taskText: agent.taskText,
    busy: agent.busy,
    movementBlocksNewRun,
  });
  const blocker = taskActive ? "Codex is working on the current task. Stop it or wait for it to finish." : readiness.blocker;
  const locked = taskActive || agent.busy;

  const options = useMemo(() => members.filter((peer) => peer.hostRef), [members]);

  async function chooseFolder() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") agent.setWorkspace(chosen);
  }

  function run() {
    if (!readiness.ready || taskActive) return;
    rememberWorkspace(memoryKey, agent.workspace.trim());
    if (!remote || !targetHostRef) { void agent.start(); return; }
    if (readiness.action === "review_movement") void agent.proposeRemoteMovement(roomId, targetHostRef);
    else void agent.startRemote(roomId, remote.peerSessionId, targetHostRef);
  }

  return <div className="panel-inner">
    <div className="panel-intro">
      <h2>Run Codex</h2>
      <p>Codex works in its own session, with its own sign-in and sandbox, on the device you choose. Pastey moves files only after you approve, and shows what Codex returns.</p>
    </div>

    <NativeAgentTaskState roomId={roomId} agent={agent} peers={peers} members={members} />

    <section className="card" aria-label="Run with Codex">
      <div className="card-body form-grid">
        <Field label="Run on" htmlFor="codex-target">
          <select id="codex-target" className="select" value={target} onChange={(event) => setTarget(event.target.value)} disabled={locked}>
            <option value="local">This device</option>
            {options.map((peer) => <option key={peer.hostRef} value={peer.hostRef}>{peer.displayName}{peers.some((routeable) => routeable.hostRef === peer.hostRef) ? "" : " (not connected)"}</option>)}
          </select>
        </Field>

        <div className="availability" role="status">
          <Dot tone={availabilityTone(availability, targetHostRef !== null && !remote)} pulse={availability.state === "checking"} />
          <span className="spacer">{targetHostRef && !remote ? `${targetName} is not connected right now.` : availabilityCopy(availability, targetHostRef ? targetName : null, effectiveSource === "send_from_here")}</span>
          {remote ? <button type="button" className="btn btn-link small" disabled={availability.state === "checking"} onClick={() => void remoteCapabilities.refresh()}>Check again</button>
            : !targetHostRef && availability.state !== "available" ? <button type="button" className="btn btn-link small" onClick={() => void agent.refreshCapabilities()}>Check again</button> : null}
        </div>

        {targetHostRef ? <Field label="Where is the folder?">
          <div className="choices" role="radiogroup" aria-label="Where is the folder?">
            <label className={`choice${source === "on_target" ? " selected" : ""}`}>
              <input type="radio" name="codex-source" checked={source === "on_target"} onChange={() => setSource("on_target")} disabled={locked || movementBlocksNewRun} />
              <span><strong>Already on {targetName}</strong><small>Codex works in that folder in place. No files move.</small></span>
            </label>
            <label className={`choice${source === "send_from_here" ? " selected" : ""}`}>
              <input type="radio" name="codex-source" checked={source === "send_from_here"} onChange={() => setSource("send_from_here")} disabled={locked || movementBlocksNewRun} />
              <span><strong>On this device</strong><small>Pastey sends a copy after you review it. Codex works on the copy and the result comes back to be applied here.</small></span>
            </label>
          </div>
        </Field> : null}

        {effectiveSource === "on_target" ? <Field label={`Folder path on ${targetName}`} htmlFor="codex-workspace" hint={agent.workspace.trim() && !looksLikeFullPath(agent.workspace) ? `This does not look like a full path. ${targetName} looks for exactly this folder.` : `Type the full path as it appears on ${targetName}, for example C:\\Users\\you\\Projects\\app. Pastey cannot browse that device.`}>
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

        {effectiveSource === "on_target" || !targetHostRef ? <label className="check">
          <input type="checkbox" checked={agent.newConversation} onChange={(event) => agent.setNewConversation(event.target.checked)} disabled={locked} />
          <span>Start a new conversation<small>Otherwise Codex continues your earlier conversation in this folder, if there is one.</small></span>
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

/** The current or unresolved task: lifecycle, the Agent's own requests and
 * response, workspace movement, and the explicit repair actions. */
function NativeAgentTaskState({ roomId, agent, peers, members }: { roomId: string; agent: NativeAgentController; peers: readonly BridgePeerSession[]; members: readonly BridgePeerSession[] }) {
  if (!agent.status && !agent.movement && !agent.message) return null;
  const taskHostName = agent.taskHostRef
    ? members.find((peer) => peer.hostRef === agent.taskHostRef)?.displayName ?? "the selected device"
    : null;
  const taskPeer = agent.taskHostRef ? peers.find((peer) => peer.hostRef === agent.taskHostRef) ?? null : null;
  const movementPeer = agent.movement ? peers.find((peer) => peer.hostRef === agent.movement?.targetHostRef) ?? null : null;
  const movementHostName = agent.movement ? members.find((peer) => peer.hostRef === agent.movement?.targetHostRef)?.displayName ?? "the selected device" : null;
  const remoteReconciliationRequired = nativeAgentRemoteReconciliationRequired(agent.status, agent.movement);
  const consequenceAbandonmentRequired = nativeAgentConsequenceAbandonmentRequired(agent.movement);
  const resultReturnRetryRequired = agent.movement?.code === "result_return_retry_required"
    || (agent.movement?.state === "interrupted" && ["result_apply_interrupted", "conflict_result_retention_required", "conflict_result_retention_failed"].includes(agent.movement.code ?? ""));
  const status = agent.status;
  const conversation = nativeAgentConversationLabel(status);

  const actions = agent.movement?.state === "awaiting_approval" && movementPeer
    ? <button type="button" className="btn btn-primary" disabled={agent.busy} onClick={() => void agent.approveRemoteMovement(roomId, movementPeer.peerSessionId)}>Approve and send</button>
    : remoteReconciliationRequired && !agent.recoveryTaskId
      ? <button type="button" className="btn" disabled={agent.busy} onClick={() => void agent.refreshRecoveryProjection()}>Load recovery details</button>
      : remoteReconciliationRequired
        ? <>{agent.recoveryTargetHostRef ? <button type="button" className="btn btn-primary" disabled={agent.busy} onClick={() => void agent.reconcileRemote()}>Reconcile</button> : null}<button type="button" className="btn" disabled={agent.busy || !agent.recoveryTaskId} onClick={() => void agent.stopRecovery()}>Stop</button></>
        : consequenceAbandonmentRequired
          ? <>{resultReturnRetryRequired ? <button type="button" className="btn btn-primary" disabled={agent.busy || agent.retryingResultReturn} onClick={() => void agent.retryResultReturn()}>{agent.retryingResultReturn ? "Retrying result Return…" : "Retry result Return"}</button> : null}<button type="button" className="btn btn-danger" disabled={agent.busy} onClick={() => void agent.stopRecovery()}>Abandon recovery</button></>
          : resultReturnRetryRequired
            ? <button type="button" className="btn btn-primary" disabled={agent.busy || agent.retryingResultReturn} onClick={() => void agent.retryResultReturn()}>{agent.retryingResultReturn ? "Retrying result Return…" : "Retry result Return"}</button>
            : agent.canCancel ? <button type="button" className="btn btn-danger" disabled={agent.busy} onClick={() => void (taskPeer ? agent.cancelRemote(roomId, taskPeer.peerSessionId, taskPeer.hostRef!) : agent.cancel())}>Stop Codex</button> : null;

  return <section className="card" aria-label="Codex task">
    {status ? <header className="card-header">
      <div className="stack" style={{ gap: 2 }}>
        <h3>{nativeAgentDisplayName(status.agentId)} · {status.workspaceName}</h3>
        <span className="xsmall subtle">{taskHostName ? `On ${taskHostName}` : "On this device"}{conversation ? ` · ${conversation}` : ""}</span>
      </div>
      <StatusBadge tone={lifecycleTone(status)}>{nativeAgentLifecycleLabel(status)}</StatusBadge>
    </header> : null}
    <div className="card-body">
      {status?.result ? <p className="small muted">{status.result}</p> : null}
      {nativeAgentConversationUnavailable(status) ? <Notice tone="bad">{nativeAgentDisplayName(status?.agentId)} on {taskHostName ?? "this device"} could not reopen your earlier conversation, so nothing ran. Choose “Start a new conversation” to continue without that context.</Notice> : null}
      {status ? <NativeDecisionPanel status={status} roomId={roomId} remote={!!agent.taskHostRef} hostName={taskHostName ?? "this device"} /> : null}
      {nativeAgentAwaitingNativeDecision(status) && !nativeDecisionsAwaitUser(status!, !!agent.taskHostRef, Date.now() / 1000) ? <Notice tone="warn">{nativeAgentDisplayName(status?.agentId)} on {taskHostName ?? "this device"} is waiting for a native request that cannot be answered from here, so the task cannot continue; stop it to interrupt the turn. To run without approval prompts, change the approval policy in that device’s own Codex configuration.</Notice> : null}
      {status ? <NativeAgentResponse status={status} hostName={taskHostName} /> : null}
      {remoteReconciliationRequired && !agent.movement ? <Notice tone="warn">The remote task outcome is indeterminate. Pastey will not treat it as complete until it is reconciled or explicitly stopped.</Notice> : null}
      {agent.movement ? <div className="stack">
        <div className="row"><strong className="spacer">{agent.movement.state === "awaiting_approval" ? "Review workspace movement" : `Workspace movement · ${agent.movement.state.replace(/_/g, " ")}`}</strong>{agent.movement.state === "awaiting_approval" ? <StatusBadge tone="pending">Needs your approval</StatusBadge> : null}</div>
        <p className="small muted">{agent.movement.reviewSummary}</p>
        {agent.movement.state === "awaiting_approval" ? <ol className="small">
          <li>Send “{agent.movement.sourceWorkspaceName}” to {movementHostName}.</li>
          <li>Ask that device to run Codex. It runs only if that device accepts.</li>
          <li>Return and apply the result here if the source is unchanged.</li>
        </ol> : null}
        {agent.movement.state === "awaiting_approval" && !movementPeer ? <Notice tone="warn">{movementHostName} is not connected. You can approve once it reconnects.</Notice> : null}
        {remoteReconciliationRequired ? <Notice tone="warn">The remote task outcome is indeterminate. Pastey will not reuse this workspace until the task is reconciled or explicitly stopped.</Notice> : null}
        {consequenceAbandonmentRequired ? <Notice tone="bad">{agent.movement.code === "native_agent_result_snapshot_recovery_failed" ? "Codex completed, but Pastey could not safely capture its result for return. Abandon this movement to release the original workspace; Codex will not run again." : "Pastey could not retain the returned result. Retry its exact Return or abandon this movement; the Agent’s recorded task outcome will remain unchanged."}</Notice> : null}
        {resultReturnRetryRequired ? <Notice tone="info">The exact Agent result is durable. Pastey can retry its Return without running Codex again.</Notice> : null}
        {agent.movement.state === "conflict_recovery_required" ? <>
          <Notice tone="warn">Your original workspace changed while Codex was working. Pastey preserved the result and did not overwrite your changes.</Notice>
          <div className="row"><button type="button" className="btn" disabled={agent.busy} onClick={() => void agent.revealConflictResult()}>Reveal result</button><button type="button" className="btn btn-danger" disabled={agent.busy} onClick={() => void agent.discardConflictResult()}>Discard result</button></div>
        </> : null}
      </div> : null}
      {agent.message ? <p className="error-text" role="alert">{agent.message}</p> : null}
    </div>
    {actions ? <footer className="card-footer">{actions}</footer> : null}
  </section>;
}

function lifecycleTone(status: NativeAgentTaskStatus): LifecycleTone {
  if (nativeAgentAwaitingNativeDecision(status)) return "pending";
  switch (status.state) {
    case "running": return "live";
    case "queued": return "pending";
    case "completed": return "complete";
    case "failed": case "interrupted": return "danger";
    default: return "neutral";
  }
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

/** The Agent's returned response, kept apart from lifecycle state. Native
 * output is untrusted and possibly remote: it is shown only as escaped
 * plain text (never markup, a command, a path or a link). */
function NativeAgentResponse({ status, hostName }: { status: NativeAgentTaskStatus; hostName: string | null }) {
  const response = nativeAgentResponse(status);
  if (!response) return null;
  const agentName = nativeAgentDisplayName(status.agentId);
  return <section className="stack" aria-label={`${agentName} response`}>
    <span className="subhead">{agentName} response{hostName ? ` · from ${hostName}` : ""}</span>
    {response.kind === "text"
      ? <><pre className="agent-response-text">{response.text}</pre>{response.truncated ? <small className="subtle">Pastey shortened this response to fit its size limit.</small> : null}</>
      : response.kind === "undisplayable"
        ? <small className="muted">The task completed with output Pastey cannot display as text.</small>
        : <small className="muted">The task completed, but no response text is available.</small>}
  </section>;
}

import type { BridgePeerSession } from "../../lib/bridgePeers";
import { nativeDecisionsAwaitUser } from "../../lib/nativeDecisions";
import { providerLabel } from "../../lib/nativeSessions";
import { taskOutcome, taskStages, type TaskOutcome } from "../../lib/nativeTaskOutcome";
import type { NativeAgentTaskStatus } from "../../lib/tauri";
import {
  nativeAgentAwaitingNativeDecision,
  nativeAgentConsequenceAbandonmentRequired,
  nativeAgentConversationLabel,
  nativeAgentDisplayName,
  nativeAgentRemoteReconciliationRequired,
  nativeAgentResponse,
  StatusBadge,
  type LifecycleTone,
} from "./AgentTaskLifecycle";
import type { NativeAgentController } from "./NativeAgentPanel";
import { NativeDecisionPanel } from "./NativeDecisionPanel";
import { Icon, Notice } from "./ui";

/** The one task this Bridge follows: what it is, where it is now, what its
 * outcome means and what to do next, with the Agent's own requests, its
 * reply, any workspace movement and the explicit repair actions. Shown
 * once: inside the session it continues while that session is open,
 * otherwise at the top of Agents. */
export function AgentTaskTracker({ roomId, agent, peers, members, inSession = false, onOpenSession, onLookAgain }: {
  roomId: string;
  agent: NativeAgentController;
  peers: readonly BridgePeerSession[];
  members: readonly BridgePeerSession[];
  /** Rendered inside the session it continues. */
  inSession?: boolean;
  /** Opens the session this task continues. */
  onOpenSession?: () => void;
  /** Reads the continued session again (inside it). */
  onLookAgain?: () => void;
}) {
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
  const continues = agent.continues;
  const agentName = nativeAgentDisplayName(status?.agentId);
  const hostName = taskHostName ?? "this device";
  const context = { hostName, remote: !!agent.taskHostRef, continuing: !!continues || status?.agentId === "agent.coding.codex_thread", agentName };
  const outcome = status ? taskOutcome(status, context) : null;
  const ended = !!status && !["queued", "running"].includes(status.state);
  const conversation = continues ? null : nativeAgentConversationLabel(status);

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
            : agent.canCancel ? <button type="button" className="btn btn-danger" disabled={agent.busy} onClick={() => void (taskPeer ? agent.cancelRemote(roomId, taskPeer.peerSessionId, taskPeer.hostRef!) : agent.cancel())}>Stop {agentName}</button> : null;
  const followUps = <>
    {continues && !inSession && onOpenSession ? <button type="button" className="btn btn-sm" onClick={onOpenSession}>Open the session</button> : null}
    {continues && inSession && ended && onLookAgain ? <button type="button" className="btn btn-sm" onClick={onLookAgain}><Icon name="refresh" small />Look at it again</button> : null}
    {agent.canDismiss ? <button type="button" className="btn btn-ghost btn-sm" onClick={agent.dismiss}>Dismiss</button> : null}
  </>;

  return <section className={`card task-tracker${inSession ? " in-session" : ""}`} aria-label={`${agentName} task`}>
    {status ? <header className="card-header">
      <div className="stack" style={{ gap: 2, minWidth: 0 }}>
        <h3>{inSession ? "Your continuation" : continues ? `Continuing “${continues.title ?? "untitled session"}”` : `${agentName} · ${status.workspaceName}`}</h3>
        <span className="xsmall subtle">{taskHostName ? `On ${taskHostName}` : "On this device"}{continues && !inSession ? ` · ${providerLabel(continues.provider)} session in ${continues.workspaceLabel}` : ""}{conversation ? ` · ${conversation}` : ""}</span>
      </div>
      <StatusBadge tone={outcomeTone(outcome, status)}>{outcome?.headline ?? status.state}</StatusBadge>
    </header> : null}
    <div className="card-body">
      {status ? <TaskStages status={status} remote={!!agent.taskHostRef} hostName={hostName} /> : null}
      {outcome && (outcome.detail || outcome.next) ? <div className={`task-outcome tone-${outcome.tone}`} role={outcome.tone === "danger" ? "alert" : "status"}>
        {outcome.detail ? <p>{outcome.detail}</p> : null}
        {outcome.next ? <p className="task-next"><strong>Next:</strong> {outcome.next}</p> : null}
      </div> : null}
      {status ? <NativeDecisionPanel status={status} roomId={roomId} remote={!!agent.taskHostRef} hostName={hostName} /> : null}
      {nativeAgentAwaitingNativeDecision(status) && !nativeDecisionsAwaitUser(status!, !!agent.taskHostRef, Date.now() / 1000) ? <Notice tone="warn">{agentName} on {hostName} is waiting for a native request that cannot be answered from here, so the task cannot continue; stop it to interrupt the turn. To run without approval prompts, change the approval policy in that device’s own Codex configuration.</Notice> : null}
      {status ? <NativeAgentResponse status={status} hostName={taskHostName} /> : null}
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
    {actions || continues || agent.canDismiss ? <footer className="card-footer"><span className="row spacer">{followUps}</span>{actions}</footer> : null}
  </section>;
}

/** Where the task is: sent, accepted there, running, result. */
function TaskStages({ status, remote, hostName }: { status: NativeAgentTaskStatus; remote: boolean; hostName: string }) {
  const stages = taskStages(status, { remote, hostName });
  return <ol className="task-stages" aria-label="Progress">
    {stages.map((stage) => <li key={stage.id} className={stage.state} aria-current={stage.state === "current" ? "step" : undefined}>
      <span className="mark" aria-hidden="true">{stage.state === "done" ? <Icon name="check" /> : stage.state === "stopped" ? <Icon name="close" /> : null}</span>
      <span>{stage.label}<span className="sr-only"> ({stage.state === "current" ? "now" : stage.state === "stopped" ? "ended here" : stage.state === "done" ? "done" : "not yet"})</span></span>
    </li>)}
  </ol>;
}

function outcomeTone(outcome: TaskOutcome | null, status: NativeAgentTaskStatus): LifecycleTone {
  if (outcome) return outcome.tone;
  return status.state === "running" ? "live" : "neutral";
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

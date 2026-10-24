import type { BridgePeerSession } from "../../lib/bridgePeers";
import { nativeDecisionsAwaitUser } from "../../lib/nativeDecisions";
import { taskOutcome, taskStages, type TaskOutcome } from "../../lib/nativeTaskOutcome";
import type { NativeAgentTaskStatus } from "../../lib/tauri";
import {
  nativeAgentAwaitingNativeDecision,
  nativeAgentConsequenceAbandonmentRequired,
  nativeAgentDisplayName,
  nativeAgentRemoteReconciliationRequired,
  nativeAgentResponse,
  type PastTurn,
} from "./AgentTaskLifecycle";
import { Bubble, SystemLine } from "./Conversation";
import type { NativeAgentController } from "./NativeAgentPanel";
import { NativeDecisionPanel } from "./NativeDecisionPanel";
import { Dot, Icon, Notice } from "./ui";

type Members = readonly BridgePeerSession[];

function hostNameOf(hostRef: string | null, members: Members): string {
  return hostRef ? members.find((peer) => peer.hostRef === hostRef)?.displayName ?? "the selected device" : "this device";
}

/** The turn this Bridge follows, as the last turn of its conversation: what
 * this device's user sent, then the Agent's side as it stands. Waiting for
 * the other device's Accept, running, an approval it asks for, its reply,
 * a failure and what to do about it, and every repair action are part of
 * the turn, not a separate panel. Nothing is shown as replied before the
 * reply arrives: Pastey receives a turn's reply only when it ends, so the
 * turn shows where the task is, never text that has not come. */
export function TaskTurn({ roomId, agent, peers, members }: {
  roomId: string;
  agent: NativeAgentController;
  peers: readonly BridgePeerSession[];
  members: Members;
}) {
  const status = agent.status;
  const taskHostName = agent.taskHostRef ? hostNameOf(agent.taskHostRef, members) : null;
  const hostName = taskHostName ?? "this device";
  const taskPeer = agent.taskHostRef ? peers.find((peer) => peer.hostRef === agent.taskHostRef) ?? null : null;
  const movementPeer = agent.movement ? peers.find((peer) => peer.hostRef === agent.movement?.targetHostRef) ?? null : null;
  const movementHostName = agent.movement ? members.find((peer) => peer.hostRef === agent.movement?.targetHostRef)?.displayName ?? "the selected device" : null;
  const remoteReconciliationRequired = nativeAgentRemoteReconciliationRequired(agent.status, agent.movement);
  const consequenceAbandonmentRequired = nativeAgentConsequenceAbandonmentRequired(agent.movement);
  const resultReturnRetryRequired = agent.movement?.code === "result_return_retry_required"
    || (agent.movement?.state === "interrupted" && ["result_apply_interrupted", "conflict_result_retention_required", "conflict_result_retention_failed"].includes(agent.movement.code ?? ""));
  const agentName = nativeAgentDisplayName(status?.agentId ?? "agent.coding.codex");
  const context = { hostName, remote: !!agent.taskHostRef, continuing: agent.origin?.kind === "session" || status?.agentId === "agent.coding.codex_thread", agentName };
  const outcome = status ? taskOutcome(status, context) : null;
  const active = !!status && ["queued", "running"].includes(status.state);
  const response = status ? nativeAgentResponse(status) : null;
  const followUp = agent.origin?.kind === "new" && agent.pastTurns.length > 0;

  const actions = agent.movement?.state === "awaiting_approval" && movementPeer
    ? <button type="button" className="btn btn-primary btn-sm" disabled={agent.busy} onClick={() => void agent.approveRemoteMovement(roomId, movementPeer.peerSessionId)}>Approve and send</button>
    : remoteReconciliationRequired && !agent.recoveryTaskId
      ? <button type="button" className="btn btn-sm" disabled={agent.busy} onClick={() => void agent.refreshRecoveryProjection()}>Load recovery details</button>
      : remoteReconciliationRequired
        ? <>{agent.recoveryTargetHostRef ? <button type="button" className="btn btn-primary btn-sm" disabled={agent.busy} onClick={() => void agent.reconcileRemote()}>Reconcile</button> : null}<button type="button" className="btn btn-sm" disabled={agent.busy || !agent.recoveryTaskId} onClick={() => void agent.stopRecovery()}>Stop</button></>
        : consequenceAbandonmentRequired
          ? <>{resultReturnRetryRequired ? <button type="button" className="btn btn-primary btn-sm" disabled={agent.busy || agent.retryingResultReturn} onClick={() => void agent.retryResultReturn()}>{agent.retryingResultReturn ? "Retrying result Return…" : "Retry result Return"}</button> : null}<button type="button" className="btn btn-danger btn-sm" disabled={agent.busy} onClick={() => void agent.stopRecovery()}>Abandon recovery</button></>
          : resultReturnRetryRequired
            ? <button type="button" className="btn btn-primary btn-sm" disabled={agent.busy || agent.retryingResultReturn} onClick={() => void agent.retryResultReturn()}>{agent.retryingResultReturn ? "Retrying result Return…" : "Retry result Return"}</button>
            : agent.canCancel ? <button type="button" className="btn btn-danger btn-sm" disabled={agent.busy} onClick={() => void (taskPeer ? agent.cancelRemote(roomId, taskPeer.peerSessionId, taskPeer.hostRef!) : agent.cancel())}>Stop {agentName}</button> : null;

  return <>
    {agent.prompt ? <Bubble from="me" label="Your message">{agent.prompt}</Bubble> : null}
    {status || agent.movement ? <Bubble from="agent" rich tone={outcome?.tone ?? (agent.movement?.state === "awaiting_approval" ? "pending" : "neutral")} label={`${agentName} turn`} meta={<>{agentName}{taskHostName ? ` on ${taskHostName}` : " on this device"}{response?.kind === "text" ? " · its own account, not verified" : ""}</>}>
      {status && response?.kind !== "text" ? <div className="turn-status"><Dot tone={outcome?.tone === "danger" ? "bad" : outcome?.tone === "complete" ? "ok" : outcome?.tone === "live" ? "live" : outcome?.tone === "pending" ? "warn" : "neutral"} pulse={active} /><strong>{outcome?.headline ?? status.state}</strong></div> : null}
      {status && status.state !== "completed" ? <TaskStages status={status} remote={!!agent.taskHostRef} hostName={hostName} /> : null}
      {response?.kind === "text"
        ? <><div className="turn-reply">{response.text}</div>{response.truncated ? <small className="subtle">Pastey shortened this reply to fit its size limit.</small> : null}</>
        : response?.kind === "undisplayable" ? <small className="muted">The task completed with output Pastey cannot display as text.</small>
          : response?.kind === "none" ? <small className="muted">The task completed, but no reply text came back.</small> : null}
      {outcome && status?.state !== "completed" && (outcome.detail || outcome.next) ? <div className="turn-outcome">
        {outcome.detail ? <p>{outcome.detail}</p> : null}
        {outcome.next ? <p><strong>Next:</strong> {outcome.next}</p> : null}
      </div> : null}
      {status?.conversation === "new" && followUp ? <p className="small muted">Codex could not find this conversation and started a new one, without the earlier turns.</p> : null}
      {status ? <NativeDecisionPanel status={status} roomId={roomId} remote={!!agent.taskHostRef} hostName={hostName} /> : null}
      {nativeAgentAwaitingNativeDecision(status) && !nativeDecisionsAwaitUser(status!, !!agent.taskHostRef, Date.now() / 1000) ? <Notice tone="warn">{agentName} on {hostName} is waiting for a native request that cannot be answered from here, so the task cannot continue; stop it to interrupt the turn. To run without approval prompts, change the approval policy in that device’s own Codex configuration.</Notice> : null}
      {agent.movement ? <div className="stack">
        <strong>{agent.movement.state === "awaiting_approval" ? "Review what is sent" : `Workspace movement · ${agent.movement.state.replace(/_/g, " ")}`}</strong>
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
          <div className="row"><button type="button" className="btn btn-sm" disabled={agent.busy} onClick={() => void agent.revealConflictResult()}>Reveal result</button><button type="button" className="btn btn-danger btn-sm" disabled={agent.busy} onClick={() => void agent.discardConflictResult()}>Discard result</button></div>
        </> : null}
      </div> : null}
      {actions ? <div className="turn-actions">{actions}</div> : null}
      {status ? <details className="disclosure turn-details">
        <summary><Icon name="chevron" small />Task details</summary>
        <dl className="facts disclosure-body">
          <dt>Task</dt><dd className="mono">{status.taskId}</dd>
          <dt>Runs on</dt><dd>{hostName}</dd>
          <dt>Agent</dt><dd className="mono">{status.agentId}</dd>
          <dt>Folder</dt><dd>{status.workspaceName}</dd>
          <dt>State</dt><dd className="mono">{status.state}{status.code ? ` · ${status.code}` : ""}</dd>
          {status.conversation ? <><dt>Conversation</dt><dd>{status.conversation === "continued" ? "continued" : "new"}</dd></> : null}
        </dl>
      </details> : null}
    </Bubble> : null}
    {agent.message ? <SystemLine tone="bad">{agent.message}</SystemLine> : null}
  </>;
}

/** An earlier turn of the same conversation: what was sent and how it
 * ended, without its repair actions (they belong to the latest turn). */
export function PastTurnView({ turn, hostName }: { turn: PastTurn; hostName: string }) {
  const agentName = nativeAgentDisplayName(turn.status.agentId);
  const response = nativeAgentResponse(turn.status);
  const outcome = taskOutcome(turn.status, { hostName, remote: hostName !== "this device", continuing: false, agentName });
  return <>
    <Bubble from="me">{turn.prompt}</Bubble>
    <Bubble from="agent" rich tone={response?.kind === "text" ? "neutral" : outcome.tone} meta={<>{agentName} on {hostName}{response?.kind === "text" ? " · its own account, not verified" : ""}</>}>
      {response?.kind === "text" ? <div className="turn-reply">{response.text}</div> : <div className="turn-status"><strong>{outcome.headline}</strong></div>}
    </Bubble>
  </>;
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

/** The followed task in one line, where its conversation is not open: what
 * it is, where it stands, and the way back to it. */
export function TaskBanner({ agent, members, onOpen }: { agent: NativeAgentController; members: Members; onOpen: () => void }) {
  const status = agent.status;
  if (!status && !agent.movement) return null;
  const hostName = hostNameOf(agent.taskHostRef, members);
  const agentName = nativeAgentDisplayName(status?.agentId ?? "agent.coding.codex");
  const outcome: TaskOutcome | null = status ? taskOutcome(status, { hostName, remote: !!agent.taskHostRef, continuing: agent.origin?.kind === "session", agentName }) : null;
  const active = !!status && ["queued", "running"].includes(status.state);
  const what = agent.origin?.kind === "session"
    ? `“${agent.origin.session.title ?? "untitled session"}”`
    : agent.origin?.kind === "new" ? `New conversation in ${agent.origin.place.label}` : status?.workspaceName ?? "Workspace movement";
  return <button type="button" className={`task-banner tone-${outcome?.tone ?? "pending"}`} onClick={onOpen}>
    <Dot tone={outcome?.tone === "danger" ? "bad" : outcome?.tone === "complete" ? "ok" : outcome?.tone === "live" ? "live" : "warn"} pulse={active} />
    <span className="task-banner-text"><strong>{outcome?.headline ?? "Review what is sent"}</strong><small>{what} · {hostName}</small></span>
    <Icon name="chevron" small />
  </button>;
}

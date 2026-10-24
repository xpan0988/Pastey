/** Where one Agent task is, and what its outcome means for the user.
 * Presentation only: Rust decides every state and code; this explains
 * them in one place, so a run reads the same wherever it is shown. */
import type { NativeAgentTaskStatus } from "./tauri";

export type StageState = "done" | "current" | "todo" | "stopped";

export interface TaskStage {
  id: "sent" | "accepted" | "running" | "result";
  label: string;
  state: StageState;
}

export interface TaskOutcome {
  tone: "neutral" | "pending" | "live" | "danger" | "complete";
  headline: string;
  detail: string | null;
  /** What the user can do about it, when anything. */
  next: string | null;
  /** Definitely nothing ran: a refusal, a Deny or an expiry before start. */
  nothingRan: boolean;
}

export interface TaskContext {
  /** The device it runs on, or "this device". */
  hostName: string;
  remote: boolean;
  /** It continues an existing session rather than starting a new turn of
   * Pastey's own conversation. */
  continuing: boolean;
  agentName: string;
}

const AWAITING_NATIVE_DECISION = "native_agent_awaiting_native_decision";
const REVIEW_REQUIRED = "native_agent_review_required";
/** Codes that end a task before anything ran. */
const NOTHING_RAN = new Set([
  "native_agent_start_failed",
  "native_agent_admission_denied",
  "native_agent_review_expired",
  "native_agent_admission_revoked",
  "native_agent_not_delivered",
  "native_agent_start_rejected",
  "native_agent_conversation_unavailable",
]);

export function taskNothingRan(status: Pick<NativeAgentTaskStatus, "state" | "code">): boolean {
  return status.state === "failed" && NOTHING_RAN.has(status.code ?? "");
}

/** The stages a task passes, with where it is now. A task on another
 * device passes that device's Accept (or its unattended permission). */
export function taskStages(status: Pick<NativeAgentTaskStatus, "state" | "code">, context: Pick<TaskContext, "hostName" | "remote">): TaskStage[] {
  const stages: TaskStage[] = context.remote
    ? [
      { id: "sent", label: `Sent to ${context.hostName}`, state: "done" },
      { id: "accepted", label: "Accepted there", state: "todo" },
      { id: "running", label: "Running", state: "todo" },
      { id: "result", label: "Result", state: "todo" },
    ]
    : [
      { id: "sent", label: "Started here", state: "done" },
      { id: "running", label: "Running", state: "todo" },
      { id: "result", label: "Result", state: "todo" },
    ];
  const set = (id: TaskStage["id"], state: StageState) => {
    const stage = stages.find((candidate) => candidate.id === id);
    if (stage) stage.state = state;
  };
  const through = (id: TaskStage["id"]) => {
    for (const stage of stages) {
      if (stage.id === id) break;
      stage.state = "done";
    }
  };
  switch (status.state) {
    case "queued":
      if (context.remote) set("accepted", "current");
      else set("running", "current");
      break;
    case "running":
      through("running");
      set("running", "current");
      break;
    case "completed":
      for (const stage of stages) stage.state = "done";
      break;
    default: {
      if (taskNothingRan(status)) {
        // It ended where it was refused: at the Accept for a Deny, expiry or
        // revoked access there; at the start otherwise.
        const refusedAt = context.remote && ["native_agent_admission_denied", "native_agent_review_expired", "native_agent_admission_revoked", "native_agent_not_delivered"].includes(status.code ?? "")
          ? "accepted"
          : "running";
        through(refusedAt);
        set(refusedAt, "stopped");
      } else {
        through("result");
        set("result", "stopped");
      }
    }
  }
  // Each stage says what happened there, not only its name.
  const words: Record<TaskStage["id"], Partial<Record<StageState, string>>> = {
    sent: {},
    accepted: { current: "Waiting for Accept", stopped: "Not accepted", todo: "Accept there" },
    running: { stopped: status.state === "failed" && taskNothingRan(status) ? "Did not start" : "Running" },
    result: { stopped: status.state === "interrupted" ? "Outcome unknown" : status.state === "cancelled" ? "Stopped" : "Failed" },
  };
  for (const stage of stages) stage.label = words[stage.id][stage.state] ?? stage.label;
  return stages;
}

/** The task's state in words: a headline, why, and what to do next. */
export function taskOutcome(status: Pick<NativeAgentTaskStatus, "state" | "code" | "result">, context: TaskContext): TaskOutcome {
  const { hostName, remote, continuing, agentName } = context;
  const there = remote ? ` on ${hostName}` : "";
  const reread = continuing ? `Look at the session again to see where it stands now.` : null;
  const result = status.result?.trim() || null;
  switch (status.state) {
    case "queued":
      return status.code === REVIEW_REQUIRED
        ? { tone: "pending", headline: `Waiting for someone on ${hostName} to Accept`, detail: `${hostName} has no permission to run this without asking, so it shows its user a request. Nothing runs until they accept; if nobody answers, the request expires and nothing runs.`, next: null, nothingRan: false }
        : { tone: "pending", headline: remote ? `Sent to ${hostName}` : "Starting", detail: remote ? `${hostName} is checking the request.` : null, next: null, nothingRan: false };
    case "running":
      return status.code === AWAITING_NATIVE_DECISION
        ? { tone: "pending", headline: `${agentName} is waiting for an approval`, detail: `${agentName}${there} asked before running a command or changing files. Answer it below; nothing continues until it is answered or the task is stopped.`, next: null, nothingRan: false }
        : { tone: "live", headline: `${agentName} is working${there}`, detail: continuing ? `It runs in the session itself, as one more turn of it.` : null, next: null, nothingRan: false };
    case "completed":
      return { tone: "complete", headline: "Completed", detail: continuing ? `The session${there} now ends with this turn. Its reply is below; it is ${agentName}'s own account, not verified.` : `Its reply is below; it is ${agentName}'s own account, not verified.`, next: null, nothingRan: false };
    case "cancelled":
      return { tone: "neutral", headline: "Stopped", detail: `Stopped at your request. Anything ${agentName} did before it stopped stays${remote ? ` on ${hostName}` : ""}; nothing runs again on its own.`, next: reread, nothingRan: false };
    case "interrupted":
      if (["native_agent_outcome_unknown", "native_agent_reconciliation_required"].includes(status.code ?? "")) {
        return { tone: "danger", headline: "Outcome unknown", detail: remote ? `This device lost track of the run on ${hostName} while ${agentName} worked (the connection dropped, or Pastey restarted there). It may have finished, failed or stopped.` : `Pastey lost track of ${agentName} while it worked. It may have finished, failed or stopped.`, next: remote ? `Reconcile asks ${hostName} what it knows now. If it does not know either, check the folder there yourself, then Stop. Nothing runs again on its own.` : "Check the folder yourself, then Stop. Nothing runs again on its own.", nothingRan: false };
      }
      return { tone: "danger", headline: "Interrupted", detail: `${agentName}${there} reports that the turn was interrupted before it finished.${result ? ` ${result}` : ""}`, next: reread, nothingRan: false };
    case "failed":
      break;
    default:
      return { tone: "neutral", headline: String(status.state).replace(/_/g, " "), detail: result, next: null, nothingRan: false };
  }
  switch (status.code) {
    case "native_agent_start_failed":
      return { tone: "danger", headline: "Did not start · nothing ran", detail: result ?? `${agentName}${there} refused before running anything.`, next: startFailedNext(result, hostName, continuing), nothingRan: true };
    case "native_agent_admission_denied":
      return { tone: "neutral", headline: `Declined on ${hostName} · nothing ran`, detail: `Someone on ${hostName} chose Deny.`, next: null, nothingRan: true };
    case "native_agent_review_expired":
      return { tone: "neutral", headline: "Nobody accepted in time · nothing ran", detail: `The request waited on ${hostName} until it expired.`, next: `Send it again when someone is at ${hostName}.`, nothingRan: true };
    case "native_agent_admission_revoked":
      return { tone: "danger", headline: "Access ended before it started · nothing ran", detail: `Between the request and the Accept, ${hostName} stopped allowing it: the folder's sharing or one of its switches changed, it expired, or this Bridge's connection changed.`, next: `Check the folder's sharing on ${hostName} (Bridge details → Workspaces there), then try again.`, nothingRan: true };
    case "native_agent_not_delivered":
      return { tone: "danger", headline: "Not delivered · nothing ran", detail: result ?? `${hostName} refused the request before acting on it.`, next: null, nothingRan: true };
    case "native_agent_start_rejected":
      return { tone: "danger", headline: `${hostName === "this device" ? "Pastey" : hostName} could not start it · nothing ran`, detail: result ?? `It was accepted, but ${agentName} refused its input or could not start.`, next: reread, nothingRan: true };
    case "native_agent_conversation_unavailable":
      return { tone: "danger", headline: "Conversation unavailable · nothing ran", detail: `${agentName}${there} could not reopen your earlier conversation in this folder.`, next: "Start a new conversation to run without that context.", nothingRan: true };
    case "native_agent_interrupted_on_restart":
      return { tone: "danger", headline: "Stopped when Pastey quit", detail: `Pastey quit while ${agentName} worked, and ${agentName} stopped with it. Whatever it did before then stays in the folder; it does not run again on its own.`, next: reread, nothingRan: false };
    case "native_agent_failed":
      return { tone: "danger", headline: `${agentName} reported the turn failed`, detail: `The turn ran${there} and ended with a failure${result ? `: ${result}` : "."}${continuing ? " The session now ends with that failed turn." : ""}`, next: continuing ? `Open the session to see what it recorded, then continue it or start a new task.` : "Look at the folder or its sessions to see what happened, then try again.", nothingRan: false };
    default:
      return { tone: "danger", headline: "Failed", detail: result, next: reread, nothingRan: false };
  }
}

/** The fix for a refusal before start, from the Host's own reason. */
function startFailedNext(result: string | null, hostName: string, continuing: boolean): string | null {
  const reason = result?.toLowerCase() ?? "";
  if (reason.includes("open in another codex client")) return `Close the thread in the Codex app or the terminal on ${hostName}, then look at the session again and continue.`;
  if (reason.includes("changed since you looked") || reason.includes("no longer matches")) return "Look at the session again, read what changed, then continue.";
  if (reason.includes("turn in progress") || reason.includes("pastey task is running")) return "Wait for that turn to finish, then look at the session again.";
  if (reason.includes("writer locks")) return `Update Codex on ${hostName} to 0.163 or later.`;
  return continuing ? "Look at the session again, then try once more." : null;
}

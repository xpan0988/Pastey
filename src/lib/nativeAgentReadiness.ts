/** What Run with Codex still needs before it can start, in the order the
 * user should fix it. Presentation only: Rust re-checks the Host session,
 * the capability, the invocation and every permission when it is sent, and
 * the executing device still decides (its Accept or its own unattended
 * permission). Which folders it shares, and for what, comes only from
 * that device's own answer; reading its sessions is a separate authority. */
import type { NativeAgentTargetAvailability } from "../features/workspace/AgentTaskLifecycle";
import type { SharedWorkspace } from "./tauri";
import { peerShareGuidance, type PeerShareKnowledge } from "./workspaceShares";

/** Where the folder Codex works in lives, for a run on another device. */
export type CodexWorkspaceSource =
  /** A folder the executing device shares with this Bridge, chosen by its
   * name; that device resolves its own path. Nothing moves. */
  | "shared"
  /** Already on the executing device, by a path typed here: Codex works
   * there, nothing moves. */
  | "on_target"
  /** On this device: Pastey reviews sending it, Codex runs on the copy,
   * and the result returns here to be applied. */
  | "send_from_here";

export type CodexRunTarget =
  | { kind: "local" }
  | { kind: "remote"; name: string; connected: boolean };

export type CodexRunAction = "run_local" | "run_remote" | "review_movement";

export interface CodexRunInput {
  target: CodexRunTarget;
  availability: NativeAgentTargetAvailability;
  workspaceSource: CodexWorkspaceSource;
  workspace: string;
  taskText: string;
  busy: boolean;
  movementBlocksNewRun: boolean;
  /** For "shared": what the executing device shares, and the one chosen. */
  shares?: PeerShareKnowledge;
  sharedWorkspace?: SharedWorkspace | null;
}

export type CodexRequirementId = "device" | "codex" | "workspace" | "task" | "movement";

export interface CodexRequirement {
  id: CodexRequirementId;
  label: string;
  met: boolean;
  /** What to do when it is not met. */
  reason: string | null;
}

export interface CodexRunReadiness {
  action: CodexRunAction;
  actionLabel: string;
  requirements: CodexRequirement[];
  /** The first unmet requirement's reason, or null when it can start. */
  blocker: string | null;
  ready: boolean;
  /** What happens after the click, shown when ready. */
  next: string;
}

export function codexRunAction(target: CodexRunTarget, source: CodexWorkspaceSource): CodexRunAction {
  if (target.kind === "local") return "run_local";
  return source === "send_from_here" ? "review_movement" : "run_remote";
}

export function codexRunReadiness(input: CodexRunInput): CodexRunReadiness {
  const { target, availability } = input;
  const where = target.kind === "remote" ? target.name : "this device";
  const action = codexRunAction(target, input.workspaceSource);
  const requirements: CodexRequirement[] = [];

  if (target.kind === "remote") {
    requirements.push({
      id: "device",
      label: `${target.name} is connected`,
      met: target.connected,
      reason: target.connected ? null : `${target.name} is not connected right now. Codex can run there once this Bridge reconnects.`,
    });
  }

  const codexReady = availability.state === "available";
  requirements.push({
    id: "codex",
    label: `Codex is available on ${where}`,
    met: codexReady,
    reason: codexReady ? null : availabilityBlocker(availability, where),
  });

  if (target.kind === "remote" && input.workspaceSource === "shared") {
    const shares = input.shares ?? { state: "unknown" };
    const chosen = input.sharedWorkspace ?? null;
    const problem = shares.state === "unknown"
      ? `Asking ${where} which folders it shares with this Bridge…`
      : peerShareGuidance(where, shares, "run", chosen) ?? (chosen ? null : `Choose one of the folders ${where} shares.`);
    requirements.push({
      id: "workspace",
      label: chosen ? `${chosen.label}, shared by ${where} for agent requests` : `A folder ${where} shares for agent requests`,
      met: problem === null,
      reason: problem,
    });
  } else {
    const hasWorkspace = input.workspace.trim().length > 0;
    requirements.push({
      id: "workspace",
      label: action === "run_remote" ? `Folder on ${where}` : "Folder on this device",
      met: hasWorkspace,
      reason: hasWorkspace ? null : action === "run_remote"
        ? `Enter the full path of the folder on ${where} where Codex should work.`
        : action === "review_movement"
          ? "Choose the folder on this device to send."
          : "Choose the folder Codex should work in.",
    });
  }

  const hasTask = input.taskText.trim().length > 0;
  requirements.push({
    id: "task",
    label: "Task description",
    met: hasTask,
    reason: hasTask ? null : "Describe what Codex should do.",
  });

  if (input.movementBlocksNewRun) {
    requirements.push({
      id: "movement",
      label: "No unresolved workspace movement",
      met: false,
      reason: "An earlier workspace movement is still unresolved. Finish or resolve it above before starting another run.",
    });
  }

  const blocker = requirements.find((requirement) => !requirement.met)?.reason ?? null;
  return {
    action,
    actionLabel: input.busy ? "Starting…" : action === "review_movement" ? "Review movement" : "Run with Codex",
    requirements,
    blocker,
    ready: blocker === null && !input.busy,
    next: action === "run_remote"
      ? `${where} asks its user to accept unless you hold an unattended permission there. No files move.`
      : action === "review_movement"
        ? `Next you review exactly what is sent to ${where}. Nothing moves until you approve.`
        : "Codex runs on this device in its own native session.",
  };
}

function availabilityBlocker(availability: NativeAgentTargetAvailability, where: string): string {
  switch (availability.state) {
    case "checking": return `Checking Codex on ${where}…`;
    case "unknown": return `Pastey could not check Codex on ${where}. Check again.`;
    case "incompatible": return `Codex on ${where} needs an update before Pastey can use it.`;
    case "unavailable": return `Codex is not available on ${where}.`;
    case "available": return "";
  }
}

/** A path typed for another device. Only a hint: the executing device
 * resolves and checks it. */
export function looksLikeFullPath(path: string): boolean {
  const value = path.trim();
  return /^[a-zA-Z]:[\\/]/.test(value) || value.startsWith("/") || value.startsWith("\\\\");
}

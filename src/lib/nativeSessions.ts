/** Presentation for native session discovery. Rust decides every status;
 * these helpers only explain it. Nothing here infers that a session runs or
 * finished from its last message. */
import type {
  CreateSessionReadGrant,
  EvidenceBasis,
  LocalNativeSession,
  NativeSessionFilters,
  NativeSessionProvider,
  RemoteNativeSession,
  RemoteNativeSessionResponse,
  RemoteNativeSessionSnapshot,
  SessionReadGrant,
} from "./tauri";

/** A session as the panel shows it, whether it is on this device or another. */
export type SessionView = Omit<RemoteNativeSession, "scopeRef" | "workspaceLabel"> & { scopeRef: string; workspaceLabel: string };

export function localSessionView(session: LocalNativeSession): SessionView {
  const { workspace, originRaw: _originRaw, ...rest } = session;
  const parts = workspace.split(/[\\/]/).filter(Boolean);
  return { ...rest, scopeRef: workspace, workspaceLabel: parts[parts.length - 1] ?? workspace };
}

export function providerLabel(provider: NativeSessionProvider): string {
  return provider === "claude_code" ? "Claude Code" : "Codex";
}

const ORIGINS: Record<string, string> = { desktop: "desktop app", cli: "terminal", pastey: "Pastey", sdk: "SDK", other: "other", unknown: "unknown origin" };

export function originLabel(origin: string): string {
  return ORIGINS[origin] ?? origin;
}

export interface StatusPresentation {
  label: string;
  tone: "live" | "done" | "warning" | "unknown";
  explanation: string;
}

/** What a session's status means, from Rust's status and its basis only. */
export function sessionStatusPresentation(session: Pick<SessionView, "status" | "statusBasis" | "live" | "provider">): StatusPresentation {
  const name = providerLabel(session.provider);
  switch (session.status) {
    case "running":
      return { label: "Running", tone: "live", explanation: `${name} reports it executing now (${session.live.source}).` };
    case "idle":
      return session.live.state === "attached"
        ? { label: "Open, idle", tone: "live", explanation: `A ${name} process holds it but is not executing.` }
        : { label: "Idle", tone: "done", explanation: "No recorded turn, and nothing runs it." };
    case "completed":
      return { label: "Last turn ended", tone: "done", explanation: "Nothing runs it, and its last recorded turn ended normally. That is not proof the task is done." };
    case "interrupted":
      return session.statusBasis === "inference"
        ? { label: "Interrupted (inferred)", tone: "warning", explanation: "Nothing runs it, yet its history stops mid-turn: the process ended before the turn did." }
        : { label: "Interrupted", tone: "warning", explanation: "Its last recorded turn was interrupted." };
    case "failed":
      return { label: "Last turn failed", tone: "warning", explanation: "Nothing runs it, and its last recorded turn failed." };
    case "unknown":
      return { label: "Unknown", tone: "unknown", explanation: `Whether it runs now cannot be shown: ${session.live.detail ?? "no live source"}.` };
  }
}

export function resumeLabel(session: Pick<SessionView, "resume">): string {
  switch (session.resume.verdict) {
    case "safe": return "Could be continued safely now";
    case "unsafe": return "Held by a running process";
    case "unknown": return "Continuing is not shown to be safe";
  }
}

export function relativeTime(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return "unknown time";
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
  if (seconds < 2 * 86400) return `${Math.round(seconds / 3600)} h ago`;
  return `${Math.round(seconds / 86400)} days ago`;
}

export function evidenceGroups(evidence: Array<{ basis: EvidenceBasis; statement: string; source: string }>) {
  return {
    facts: evidence.filter((item) => item.basis === "fact"),
    inferences: evidence.filter((item) => item.basis === "inference"),
    unknowns: evidence.filter((item) => item.basis === "unknown"),
  };
}

/** What to tell the user when a peer Host does not answer with sessions. */
export function remoteOutcomeMessage(response: Pick<RemoteNativeSessionResponse, "outcome">, hostName: string): string | null {
  switch (response.outcome) {
    case "ok": return null;
    case "not_authorized": return `${hostName} has not allowed this device to read its sessions. On ${hostName}, open Settings → Session reading, choose this device and the folder to share, then look again here.`;
    case "not_found": return `${hostName} has no such session in the folders it shares with this device.`;
    case "busy": return `${hostName} is still answering an earlier request from this Bridge. Try again in a moment.`;
    case "unavailable": return `${hostName} could not read its sessions now.`;
  }
}

export const ACTIVITY_FILTERS: ReadonlyArray<{ seconds: number | null; label: string }> = [
  { seconds: null, label: "Any time" },
  { seconds: 3600, label: "Last hour" },
  { seconds: 86400, label: "Last day" },
  { seconds: 7 * 86400, label: "Last week" },
];

export function sessionFilters(form: { provider: string; text: string; activeWithinSecs: number | null; scopeRef: string }): NativeSessionFilters {
  return {
    provider: form.provider === "claude_code" || form.provider === "codex" ? form.provider : null,
    text: form.text.trim() ? form.text.trim() : null,
    activeWithinSecs: form.activeWithinSecs,
    scopeRef: form.scopeRef || null,
  };
}

/** A local snapshot in the shape the panel shows. */
export function snapshotActivityLine(snapshot: { historyComplete: boolean; activity: Omit<RemoteNativeSessionSnapshot["activity"], "changedElsewhere"> & { changedElsewhere?: number } }): string {
  const activity = snapshot.activity;
  const elsewhere = "changedElsewhere" in activity && activity.changedElsewhere ? ` (+${activity.changedElsewhere} outside the folder)` : "";
  return `${snapshot.historyComplete ? "" : "Newest part of the history: "}${activity.userTurns} prompt(s), ${activity.toolCalls} tool call(s), ${activity.commands} command(s) (${activity.failedCommands} failed), ${activity.changedFiles.length} changed file(s)${elsewhere}.`;
}

export const READ_GRANT_DURATIONS: ReadonlyArray<{ hours: number; label: string }> = [
  { hours: 1, label: "1 hour" },
  { hours: 24, label: "1 day" },
  { hours: 168, label: "7 days" },
  { hours: 720, label: "30 days" },
];

export function readGrantFormProblem(form: Partial<CreateSessionReadGrant>): string | null {
  if (!form.deviceFingerprint || !form.roomId) return "Choose a trusted device that is connected now.";
  if (!form.workspacePath) return "Choose the one folder whose sessions it may read.";
  if (!READ_GRANT_DURATIONS.some((duration) => duration.hours === form.durationHours)) return "Choose how long the permission lasts.";
  return null;
}

export function readGrantStateLabel(grant: SessionReadGrant): string {
  switch (grant.state) {
    case "active": return grant.shareExcerpts ? "Active · statuses and short excerpts" : "Active · statuses only";
    case "expired": return "Expired";
    case "revoked": return "Revoked";
    case "unavailable": return "Not usable: the device was revoked, the Bridge ended, or the folder moved";
  }
}

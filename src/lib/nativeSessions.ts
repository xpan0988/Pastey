/** Presentation for native session discovery. Rust decides every status;
 * these helpers only explain it. Nothing here infers that a session runs or
 * finished from its last message. */
import { shareLocation } from "./workspaceShares";
import type {
  SharedWorkspace,
  EvidenceBasis,
  LocalNativeSession,
  NativeSessionFilters,
  NativeSessionProvider,
  RemoteNativeSession,
  RemoteNativeSessionResponse,
  RemoteNativeSessionSnapshot,
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
      return liveSourceKind(session.live.source) === "pastey"
        ? { label: "Running", tone: "live", explanation: "A Pastey task is running a turn in it now." }
        : { label: "Running", tone: "live", explanation: `${name} reports it executing now (${session.live.source}).` };
    case "idle":
      return session.live.state === "attached"
        ? liveSourceKind(session.live.source) === "pastey"
          ? { label: "Open in Pastey, idle", tone: "done", explanation: "Pastey's own conversation in this folder keeps it open between tasks. Nothing is running." }
          : { label: "Open, idle", tone: "live", explanation: `A ${name} process holds it but is not executing.` }
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

/** What the last look found about who has the session open. An
 * observation, not a promise: continuing is decided again at launch. */
export function resumeLabel(session: Pick<SessionView, "resume">): string {
  switch (session.resume.verdict) {
    case "safe": return "Nothing else had it open when Pastey looked";
    case "unsafe": return "Open in another process when Pastey looked";
    case "unknown": return "Whether anything had it open could not be shown";
  }
}

/** The live sources Rust names (`sessions/codex.rs`, `sessions/claude.rs`).
 * Each answers "is it open anywhere?" with a different reach. */
export type LiveSourceKind = "pastey" | "lock" | "process" | "claude" | "other";

export function liveSourceKind(source: string): LiveSourceKind {
  if (source === "Pastey's own Codex session") return "pastey";
  if (source === "Codex thread writer lock") return "lock";
  if (source === "process list") return "process";
  if (source.startsWith("claude agents")) return "claude";
  return "other";
}

/** "Is it open anywhere?" in plain words, with how Pastey knows. */
export function sessionHoldLine(session: Pick<SessionView, "provider" | "live">, hostName: string): { text: string; how: string } {
  const live = session.live;
  const kind = liveSourceKind(live.source);
  if (kind === "pastey") {
    return live.state === "running"
      ? { text: "A Pastey task is running a turn in it now.", how: "Pastey's own record of the Codex sessions it has open." }
      : { text: "Pastey's own conversation in this folder has it open, idle. Continuing closes that first; nothing is lost.", how: "Pastey's own record of the Codex sessions it has open." };
  }
  if (kind === "lock") {
    const how = "Codex's writer lock for this exact thread, read without taking it.";
    if (live.state === "running") return { text: `Open in the Codex app or a terminal on ${hostName}, in the middle of a turn.`, how };
    if (live.state === "attached") return { text: `Open in the Codex app or a terminal on ${hostName}.`, how };
    return { text: `Not open in any Codex client on ${hostName}.`, how };
  }
  if (kind === "process") {
    const how = `${hostName}'s list of running programs. There, Codex's per-thread lock cannot be read without taking it, so this covers every Codex app and terminal on the device at once.`;
    if (live.state === "not_running") return { text: `No Codex app or terminal is running on ${hostName}, so nothing has it open.`, how };
    return { text: `Codex is running on ${hostName} (the app or a terminal), and Pastey cannot tell which threads it has open. If it has this one open, Codex refuses the continuation and nothing runs.`, how };
  }
  if (kind === "claude") {
    const how = "Claude Code's own list of active sessions (claude agents).";
    switch (live.state) {
      case "running": return { text: `Claude Code is running a turn in it on ${hostName}.`, how };
      case "attached": return { text: `A Claude Code process on ${hostName} holds it, idle (the desktop app, a terminal or a background session).`, how };
      case "not_running": return { text: `No Claude Code process on ${hostName} holds it.`, how };
      case "unknown": return { text: `Whether a Claude Code process holds it cannot be shown: ${live.detail ?? "no answer"}.`, how };
    }
  }
  return { text: live.state === "unknown" ? `Not known: ${live.detail ?? "no live source"}.` : `${live.state.replace("_", " ")}${live.detail ? `: ${live.detail}` : ""}.`, how: live.source };
}

/** Where a session lives and why it outlasts any app: it is the
 * provider's own saved history on that device, not a running program. */
export function sessionPersistenceLine(session: Pick<SessionView, "provider" | "workspaceLabel">, hostName: string): string {
  return session.provider === "codex"
    ? `Saved by Codex on ${hostName}, for the folder ${session.workspaceLabel}. A thread is Codex's saved history, not a running program: it stays when the Codex app or a terminal quits, and Pastey reads it through a read-only Codex of its own.`
    : `Saved by Claude Code on ${hostName}, for the folder ${session.workspaceLabel}. A session is Claude Code's saved transcript, not a running program: it stays when Claude Code quits, and Pastey reads that transcript directly.`;
}

/** A short verdict for a list row: can it be continued from here now? */
export function continuationChip(
  session: Pick<SessionView, "provider" | "resume" | "lastTurn" | "status" | "live">,
  onThisDevice: boolean,
  hostName?: string,
  share?: Pick<SharedWorkspace, "label" | "readSessions" | "agentRequests"> | null,
): { label: string; tone: "ok" | "live" | "warn" | "neutral" } {
  if (session.status === "running") return { label: "Running", tone: "live" };
  const availability = continuationAvailability(session, onThisDevice, hostName, share);
  if (availability.allowed) return session.resume.verdict === "safe" ? { label: "Can continue", tone: "ok" } : { label: "Can try", tone: "neutral" };
  if (session.resume.verdict === "unsafe") return { label: "Open elsewhere", tone: "warn" };
  if (!onThisDevice && session.provider !== "codex") return { label: "Only on that device", tone: "neutral" };
  return { label: "Can't continue", tone: "neutral" };
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
    case "not_authorized": return `${hostName} hasn't shared any session-readable workspace with this Bridge. ${shareLocation(hostName)} and choose Share a workspace, with “Sessions” on. Then look again here.`;
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

/** Whether one session may be continued from here, and why not. Mirrors the
 * Host's own check (`continuation_problem`), which runs again at the moment
 * of launch; this only explains it in advance. */
export function continuationAvailability(
  session: Pick<SessionView, "provider" | "resume" | "lastTurn"> & { live?: Pick<SessionView["live"], "source"> },
  onThisDevice: boolean,
  hostName = onThisDevice ? "this device" : "the other device",
  /** On another device: the share of the session's folder, as that device
   * last listed it (`null`: no longer listed; omitted: not known). */
  share?: Pick<SharedWorkspace, "label" | "readSessions" | "agentRequests"> | null,
): { allowed: boolean; reason: string } {
  if (!onThisDevice && session.provider !== "codex") {
    return { allowed: false, reason: `Continuing a Claude Code session on another device is not available: a background Claude Code session can outlive Pastey on ${hostName}, and its outcome could be lost. Continue it on ${hostName}.` };
  }
  if (session.resume.verdict === "unsafe") {
    return {
      allowed: false,
      reason: session.provider === "codex"
        ? liveSourceKind(session.live?.source ?? "") === "pastey"
          ? "A Pastey task is running a turn in this thread now. Wait for it to finish, then look again."
          : `This Codex thread is open in another Codex client on ${hostName} (the Codex app or a terminal). Close it there, then look again.`
        : "A Claude Code process holds this session (the desktop app, a terminal or a background session). Let it finish or close it, then look again.",
    };
  }
  if (session.provider === "codex") {
    if (session.lastTurn.outcome === "unreadable") return { allowed: false, reason: "This thread's history cannot be read, so it cannot be continued safely." };
    if (!onThisDevice) {
      if (share === null) return { allowed: false, reason: `${hostName} no longer shares this folder with this Bridge. Look for sessions again.` };
      if (share && !(share.readSessions && share.agentRequests)) return { allowed: false, reason: `${hostName} shares ${share.label} for its sessions, but not for agent requests, so a continuation would be refused. ${shareLocation(hostName)} → ${share.label}, and turn on “Agent requests”.` };
      if (!session.lastTurn.marker) return { allowed: false, reason: `${hostName} did not say which turn this thread ends with, so Pastey cannot check that it is unchanged. Update Pastey on ${hostName}.` };
      return { allowed: true, reason: `Someone on ${hostName} must Accept. Then Codex there reopens this thread, refusing if the Codex app or a terminal has it open, and Pastey checks that it still ends with the turn you see here.` };
    }
    return { allowed: true, reason: "Codex reopens this thread, refusing if the Codex app or a terminal has it open, and Pastey checks that it still ends with the turn you see here." };
  }
  if (session.resume.verdict === "unknown") {
    return { allowed: false, reason: `Pastey cannot show that nothing holds this session: ${session.resume.reasons.join("; ") || "no live source"}.` };
  }
  return { allowed: true, reason: "Claude Code continues it under its own id only if nothing holds it at launch; otherwise Pastey stops the copy it starts and reports the outcome as unknown." };
}

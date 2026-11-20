import { invoke } from "@tauri-apps/api/core";

/** Agent collaborations on this device (`src-tauri/src/collaboration.rs`).
 * Members are Pastey conversations, one per provider per folder. A message
 * between members waits for this device's user to approve it, unless the
 * user gave its recipient an auto grant (a few turns within a few hours); the
 * user's own messages need no approval. Delivery always happens at a turn
 * boundary. */

export type CollaborationProvider = "codex" | "claude_code";
export type CollaborationState = "active" | "paused" | "ended";
export type CollaborationTaskState = "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted";
/** Where an entry's content comes from: Pastey's own fact, an Agent's
 * statement (what it said, not proof of it), or this device's user. */
export type CollaborationBasis = "fact" | "claim" | "user";

/** Bounds of the auto grant (`collaboration.rs`). */
export const MAX_AUTO_TURNS = 20;
export const MAX_AUTO_HOURS = 24;
export const DEFAULT_AUTO_TURNS = 3;
export const DEFAULT_AUTO_HOURS = 2;

/** This device's user lets the member take `turns` turns until `expiresAt`
 * (seconds) without an approval. */
export interface AutoGrant {
  turns: number;
  used: number;
  grantedAt: number;
  expiresAt: number;
}

export type PlanStepState = "pending" | "in_progress" | "completed";
export type ActionState = "running" | "done" | "failed";

/** What a running turn's harness reported so far (runtime reports; the
 * plan is the Agent's own statement). */
export interface TaskActivity {
  since: number;
  actions: number;
  commands: number;
  failedCommands: number;
  files: string[];
  filesChanged: number;
  plan: Array<{ text: string; state: PlanStepState }>;
  recent: Array<{ label: string; state: ActionState }>;
}

export interface RunningTurn {
  taskId: string;
  capabilityId: string;
  activity?: TaskActivity | null;
}

export interface CollaborationMember {
  alias: string;
  provider: CollaborationProvider;
  workspace: string;
  role: string;
  paused: boolean;
  grant?: AutoGrant;
  /** The automatic turn running now. */
  autoTask?: string;
  /** A task, movement or unresolved outcome holds its folder now. */
  busy: boolean;
  /** Messages for it that wait for approval, are approved, or wait for its
   * automatic turn. */
  waiting: number;
  /** The turn running in its folder, with what it has done so far. */
  live?: RunningTurn;
}

export interface CollaborationSummary {
  id: string;
  name: string;
  goal: string;
  state: CollaborationState;
  members: CollaborationMember[];
  needsYou: number;
  lastActivityAt: number;
}

export interface PublishedSummary {
  done?: string;
  decisions?: string;
  open?: string;
  next?: string;
}

export type CollaborationUserAction =
  | { type: "created"; name: string; goal: string }
  | { type: "paused" }
  | { type: "resumed" }
  | { type: "ended" }
  | { type: "member_resumed"; alias: string };

/** What the Journal keeps of a turn's reported activity. */
export interface TurnActivity {
  actions: number;
  commands: number;
  failedCommands: number;
  filesChanged: number;
  files?: string[];
}

export type GrantEnd = "used_up" | "expired" | "withdrawn" | "collaboration_ended";

export type CollaborationEntryBody =
  | { type: "turn"; taskId: string; origin: "collaboration" | "user"; state: CollaborationTaskState; code?: string; automatic?: boolean; activity?: TurnActivity }
  | { type: "message"; from: string; to: string; subject: string; body: string; hop: number }
  | { type: "summary"; summary: PublishedSummary }
  | { type: "attention"; text: string }
  | { type: "user"; action: CollaborationUserAction }
  | { type: "member"; alias: string; provider: CollaborationProvider; role: string }
  | { type: "grant"; alias: string; change: "granted"; turns: number; expiresAt: number }
  | { type: "grant"; alias: string; change: "ended"; reason: GrantEnd };

export type CollaborationEntryStatus =
  | { state: "waiting" }
  | { state: "auto"; attempts: number }
  | { state: "approved"; attempts: number }
  | { state: "scheduled"; taskId: string; attempts: number; auto?: boolean }
  | { state: "included"; taskId: string }
  | { state: "dismissed" }
  | { state: "undeliverable"; reason: string }
  | { state: "open" }
  | { state: "resolved" };

export interface CollaborationEntry {
  id: string;
  seq: number;
  at: number;
  member?: string;
  basis: CollaborationBasis;
  body: CollaborationEntryBody;
  status?: CollaborationEntryStatus;
}

export interface CollaborationView extends CollaborationSummary {
  createdAt: number;
  entries: CollaborationEntry[];
}

export interface CollaborationMemberInput {
  alias: string;
  provider: CollaborationProvider;
  workspace: string;
  role: string;
}

/** The user's alias in a collaboration. */
export const USER_ALIAS = "user";

export function listCollaborations(): Promise<CollaborationSummary[]> {
  return invoke("list_collaborations");
}

export function getCollaboration(collaborationId: string): Promise<CollaborationView> {
  return invoke("get_collaboration", { collaborationId });
}

export function createCollaboration(name: string, goal: string, members: CollaborationMemberInput[]): Promise<CollaborationView> {
  return invoke("create_collaboration", { name, goal, members });
}

export function setCollaborationState(collaborationId: string, collaborationState: CollaborationState): Promise<CollaborationView> {
  return invoke("set_collaboration_state", { collaborationId, collaborationState });
}

export function deleteCollaboration(collaborationId: string): Promise<void> {
  return invoke("delete_collaboration", { collaborationId });
}

/** A message this device's user writes to a member: delivered at the
 * member's next turn boundary, without another approval. */
export function sendCollaborationMessage(collaborationId: string, to: string, text: string): Promise<CollaborationView> {
  return invoke("send_collaboration_message", { collaborationId, to, text });
}

export function approveCollaborationMessages(collaborationId: string, entryIds: string[]): Promise<CollaborationView> {
  return invoke("approve_collaboration_messages", { collaborationId, entryIds });
}

export function dismissCollaborationEntry(collaborationId: string, entryId: string): Promise<CollaborationView> {
  return invoke("dismiss_collaboration_entry", { collaborationId, entryId });
}

export function resumeCollaborationMember(collaborationId: string, alias: string): Promise<CollaborationView> {
  return invoke("resume_collaboration_member", { collaborationId, alias });
}

/** Lets a member take up to `turns` turns within `hours` without an
 * approval, on this device's provider account and the conversation's own
 * native permissions. */
export function setCollaborationGrant(collaborationId: string, alias: string, turns: number, hours: number): Promise<CollaborationView> {
  return invoke("set_collaboration_grant", { collaborationId, alias, turns, hours });
}

export function withdrawCollaborationGrant(collaborationId: string, alias: string): Promise<CollaborationView> {
  return invoke("withdraw_collaboration_grant", { collaborationId, alias });
}

export function collaborationNeedsYouCount(): Promise<number> {
  return invoke("collaboration_needs_you_count");
}

// --- view model (pure) ------------------------------------------------------

export type JournalFilter = "activity" | "summaries" | "needs-you";

/** Whether an entry waits for the user: a message to approve, or an item
 * (a message to `user`, a problem) that is still open. */
export function needsUser(entry: CollaborationEntry): boolean {
  return entry.status?.state === "waiting" || entry.status?.state === "open";
}

/** One Journal, three readings. Newest first. */
export function journalEntries(entries: CollaborationEntry[], filter: JournalFilter): CollaborationEntry[] {
  const chosen = entries.filter((entry) => {
    if (filter === "summaries") return entry.body.type === "summary";
    if (filter === "needs-you") return needsUser(entry);
    return true;
  });
  return [...chosen].sort((left, right) => right.seq - left.seq);
}

export function journalCount(entries: CollaborationEntry[], filter: JournalFilter): number {
  return journalEntries(entries, filter).length;
}

/** Messages that wait for approval, by recipient. */
export function waitingByRecipient(entries: CollaborationEntry[]): Record<string, string[]> {
  const waiting: Record<string, string[]> = {};
  for (const entry of entries) {
    if (entry.status?.state !== "waiting" || entry.body.type !== "message") continue;
    (waiting[entry.body.to] ??= []).push(entry.id);
  }
  return waiting;
}

export function basisLabel(basis: CollaborationBasis): string {
  return basis === "fact" ? "Pastey" : basis === "claim" ? "Agent's statement" : "You";
}

export function providerLabel(provider: CollaborationProvider): string {
  return provider === "codex" ? "Codex" : "Claude Code";
}

export function turnOutcomeLabel(state: CollaborationTaskState, code?: string): string {
  if (state === "completed") return "completed";
  if (state === "cancelled") return "stopped";
  if (state === "running" || state === "queued") return "running";
  if (code === "native_agent_start_failed" || code === "native_agent_conversation_unavailable") return "did not start";
  if (code === "native_agent_outcome_unknown") return "outcome unknown";
  return state === "interrupted" ? "interrupted" : "failed";
}

export function statusLabel(status: CollaborationEntryStatus | undefined): string | null {
  switch (status?.state) {
    case "waiting": return "Waiting for your approval";
    case "auto": return status.attempts > 0 ? `Starts its next automatic turn · retry ${status.attempts}` : "Starts its next automatic turn";
    case "approved": return status.attempts > 0 ? `Approved · retry ${status.attempts}` : "Approved · delivered at the next turn";
    case "scheduled": return status.auto ? "In an automatic turn that started" : "In a turn that started";
    case "included": return "Delivered";
    case "dismissed": return "Dismissed";
    case "undeliverable": return `Not delivered: ${status.reason}`;
    case "open": return "Needs you";
    case "resolved": return "Done";
    default: return null;
  }
}

/** Whether a grant can still start a turn at `now` (seconds). */
export function grantUsable(grant: AutoGrant | undefined, now: number): grant is AutoGrant {
  return !!grant && grant.used < grant.turns && now < grant.expiresAt;
}

/** "Automatic · 2 of 3 turns left · until 16:40", or "Asks you". */
export function policyLabel(member: CollaborationMember, now: number, formatTime: (seconds: number) => string): string {
  const grant = member.grant;
  if (!grantUsable(grant, now)) return "Asks you before each turn";
  const left = grant.turns - grant.used;
  return `Automatic · ${left} of ${grant.turns} turn${grant.turns === 1 ? "" : "s"} left · until ${formatTime(grant.expiresAt)}`;
}

export function grantEndLabel(reason: GrantEnd): string {
  switch (reason) {
    case "used_up": return "its automatic turns are used up";
    case "expired": return "its automatic turns expired";
    case "withdrawn": return "you withdrew its automatic turns";
    case "collaboration_ended": return "the collaboration ended";
  }
}

/** The step the Agent says it is on: the first in progress, else the first
 * pending. */
export function currentStep(activity: TaskActivity | null | undefined): string | null {
  const plan = activity?.plan ?? [];
  return (plan.find((step) => step.state === "in_progress") ?? plan.find((step) => step.state === "pending"))?.text ?? null;
}

/** "3 commands (1 failed) · 2 files changed", from reported counts. */
export function activityCounts(activity: { commands: number; failedCommands: number; filesChanged: number } | null | undefined): string | null {
  if (!activity) return null;
  const parts: string[] = [];
  if (activity.commands > 0) parts.push(`${activity.commands} command${activity.commands === 1 ? "" : "s"}${activity.failedCommands > 0 ? ` (${activity.failedCommands} failed)` : ""}`);
  if (activity.filesChanged > 0) parts.push(`${activity.filesChanged} file${activity.filesChanged === 1 ? "" : "s"} changed`);
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** "4 m 05 s" from whole seconds. */
export function elapsedLabel(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(total / 60);
  const rest = total % 60;
  return minutes > 0 ? `${minutes} m ${String(rest).padStart(2, "0")} s` : `${rest} s`;
}

export function memberStateLabel(member: CollaborationMember): string {
  if (member.paused) return "Paused";
  if (member.busy) return "Working";
  return member.waiting > 0 ? `${member.waiting} waiting` : "Idle";
}

/** A usable alias from a folder name: lowercase letters, digits, - and _,
 * at most 32 characters, never `user`, and not one already taken. */
export function suggestAlias(workspace: string, taken: string[]): string {
  const folder = workspace.split(/[\\/]/).filter(Boolean).pop() ?? "";
  let base = folder.toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 28) || "agent";
  if (base === USER_ALIAS) base = "member";
  let alias = base;
  for (let index = 2; taken.includes(alias); index += 1) alias = `${base}-${index}`;
  return alias;
}

export function validAlias(alias: string): boolean {
  return /^[a-z0-9_-]{1,32}$/.test(alias) && alias !== USER_ALIAS;
}

export function folderName(workspace: string): string {
  return workspace.split(/[\\/]/).filter(Boolean).pop() ?? workspace;
}

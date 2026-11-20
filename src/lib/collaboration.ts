import { invoke } from "@tauri-apps/api/core";

/** Agent collaborations on this device (`src-tauri/src/collaboration.rs`).
 * Members are Pastey conversations, one per provider per folder. A message
 * between members waits for this device's user to approve it; the user's own
 * messages need no approval. Delivery always happens at a turn boundary. */

export type CollaborationProvider = "codex" | "claude_code";
export type CollaborationState = "active" | "paused" | "ended";
export type CollaborationTaskState = "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted";
/** Where an entry's content comes from: Pastey's own fact, an Agent's
 * statement (what it said, not proof of it), or this device's user. */
export type CollaborationBasis = "fact" | "claim" | "user";

export interface CollaborationMember {
  alias: string;
  provider: CollaborationProvider;
  workspace: string;
  role: string;
  paused: boolean;
  /** A task, movement or unresolved outcome holds its folder now. */
  busy: boolean;
  /** Messages for it that wait for approval or are approved. */
  waiting: number;
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

export type CollaborationEntryBody =
  | { type: "turn"; taskId: string; origin: "collaboration" | "user"; state: CollaborationTaskState; code?: string }
  | { type: "message"; from: string; to: string; subject: string; body: string; hop: number }
  | { type: "summary"; summary: PublishedSummary }
  | { type: "attention"; text: string }
  | { type: "user"; action: CollaborationUserAction }
  | { type: "member"; alias: string; provider: CollaborationProvider; role: string };

export type CollaborationEntryStatus =
  | { state: "waiting" }
  | { state: "approved"; attempts: number }
  | { state: "scheduled"; taskId: string; attempts: number }
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
    case "approved": return status.attempts > 0 ? `Approved · retry ${status.attempts}` : "Approved · delivered at the next turn";
    case "scheduled": return "In a turn that started";
    case "included": return "Delivered";
    case "dismissed": return "Dismissed";
    case "undeliverable": return `Not delivered: ${status.reason}`;
    case "open": return "Needs you";
    case "resolved": return "Done";
    default: return null;
  }
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

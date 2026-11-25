import { invoke } from "@tauri-apps/api/core";
import type { NativeDecisionReview } from "./tauri";

/** Agent collaborations on this device (`src-tauri/src/collaboration.rs`).
 * Members are Pastey conversations, one per provider per folder. A message
 * between members waits for this device's user to approve it, unless the
 * user gave its recipient an auto grant (a few turns within a few hours); the
 * user's own messages need no approval. Delivery always happens at a turn
 * boundary. A member's first turn carries a briefing compiled from the
 * Journal, and Pastey scans each member's folder at its turns' start and end
 * for file state (paths only). A collaboration may span one other device
 * paired by code: its members there run there, admitted by that device's
 * user, and each device sends the other the entries it writes. */

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
  /** The native Agent is blocked until its own client answers a request
   * (an approval Pastey never gives by itself). */
  awaitingDecision?: boolean;
  /** The native requests of this turn, as reviews this user may decide. */
  decisions?: NativeDecisionReview[];
}

/** Native permission, sandbox and approval settings a member's turns run
 * with. Pastey passes them to the Agent's own mechanisms; it approves
 * nothing itself. */
export interface ExecutionProfile {
  name: string;
  claude: {
    permissionMode?: string;
    allow?: string[];
    deny?: string[];
    sandbox?: boolean;
    sandboxAutoAllow?: boolean;
    allowedDomains?: string[];
    allowWrite?: string[];
  };
  codex: {
    approvalPolicy?: string;
    approvalsReviewer?: string;
    sandbox?: string;
    networkAccess?: boolean;
    writableRoots?: string[];
  };
}

/** The user's authorised default for every collaboration member. */
export interface ExecutionDefault {
  profile: ExecutionProfile;
  authorizedAt: number;
}

export type ExecutionSource = "member" | "collaboration" | "default" | "none";

/** What a member's next turn runs with: configured, not verified. */
export interface EffectiveExecution {
  name: string;
  source: ExecutionSource;
  lines: string[];
  authorizedAt?: number;
}

export type ExecutionChoice =
  | { kind: "inherit" }
  | { kind: "ask" }
  | { kind: "profile"; profile: ExecutionProfile; confirmed: boolean };

/** The native model and effort of every turn the collaboration starts for
 * a member (absent: the Agent's own default). */
export interface MemberOptions {
  model?: string;
  effort?: string;
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
  /** It runs on the linked device; this device knows no folder of it. */
  remote?: boolean;
  /** Invited on the linked device, not yet accepted there. */
  invite?: { scopeRef: string; sent: boolean };
  /** Model and effort of the turns the collaboration starts for it. */
  options?: MemberOptions;
  /** Its own execution choice (absent: the collaboration's). */
  execution?: { kind: "ask" } | { kind: "profile"; profile: ExecutionProfile; authorizedAt: number };
  /** What its next turn runs with (views only). */
  effectiveExecution?: EffectiveExecution;
}

/** The other device a collaboration spans. */
export interface CollaborationLink {
  deviceLabel: string;
  /** This device created the collaboration. */
  home: boolean;
  /** Entries written here that device has not stored yet. */
  unsent: number;
}

export interface CollaborationSummary {
  id: string;
  name: string;
  goal: string;
  state: CollaborationState;
  members: CollaborationMember[];
  link?: CollaborationLink;
  needsYou: number;
  lastActivityAt: number;
  /** Its members' execution choice (absent: your default). */
  execution?: { kind: "ask" } | { kind: "profile"; profile: ExecutionProfile; authorizedAt: number };
  /** Member turns may start: every member was ready and you released it. */
  released?: boolean;
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
  | { type: "member_resumed"; alias: string }
  | { type: "goal_changed"; goal: string }
  | { type: "role_changed"; alias: string; role: string }
  /** The user confirmed or rejected the decisions of summary `entry`. */
  | { type: "decided"; entry: string; confirmed: boolean }
  /** This device's user accepted the invitation for `alias`. */
  | { type: "accepted"; alias: string }
  /** The model and effort of `alias`'s turns changed. */
  | { type: "options_changed"; alias: string; model?: string; effort?: string }
  /** Every member was ready, and the user released the collaboration. */
  | { type: "released" }
  /** The execution choice of the collaboration, or of `alias`, changed. */
  | { type: "execution_changed"; alias?: string; name: string };

/** Paths that differ between two scans of a member's folder (each list at
 * most 20; `total` counts all). */
export interface FileChanges {
  added?: string[];
  changed?: string[];
  removed?: string[];
  total: number;
}

/** A turn's file state from Pastey's scans at its start and end. */
export type FileState =
  | ({ state: "observed" } & FileChanges)
  | { state: "unavailable"; reason: string };

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
  | { type: "turn"; taskId: string; origin: "collaboration" | "user"; state: CollaborationTaskState; code?: string; automatic?: boolean; activity?: TurnActivity; files?: FileState }
  | { type: "message"; from: string; to: string; subject: string; body: string; hop: number }
  | { type: "summary"; summary: PublishedSummary }
  | { type: "attention"; text: string }
  | { type: "user"; action: CollaborationUserAction }
  | { type: "member"; alias: string; provider: CollaborationProvider; role: string; change?: "joined" | "left" | "invited" | "declined" }
  /** A member's folder changed between its turns. */
  | ({ type: "files_outside"; alias: string } & FileChanges)
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
  /** For a member on the linked device: delivered (or not) there. */
  | { state: "elsewhere" }
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
  /** Written on the linked device (its HostRef). */
  host?: string;
  /** For an entry written here in a linked collaboration. */
  outbox?: { state: "pending" } | { state: "delivered"; at: number };
}

/** A member of a linked device, invited from here: the folder is the one
 * that device shares with this one (`scopeRef`). */
export interface RemoteMemberInput {
  alias: string;
  provider: CollaborationProvider;
  role: string;
  scopeRef: string;
}

/** An invitation from a linked device, waiting for this device's user. */
export interface CollaborationInvitation {
  collaborationId: string;
  bridgeId: string;
  deviceLabel: string;
  displayFingerprint: string;
  name: string;
  goal: string;
  members: Array<{ alias: string; provider: CollaborationProvider; role: string }>;
  member: { alias: string; provider: CollaborationProvider; role: string };
  /** This device's own folder. */
  workspace: string;
  receivedAt: number;
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
  options?: MemberOptions;
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

/** Adds a conversation of this device. Its first turn carries the briefing;
 * `firstMessage`, when given, starts that turn without another approval. */
export function addCollaborationMember(collaborationId: string, member: CollaborationMemberInput, firstMessage?: string): Promise<CollaborationView> {
  return invoke("add_collaboration_member", { collaborationId, member, firstMessage: firstMessage?.trim() || null });
}

/** Removes a member: its grant ends and messages waiting for it are not
 * delivered. Its conversation is not changed. */
export function removeCollaborationMember(collaborationId: string, alias: string): Promise<CollaborationView> {
  return invoke("remove_collaboration_member", { collaborationId, alias });
}

export function setCollaborationGoal(collaborationId: string, goal: string): Promise<CollaborationView> {
  return invoke("set_collaboration_goal", { collaborationId, goal });
}

export function setCollaborationRole(collaborationId: string, alias: string, role: string): Promise<CollaborationView> {
  return invoke("set_collaboration_role", { collaborationId, alias, role });
}

export function decideCollaborationSummary(collaborationId: string, entryId: string, confirmed: boolean): Promise<CollaborationView> {
  return invoke("decide_collaboration_summary", { collaborationId, entryId, confirmed });
}

/** Invites a conversation on the device a Bridge is paired with, in a
 * folder it shares with this one (Sessions and Agent requests on). Its user
 * accepts or declines there. */
export function inviteCollaborationMember(collaborationId: string, roomId: string, member: RemoteMemberInput, firstMessage?: string): Promise<CollaborationView> {
  return invoke("invite_collaboration_member", { collaborationId, roomId, member, firstMessage: firstMessage?.trim() || null });
}

export function listCollaborationInvitations(): Promise<CollaborationInvitation[]> {
  return invoke("list_collaboration_invitations");
}

export function answerCollaborationInvitation(collaborationId: string, alias: string, accept: boolean): Promise<void> {
  return invoke("answer_collaboration_invitation", { collaborationId, alias, accept });
}

export function collaborationNeedsYouCount(): Promise<number> {
  return invoke("collaboration_needs_you_count");
}

export type FactBasis = "configured" | "probed" | "measured" | "unknown";

export interface MemberFact { text: string; basis: FactBasis }

export interface ReadinessCheck { name: string; ok?: boolean; basis: FactBasis; detail: string }

export interface MemberReadiness {
  alias: string;
  provider: CollaborationProvider;
  ready: boolean;
  checks: ReadinessCheck[];
  facts: MemberFact[];
}

/** Checks every member now, without a model call (a few seconds). */
export function checkCollaborationReadiness(collaborationId: string): Promise<MemberReadiness[]> {
  return invoke("check_collaboration_readiness", { collaborationId });
}

/** Releases the collaboration once every member is ready (checked again). */
export function releaseCollaboration(collaborationId: string): Promise<CollaborationView> {
  return invoke("release_collaboration", { collaborationId });
}

export function listExecutionPresets(): Promise<ExecutionProfile[]> {
  return invoke("list_execution_presets");
}

export function getExecutionDefault(): Promise<ExecutionDefault | null> {
  return invoke("get_execution_default");
}

/** Authorises (after the user confirmed exactly what it passes) or revokes
 * (`null`) the default every collaboration member inherits. */
export function setExecutionDefault(profile: ExecutionProfile | null, confirmed: boolean): Promise<ExecutionDefault | null> {
  return invoke("set_execution_default", { profile, confirmed });
}

/** Sets a collaboration's execution choice, or one member's (`alias`). */
export function setCollaborationExecution(collaborationId: string, alias: string | null, choice: ExecutionChoice): Promise<CollaborationView> {
  return invoke("set_collaboration_execution", { collaborationId, alias, choice });
}

/** Where a member's effective profile comes from, in words. */
export function executionSourceLabel(source: ExecutionSource): string {
  switch (source) {
    case "member": return "chosen for this member";
    case "collaboration": return "chosen for this collaboration";
    case "default": return "your default";
    case "none": return "nothing chosen";
  }
}

/** What a profile passes to each Agent, in plain lines (mirrors Rust's
 * `ExecutionProfileV1::describe`, for consent before anything is stored). */
export function describeProfile(profile: ExecutionProfile, provider: CollaborationProvider): string[] {
  if (provider === "claude_code") {
    const claude = profile.claude;
    const lines: string[] = [];
    if (claude.permissionMode) lines.push(`Permission mode: ${claude.permissionMode}${claude.permissionMode === "auto" ? " (a classifier's judgement, not a boundary)" : ""}`);
    if (claude.sandbox) lines.push(`Sandbox on: commands write only in the member's folder${(claude.allowWrite ?? []).length ? ` and ${(claude.allowWrite ?? []).join(", ")}` : ""}; network to ${(claude.allowedDomains ?? []).length ? (claude.allowedDomains ?? []).join(", ") : "no host"}${claude.sandboxAutoAllow ? "; sandboxed commands run without a prompt" : ""}`);
    if ((claude.allow ?? []).length) lines.push(`Allowed: ${(claude.allow ?? []).join(", ")}`);
    if ((claude.deny ?? []).length) lines.push(`Denied: ${(claude.deny ?? []).join(", ")}`);
    return lines.length ? lines : ["Nothing passed: Claude Code's own settings; a headless turn declines what would prompt."];
  }
  const codex = profile.codex;
  const lines: string[] = [];
  if (codex.sandbox) lines.push(`Sandbox: ${codex.sandbox}${(codex.writableRoots ?? []).length ? ` (also writes ${(codex.writableRoots ?? []).join(", ")})` : ""}; network ${codex.networkAccess ? "on" : "off"}`);
  if (codex.approvalPolicy) lines.push(`Approvals: ${codex.approvalPolicy}${codex.approvalsReviewer === "auto_review" ? ", decided by Codex's auto-review (a model's judgement, not a boundary)" : codex.approvalsReviewer === "user" ? ", asked of you" : ""}`);
  return lines.length ? lines : ["Nothing passed: Codex's own configuration."];
}

/** Stops the turn running for this device's member: Stop's own
 * cancellation. Its outcome shows when the turn really ends. */
export function stopCollaborationMember(collaborationId: string, alias: string): Promise<CollaborationView> {
  return invoke("stop_collaboration_member", { collaborationId, alias });
}

/** Sets the model and effort of every turn the collaboration starts for a
 * member of this device (empty: the Agent's own default). */
export function setCollaborationMemberOptions(collaborationId: string, alias: string, model: string | null, effort: string | null): Promise<CollaborationView> {
  return invoke("set_collaboration_member_options", { collaborationId, alias, model, effort });
}

/** The native decisions a member's running turn waits for: each pending
 * review, or one when the Agent waits on a request Pastey cannot show. */
export function pendingDecisions(member: CollaborationMember): number {
  const live = member.live;
  if (!live) return 0;
  const pending = (live.decisions ?? []).filter((review) => review.state === "pending").length;
  return pending === 0 && live.awaitingDecision ? 1 : pending;
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
    case "elsewhere": return "Its device decides when it runs";
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
  if (grant && member.autoTask && grant.used >= grant.turns) return "Automatic · its last automatic turn is running";
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

/** The user's latest choice on a summary's decisions, or null. */
export function summaryDecision(entries: CollaborationEntry[], entryId: string): boolean | null {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const body = entries[index].body;
    if (body.type === "user" && body.action.type === "decided" && body.action.entry === entryId) return body.action.confirmed;
  }
  return null;
}

/** The device whose user made the latest choice on a summary's decisions:
 * undefined for this device's user (or when nobody chose). */
export function summaryDecider(entries: CollaborationEntry[], entryId: string): string | undefined {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index];
    if (entry.body.type === "user" && entry.body.action.type === "decided" && entry.body.action.entry === entryId) return entry.host;
  }
  return undefined;
}

/** Listed paths of a change, added first. */
export function changedPaths(changes: FileChanges): string[] {
  return [...(changes.added ?? []), ...(changes.changed ?? []), ...(changes.removed ?? [])];
}

/** "3 files changed: a.py, b.py +1", from Pastey's scans. */
export function fileChangesLabel(changes: FileChanges, shown = 3): string {
  if (changes.total === 0) return "no files changed";
  const paths = changedPaths(changes).slice(0, shown);
  const more = changes.total - paths.length;
  return `${changes.total} file${changes.total === 1 ? "" : "s"} changed: ${paths.join(", ")}${more > 0 ? ` +${more}` : ""}`;
}

export function fileStateLabel(files: FileState): string {
  return files.state === "observed" ? fileChangesLabel(files) : `file state unavailable: ${files.reason}`;
}

/** "4 m 05 s" from whole seconds. */
export function elapsedLabel(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(total / 60);
  const rest = total % 60;
  return minutes > 0 ? `${minutes} m ${String(rest).padStart(2, "0")} s` : `${rest} s`;
}

/** Where an entry comes from: "Pastey", "Agent's statement" or "You", and
 * on which device when it is the linked one. */
export function originLabel(entry: CollaborationEntry, link: CollaborationLink | undefined): string {
  if (!entry.host) return basisLabel(entry.basis);
  const device = link?.deviceLabel ?? "the other device";
  return entry.basis === "user" ? `Its user, on ${device}` : `${basisLabel(entry.basis)} · on ${device}`;
}

/** Whether the linked device has stored an entry written here, for display. */
export function outboxLabel(entry: CollaborationEntry, link: CollaborationLink | undefined): string | null {
  if (!entry.outbox || !link) return null;
  return entry.outbox.state === "pending" ? `Waiting for ${link.deviceLabel}` : null;
}

export function memberStateLabel(member: CollaborationMember): string {
  if (member.invite) return member.invite.sent ? "Invited · waiting for its device's user" : "Invited · not yet sent";
  if (member.remote) return member.waiting > 0 ? `${member.waiting} waiting` : "On the other device";
  if (pendingDecisions(member) > 0) return "Waiting for your approval";
  if (member.paused) return member.busy ? "Paused · its last turn is still ending" : "Paused";
  if (member.busy && !member.live) return "Busy · a turn is ending, or another task holds its folder";
  if (member.busy) return "Working";
  return member.waiting > 0 ? `${member.waiting} waiting` : "Idle";
}

/** What a member's turns run with, as this device's user chose. */
export function optionsLabel(options: MemberOptions | undefined): string {
  const model = options?.model?.trim();
  const effort = options?.effort?.trim();
  if (!model && !effort) return "Model and effort: the Agent's own default";
  return `Model ${model || "default"} · effort ${effort || "default"}`;
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

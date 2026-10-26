/** Folders shared over one Bridge, as each side presents them. Presentation
 * only: the Host that owns a folder creates, changes and ends its shares,
 * and resolves every reference a peer sends against its own records. What
 * this device knows about another device's shares is only what that device
 * last said. */
import type { SharedWorkspace, WorkspaceShare } from "./tauri";

/** What this device knows about the folders another device shares with it. */
export type PeerShareKnowledge =
  | { state: "unknown" }
  /** `fullAccess`: that device gave this one Full Access there. */
  | { state: "known"; workspaces: SharedWorkspace[]; fullAccess?: boolean };

export type ShareNeed = "read" | "run" | "continue";

export const SHARE_DURATIONS: ReadonlyArray<{ hours: number; label: string }> = [
  { hours: 1, label: "1 hour" },
  { hours: 24, label: "1 day" },
  { hours: 168, label: "7 days" },
  { hours: 720, label: "30 days" },
];

/** Where the folder's owner manages it, on their own device. */
export function shareLocation(owner: string): string {
  return `On ${owner}, open this Bridge, then Bridge details → Workspaces`;
}

/** What one share allows, in a few words. */
export function shareSummary(share: Pick<SharedWorkspace, "readSessions" | "shareExcerpts" | "agentRequests">): string {
  const parts: string[] = [];
  if (share.readSessions) parts.push(share.shareExcerpts ? "sessions with excerpts" : "sessions");
  if (share.agentRequests) parts.push("agent requests");
  if (!parts.length) return "Shared by name only";
  const text = parts.join(" and ");
  return text[0].toUpperCase() + text.slice(1);
}

export function allows(share: Pick<SharedWorkspace, "readSessions" | "agentRequests">, need: ShareNeed): boolean {
  switch (need) {
    case "read": return share.readSessions;
    case "run": return share.agentRequests;
    case "continue": return share.readSessions && share.agentRequests;
  }
}

const NEED_SWITCHES: Record<ShareNeed, string> = {
  read: "“Sessions”",
  run: "“Agent requests”",
  continue: "“Sessions” and “Agent requests”",
};

const NEED_WORDS: Record<ShareNeed, string> = {
  read: "session-readable",
  run: "workspace for agent requests",
  continue: "workspace for continuing sessions",
};

/** Why nothing (or this folder) can be used for `need` yet, and the exact
 * step on the owner's device. Null when it can. */
export function peerShareGuidance(owner: string, knowledge: PeerShareKnowledge, need: ShareNeed, chosen?: SharedWorkspace | null): string | null {
  if (knowledge.state === "unknown") return null;
  if (chosen) {
    if (allows(chosen, need)) return null;
    return `${owner} shares ${chosen.label} with this Bridge, but has not turned on ${NEED_SWITCHES[need]} for it. ${shareLocation(owner)} → ${chosen.label}, and turn ${need === "continue" ? "them" : "it"} on.`;
  }
  if (knowledge.workspaces.some((share) => allows(share, need))) return null;
  if (knowledge.fullAccess) {
    return `${owner} gave this device Full Access. Add one of its folders by its path to see its sessions and work in it.`;
  }
  if (!knowledge.workspaces.length) {
    return `${owner} hasn't shared any ${need === "read" ? "session-readable workspace" : "workspace"} with this Bridge. ${shareLocation(owner)} and choose Share a workspace.`;
  }
  const names = knowledge.workspaces.map((share) => share.label).join(", ");
  return `${owner} shares ${names} with this Bridge, but no ${NEED_WORDS[need]} yet. ${shareLocation(owner)}, open the folder and turn on ${NEED_SWITCHES[need]}.`;
}

/** The shares this Host keeps for one Bridge that are worth showing: the
 * live ones, and ended ones that can still be renewed or explained. */
export function visibleShares(shares: readonly WorkspaceShare[]): WorkspaceShare[] {
  const order = { active: 0, expired: 1, unavailable: 2, revoked: 3 } as const;
  return shares
    .filter((share) => share.state !== "revoked")
    .sort((left, right) => order[left.state] - order[right.state] || left.label.localeCompare(right.label));
}

export function shareStateNote(share: Pick<WorkspaceShare, "state">): string | null {
  switch (share.state) {
    case "active": return null;
    case "expired": return "Ended. Renew it to share the folder again.";
    case "unavailable": return "Not usable: the folder moved, the device is no longer trusted, or the Bridge ended.";
    case "revoked": return "No longer shared.";
  }
}

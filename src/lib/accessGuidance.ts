/** What this device can do on another device of a Bridge, authority by
 * authority, and where each is decided. Presentation only: each authority
 * is granted only on the device that owns it, inside this same Bridge
 * there: Full Access for everything at once, or folder by folder with each
 * run accepted there. Nothing here can be satisfied from this side, and
 * trust alone allows nothing. */
import { controlLine } from "./fullAccess";
import { allows, peerShareGuidance, shareLocation, type PeerShareKnowledge } from "./workspaceShares";

export interface RemoteAccessFacts {
  remoteName: string;
  /** How this device appears on the other one. */
  thisDeviceName: string;
  thisFingerprint: string | null;
  paired: boolean;
  connected: boolean;
  /** Whether the most recent run from here waited for the other device's
   * Accept, when this device has seen one. */
  lastRunWaitedForAccept: boolean | null;
  /** The folders the other device shares with this Bridge, as it said. */
  sharing: PeerShareKnowledge;
}

export type AccessState = "ok" | "decided_there" | "needs_action" | "unknown";

export interface AccessLine {
  id: "pairing" | "control" | "run" | "decisions" | "reading" | "continue";
  label: string;
  state: AccessState;
  detail: string;
  /** Exact steps, on the device named in each step. */
  steps: string[];
}

/** Trust is decided once, on the other device, inside this Bridge. */
export function trustSteps(facts: Pick<RemoteAccessFacts, "remoteName" | "thisDeviceName" | "thisFingerprint">): string[] {
  return [
    `${shareLocation(facts.remoteName)}. If it asks to trust ${facts.thisDeviceName} first, check that it shows ${facts.thisFingerprint ? `the fingerprint ${facts.thisFingerprint}` : "this device's fingerprint (Settings → This device here)"}, then choose Trust. This is needed once.`,
  ];
}

function shareSteps(facts: RemoteAccessFacts, switches: string): string[] {
  return [
    ...trustSteps(facts),
    `Choose Share a workspace…, pick the folder on ${facts.remoteName}, and turn on ${switches}. A folder it already shares can be changed there too.`,
  ];
}

export function remoteAccessGuidance(facts: RemoteAccessFacts): AccessLine[] {
  const where = facts.remoteName;
  const sharing = facts.sharing;
  const shared = sharing.state === "known" ? sharing.workspaces : [];
  const lines: AccessLine[] = [];
  lines.push(facts.paired
    ? { id: "pairing", label: "Paired", state: "ok", detail: `This Bridge is paired with ${where}.`, steps: [] }
    : { id: "pairing", label: "Not paired", state: "needs_action", detail: "This Bridge has no verified device, so nothing can run or be read.", steps: ["Here: Bridge details → Pair this Bridge, and enter the code on the other device."] });

  // Full Access is the one switch for using that device unattended.
  const fullAccess = sharing.state === "known" ? sharing.fullAccess === true : null;
  const fullAccessSteps = [`On ${where}: open this Bridge, then Bridge details → Full Access → Allow Full Access…, read what it allows and confirm. Only ${where}'s user can do this, and it lets this device control ${where}, never the other way round.`];
  lines.push({
    id: "control",
    label: "Full Access",
    state: fullAccess === null ? "unknown" : fullAccess ? "ok" : "decided_there",
    detail: controlLine(where, fullAccess),
    steps: fullAccess ? [] : fullAccessSteps,
  });

  const runnable = shared.filter((share) => allows(share, "run")).map((share) => share.label);
  lines.push({
    id: "run",
    label: "Run Codex",
    state: fullAccess ? "ok" : "decided_there",
    detail: (fullAccess
      ? `Runs start on ${where} at once, in its shared folders or any folder you name by path.`
      : facts.lastRunWaitedForAccept === true
        ? `Your last run waited for someone on ${where} to click Accept: ${where} has not given this device Full Access.`
        : `Each run waits for someone on ${where} to click Accept, unless ${where}'s user gives this device Full Access.`)
      + (runnable.length ? ` You can pick ${runnable.join(", ")} by name.` : ""),
    steps: fullAccess ? [] : runnable.length ? fullAccessSteps : [...fullAccessSteps, ...shareSteps(facts, "“Agent requests”")],
  });

  lines.push({
    id: "decisions",
    label: "Codex's own approval prompts",
    state: facts.connected ? "ok" : "unknown",
    detail: `Answered here, by you. Pastey proves this device's identity to ${where} before each run; this needs no trust or permission.`,
    steps: [],
  });

  const readable = shared.filter((share) => allows(share, "read")).map((share) => share.label);
  if (sharing.state === "unknown") {
    lines.push({ id: "reading", label: "Read sessions", state: "unknown", detail: `Not checked yet. Opening this tab asks ${where} what it shares.`, steps: shareSteps(facts, "“Sessions”") });
  } else if (readable.length) {
    lines.push({ id: "reading", label: "Read sessions", state: "ok", detail: `${where} shares the sessions of: ${readable.join(", ")}.`, steps: [] });
  } else {
    lines.push({ id: "reading", label: "Read sessions", state: "needs_action", detail: peerShareGuidance(where, sharing, "read") ?? `${where} has not allowed this device to read its sessions.`, steps: shareSteps(facts, "“Sessions”") });
  }

  // Continuing names a session only in a folder this device may read there
  // and ask to run in; without Full Access each continuation still waits
  // for that Accept.
  const continuable = shared.filter((share) => allows(share, "continue")).map((share) => share.label);
  lines.push(sharing.state === "known" && !continuable.length
    ? { id: "continue", label: "Continue its sessions", state: "needs_action", detail: peerShareGuidance(where, sharing, "continue") ?? `Needs a folder ${where} shares with both “Sessions” and “Agent requests”.`, steps: fullAccess ? [] : shareSteps(facts, "“Sessions” and “Agent requests”") }
    : {
      id: "continue",
      label: "Continue its sessions",
      state: fullAccess ? "ok" : "decided_there",
      detail: `Codex threads in ${continuable.length ? continuable.join(", ") : `folders ${where} shares for sessions and agent requests`}: ${fullAccess ? "each continuation starts at once" : `each continuation waits for someone on ${where} to click Accept`}. Claude Code sessions are continued only on ${where} itself.`,
      steps: [],
    });
  return lines;
}

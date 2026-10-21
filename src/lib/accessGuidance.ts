/** What this device can do on another device of a Bridge, authority by
 * authority, and where each is decided. Presentation only: each authority
 * stays separate and is granted only on the device that owns it, inside
 * this same Bridge there. Nothing here can be satisfied from this side, and
 * trust alone allows nothing. */
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
  id: "pairing" | "run" | "decisions" | "reading" | "continue";
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

  const runnable = shared.filter((share) => allows(share, "run")).map((share) => share.label);
  lines.push({
    id: "run",
    label: "Run Codex",
    state: "decided_there",
    detail: (facts.lastRunWaitedForAccept === true
      ? `Your last run waited for someone on ${where} to click Accept: ${where} has no unattended permission for this device in that folder.`
      : `Each run waits for someone on ${where} to click Accept, unless ${where}'s user lets this device run in that folder without asking.`)
      + (runnable.length ? ` You can pick ${runnable.join(", ")} by name.` : ""),
    steps: runnable.length
      ? [`${shareLocation(where)} → ${runnable[0]} → Allow without asking…, to skip the Accept for a few runs.`]
      : shareSteps(facts, "“Agent requests”"),
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
  // and ask to run in, and each continuation still waits for that Accept.
  const continuable = shared.filter((share) => allows(share, "continue")).map((share) => share.label);
  lines.push(sharing.state === "known" && !continuable.length
    ? { id: "continue", label: "Continue its sessions", state: "needs_action", detail: peerShareGuidance(where, sharing, "continue") ?? `Needs a folder ${where} shares with both “Sessions” and “Agent requests”.`, steps: shareSteps(facts, "“Sessions” and “Agent requests”") }
    : {
      id: "continue",
      label: "Continue its sessions",
      state: "decided_there",
      detail: `Codex threads in ${continuable.length ? continuable.join(", ") : `folders ${where} shares for sessions and agent requests`}: each continuation waits for someone on ${where} to click Accept. Claude Code sessions are continued only on ${where} itself.`,
      steps: [],
    });
  return lines;
}

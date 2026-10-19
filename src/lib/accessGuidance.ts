/** What this device can do on another device of a Bridge, authority by
 * authority, and where each is decided. Presentation only: each authority
 * stays separate and is granted only on the device that owns it. Nothing
 * here can be satisfied from this side, and trust alone allows nothing. */

export type SessionReadingKnowledge =
  | { state: "unknown" }
  | { state: "allowed"; folders: string[] }
  | { state: "not_allowed" };

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
  sessionReading: SessionReadingKnowledge;
}

export type AccessState = "ok" | "decided_there" | "needs_action" | "unknown";

export interface AccessLine {
  id: "pairing" | "run" | "decisions" | "reading";
  label: string;
  state: AccessState;
  detail: string;
  /** Exact steps, on the device named in each step. */
  steps: string[];
}

export function trustSteps(facts: Pick<RemoteAccessFacts, "remoteName" | "thisDeviceName" | "thisFingerprint">): string[] {
  return [
    `On ${facts.remoteName}: Settings → Devices & trust. Under “Connected now”, find ${facts.thisDeviceName}${facts.thisFingerprint ? ` and check its fingerprint is ${facts.thisFingerprint}` : ""}, then choose Trust…. If it is not listed, choose “Show this device” here first.`,
  ];
}

export function remoteAccessGuidance(facts: RemoteAccessFacts): AccessLine[] {
  const where = facts.remoteName;
  const lines: AccessLine[] = [];
  lines.push(facts.paired
    ? { id: "pairing", label: "Paired", state: "ok", detail: `This Bridge is paired with ${where}.`, steps: [] }
    : { id: "pairing", label: "Not paired", state: "needs_action", detail: "This Bridge has no verified device, so nothing can run or be read.", steps: ["Here: Bridge details → Pair this Bridge, and enter the code on the other device."] });

  const unattendedSteps = [
    ...trustSteps(facts),
    `On ${where}: Settings → Permissions → Unattended Codex access → Add a permission…, choose ${facts.thisDeviceName}, one folder, how long and how many runs.`,
  ];
  lines.push({
    id: "run",
    label: "Run Codex",
    state: "decided_there",
    detail: facts.lastRunWaitedForAccept === true
      ? `Your last run waited for someone on ${where} to click Accept: ${where} has no unattended permission for this device in that folder.`
      : `Each run waits for someone on ${where} to click Accept, unless ${where}'s user gave this device unattended access to that folder there.`,
    steps: unattendedSteps,
  });

  lines.push({
    id: "decisions",
    label: "Codex's own approval prompts",
    state: facts.connected ? "ok" : "unknown",
    detail: `Answered here, by you. Pastey proves this device's identity to ${where} before each run; this needs no trust or permission.`,
    steps: [],
  });

  const readingSteps = [
    ...trustSteps(facts),
    `On ${where}: Settings → Permissions → Session reading → Add a permission…, choose ${facts.thisDeviceName} and the folder to share.`,
  ];
  switch (facts.sessionReading.state) {
    case "allowed":
      lines.push({ id: "reading", label: "Read sessions", state: "ok", detail: `${where} shares the sessions of: ${facts.sessionReading.folders.join(", ") || "its shared folders"}.`, steps: [] });
      break;
    case "not_allowed":
      lines.push({ id: "reading", label: "Read sessions", state: "needs_action", detail: `${where} has not allowed this device to read its sessions.`, steps: readingSteps });
      break;
    default:
      lines.push({ id: "reading", label: "Read sessions", state: "unknown", detail: `Not checked yet. Looking for sessions below asks ${where}.`, steps: readingSteps });
  }
  return lines;
}

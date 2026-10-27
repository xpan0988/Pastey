/** Presentation for Full Access and workspace share expiry. Rust owns every
 * rule; these helpers only explain them, in the words the consent dialog
 * and the controlling device show. */

/** The rights of the Pastey process on the controlled device, which
 * everything Full Access starts there inherits. */
export type ProcessRights = "standard" | "administrator" | "unknown";

/** What Full Access lets `controller` do on this device without anyone here
 * accepting each request. Shown before it is given, and beside it after. */
export function fullAccessAllows(controller: string): string[] {
  return [
    `Run Codex in any folder on this device, and continue Codex sessions here.`,
    `Read the Claude Code and Codex sessions in folders shared with ${controller}, and open more folders here by their path.`,
    `Open a terminal on this device (PowerShell on Windows, your shell on macOS) with exactly the rights Pastey has here. Anything Pastey's user account can do, ${controller} can do: read, change or delete your files, reach any folder, and run programs. Pastey's folder rules decide only which folders are offered for sessions; they do not limit the terminal or Codex.`,
    `Send a folder here and run Codex on it.`,
  ];
}

/** What Full Access never does. */
export function fullAccessLimits(controller: string): string[] {
  return [
    `It does not let this device control ${controller}. That is only ${controller}'s own choice, made there.`,
    `Pastey never answers Codex's own questions. Whether Codex asks before a command or file change follows Codex's own settings on this device; when it asks, the question goes to the person using ${controller}.`,
    "It never raises its rights: a Windows UAC prompt, an administrator password, or a macOS privacy prompt for something Pastey has not been allowed appears on this device, where nobody may be there to answer. What Pastey has already been allowed, it has too.",
    "Nothing runs while this device is asleep, signed out, or Pastey is quit.",
    "Claude Code sessions here can still be continued only on this device.",
  ];
}

/** A warning about the rights everything Full Access starts here inherits,
 * when they are more than an ordinary user's or cannot be told. */
export function processRightsWarning(rights: ProcessRights): string | null {
  switch (rights) {
    case "administrator":
      return "Pastey is running as administrator on this device, so its terminal and Codex runs would have administrator rights too, with no UAC prompt. Unless you mean that, quit Pastey and start it normally (not \"Run as administrator\") before giving Full Access.";
    case "unknown":
      return "Pastey could not tell whether it runs as administrator here. Whatever rights it has, the other device gets.";
    case "standard":
      return null;
  }
}

/** Everything that ends it. */
export const FULL_ACCESS_ENDS: readonly string[] = [
  "Revoke Full Access here, or in Settings → Permissions.",
  "Forget or revoke the device in Settings → Devices & trust.",
  "Burn this Bridge, or pair it with another device.",
];

/** One line for the controlling device about its own standing on `host`. */
export function controlLine(host: string, fullAccess: boolean | null): string {
  if (fullAccess === null) return `Not known yet whether ${host} gave this device Full Access.`;
  return fullAccess
    ? `${host} gave this device Full Access: runs, session continuations and terminals start there without anyone accepting them.`
    : `Each run and terminal waits for someone at ${host} to click Accept. To use ${host} when nobody is there, its user turns on Full Access for this device in that Bridge's details on ${host}.`;
}

/** Whether a request from this device starts on `host` at once or waits
 * there for its user, as far as `host` last said (`fullAccess`, or null
 * before it answered). `host` decides when the request arrives. */
export function remoteStartLine(host: string, fullAccess: boolean | null): string {
  if (fullAccess === true) return `It starts on ${host} at once: ${host} gave this device Full Access.`;
  if (fullAccess === false) return `It waits for someone on ${host} to accept: ${host} has not given this device Full Access.`;
  return `${host} asks its user to accept unless it gave this device Full Access.`;
}

export function expiryLabel(expiresAt: number, now = Date.now() / 1000): string {
  const remaining = expiresAt - now;
  if (remaining <= 0) return "expired";
  if (remaining < 3600) return `expires in ${Math.max(1, Math.round(remaining / 60))} min`;
  if (remaining < 2 * 86400) return `expires in ${Math.round(remaining / 3600)} h`;
  return `expires in ${Math.round(remaining / 86400)} days`;
}

/** Presentation for Full Access and workspace share expiry. Rust owns every
 * rule; these helpers only explain them, in the words the consent dialog
 * and the controlling device show. */

/** What Full Access lets `controller` do on this device without anyone here
 * accepting each request. Shown before it is given, and beside it after. */
export function fullAccessAllows(controller: string): string[] {
  return [
    `Run Codex in any folder on this device, and continue Codex sessions here.`,
    `Read the Claude Code and Codex sessions in folders shared with ${controller}, and open more folders here by their path.`,
    `Open a terminal on this device (PowerShell on Windows, your shell on macOS) as your user account. Anything you can do in a terminal, ${controller} can do: read, change or delete your files and run programs.`,
    `Send a folder here and run Codex on it.`,
  ];
}

/** What Full Access never does. */
export function fullAccessLimits(controller: string): string[] {
  return [
    `It does not let this device control ${controller}. That is only ${controller}'s own choice, made there.`,
    `Codex still asks before risky commands and file changes. Those questions go to the person using ${controller}, and Pastey never answers them by itself.`,
    "It cannot get past your operating system: administrator prompts, macOS privacy permissions and Windows UAC still apply, and nothing runs while this device is asleep, signed out or Pastey is closed.",
    "Claude Code sessions here can still be continued only on this device.",
  ];
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

export function expiryLabel(expiresAt: number, now = Date.now() / 1000): string {
  const remaining = expiresAt - now;
  if (remaining <= 0) return "expired";
  if (remaining < 3600) return `expires in ${Math.max(1, Math.round(remaining / 60))} min`;
  if (remaining < 2 * 86400) return `expires in ${Math.round(remaining / 3600)} h`;
  return `expires in ${Math.round(remaining / 86400)} days`;
}

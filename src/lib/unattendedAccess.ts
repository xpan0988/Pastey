/** Presentation for Host-local unattended Codex permissions. Rust owns every
 * rule; these helpers only explain them. */
import type { CreateUnattendedGrant, SeenPeerDevice, UnattendedGrant } from "./tauri";

export const GRANT_DURATIONS: ReadonlyArray<{ hours: number; label: string }> = [
  { hours: 1, label: "1 hour" },
  { hours: 8, label: "8 hours" },
  { hours: 24, label: "1 day" },
  { hours: 72, label: "3 days" },
  { hours: 168, label: "7 days" },
];

export const GRANT_RUN_LIMITS: ReadonlyArray<number> = [1, 5, 10, 20, 50];

export function grantStateLabel(grant: UnattendedGrant): string {
  switch (grant.state) {
    case "active": return `Active · ${grant.runs} of ${grant.maxRuns} runs used`;
    case "expired": return "Expired";
    case "used_up": return `All ${grant.maxRuns} runs used`;
    case "revoked": return "Revoked";
    case "unavailable": return "Not usable: the device was revoked, the Bridge ended, or the folder moved";
  }
}

/** Devices a permission can be given to now: trusted and connected, with
 * their identity proven in the current session. */
export function grantableDevices(seen: SeenPeerDevice[]): SeenPeerDevice[] {
  return seen.filter((device) => device.trusted);
}

export function grantFormProblem(form: Partial<CreateUnattendedGrant>): string | null {
  if (!form.deviceFingerprint || !form.roomId) return "Choose a trusted device that is connected now.";
  if (!form.workspacePath) return "Choose the one folder Codex may work in.";
  if (!GRANT_DURATIONS.some((duration) => duration.hours === form.durationHours)) return "Choose how long the permission lasts.";
  if (!GRANT_RUN_LIMITS.includes(form.maxRuns ?? 0)) return "Choose how many runs it allows.";
  return null;
}

export function expiryLabel(expiresAt: number, now = Date.now() / 1000): string {
  const remaining = expiresAt - now;
  if (remaining <= 0) return "expired";
  if (remaining < 3600) return `expires in ${Math.max(1, Math.round(remaining / 60))} min`;
  if (remaining < 2 * 86400) return `expires in ${Math.round(remaining / 3600)} h`;
  return `expires in ${Math.round(remaining / 86400)} days`;
}

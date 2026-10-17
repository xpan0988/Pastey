import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import {
  createSessionReadGrant,
  listSeenPeerDevices,
  listSessionReadGrants,
  revokeSessionReadGrant,
  type SeenPeerDevice,
  type SessionReadGrant,
} from "../../lib/tauri";
import { READ_GRANT_DURATIONS, readGrantFormProblem, readGrantStateLabel } from "../../lib/nativeSessions";
import { expiryLabel, grantableDevices } from "../../lib/unattendedAccess";

/** Which trusted devices may read this device's Claude Code and Codex
 * session summaries, folder by folder. Only this device's own user decides;
 * trusting a device grants none of this. */
export function SessionReadingSettings() {
  const [seen, setSeen] = useState<SeenPeerDevice[]>([]);
  const [grants, setGrants] = useState<SessionReadGrant[]>([]);
  const [form, setForm] = useState<{ device: string; workspacePath: string; durationHours: number; shareExcerpts: boolean }>({ device: "", workspacePath: "", durationHours: 24, shareExcerpts: false });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const [nextSeen, nextGrants] = await Promise.all([listSeenPeerDevices().catch(() => []), listSessionReadGrants().catch(() => [])]);
    setSeen(nextSeen); setGrants(nextGrants);
  }
  useEffect(() => { void refresh(); }, []);

  async function act(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await refresh(); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  async function chooseWorkspace() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") setForm({ ...form, workspacePath: chosen });
  }

  const candidates = grantableDevices(seen);
  const chosen = candidates.find((device) => `${device.fingerprint}\n${device.bridgeId}` === form.device);
  const request = chosen ? { deviceFingerprint: chosen.fingerprint, roomId: chosen.bridgeId, workspacePath: form.workspacePath, durationHours: form.durationHours, shareExcerpts: form.shareExcerpts } : {};
  const problem = readGrantFormProblem(request);

  return <section className="v2-settings-group" aria-label="Session reading">
    <h2>Session reading</h2>
    <div>
      <p className="v2-settings-note">Lets one trusted device see the Claude Code and Codex sessions in one folder on this device, over one Bridge, until it expires or you revoke it. It sees statuses, titles, activity counts, changed file names and Git facts, never folder paths or whole conversations. With excerpts on, it also sees the last few prompts and replies, shortened. Reading never starts, continues or stops a session. Trust devices under Unattended Codex access above; trusting a device does not let it read anything.</p>

      <strong>Permissions</strong>
      {grants.length === 0 ? <small>None.</small> : grants.map((grant) => <div className="v2-settings-row" key={grant.grantId}><span><strong>{grant.deviceLabel} · sessions</strong><small>{grant.workspace}</small><small>{readGrantStateLabel(grant)}{grant.state === "active" ? ` · ${expiryLabel(grant.expiresAt)}` : ""}</small></span>{grant.state === "active" ? <button type="button" className="v2-button danger" disabled={pending} onClick={() => void act(() => revokeSessionReadGrant(grant.grantId))}>Revoke</button> : null}</div>)}

      <strong>Add a permission</strong>
      <label className="v2-target-select"><select value={form.device} onChange={(event) => setForm({ ...form, device: event.target.value })} disabled={pending || candidates.length === 0} aria-label="Trusted device for session reading"><option value="">{candidates.length ? "Which trusted device?" : "No trusted device connected"}</option>{candidates.map((device) => <option key={`${device.fingerprint}-${device.bridgeId}`} value={`${device.fingerprint}\n${device.bridgeId}`}>{device.peerName} · {device.displayFingerprint}</option>)}</select></label>
      <button type="button" className="v2-button" disabled={pending} onClick={() => void chooseWorkspace()}>{form.workspacePath ? `Folder: ${form.workspacePath}` : "Choose the folder whose sessions it may read…"}</button>
      <label className="v2-target-select"><select value={form.durationHours} onChange={(event) => setForm({ ...form, durationHours: Number(event.target.value) })} aria-label="Session reading length">{READ_GRANT_DURATIONS.map((duration) => <option key={duration.hours} value={duration.hours}>For {duration.label}</option>)}</select></label>
      <label><input type="checkbox" checked={form.shareExcerpts} onChange={(event) => setForm({ ...form, shareExcerpts: event.target.checked })} disabled={pending} /> Also share short excerpts of recent prompts and replies</label>
      <button type="button" className="v2-button primary" disabled={pending || problem !== null} onClick={() => void act(() => createSessionReadGrant(request as Parameters<typeof createSessionReadGrant>[0]).then(() => setForm({ ...form, workspacePath: "" })))}>Allow reading</button>
      {problem && form.device ? <small>{problem}</small> : null}
      {error ? <p className="v2-error" role="alert">{error}</p> : null}
    </div>
  </section>;
}

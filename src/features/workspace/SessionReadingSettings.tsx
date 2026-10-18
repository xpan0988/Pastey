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
import { Badge, Icon, ListRow } from "./ui";

/** Which trusted devices may read this device's Claude Code and Codex
 * session summaries, folder by folder. Only this device's own user decides;
 * trusting a device grants none of this, and reading grants no execution. */
export function SessionReadingSettings() {
  const [seen, setSeen] = useState<SeenPeerDevice[]>([]);
  const [grants, setGrants] = useState<SessionReadGrant[]>([]);
  const [form, setForm] = useState<{ device: string; workspacePath: string; durationHours: number; shareExcerpts: boolean }>({ device: "", workspacePath: "", durationHours: 24, shareExcerpts: false });
  const [adding, setAdding] = useState(false);
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
  const ordered = [...grants].sort((left, right) => Number(right.state === "active") - Number(left.state === "active"));

  return <section className="section" aria-label="Session reading">
    <div className="section-head"><h2>Session reading</h2>
      <p>Lets one trusted device see the Claude Code and Codex sessions in one folder on this device, over one Bridge, until it expires or you revoke it. It sees statuses, titles, activity counts, changed file names and Git facts, never folder paths or whole conversations. Reading never starts, continues or stops a session, and it is separate from running Codex: trusting a device does not let it read anything.</p></div>
    <div className="card">
      <div className="list">
        {ordered.length === 0 ? <p className="list-empty">No device can read sessions on this device.</p> : ordered.map((grant) => <ListRow key={grant.grantId} leading={<Icon name="sessions" />} title={`${grant.deviceLabel} · sessions`} meta={<>{grant.workspace}<br />{readGrantStateLabel(grant)}{grant.state === "active" ? ` · ${expiryLabel(grant.expiresAt)}` : ""}</>} side={grant.state === "active" ? <button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void act(() => revokeSessionReadGrant(grant.grantId))}>Revoke</button> : <Badge>{grant.state[0].toUpperCase() + grant.state.slice(1)}</Badge>} />)}
      </div>
      {adding ? <div className="add-form" aria-label="Add a reading permission">
        <strong>Allow reading</strong>
        <div className="row-wrap">
          <select className="select" value={form.device} onChange={(event) => setForm({ ...form, device: event.target.value })} disabled={pending || candidates.length === 0} aria-label="Trusted device for session reading"><option value="">{candidates.length ? "Which trusted device?" : "No trusted device connected"}</option>{candidates.map((device) => <option key={`${device.fingerprint}-${device.bridgeId}`} value={`${device.fingerprint}\n${device.bridgeId}`}>{device.peerName} · {device.displayFingerprint}</option>)}</select>
          <button type="button" className="btn" disabled={pending} onClick={() => void chooseWorkspace()}><Icon name="folder" small />{form.workspacePath ? form.workspacePath : "Folder whose sessions it may read…"}</button>
        </div>
        <div className="row-wrap">
          <select className="select" value={form.durationHours} onChange={(event) => setForm({ ...form, durationHours: Number(event.target.value) })} aria-label="Session reading length">{READ_GRANT_DURATIONS.map((duration) => <option key={duration.hours} value={duration.hours}>For {duration.label}</option>)}</select>
        </div>
        <label className="check"><input type="checkbox" checked={form.shareExcerpts} onChange={(event) => setForm({ ...form, shareExcerpts: event.target.checked })} disabled={pending} /><span>Also share short excerpts of recent prompts and replies<small>Off: statuses only.</small></span></label>
        <div className="row"><span className="spacer xsmall subtle">{problem && form.device ? problem : candidates.length === 0 ? "Trust a connected device under Devices & trust first." : ""}</span><button type="button" className="btn" disabled={pending} onClick={() => setAdding(false)}>Cancel</button><button type="button" className="btn btn-primary" disabled={pending || problem !== null} onClick={() => void act(() => createSessionReadGrant(request as Parameters<typeof createSessionReadGrant>[0]).then(() => { setForm({ ...form, workspacePath: "" }); setAdding(false); }))}>Allow reading</button></div>
      </div> : <footer className="card-footer"><button type="button" className="btn btn-sm" onClick={() => setAdding(true)}><Icon name="plus" small />Add a permission…</button></footer>}
    </div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
  </section>;
}

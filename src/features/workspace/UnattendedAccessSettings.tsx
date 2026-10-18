import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import {
  createUnattendedGrant,
  listSeenPeerDevices,
  listUnattendedGrants,
  revokeUnattendedGrant,
  type SeenPeerDevice,
  type UnattendedGrant,
} from "../../lib/tauri";
import { expiryLabel, GRANT_DURATIONS, GRANT_RUN_LIMITS, grantableDevices, grantFormProblem, grantStateLabel } from "../../lib/unattendedAccess";
import { Badge, Icon, ListRow } from "./ui";

/** Who may start Codex on this device while nobody is here to click Accept.
 * Everything here is this device's own user's decision: nothing another
 * device sends can trust a device or create, extend or renew a permission. */
export function UnattendedAccessSettings() {
  const [seen, setSeen] = useState<SeenPeerDevice[]>([]);
  const [grants, setGrants] = useState<UnattendedGrant[]>([]);
  const [form, setForm] = useState<{ device: string; workspacePath: string; durationHours: number; maxRuns: number }>({ device: "", workspacePath: "", durationHours: 24, maxRuns: 10 });
  const [adding, setAdding] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const [nextSeen, nextGrants] = await Promise.all([listSeenPeerDevices().catch(() => []), listUnattendedGrants().catch(() => [])]);
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
  const request = chosen ? { deviceFingerprint: chosen.fingerprint, roomId: chosen.bridgeId, workspacePath: form.workspacePath, durationHours: form.durationHours, maxRuns: form.maxRuns } : {};
  const problem = grantFormProblem(request);
  const ordered = [...grants].sort((left, right) => Number(right.state === "active") - Number(left.state === "active"));

  return <section className="section" aria-label="Unattended Codex access">
    <div className="section-head"><h2>Unattended Codex access</h2>
      <p>A permission lets one trusted device start Codex in one folder on this device, over one Bridge, without you clicking Accept, until it expires, runs out or you revoke it. Without one, every request waits for your Accept.</p></div>
    <div className="card">
      <div className="list">
        {ordered.length === 0 ? <p className="list-empty">No permissions. Every Codex request from another device waits for your Accept.</p> : ordered.map((grant) => <ListRow key={grant.grantId} leading={<Icon name="agent" />} title={`${grant.deviceLabel} · Codex`} meta={<>{grant.workspace}<br />{grantStateLabel(grant)}{grant.state === "active" ? ` · ${expiryLabel(grant.expiresAt)}` : ""}</>} side={grant.state === "active" ? <button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void act(() => revokeUnattendedGrant(grant.grantId))}>Revoke</button> : <Badge>{grant.state === "used_up" ? "Used up" : grant.state[0].toUpperCase() + grant.state.slice(1)}</Badge>} />)}
      </div>
      {adding ? <div className="add-form" aria-label="Add a permission">
        <strong>Allow unattended runs</strong>
        <div className="row-wrap">
          <select className="select" value={form.device} onChange={(event) => setForm({ ...form, device: event.target.value })} disabled={pending || candidates.length === 0} aria-label="Trusted device"><option value="">{candidates.length ? "Which trusted device?" : "No trusted device connected"}</option>{candidates.map((device) => <option key={`${device.fingerprint}-${device.bridgeId}`} value={`${device.fingerprint}\n${device.bridgeId}`}>{device.peerName} · {device.displayFingerprint}</option>)}</select>
          <button type="button" className="btn" disabled={pending} onClick={() => void chooseWorkspace()}><Icon name="folder" small />{form.workspacePath ? form.workspacePath : "Folder Codex may work in…"}</button>
        </div>
        <div className="row-wrap">
          <select className="select" value={form.durationHours} onChange={(event) => setForm({ ...form, durationHours: Number(event.target.value) })} aria-label="Permission length">{GRANT_DURATIONS.map((duration) => <option key={duration.hours} value={duration.hours}>For {duration.label}</option>)}</select>
          <select className="select" value={form.maxRuns} onChange={(event) => setForm({ ...form, maxRuns: Number(event.target.value) })} aria-label="Run limit">{GRANT_RUN_LIMITS.map((runs) => <option key={runs} value={runs}>Up to {runs} run{runs === 1 ? "" : "s"}</option>)}</select>
        </div>
        <p className="xsmall subtle">Codex's own approval requests are never answered by it: each one is shown on the device that started the task, where its user may approve it once or deny it; otherwise the task waits until it is stopped. This device must stay on, awake, signed in and running Pastey.</p>
        <div className="row"><span className="spacer xsmall subtle">{problem && form.device ? problem : candidates.length === 0 ? "Trust a connected device under Devices & trust first." : ""}</span><button type="button" className="btn" disabled={pending} onClick={() => setAdding(false)}>Cancel</button><button type="button" className="btn btn-primary" disabled={pending || problem !== null} onClick={() => void act(() => createUnattendedGrant(request as Parameters<typeof createUnattendedGrant>[0]).then(() => { setForm({ ...form, workspacePath: "" }); setAdding(false); }))}>Allow these runs</button></div>
      </div> : <footer className="card-footer"><button type="button" className="btn btn-sm" onClick={() => setAdding(true)}><Icon name="plus" small />Add a permission…</button></footer>}
    </div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
  </section>;
}

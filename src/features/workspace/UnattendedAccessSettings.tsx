import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import {
  createUnattendedGrant,
  getDeviceIdentity,
  listSeenPeerDevices,
  listTrustedDevices,
  listUnattendedGrants,
  revokeTrustedDevice,
  revokeUnattendedGrant,
  rotateDeviceIdentity,
  trustPeerDevice,
  type DeviceIdentity,
  type SeenPeerDevice,
  type TrustedDevice,
  type UnattendedGrant,
} from "../../lib/tauri";
import { expiryLabel, GRANT_DURATIONS, GRANT_RUN_LIMITS, grantableDevices, grantFormProblem, grantStateLabel } from "../../lib/unattendedAccess";

/** Who may start Codex on this device while nobody is here to click Accept.
 * Everything here is this device's own user's decision: nothing another
 * device sends can trust a device or create, extend or renew a permission. */
export function UnattendedAccessSettings() {
  const [identity, setIdentity] = useState<DeviceIdentity | null>(null);
  const [seen, setSeen] = useState<SeenPeerDevice[]>([]);
  const [trusted, setTrusted] = useState<TrustedDevice[]>([]);
  const [grants, setGrants] = useState<UnattendedGrant[]>([]);
  const [form, setForm] = useState<{ device: string; workspacePath: string; durationHours: number; maxRuns: number }>({ device: "", workspacePath: "", durationHours: 24, maxRuns: 10 });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const [nextIdentity, nextSeen, nextTrusted, nextGrants] = await Promise.all([
      getDeviceIdentity().catch(() => null),
      listSeenPeerDevices().catch(() => []),
      listTrustedDevices().catch(() => []),
      listUnattendedGrants().catch(() => []),
    ]);
    setIdentity(nextIdentity); setSeen(nextSeen); setTrusted(nextTrusted); setGrants(nextGrants);
  }
  useEffect(() => { void refresh(); }, []);

  async function act(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await refresh(); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  function trust(device: SeenPeerDevice) {
    const confirmed = window.confirm(`Trust ${device.peerName}?\n\nOnly continue if ${device.peerName} shows exactly this identity under Settings → This device's identity:\n\n${device.displayFingerprint}\n\nTrusting it lets you give it limited permissions below. It does not let it run anything by itself.`);
    if (confirmed) void act(() => trustPeerDevice(device.fingerprint));
  }

  async function chooseWorkspace() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") setForm({ ...form, workspacePath: chosen });
  }

  const candidates = grantableDevices(seen);
  const chosen = candidates.find((device) => `${device.fingerprint}\n${device.bridgeId}` === form.device);
  const request = chosen ? { deviceFingerprint: chosen.fingerprint, roomId: chosen.bridgeId, workspacePath: form.workspacePath, durationHours: form.durationHours, maxRuns: form.maxRuns } : {};
  const problem = grantFormProblem(request);

  return <section className="v2-settings-group" aria-label="Unattended Codex access">
    <h2>Unattended Codex access</h2>
    <div>
      <p className="v2-settings-note">A permission lets one trusted device start Codex in one folder on this device, over one Bridge, without you clicking Accept, until it expires, runs out or you revoke it. Codex's own approval requests are never answered by it: each one is shown on the device that started the task, where its user may approve it once or deny it; otherwise the task waits until it is stopped. This device must stay on, awake, signed in and running Pastey. Without a permission, every request still waits for your Accept.</p>
      <div className="v2-settings-row"><span><strong>This device's identity</strong><small>Compare this on the other device before trusting.</small></span><b>{identity?.displayFingerprint ?? "…"}</b></div>
      <button type="button" className="v2-button" disabled={pending} onClick={() => { if (window.confirm("Replace this device's identity key? Devices that trusted this one will no longer recognize it until they trust it again.")) void act(rotateDeviceIdentity); }}>Replace identity key…</button>

      <strong>Devices connected now</strong>
      {seen.length === 0 ? <small>No device has shown itself in a current session. On the other device, open the Bridge's network panel and choose “Show this device”.</small> : seen.map((device) => <div className="v2-settings-row" key={`${device.fingerprint}-${device.bridgeId}`}><span><strong>{device.peerName}</strong><small>{device.displayFingerprint}</small></span>{device.trusted ? <b>Trusted</b> : <button type="button" className="v2-button" disabled={pending} onClick={() => trust(device)}>Trust…</button>}</div>)}

      <strong>Trusted devices</strong>
      {trusted.filter((device) => !device.revoked).length === 0 ? <small>None.</small> : trusted.filter((device) => !device.revoked).map((device) => <div className="v2-settings-row" key={device.fingerprint}><span><strong>{device.label}</strong><small>{device.displayFingerprint}</small></span><button type="button" className="v2-button danger" disabled={pending} onClick={() => void act(() => revokeTrustedDevice(device.fingerprint))}>Revoke device</button></div>)}

      <strong>Permissions</strong>
      {grants.length === 0 ? <small>None.</small> : grants.map((grant) => <div className="v2-settings-row" key={grant.grantId}><span><strong>{grant.deviceLabel} · Codex</strong><small>{grant.workspace}</small><small>{grantStateLabel(grant)}{grant.state === "active" ? ` · ${expiryLabel(grant.expiresAt)}` : ""}</small></span>{grant.state === "active" ? <button type="button" className="v2-button danger" disabled={pending} onClick={() => void act(() => revokeUnattendedGrant(grant.grantId))}>Revoke</button> : null}</div>)}

      <strong>Add a permission</strong>
      <label className="v2-target-select"><select value={form.device} onChange={(event) => setForm({ ...form, device: event.target.value })} disabled={pending || candidates.length === 0} aria-label="Trusted device"><option value="">{candidates.length ? "Which trusted device?" : "No trusted device connected"}</option>{candidates.map((device) => <option key={`${device.fingerprint}-${device.bridgeId}`} value={`${device.fingerprint}\n${device.bridgeId}`}>{device.peerName} · {device.displayFingerprint}</option>)}</select></label>
      <button type="button" className="v2-button" disabled={pending} onClick={() => void chooseWorkspace()}>{form.workspacePath ? `Folder: ${form.workspacePath}` : "Choose the folder Codex may work in…"}</button>
      <label className="v2-target-select"><select value={form.durationHours} onChange={(event) => setForm({ ...form, durationHours: Number(event.target.value) })} aria-label="Permission length">{GRANT_DURATIONS.map((duration) => <option key={duration.hours} value={duration.hours}>For {duration.label}</option>)}</select></label>
      <label className="v2-target-select"><select value={form.maxRuns} onChange={(event) => setForm({ ...form, maxRuns: Number(event.target.value) })} aria-label="Run limit">{GRANT_RUN_LIMITS.map((runs) => <option key={runs} value={runs}>Up to {runs} run{runs === 1 ? "" : "s"}</option>)}</select></label>
      <button type="button" className="v2-button primary" disabled={pending || problem !== null} onClick={() => void act(() => createUnattendedGrant(request as Parameters<typeof createUnattendedGrant>[0]).then(() => setForm({ ...form, workspacePath: "" })))}>Allow these runs</button>
      {problem && form.device ? <small>{problem}</small> : null}
      {error ? <p className="v2-error" role="alert">{error}</p> : null}
    </div>
  </section>;
}

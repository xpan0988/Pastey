import { useEffect, useState } from "react";
import {
  getDeviceIdentity,
  listSeenPeerDevices,
  listTrustedDevices,
  revokeTrustedDevice,
  rotateDeviceIdentity,
  trustPeerDevice,
  type DeviceIdentity,
  type SeenPeerDevice,
  type TrustedDevice,
} from "../../lib/tauri";
import { Badge, Dialog, Icon, ListRow } from "./ui";

/** The devices this device's user trusts. Trust is the precondition for
 * giving a device a permission; by itself it lets that device do nothing.
 * Only this device's own user can trust or revoke. */
export function DeviceTrustSettings() {
  const [seen, setSeen] = useState<SeenPeerDevice[]>([]);
  const [trusted, setTrusted] = useState<TrustedDevice[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmTrust, setConfirmTrust] = useState<SeenPeerDevice | null>(null);

  async function refresh() {
    const [nextSeen, nextTrusted] = await Promise.all([
      listSeenPeerDevices().catch(() => []),
      listTrustedDevices().catch(() => []),
    ]);
    setSeen(nextSeen); setTrusted(nextTrusted);
  }
  useEffect(() => { void refresh(); }, []);

  async function act(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await refresh(); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  const activeTrusted = trusted.filter((device) => !device.revoked);

  return <>
    <section className="section" aria-label="Trusted devices">
      <div className="section-head"><h2>Trusted devices</h2><p>Trusting a device lets you give it limited permissions under Permissions. It does not let it run or read anything by itself.</p></div>
      <div className="card"><div className="list">
        {activeTrusted.length === 0 ? <p className="list-empty">No trusted devices.</p> : activeTrusted.map((device) => <ListRow key={device.fingerprint} leading={<Icon name="shield" />} title={device.label} meta={<span className="mono">{device.displayFingerprint}</span>} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void act(() => revokeTrustedDevice(device.fingerprint))}>Revoke device</button>} />)}
      </div></div>
      <span className="subhead">Connected now</span>
      <div className="card"><div className="list">
        {seen.length === 0
          ? <p className="list-empty">No other device has shown itself in a current session. On the other device, open the Bridge's Agents tab or details and choose “Show this device”.</p>
          : seen.map((device) => <ListRow key={`${device.fingerprint}-${device.bridgeId}`} leading={<Icon name="device" />} title={device.peerName} meta={<span className="mono">{device.displayFingerprint}</span>} side={device.trusted ? <Badge tone="ok">Trusted</Badge> : <button type="button" className="btn btn-sm" disabled={pending} onClick={() => setConfirmTrust(device)}>Trust…</button>} />)}
      </div></div>
    </section>
    {error ? <p className="error-text" role="alert">{error}</p> : null}

    {confirmTrust ? <TrustDialog device={confirmTrust} pending={pending} onCancel={() => setConfirmTrust(null)} onConfirm={() => { const device = confirmTrust; setConfirmTrust(null); void act(() => trustPeerDevice(device.fingerprint)); }} /> : null}
  </>;
}

function TrustDialog({ device, pending, onCancel, onConfirm }: { device: SeenPeerDevice; pending: boolean; onCancel: () => void; onConfirm: () => void }) {
  const confirmText = `Only continue if ${device.peerName} shows exactly this identity under Settings → This device:`;
  return <Dialog title={`Trust ${device.peerName}?`} onClose={onCancel} footer={<><button type="button" className="btn" onClick={onCancel}>Cancel</button><button type="button" className="btn btn-primary" disabled={pending} onClick={onConfirm}>It matches. Trust</button></>}>
    <p>{confirmText}</p>
    <p className="mono fingerprint-callout">{device.displayFingerprint}</p>
    <p className="small muted">Trusting it lets you give it limited permissions. It does not let it run anything by itself.</p>
  </Dialog>;
}

/** This device's own identity: the fingerprint others compare before they
 * trust it, and replacing its key. */
export function DeviceIdentitySettings() {
  const [identity, setIdentity] = useState<DeviceIdentity | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmRotate, setConfirmRotate] = useState(false);
  useEffect(() => { void getDeviceIdentity().then(setIdentity).catch(() => setIdentity(null)); }, []);

  async function act(action: () => Promise<DeviceIdentity>) {
    setPending(true); setError(null);
    try { setIdentity(await action()); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  return <>
    <section className="section" aria-label="This device's identity">
      <div className="section-head"><h2>This device's identity</h2><p>Other devices compare this fingerprint before they trust this device. Check it matches what they show.</p></div>
      <div className="card">
        <div className="setting-row"><span><strong>Fingerprint</strong><small>Compare this on the other device before trusting.</small></span><span className="value mono">{identity?.displayFingerprint ?? "…"}</span></div>
        <div className="setting-row"><span><strong>Replace identity key</strong><small>Devices that trusted this one stop recognizing it until they trust it again, and its Bridges need pairing again.</small></span><button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => setConfirmRotate(true)}>Replace…</button></div>
      </div>
    </section>

    {error ? <p className="error-text" role="alert">{error}</p> : null}
    {confirmRotate ? <Dialog title="Replace this device's identity key?" onClose={() => setConfirmRotate(false)} footer={<><button type="button" className="btn" onClick={() => setConfirmRotate(false)}>Cancel</button><button type="button" className="btn btn-danger-solid" disabled={pending} onClick={() => { setConfirmRotate(false); void act(rotateDeviceIdentity); }}>Replace key</button></>}>
      <p>Devices that trusted this one will no longer recognize it until they trust it again.</p>
    </Dialog> : null}
  </>;
}

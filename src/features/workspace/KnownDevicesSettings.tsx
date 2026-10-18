import { useEffect, useState } from "react";
import { forgetKnownDevice, listKnownDevices } from "../../lib/tauri";
import type { KnownDevice } from "../../lib/types";
import { Dialog, Icon, ListRow } from "./ui";

/** Devices this one paired with by code. On the same network they can
 * start a Bridge with this device without a code, by proving the identity
 * key the code confirmed. Forgetting one ends that trust everywhere. */
export function KnownDevicesSettings() {
  const [devices, setDevices] = useState<KnownDevice[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<KnownDevice | null>(null);

  useEffect(() => { void listKnownDevices().then(setDevices).catch(() => setDevices([])); }, []);

  async function forget(device: KnownDevice) {
    setConfirm(null);
    setPending(true); setError(null);
    try { setDevices(await forgetKnownDevice(device.fingerprint)); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  return <section className="section" aria-label="Paired devices">
    <div className="section-head"><h2>Paired devices</h2><p>A device you paired with by code is remembered. On the same network it can start or reconnect a Bridge with this device without a code, by proving the identity key the code confirmed. Other devices on the network always need a code. Pairing grants nothing beyond joining a Bridge.</p></div>
    <div className="card"><div className="list">
      {devices.length === 0 ? <p className="list-empty">None yet.</p> : devices.map((device) => <ListRow key={device.fingerprint} leading={<Icon name="link" />} title={device.label} meta={<span className="mono">{device.displayFingerprint}</span>} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => setConfirm(device)}>Forget…</button>} />)}
    </div></div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
    {confirm ? <Dialog title={`Forget ${confirm.label}?`} onClose={() => setConfirm(null)} footer={<><button type="button" className="btn" onClick={() => setConfirm(null)}>Cancel</button><button type="button" className="btn btn-danger-solid" onClick={() => void forget(confirm)}>Forget device</button></>}>
      <p>It will need a code to connect again, and every Bridge with it ends its current session until you pair it again. Bridge history stays.</p>
    </Dialog> : null}
  </section>;
}

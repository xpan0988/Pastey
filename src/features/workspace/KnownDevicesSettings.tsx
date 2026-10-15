import { useEffect, useState } from "react";
import { forgetKnownDevice, listKnownDevices } from "../../lib/tauri";
import type { KnownDevice } from "../../lib/types";

/** Devices this one paired with by code. On the same network they can
 * start a Bridge with this device without a code, by proving the identity
 * key the code confirmed. Forgetting one ends that trust everywhere. */
export function KnownDevicesSettings() {
  const [devices, setDevices] = useState<KnownDevice[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => { void listKnownDevices().then(setDevices).catch(() => setDevices([])); }, []);

  async function forget(device: KnownDevice) {
    if (!window.confirm(`Forget ${device.label}?\n\nIt will need a code to connect again, and every Bridge with it ends its current session until you pair it again. Bridge history stays.`)) return;
    setPending(true); setError(null);
    try { setDevices(await forgetKnownDevice(device.fingerprint)); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  return <section className="v2-settings-group" aria-label="Paired devices">
    <h2>Paired devices</h2>
    <div>
      <p className="v2-settings-note">A device you paired with by code is remembered. On the same network it can start a new Bridge with this device, or reconnect one, without a code, by proving the identity key the code confirmed. Other devices on the network always need a code. This grants nothing beyond joining a Bridge: Agent tasks still need your Accept or an unattended permission.</p>
      {devices.length === 0 ? <small>None yet.</small> : devices.map((device) => <div className="v2-settings-row" key={device.fingerprint}><span><strong>{device.label}</strong><small>{device.displayFingerprint}</small></span><button type="button" className="v2-button danger" disabled={pending} onClick={() => void forget(device)}>Forget</button></div>)}
      {error ? <p className="v2-error" role="alert">{error}</p> : null}
    </div>
  </section>;
}

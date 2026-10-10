import { useEffect, useState } from "react";
import { bridgeReachability, peerAddressProblem } from "../../lib/bridgeAddress";
import { getBridgePeerAddress, presentDeviceIdentity, setBridgePeerAddress } from "../../lib/tauri";
import { roomPeers } from "./workspaceViewModel";
import type { RoomInfo } from "../../lib/types";

const REACHABILITY_COPY = {
  connected: "Connected",
  reconnecting: "Reconnecting…",
  disconnected: "Not reachable. Pastey keeps trying.",
  none: "No other device yet",
} as const;

/** Where this Bridge's other device can be reached when it is on another
 * network, for reconnects. The address grants nothing: the Bridge's own
 * identity and session keys still decide every connection. */
export function BridgeNetworkPanel({ room }: { room: RoomInfo }) {
  const [saved, setSaved] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const reachability = bridgeReachability(room);
  const peerHostRef = roomPeers(room).find((peer) => peer.hostRef)?.hostRef ?? null;

  async function showDevice() {
    if (!peerHostRef) return;
    setBusy(true); setMessage(null);
    try {
      await presentDeviceIdentity(room.id, peerHostRef);
      setMessage("Shown. On the other device, check Settings → Unattended Codex access.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not reach the other device.");
    } finally { setBusy(false); }
  }

  useEffect(() => {
    let cancelled = false;
    setSaved(null); setDraft(""); setMessage(null);
    void getBridgePeerAddress(room.id).then((address) => {
      if (!cancelled) { setSaved(address); setDraft(address ?? ""); }
    }).catch(() => undefined);
    return () => { cancelled = true; };
  }, [room.id]);

  async function save(address: string | null) {
    if (address !== null) {
      const problem = peerAddressProblem(address);
      if (problem) { setMessage(problem); return; }
    }
    setBusy(true); setMessage(null);
    try {
      const stored = await setBridgePeerAddress(room.id, address === null ? null : address.trim());
      setSaved(stored);
      setDraft(stored ?? "");
      setMessage(stored ? "Saved. Pastey will look for the other device there when it is not on this network." : "Removed. Pastey will look only on this network.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not save this address.");
    } finally { setBusy(false); }
  }

  return <section className="v2-bridge-network" aria-label="Bridge network">
    <button type="button" className="v2-button" onClick={() => setOpen(!open)}>
      <i className={`v2-dot ${reachability === "connected" ? "connected" : reachability === "reconnecting" ? "pending" : ""}`} /> {REACHABILITY_COPY[reachability]}{saved ? " · remote address set" : ""}
    </button>
    {open ? <div className="v2-task-open">
      <strong>Reach the other device from another network</strong>
      <small>Its address on a private network you both use, such as Tailscale or WireGuard (for example home-pc.tail1234.ts.net or 100.x.y.z). Pastey tries it first when reconnecting, then this network. It gives the other device no permissions.</small>
      <input value={draft} onChange={(event) => setDraft(event.target.value)} placeholder="home-pc.tail1234.ts.net" aria-label="Remote address for this Bridge" disabled={busy} />
      <footer>
        {saved ? <button type="button" className="v2-button" disabled={busy} onClick={() => void save(null)}>Remove</button> : null}
        <button type="button" className="v2-button primary" disabled={busy || !draft.trim() || draft.trim() === saved} onClick={() => void save(draft)}>Save address</button>
      </footer>
      <strong>Show this device</strong>
      <small>Proves this device's identity to the other device for this session, so its user can compare fingerprints and choose to trust it. It grants nothing by itself.</small>
      <footer><button type="button" className="v2-button" disabled={busy || !peerHostRef} onClick={() => void showDevice()}>Show this device</button></footer>
      {message ? <small className={message.startsWith("Saved") || message.startsWith("Removed") || message.startsWith("Shown") ? "" : "v2-error"}>{message}</small> : null}
    </div> : null}
  </section>;
}

import { useEffect, useState } from "react";
import { bridgeReachability, peerAddressProblem } from "../../lib/bridgeAddress";
import { getBridgePeerAddress, openBridgePairing, presentDeviceIdentity, repairBridge, setBridgePeerAddress } from "../../lib/tauri";
import { bridgeCode, roomPeers } from "./workspaceViewModel";
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
export function BridgeNetworkPanel({ room, onPairingChanged }: { room: RoomInfo; onPairingChanged: () => Promise<void> }) {
  const [saved, setSaved] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const reachability = bridgeReachability(room);
  const pairing = room.pairing_state ?? "pairing_required";
  const [pairCode, setPairCode] = useState("");

  async function pairWithCode() {
    setBusy(true); setMessage(null);
    try {
      await repairBridge(room.id, pairCode);
      setPairCode("");
      await onPairingChanged();
      setMessage("Paired with the device that showed this code.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not pair with that code.");
    } finally { setBusy(false); }
  }

  async function showNewCode() {
    setBusy(true); setMessage(null);
    try {
      await openBridgePairing(room.id);
      await onPairingChanged();
      setMessage("Shown. On the other device, open New Bridge and enter the new code.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not create a new code.");
    } finally { setBusy(false); }
  }
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
      <i className={`v2-dot ${reachability === "connected" ? "connected" : reachability === "reconnecting" ? "pending" : ""}`} /> {pairing === "pairing_required" ? "Pairing required" : REACHABILITY_COPY[reachability]}{saved ? " · remote address set" : ""}
    </button>
    {open ? <div className="v2-task-open">
      <strong>{pairing === "paired" ? "Paired device" : pairing === "awaiting_peer" ? "Waiting for the other device" : "Pairing required"}</strong>
      <small>{pairing === "paired"
        ? "This Bridge reconnects only to the device it was paired with, which proves its identity each time. Pairing a different device starts fresh: folders shared with this Bridge and Agent tasks and conversations from before are removed."
        : pairing === "awaiting_peer"
          ? `Enter ${bridgeCode(room)} on the other device: under New Bridge, or, if it already has this Bridge, in this panel there. The code works once and expires after 15 minutes.`
          : "This Bridge has no verified device, so it cannot reconnect or receive anything. Pair it again: show a new code here and enter it in this panel on the other device, or enter the code the other device shows. Only the code decides which device is trusted; give it only to your own device. Folders shared with this Bridge and earlier Agent tasks and conversations are removed when it pairs."}</small>
      <footer><button type="button" className="v2-button" disabled={busy} onClick={() => void showNewCode()}>{pairing === "paired" ? "Pair another device" : "Show new code"}</button></footer>
      {/* Shown in every state: after the other device replaces its identity
          this side still looks paired, and only this field can re-pair it. */}
      <>
        <input value={pairCode} onChange={(event) => setPairCode(event.target.value.replace(/\D/g, "").slice(0, 8))} placeholder="Code from the other device" aria-label="Code shown on the other device" disabled={busy} />
        <footer><button type="button" className="v2-button primary" disabled={busy || pairCode.length !== 8} onClick={() => void pairWithCode()}>Pair with code</button></footer>
      </>
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
      {message ? <small className={message.startsWith("Saved") || message.startsWith("Removed") || message.startsWith("Shown") || message.startsWith("Paired") ? "" : "v2-error"}>{message}</small> : null}
    </div> : null}
  </section>;
}

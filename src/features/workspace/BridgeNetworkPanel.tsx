import { useEffect, useState } from "react";
import { bridgeReachability, peerAddressProblem } from "../../lib/bridgeAddress";
import { bridgeInternetLine } from "../../lib/internetStatus";
import { getBridgeInternetStatus, getBridgePeerAddress, openBridgePairing, presentDeviceIdentity, repairBridge, setBridgePeerAddress, type BridgeInternetStatus } from "../../lib/tauri";
import { bridgeCode, roomPeers } from "./workspaceViewModel";
import type { RoomInfo } from "../../lib/types";
import { Badge, Dot, Field, Icon, Notice, type Tone } from "./ui";

const REACHABILITY_COPY = {
  connected: "Connected",
  reconnecting: "Reconnecting…",
  disconnected: "Not reachable. Pastey keeps trying.",
  none: "No other device yet",
} as const;

/** One short status for the Bridge header and sidebar. */
export function bridgeStatus(room: RoomInfo): { label: string; tone: Tone } {
  const pairing = room.pairing_state ?? "pairing_required";
  const reachability = bridgeReachability(room);
  if (pairing === "pairing_required") return { label: "Pairing required", tone: "warn" };
  if (pairing === "awaiting_peer") return { label: "Waiting for device", tone: "neutral" };
  return {
    label: reachability === "disconnected" ? "Not reachable" : REACHABILITY_COPY[reachability],
    tone: reachability === "connected" ? "ok" : reachability === "reconnecting" ? "warn" : "neutral",
  };
}

/** Pairing and reachability for this Bridge. The remote address only says
 * where to look: the Bridge's own identity and session keys still decide
 * every connection. */
export function BridgeConnectionSection({ room, onPairingChanged }: { room: RoomInfo; onPairingChanged: () => Promise<void> }) {
  const [saved, setSaved] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; ok: boolean } | null>(null);
  const reachability = bridgeReachability(room);
  const pairing = room.pairing_state ?? "pairing_required";
  const [pairCode, setPairCode] = useState("");
  const [internet, setInternet] = useState<BridgeInternetStatus | null>(null);

  // The path can change at any time (a network change, the relay taking
  // over): asked again every few seconds while this panel is open.
  useEffect(() => {
    let cancelled = false;
    const refresh = () => void getBridgeInternetStatus(room.id).then((status) => { if (!cancelled) setInternet(status); }).catch(() => undefined);
    refresh();
    const timer = window.setInterval(refresh, 5_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [room.id]);

  async function act(action: () => Promise<string>, failure: string) {
    setBusy(true); setMessage(null);
    try { setMessage({ text: await action(), ok: true }); }
    catch (error) { setMessage({ text: error instanceof Error ? error.message : failure, ok: false }); }
    finally { setBusy(false); }
  }

  const pairWithCode = () => act(async () => {
    await repairBridge(room.id, pairCode);
    setPairCode("");
    await onPairingChanged();
    return "Paired with the device that showed this code.";
  }, "Pastey could not pair with that code.");

  const showNewCode = () => act(async () => {
    await openBridgePairing(room.id);
    await onPairingChanged();
    return "Shown. On the other device, open New Bridge and enter the new code.";
  }, "Pastey could not create a new code.");

  useEffect(() => {
    let cancelled = false;
    setSaved(null); setDraft(""); setMessage(null);
    void getBridgePeerAddress(room.id).then((address) => {
      if (!cancelled) { setSaved(address); setDraft(address ?? ""); }
    }).catch(() => undefined);
    return () => { cancelled = true; };
  }, [room.id]);

  function save(address: string | null) {
    if (address !== null) {
      const problem = peerAddressProblem(address);
      if (problem) { setMessage({ text: problem, ok: false }); return; }
    }
    void act(async () => {
      const stored = await setBridgePeerAddress(room.id, address === null ? null : address.trim());
      setSaved(stored);
      setDraft(stored ?? "");
      return stored ? "Saved. Pastey will look for the other device there when it is not on this network." : "Removed. Pastey will look only on this network.";
    }, "Pastey could not save this address.");
  }

  const statusLabel = pairing === "pairing_required" ? "Pairing required" : REACHABILITY_COPY[reachability];
  return <>
    <section className="inspector-section" aria-label="Connection">
      <h3>Connection</h3>
      <div className="row"><Dot tone={reachability === "connected" ? "ok" : reachability === "reconnecting" ? "warn" : "neutral"} /><span className="spacer small">{statusLabel}</span><Badge>{pairing === "paired" ? "Paired" : pairing === "awaiting_peer" ? "Code open" : "Not paired"}</Badge></div>
      <p className="small muted">{pairing === "paired"
        ? "This Bridge reconnects only to the device it was paired with, which proves its identity each time. Pairing a different device starts fresh: folders shared with this Bridge and Agent tasks and conversations from before are removed."
        : pairing === "awaiting_peer"
          ? `Enter ${bridgeCode(room)} on the other device: under New Bridge, or, if it already has this Bridge, in this panel there. The code works once and expires after 15 minutes.`
          : "This Bridge has no verified device, so it cannot reconnect or receive anything. Pair it again: show a new code here and enter it in this panel on the other device, or enter the code the other device shows. Only the code decides which device is trusted; give it only to your own device. Folders shared with this Bridge and earlier Agent tasks and conversations are removed when it pairs."}</p>
      {/* Shown in every state: after the other device replaces its identity
          this side still looks paired, and only this field can re-pair it.
          While paired it is one click away instead of always open. */}
      <details className="disclosure" open={pairing !== "paired"}>
        <summary><Icon name="chevron" small />{pairing === "paired" ? "Pair again or pair another device" : "Pair this Bridge"}</summary>
        <div className="disclosure-body stack">
          <Field label="Pair with a code" hint="The 8-digit code the other device shows.">
            <div className="row">
              <input className="input input-mono" value={pairCode} onChange={(event) => setPairCode(event.target.value.replace(/\D/g, "").slice(0, 8))} placeholder="4829 1736" aria-label="Code shown on the other device" disabled={busy} inputMode="numeric" />
              <button type="button" className="btn btn-primary" disabled={busy || pairCode.length !== 8} onClick={() => void pairWithCode()}>Pair</button>
            </div>
          </Field>
          <div><button type="button" className="btn btn-sm" disabled={busy} onClick={() => void showNewCode()}>{pairing === "paired" ? "Pair another device…" : "Show a new code"}</button></div>
        </div>
      </details>
    </section>

    <section className="inspector-section" aria-label="Another network">
      <h3>Reach it on another network</h3>
      <p className="small muted"><strong>Across the internet:</strong> {bridgeInternetLine(internet)}</p>
      <p className="small muted">{saved ? <>Pastey also looks for it at <span className="mono">{saved}</span>.</> : "No private-network address."} An address gives the other device no permissions.</p>
      <details className="disclosure" open={!!saved}>
        <summary><Icon name="chevron" small />{saved ? "Change address" : "Add a private-network address"}</summary>
        <div className="disclosure-body stack">
          <p className="xsmall subtle">Its address on a private network you both use, such as Tailscale or WireGuard. Pastey tries it first when reconnecting, then this network.</p>
          <input className="input input-mono" value={draft} onChange={(event) => setDraft(event.target.value)} placeholder="home-pc.tail1234.ts.net" aria-label="Remote address for this Bridge" disabled={busy} />
          <div className="row">
            {saved ? <button type="button" className="btn btn-sm" disabled={busy} onClick={() => save(null)}>Remove</button> : null}
            <button type="button" className="btn btn-sm" disabled={busy || !draft.trim() || draft.trim() === saved} onClick={() => save(draft)}>Save address</button>
          </div>
        </div>
      </details>
    </section>
    {message ? <Notice tone={message.ok ? "ok" : "bad"}>{message.text}</Notice> : null}
  </>;
}

/** Proves this device's identity to the other device for this session, so
 * its user can compare fingerprints and choose to trust it. */
export function ShowDeviceIdentity({ room }: { room: RoomInfo }) {
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; ok: boolean } | null>(null);
  const peerHostRef = roomPeers(room).find((peer) => peer.hostRef)?.hostRef ?? null;

  async function showDevice() {
    if (!peerHostRef) return;
    setBusy(true); setMessage(null);
    try {
      await presentDeviceIdentity(room.id, peerHostRef);
      setMessage({ text: "Shown. On the other device, this Bridge's Workspaces can now trust this one.", ok: true });
    } catch (error) {
      setMessage({ text: error instanceof Error ? error.message : "Pastey could not reach the other device.", ok: false });
    } finally { setBusy(false); }
  }

  return <section className="inspector-section" aria-label="Show this device">
    <h3>Let the other device trust this one</h3>
    <p className="small muted">While this Bridge is open, Pastey shows this device's identity to the other device once per connection, so its user can compare fingerprints and choose to trust it. It grants nothing by itself. If the other device says it has not seen this one, show it again.</p>
    <div><button type="button" className="btn btn-sm" disabled={busy || !peerHostRef} onClick={() => void showDevice()}>Show this device again</button></div>
    {message ? <Notice tone={message.ok ? "ok" : "bad"}>{message.text}</Notice> : null}
  </section>;
}

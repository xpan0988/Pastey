import { useEffect, useState } from "react";
import { internetSettingLine, relayUrlProblem } from "../../lib/internetStatus";
import { getInternetConnectivity, setInternetConnectivity, type InternetConnectivity } from "../../lib/tauri";
import { Dot, Icon, Switch } from "./ui";

/** Whether this device can be reached by its paired devices from other
 * networks, and through which relay. Device-wide: each Bridge still pairs,
 * reconnects and authorizes exactly as on a LAN. */
export function InternetSettings() {
  const [view, setView] = useState<InternetConnectivity | null>(null);
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getInternetConnectivity().then((next) => { if (!cancelled) { setView(next); setDraft(next.relayUrl ?? ""); } }).catch(() => undefined);
    return () => { cancelled = true; };
  }, []);

  async function apply(enabled: boolean, relayUrl: string | null) {
    const problem = relayUrl ? relayUrlProblem(relayUrl) : null;
    if (problem) { setError(problem); return; }
    setPending(true); setError(null);
    try {
      const next = await setInternetConnectivity(enabled, relayUrl ? relayUrl.trim() : null);
      setView(next); setDraft(next.relayUrl ?? "");
    } catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  const line = internetSettingLine(view);
  const relay = view?.relayUrl ?? null;
  return <section className="section" aria-label="Across the internet">
    <div className="section-head"><h2>Across the internet</h2><p>Lets devices already paired with this one reach it from other networks: directly when the networks allow it, otherwise through a relay. Both devices must turn this on. Pair on the same network first; a Bridge code is never used across the internet.</p></div>
    <div className="card">
      <div className="setting-row"><span><strong>Connect across the internet</strong><small><Dot tone={line.tone} /> {line.text}</small></span>
        <Switch label="Connect across the internet" checked={view?.enabled ?? false} disabled={pending || !view} onChange={(value) => void apply(value, relay)} /></div>
      <div style={{ padding: "12px 16px" }}><details className="disclosure">
        <summary><Icon name="chevron" small />{relay ? "Relay: your own" : "Relay: n0's public relays"}</summary>
        <div className="disclosure-body stack">
          <p className="xsmall subtle">A relay passes traffic along when the two networks block a direct path. It sees this device's IP address, its connection key and how much is sent, never what: everything is encrypted between the two devices. n0's public relays are offered for development and hobby use. For dependable use, run your own iroh relay and enter its address; with your own relay Pastey publishes nothing to n0.</p>
          <input className="input input-mono" value={draft} onChange={(event) => setDraft(event.target.value)} placeholder="https://relay.example.com" aria-label="Relay address" disabled={pending || !view} />
          <div className="row">
            {relay ? <button type="button" className="btn btn-sm" disabled={pending} onClick={() => void apply(view?.enabled ?? false, null)}>Use n0's relays</button> : null}
            <button type="button" className="btn btn-sm" disabled={pending || !view || !draft.trim() || draft.trim() === relay} onClick={() => void apply(view?.enabled ?? false, draft)}>Save relay</button>
          </div>
        </div>
      </details></div>
    </div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
  </section>;
}

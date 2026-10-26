import { useCallback, useEffect, useState } from "react";
import {
  closeHostedDeveloperTerminal,
  getBridgeFullAccess,
  getDeveloperTerminalWorkspace,
  grantFullAccess,
  listFullAccess,
  removeFullAccessFolder,
  revokeFullAccess,
  type BridgeFullAccess,
  type FullAccessGrant,
} from "../../lib/tauri";
import { FULL_ACCESS_ENDS, fullAccessAllows, fullAccessLimits, processRightsWarning, type ProcessRights } from "../../lib/fullAccess";
import type { DeveloperTerminalSession, RoomInfo } from "../../lib/types";
import { Badge, Dialog, Icon, ListRow, Notice, errorText } from "./ui";

const OFFER_DISMISSED_KEY = "pastey:full-access-offer-dismissed-v1";

function sinceLabel(grantedAt: number): string {
  return new Date(grantedAt * 1000).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/** Full Access for one Bridge, as this device's user sees it. Asked again
 * when the window regains focus. */
function useBridgeFullAccess(roomId: string) {
  const [state, setState] = useState<BridgeFullAccess | null>(null);
  const refresh = useCallback(async () => {
    setState(await getBridgeFullAccess(roomId).catch(() => null));
  }, [roomId]);
  useEffect(() => {
    void refresh();
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);
  return { state, setState, refresh };
}

/** Bridge details → Full Access: whether the device this Bridge is paired
 * with may control this device without anyone here, and the only place
 * (with Settings → Permissions) to give or revoke it. */
export function BridgeFullAccessSection({ room, peerName }: { room: RoomInfo; peerName: string }) {
  const { state, setState } = useBridgeFullAccess(room.id);
  const [consent, setConsent] = useState(false);
  const [confirmRevoke, setConfirmRevoke] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function act(action: () => Promise<BridgeFullAccess>) {
    setPending(true); setError(null);
    try { setState(await action()); return true; }
    catch (nextError) { setError(errorText(nextError, "Pastey could not change Full Access.")); return false; }
    finally { setPending(false); }
  }

  async function removeFolder(grant: FullAccessGrant, folder: string) {
    setPending(true); setError(null);
    try { await removeFullAccessFolder(grant.grantId, folder); setState(await getBridgeFullAccess(room.id)); }
    catch (nextError) { setError(errorText(nextError, "Pastey could not remove that folder.")); }
    finally { setPending(false); }
  }

  const grant = state?.grant ?? null;
  return <section className="inspector-section" aria-label="Full Access">
    <div className="row"><h3 className="spacer">Full Access</h3>{state ? <Badge tone={grant ? "warn" : "neutral"}>{grant ? "On" : "Off"}</Badge> : null}</div>
    {state === null ? <p className="small muted">Checking…</p>
      : grant ? <>
        <p className="small">{grant.deviceLabel} can control this device without anyone here: run Codex, continue sessions and open a terminal with Pastey's rights. Since {sinceLabel(grant.grantedAt)}.</p>
        {state.processRights === "administrator" ? <Notice tone="bad">{processRightsWarning(state.processRights)}</Notice> : null}
        {grant.folders.length ? <div className="card"><div className="list">
          {grant.folders.map((folder) => <ListRow key={folder} leading={<Icon name="folder" small />} title={folder.split(/[\\/]/).filter(Boolean).pop() ?? folder} meta={<span className="mono">{folder}</span>} side={<button type="button" className="btn btn-sm" disabled={pending} onClick={() => void removeFolder(grant, folder)}>Remove</button>} />)}
        </div></div> : null}
        {grant.folders.length ? <p className="xsmall subtle">Folders {grant.deviceLabel} opened here by path. Removing one stops offering it; Full Access still lets it open folders.</p> : null}
        <div><button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => setConfirmRevoke(true)}>Revoke Full Access</button></div>
      </>
      : state.paired ? <>
        <p className="small muted">Off. Every Codex run, session continuation and terminal {peerName} asks for waits for someone here to click Accept.</p>
        <div><button type="button" className="btn btn-sm" disabled={pending} onClick={() => setConsent(true)}><Icon name="shield" small />Allow Full Access…</button></div>
      </>
      : <p className="small muted">Pair this Bridge first. Full Access can only be given to the device a pairing code proved.</p>}
    {error ? <p className="error-text" role="alert">{error}</p> : null}

    {consent && state ? <FullAccessConsentDialog peerName={peerName} fingerprint={state.displayFingerprint} rights={state.processRights} pending={pending} onClose={() => setConsent(false)} onGrant={() => void act(() => grantFullAccess(room.id, true)).then((ok) => { if (ok) setConsent(false); })} /> : null}
    {confirmRevoke && grant ? <Dialog title={`Revoke ${grant.deviceLabel}'s Full Access?`} onClose={() => { if (!pending) setConfirmRevoke(false); }} footer={<><button type="button" className="btn" disabled={pending} onClick={() => setConfirmRevoke(false)}>Cancel</button><button type="button" className="btn btn-danger-solid" disabled={pending} onClick={() => void act(() => revokeFullAccess(room.id)).then(() => setConfirmRevoke(false))}>Revoke</button></>}>
      <p>From now on, everything {grant.deviceLabel} asks for here waits for your Accept again. A terminal it has open on this device closes now, and Codex tasks Full Access started here are asked to stop.</p>
      <p className="small muted">A stop is a request: Pastey no longer accepts those tasks' results, but cannot always confirm when Codex stops, and anything it already did stays. Giving Full Access again later is a new decision.</p>
    </Dialog> : null}
  </section>;
}

/** The informed-consent step. Granting requires reading what it allows and
 * ticking that you understand; the fingerprint names exactly which device. */
export function FullAccessConsentDialog({ peerName, fingerprint, rights, pending, onClose, onGrant }: { peerName: string; fingerprint: string | null; rights: ProcessRights; pending: boolean; onClose: () => void; onGrant: () => void }) {
  const [understood, setUnderstood] = useState(false);
  const warning = processRightsWarning(rights);
  return <Dialog
    title={`Let ${peerName} control this device?`}
    labelledBy="full-access-title"
    wide
    onClose={() => { if (!pending) onClose(); }}
    footer={<><button type="button" className="btn" disabled={pending} onClick={onClose}>Cancel</button><button type="button" className="btn btn-danger-solid" disabled={pending || !understood} onClick={onGrant}>{pending ? "Allowing…" : "Allow Full Access"}</button></>}
  >
    <p>With Full Access, {peerName} can do these here <strong>without anyone at this device clicking Accept</strong>, even when you are away:</p>
    {warning ? <Notice tone={rights === "administrator" ? "bad" : "warn"}>{warning}</Notice> : null}
    <ul className="small consent-list">{fullAccessAllows(peerName).map((line) => <li key={line}>{line}</li>)}</ul>
    <p className="small"><strong>It does not:</strong></p>
    <ul className="small consent-list">{fullAccessLimits(peerName).map((line) => <li key={line}>{line}</li>)}</ul>
    <p className="small"><strong>It lasts</strong> through reconnects and restarts until one of these ends it:</p>
    <ul className="small consent-list">{FULL_ACCESS_ENDS.map((line) => <li key={line}>{line}</li>)}</ul>
    {fingerprint ? <><p className="small muted">The device this Bridge is paired with, as its pairing code proved. It should match {peerName}'s Settings → This device:</p><p className="mono fingerprint-callout">{fingerprint}</p></> : null}
    <label className="check"><input type="checkbox" checked={understood} disabled={pending} onChange={(event) => setUnderstood(event.target.checked)} /><span>I understand that {peerName} can use this device with Pastey's rights here{rights === "administrator" ? ", which are administrator rights" : ""}, while Full Access is on.</span></label>
  </Dialog>;
}

/** Settings → Permissions: every device this one gave Full Access, across
 * Bridges, each revocable here. */
export function FullAccessSettings() {
  const [grants, setGrants] = useState<FullAccessGrant[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => setGrants(await listFullAccess().catch(() => [])), []);
  useEffect(() => { void refresh(); }, [refresh]);

  async function act(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await refresh(); }
    catch (nextError) { setError(errorText(nextError, "Pastey could not change Full Access.")); }
    finally { setPending(false); }
  }

  return <section className="section" aria-label="Full Access">
    <div className="section-head"><h2>Full Access</h2>
      <p>Devices that can control this one through Pastey without anyone here: run Codex, continue sessions and open a terminal as you. Give it from inside a Bridge (Bridge details → Full Access). It applies in one direction only.</p></div>
    <div className="card"><div className="list">
      {grants.length === 0 ? <p className="list-empty">No device has Full Access. Every request from another device waits for your Accept.</p>
        : grants.map((grant) => <ListRow key={grant.grantId} leading={<Icon name="shield" />} title={grant.deviceLabel} meta={<><span className="mono">{grant.displayFingerprint}</span> · since {sinceLabel(grant.grantedAt)}{grant.folders.length ? ` · ${grant.folders.length} folder${grant.folders.length === 1 ? "" : "s"} opened` : ""}</>} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void act(() => revokeFullAccess(grant.bridgeId))}>Revoke</button>} />)}
    </div></div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
  </section>;
}

/** Above a Bridge's work on the device that would be controlled: once
 * paired, says plainly that Full Access exists and is off; while it is on,
 * shows any terminal the other device has open here, with Close. */
export function FullAccessBanner({ room, peerName, onOpenDetails }: { room: RoomInfo; peerName: string; onOpenDetails: () => void }) {
  const { state } = useBridgeFullAccess(room.id);
  const [hosted, setHosted] = useState<DeveloperTerminalSession[]>([]);
  const [dismissed, setDismissed] = useState<boolean>(() => {
    try { return (JSON.parse(window.localStorage.getItem(OFFER_DISMISSED_KEY) ?? "[]") as string[]).includes(room.id); }
    catch { return false; }
  });
  const [error, setError] = useState<string | null>(null);
  const on = !!state?.grant;

  useEffect(() => {
    if (!on) { setHosted([]); return; }
    let cancelled = false;
    const look = () => void getDeveloperTerminalWorkspace(room.id)
      .then((workspace) => { if (!cancelled) setHosted(workspace.sessions.filter((session) => session.role === "host" && session.state === "active")); })
      .catch(() => undefined);
    look();
    const timer = window.setInterval(look, 4_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [on, room.id]);

  function dismiss() {
    setDismissed(true);
    try {
      const current = JSON.parse(window.localStorage.getItem(OFFER_DISMISSED_KEY) ?? "[]") as string[];
      window.localStorage.setItem(OFFER_DISMISSED_KEY, JSON.stringify([...current.filter((id) => id !== room.id), room.id].slice(-64)));
    } catch { /* a convenience only */ }
  }

  async function close(session: DeveloperTerminalSession) {
    setError(null);
    try { setHosted((await closeHostedDeveloperTerminal(room.id, session.terminalSessionId)).sessions.filter((next) => next.role === "host" && next.state === "active")); }
    catch (nextError) { setError(errorText(nextError, "Pastey could not close that terminal.")); }
  }

  if (!state) return null;
  if (on) {
    if (!hosted.length) return null;
    return <div className="full-access-banner"><Notice tone="warn" title={`${peerName} has a terminal open on this device`} action={<>{hosted.map((session) => <button key={session.terminalSessionId} type="button" className="btn btn-sm" onClick={() => void close(session)}>Close terminal</button>)}</>}>
      It opened with the Full Access you gave {peerName}{hosted[0].environmentLabel ? ` (${hosted[0].environmentLabel})` : ""}. Revoke Full Access in Bridge details to stop it opening more.
    </Notice>{error ? <p className="error-text" role="alert">{error}</p> : null}</div>;
  }
  if (!state.paired || dismissed) return null;
  return <div className="full-access-banner"><Notice tone="info" title={`Use this device from ${peerName} when nobody is here?`} action={<><button type="button" className="btn btn-sm" onClick={onOpenDetails}>Review Full Access…</button><button type="button" className="btn btn-ghost btn-sm" onClick={dismiss}>Not now</button></>}>
    Right now every Codex run, session continuation and terminal {peerName} asks for waits for someone here to click Accept. Full Access lets {peerName} control this device without that, until you revoke it. It never lets this device control {peerName}.
  </Notice></div>;
}

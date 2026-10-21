import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import type { BridgePeerSession } from "../../lib/bridgePeers";
import {
  createUnattendedGrant,
  listBridgeWorkspaceShares,
  listRemoteSharedWorkspaces,
  listSeenPeerDevices,
  listUnattendedGrants,
  revokeUnattendedGrant,
  shareBridgeWorkspace,
  stopSharingWorkspace,
  trustPeerDevice,
  updateWorkspaceShare,
  type SeenPeerDevice,
  type SharedWorkspace,
  type UnattendedGrant,
  type WorkspaceShare,
} from "../../lib/tauri";
import type { RoomInfo } from "../../lib/types";
import { expiryLabel, GRANT_DURATIONS, GRANT_RUN_LIMITS } from "../../lib/unattendedAccess";
import { peerShareGuidance, SHARE_DURATIONS, shareLocation, shareStateNote, shareSummary, visibleShares, type PeerShareKnowledge } from "../../lib/workspaceShares";
import { Badge, Dialog, Icon, ListRow, Notice, Switch, errorText } from "./ui";

const FOCUS_REFRESH_MS = 15_000;

/** What another device of this Bridge shares with this one, as it last said.
 * Asked again when its connection changes and when the window regains
 * focus; never inferred. */
export function usePeerSharedWorkspaces(roomId: string, hostRef: string | null, routeable: boolean) {
  const [knowledge, setKnowledge] = useState<PeerShareKnowledge>({ state: "unknown" });
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const asked = useRef(0);
  const askedAt = useRef(0);

  const refresh = useCallback(async () => {
    if (!hostRef || !routeable) return;
    const ask = ++asked.current;
    askedAt.current = Date.now();
    setLoading(true);
    try {
      const response = await listRemoteSharedWorkspaces(roomId, hostRef);
      if (ask !== asked.current) return;
      if (response.outcome === "ok" || response.outcome === "not_authorized") {
        setKnowledge({ state: "known", workspaces: response.scopes });
        setMessage(null);
      } else {
        setMessage(response.outcome === "busy" ? "It is still answering an earlier request. Check again in a moment." : "It could not answer now.");
      }
    } catch (error) {
      if (ask === asked.current) setMessage(errorText(error, "Pastey could not ask the other device."));
    } finally {
      if (ask === asked.current) setLoading(false);
    }
  }, [hostRef, roomId, routeable]);

  useEffect(() => { setKnowledge({ state: "unknown" }); setMessage(null); }, [hostRef, roomId]);
  useEffect(() => {
    void refresh();
    // The other device answers one query per Bridge at a time, so focus
    // asks again only when the last answer is not recent.
    const onFocus = () => { if (Date.now() - askedAt.current > FOCUS_REFRESH_MS) void refresh(); };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  /** A fresh answer from a session query carries the same list. */
  const adopt = useCallback((workspaces: SharedWorkspace[]) => setKnowledge({ state: "known", workspaces }), []);
  return { knowledge, loading, message, refresh, adopt };
}

export type PeerShares = ReturnType<typeof usePeerSharedWorkspaces>;

/** Workspaces over this Bridge: the folders this device shares, managed
 * here by this device's user, and the folders each other device shares,
 * which only that device's user can change. */
export function BridgeWorkspaces({ room, members, peers }: { room: RoomInfo; members: readonly BridgePeerSession[]; peers: readonly BridgePeerSession[] }) {
  const others = members.filter((member) => member.hostRef);
  const peerName = others.length === 1 ? others[0].displayName : "the other device";
  return <section className="inspector-section" aria-label="Workspaces">
    <h3>Workspaces</h3>
    <p className="small muted">Folders shared over this Bridge. Each device shares only its own folders, and chooses what the other device may do in them.</p>
    <LocalShares room={room} peerName={peerName} />
    {others.map((member) => <PeerSharesCard key={member.hostRef} roomId={room.id} hostRef={member.hostRef!} name={member.displayName} routeable={peers.some((peer) => peer.hostRef === member.hostRef)} />)}
  </section>;
}

type Draft = { path: string; readSessions: boolean; shareExcerpts: boolean; agentRequests: boolean; durationHours: number };

/** The folders this device shares with this Bridge's device. Every change
 * is this device's own command; the other device can only ask. */
function LocalShares({ room, peerName }: { room: RoomInfo; peerName: string }) {
  const [shares, setShares] = useState<WorkspaceShare[]>([]);
  const [seen, setSeen] = useState<SeenPeerDevice[]>([]);
  const [unattended, setUnattended] = useState<UnattendedGrant[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [allowing, setAllowing] = useState<WorkspaceShare | null>(null);
  const [confirmTrust, setConfirmTrust] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const [nextShares, nextSeen, nextUnattended] = await Promise.all([
      listBridgeWorkspaceShares(room.id).catch(() => []),
      listSeenPeerDevices().catch(() => []),
      listUnattendedGrants().catch(() => []),
    ]);
    setShares(nextShares);
    setSeen(nextSeen.filter((device) => device.bridgeId === room.id));
    setUnattended(nextUnattended.filter((grant) => grant.bridgeId === room.id && grant.state === "active"));
  }, [room.id]);

  useEffect(() => {
    void refresh();
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);
  // The other device shows itself when it opens this Bridge; look again
  // until it has.
  useEffect(() => {
    if (seen.length) return;
    const timer = window.setInterval(() => void refresh(), 10_000);
    return () => window.clearInterval(timer);
  }, [refresh, seen.length]);

  async function act(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await refresh(); return true; }
    catch (nextError) { setError(errorText(nextError, "Pastey could not change this share.")); return false; }
    finally { setPending(false); }
  }

  // One device per Bridge is the usual case; with more, the newest shown.
  const device = seen.length ? seen[seen.length - 1] : null;
  const name = device?.peerName ?? peerName;
  const visible = visibleShares(shares);

  async function chooseFolder() {
    setError(null);
    const chosen = await open({ directory: true, multiple: false, title: `Share a folder of this device with ${name}` });
    if (typeof chosen === "string") setDraft({ path: chosen, readSessions: false, shareExcerpts: false, agentRequests: false, durationHours: 24 });
  }

  const shareBlocker = !device
    ? `${name} hasn't shown its identity on this connection yet. It does when this Bridge is open on ${name}.`
    : !device.trusted ? `Trust ${name} first, once: check its fingerprint, then share.` : null;

  return <div className="stack">
    <span className="subhead">Shared from this device</span>
    {visible.length ? <div className="card"><div className="list">
      {visible.map((share) => <ShareRow
        key={share.grantId}
        share={share}
        peerName={name}
        unattended={unattended.filter((grant) => grant.workspace === share.workspace && grant.capabilityId === "agent.coding.codex")}
        pending={pending}
        onUpdate={(next) => void act(() => updateWorkspaceShare({ grantId: share.grantId, readSessions: next.readSessions, shareExcerpts: next.shareExcerpts, agentRequests: next.agentRequests, durationHours: next.durationHours ?? null }))}
        onStop={() => void act(() => stopSharingWorkspace(share.grantId))}
        onAllowWithoutAsking={() => setAllowing(share)}
        onRevokeUnattended={(grantId) => void act(() => revokeUnattendedGrant(grantId))}
      />)}
    </div></div> : <p className="small muted">This device shares no folder with {name}. Sharing a folder lets {name} pick it by name, without its path, and only for what you turn on.</p>}
    {device && !device.trusted ? <Notice tone="info" title={`${name} is not trusted on this device yet`} action={<button type="button" className="btn btn-sm" disabled={pending} onClick={() => setConfirmTrust(true)}>Trust {name}…</button>}>Needed once, before you share anything with it. Trust alone allows nothing.</Notice> : null}
    <div className="row"><button type="button" className="btn btn-sm" disabled={pending || !!shareBlocker} onClick={() => void chooseFolder()}><Icon name="folder" small />Share a workspace…</button>{shareBlocker ? <span className="xsmall subtle spacer">{shareBlocker}</span> : null}</div>
    {!device ? <div><button type="button" className="btn btn-link small" onClick={() => void refresh()}>Check again</button></div> : null}
    {error ? <p className="error-text" role="alert">{error}</p> : null}

    {confirmTrust && device ? <Dialog title={`Trust ${device.peerName}?`} labelledBy="share-trust-title" onClose={() => setConfirmTrust(false)} footer={<><button type="button" className="btn" onClick={() => setConfirmTrust(false)}>Cancel</button><button type="button" className="btn btn-primary" disabled={pending} onClick={() => { setConfirmTrust(false); void act(() => trustPeerDevice(device.fingerprint)); }}>It matches. Trust</button></>}>
      <p>{`Only continue if ${device.peerName} shows exactly this identity under Settings → This device:`}</p>
      <p className="mono fingerprint-callout">{device.displayFingerprint}</p>
      <p className="small muted">Trusting it lets you share folders with it and give it limited permissions here. It does not let it read or run anything by itself.</p>
    </Dialog> : null}

    {draft && device ? <ShareDialog draft={draft} peerName={device.peerName} pending={pending} onChange={setDraft} onClose={() => setDraft(null)} onShare={() => void act(() => shareBridgeWorkspace({ roomId: room.id, deviceFingerprint: device.fingerprint, workspacePath: draft.path, durationHours: draft.durationHours, readSessions: draft.readSessions, shareExcerpts: draft.readSessions && draft.shareExcerpts, agentRequests: draft.agentRequests })).then((ok) => { if (ok) setDraft(null); })} /> : null}

    {allowing ? <AllowWithoutAskingDialog share={allowing} peerName={name} pending={pending} onClose={() => setAllowing(null)} onAllow={(durationHours, maxRuns) => void act(() => createUnattendedGrant({ deviceFingerprint: allowing.deviceFingerprint, roomId: room.id, workspacePath: allowing.workspace, durationHours, maxRuns })).then((ok) => { if (ok) setAllowing(null); })} /> : null}
  </div>;
}

function CapabilityChecks({ value, peerName, disabled, onChange }: {
  value: Pick<Draft, "readSessions" | "shareExcerpts" | "agentRequests">;
  peerName: string;
  disabled: boolean;
  onChange: (next: Pick<Draft, "readSessions" | "shareExcerpts" | "agentRequests">) => void;
}) {
  return <div className="stack">
    <label className="check"><input type="checkbox" checked={value.readSessions} disabled={disabled} onChange={(event) => onChange({ ...value, readSessions: event.target.checked, shareExcerpts: event.target.checked && value.shareExcerpts })} /><span>Sessions<small>{peerName} can see the Claude Code and Codex sessions in this folder: statuses, titles, activity, changed file names and Git facts. Never whole conversations.</small></span></label>
    {value.readSessions ? <label className="check nested"><input type="checkbox" checked={value.shareExcerpts} disabled={disabled} onChange={(event) => onChange({ ...value, shareExcerpts: event.target.checked })} /><span>Short excerpts too<small>Up to four recent prompts and replies, shortened.</small></span></label> : null}
    <label className="check"><input type="checkbox" checked={value.agentRequests} disabled={disabled} onChange={(event) => onChange({ ...value, agentRequests: event.target.checked })} /><span>Agent requests<small>{peerName} can ask to run Codex here, or to continue a Codex thread here when it can also see sessions. Each request waits for your Accept on this device.</small></span></label>
  </div>;
}

function ShareDialog({ draft, peerName, pending, onChange, onClose, onShare }: { draft: Draft; peerName: string; pending: boolean; onChange: (draft: Draft) => void; onClose: () => void; onShare: () => void }) {
  const folderName = draft.path.split(/[\\/]/).filter(Boolean).pop() ?? draft.path;
  return <Dialog title={`Share ${folderName} with ${peerName}`} labelledBy="share-workspace-title" onClose={() => { if (!pending) onClose(); }} footer={<><button type="button" className="btn" disabled={pending} onClick={onClose}>Cancel</button><button type="button" className="btn btn-primary" disabled={pending} onClick={onShare}>{pending ? "Sharing…" : "Share"}</button></>}>
    <p className="small">{peerName} sees this folder only as “{folderName}”, never its path:</p>
    <p className="mono small">{draft.path}</p>
    <CapabilityChecks value={draft} peerName={peerName} disabled={pending} onChange={(next) => onChange({ ...draft, ...next })} />
    <label className="field-label" htmlFor="share-duration">For how long</label>
    <select id="share-duration" className="select" value={draft.durationHours} disabled={pending} onChange={(event) => onChange({ ...draft, durationHours: Number(event.target.value) })}>{SHARE_DURATIONS.map((duration) => <option key={duration.hours} value={duration.hours}>{duration.label}</option>)}</select>
    {!draft.readSessions && !draft.agentRequests ? <p className="xsmall subtle">With nothing turned on, {peerName} sees only the folder's name. You can turn things on later.</p> : null}
  </Dialog>;
}

function ShareRow({ share, peerName, unattended, pending, onUpdate, onStop, onAllowWithoutAsking, onRevokeUnattended }: {
  share: WorkspaceShare;
  peerName: string;
  unattended: UnattendedGrant[];
  pending: boolean;
  onUpdate: (next: { readSessions: boolean; shareExcerpts: boolean; agentRequests: boolean; durationHours?: number }) => void;
  onStop: () => void;
  onAllowWithoutAsking: () => void;
  onRevokeUnattended: (grantId: string) => void;
}) {
  const active = share.state === "active";
  const note = shareStateNote(share);
  const set = (change: Partial<{ readSessions: boolean; shareExcerpts: boolean; agentRequests: boolean }>) => {
    const next = { readSessions: share.readSessions, shareExcerpts: share.shareExcerpts, agentRequests: share.agentRequests, ...change };
    onUpdate({ ...next, shareExcerpts: next.readSessions && next.shareExcerpts });
  };
  return <div className="list-row share-row" aria-label={`Shared folder ${share.label}`}>
    <span className="list-row-main stack">
      <span className="row"><Icon name="folder" small /><strong className="spacer">{share.label}</strong>{active ? <Badge tone="ok">{shareSummary(share)}</Badge> : <Badge>{share.state === "expired" ? "Ended" : "Not usable"}</Badge>}</span>
      <span className="list-row-meta mono">{share.workspace}</span>
      <span className="list-row-meta">With {share.deviceLabel} · {active ? expiryLabel(share.expiresAt) : note}</span>
      {active ? <span className="stack share-switches">
        <span className="row"><span className="spacer small">Sessions{share.readSessions ? share.shareExcerpts ? " · with excerpts" : " · statuses only" : ""}</span><Switch label={`${share.label}: sessions`} checked={share.readSessions} disabled={pending} onChange={(value) => set({ readSessions: value })} /></span>
        {share.readSessions ? <span className="row"><span className="spacer xsmall subtle">Short excerpts of recent prompts and replies</span><Switch label={`${share.label}: excerpts`} checked={share.shareExcerpts} disabled={pending} onChange={(value) => set({ shareExcerpts: value })} /></span> : null}
        <span className="row"><span className="spacer small">Agent requests<span className="xsmall subtle"> · each waits for your Accept</span></span><Switch label={`${share.label}: agent requests`} checked={share.agentRequests} disabled={pending} onChange={(value) => set({ agentRequests: value })} /></span>
        {share.agentRequests ? unattended.length
          ? unattended.map((grant) => <span className="row" key={grant.grantId}><span className="spacer xsmall">Codex runs without asking: {grant.maxRuns - grant.runs} of {grant.maxRuns} left, {expiryLabel(grant.expiresAt)}</span><button type="button" className="btn btn-sm" disabled={pending} onClick={() => onRevokeUnattended(grant.grantId)}>Revoke</button></span>)
          : <span className="row"><span className="spacer xsmall subtle">Every Codex run waits for your Accept here.</span><button type="button" className="btn btn-link small" disabled={pending} onClick={onAllowWithoutAsking}>Allow without asking…</button></span> : null}
      </span> : null}
    </span>
    <span className="list-row-side stack">
      {share.state === "expired" ? <button type="button" className="btn btn-sm" disabled={pending} onClick={() => onUpdate({ readSessions: share.readSessions, shareExcerpts: share.shareExcerpts, agentRequests: share.agentRequests, durationHours: 24 })}>Renew for 1 day</button> : null}
      <button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={onStop} title={`${peerName} can no longer pick this folder, read its sessions or ask to run anything in it`}>Stop sharing</button>
    </span>
  </div>;
}

function AllowWithoutAskingDialog({ share, peerName, pending, onClose, onAllow }: { share: WorkspaceShare; peerName: string; pending: boolean; onClose: () => void; onAllow: (durationHours: number, maxRuns: number) => void }) {
  const [durationHours, setDurationHours] = useState(8);
  const [maxRuns, setMaxRuns] = useState(5);
  return <Dialog title={`Let ${peerName} run Codex in ${share.label} without asking?`} labelledBy="allow-unattended-title" onClose={() => { if (!pending) onClose(); }} footer={<><button type="button" className="btn" disabled={pending} onClick={onClose}>Cancel</button><button type="button" className="btn btn-primary" disabled={pending} onClick={() => onAllow(durationHours, maxRuns)}>Allow these runs</button></>}>
    <p className="small">Runs from {peerName} in this folder then start without your Accept, until the time or the runs run out, or you revoke it. Codex's own approval prompts still go to {peerName}'s user, and continuing an existing thread still waits for your Accept.</p>
    <div className="row-wrap">
      <select className="select" value={durationHours} disabled={pending} onChange={(event) => setDurationHours(Number(event.target.value))} aria-label="How long">{GRANT_DURATIONS.map((duration) => <option key={duration.hours} value={duration.hours}>For {duration.label}</option>)}</select>
      <select className="select" value={maxRuns} disabled={pending} onChange={(event) => setMaxRuns(Number(event.target.value))} aria-label="How many runs">{GRANT_RUN_LIMITS.map((runs) => <option key={runs} value={runs}>{runs === 1 ? "1 run" : `${runs} runs`}</option>)}</select>
    </div>
  </Dialog>;
}

/** What one other device shares with this one, as it last said. Changing
 * it is that device's own decision; this says exactly where. */
function PeerSharesCard({ roomId, hostRef, name, routeable }: { roomId: string; hostRef: string; name: string; routeable: boolean }) {
  const shares = usePeerSharedWorkspaces(roomId, hostRef, routeable);
  return <div className="stack">
    <span className="subhead">Shared by {name}</span>
    <PeerSharesList name={name} shares={shares} routeable={routeable} />
  </div>;
}

export function PeerSharesList({ name, shares, routeable, need }: { name: string; shares: PeerShares; routeable: boolean; need?: "read" | "run" | "continue" }) {
  if (!routeable) return <p className="small muted">{name} is not connected right now. What it shares appears here once it reconnects.</p>;
  if (shares.knowledge.state === "unknown") return <p className="small muted">{shares.message ?? (shares.loading ? `Asking ${name}…` : `Not asked yet.`)} {shares.message ? <button type="button" className="btn btn-link small" onClick={() => void shares.refresh()}>Check again</button> : null}</p>;
  const workspaces = shares.knowledge.workspaces;
  const guidance = peerShareGuidance(name, shares.knowledge, need ?? "read");
  return <>
    {workspaces.length ? <div className="card"><div className="list">
      {workspaces.map((share) => <ListRow key={share.scopeRef} leading={<Icon name="folder" small />} title={share.label} meta={`${shareSummary(share)}${share.agentRequests ? ` · each run waits for ${name}'s Accept` : ""}${share.expiresAt ? ` · ${expiryLabel(share.expiresAt)}` : ""}`} />)}
    </div></div> : null}
    {guidance && (need || !workspaces.length) ? <p className="small muted">{guidance}</p> : null}
    <div className="row"><span className="xsmall subtle spacer">{workspaces.length ? `Only ${name}'s user can change these. ${shareLocation(name)}.` : ""}</span><button type="button" className="btn btn-link small" disabled={shares.loading} onClick={() => void shares.refresh()}>{shares.loading ? "Asking…" : "Check again"}</button></div>
  </>;
}

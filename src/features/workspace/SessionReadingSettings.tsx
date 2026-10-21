import { useEffect, useState } from "react";
import { listRooms, listSessionReadGrants, stopSharingWorkspace, type SessionReadGrant } from "../../lib/tauri";
import type { RoomInfo } from "../../lib/types";
import { expiryLabel } from "../../lib/unattendedAccess";
import { shareStateNote, shareSummary } from "../../lib/workspaceShares";
import { Badge, Icon, ListRow } from "./ui";
import { bridgeCode } from "./workspaceViewModel";

/** Every folder this device shares, across its Bridges, with Stop sharing
 * beside each. Shares are created and changed inside their Bridge, where
 * its device is already known; this page only gathers them. Only this
 * device's own user decides; trusting a device shares nothing. */
export function SessionReadingSettings() {
  const [shares, setShares] = useState<SessionReadGrant[]>([]);
  const [rooms, setRooms] = useState<RoomInfo[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const [nextShares, nextRooms] = await Promise.all([listSessionReadGrants().catch(() => []), listRooms().catch(() => [])]);
    setShares(nextShares); setRooms(nextRooms);
  }
  useEffect(() => { void refresh(); }, []);

  async function act(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await refresh(); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  const bridgeName = (bridgeId: string) => {
    const room = rooms.find((candidate) => candidate.id === bridgeId);
    return room ? `Bridge ${bridgeCode(room)}` : "an ended Bridge";
  };
  const ordered = [...shares]
    .filter((share) => share.state !== "revoked")
    .sort((left, right) => Number(right.state === "active") - Number(left.state === "active"));

  return <section className="section" aria-label="Shared workspaces">
    <div className="section-head"><h2>Shared workspaces</h2>
      <p>Folders this device shares, each with one trusted device over one Bridge, until it ends or you stop sharing it. For each folder you choose whether that device can see its Claude Code and Codex sessions (statuses, titles, activity, changed file names and Git facts, never folder paths or whole conversations) and whether it can ask to run Codex there, which always waits for your Accept unless you allowed a few runs without asking. Trusting a device does not let it read anything.</p></div>
    <div className="card">
      <div className="list">
        {ordered.length === 0 ? <p className="list-empty">This device shares no folder.</p> : ordered.map((share) => <ListRow key={share.grantId} leading={<Icon name="folder" />} title={`${share.label} · with ${share.deviceLabel}`} meta={<>{share.workspace}<br />{share.state === "active" ? `${shareSummary(share)} · ${bridgeName(share.bridgeId)} · ${expiryLabel(share.expiresAt)}` : shareStateNote(share)}</>} side={share.state === "active" || share.state === "expired" ? <button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void act(() => stopSharingWorkspace(share.grantId))}>Stop sharing</button> : <Badge>Not usable</Badge>} />)}
      </div>
      <footer className="card-footer"><span className="small muted">To share a folder or change what it allows, open its Bridge, then Bridge details → Workspaces.</span></footer>
    </div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
  </section>;
}

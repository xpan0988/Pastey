import { useEffect, useMemo, useState } from "react";
import {
  getBridgeNodeListProjection,
  listAuthorizedSearchFolders,
  listBridgeWorkspaceShares,
  listUnattendedGrants,
  revokeAuthorizedSearchFolder,
  revokeUnattendedGrant,
  type AuthorizedSearchFolder,
  type UnattendedGrant,
} from "../../lib/tauri";
import type { BridgeNodeListProjectionV1, RoomInfo } from "../../lib/types";
import { expiryLabel } from "../../lib/unattendedAccess";
import { BridgeConnectionSection, ShowDeviceIdentity } from "./BridgeNetworkPanel";
import { BridgeWorkspaces } from "./BridgeWorkspaces";
import { Dialog, Dot, Icon, ListRow } from "./ui";
import { bridgeCode, roomMembers, roomPeers } from "./workspaceViewModel";

/** Everything about the current Bridge that is not the work itself:
 * connection, members, the folders shared over it and what each allows,
 * anything else this device allows over it, and Burn. */
export function BridgeDetails({ room, onClose, onRefresh, onBurn, onOpenSettings, onOpenDevice }: {
  room: RoomInfo;
  onClose: () => void;
  onRefresh: () => Promise<void>;
  onBurn: (room: RoomInfo) => Promise<void>;
  onOpenSettings: () => void;
  onOpenDevice: (hostRef: string | null) => void;
}) {
  const [confirmBurn, setConfirmBurn] = useState(false);
  const [burning, setBurning] = useState(false);
  return <aside className="inspector" aria-label="Bridge details">
    <header className="inspector-header">
      <h2>Bridge {bridgeCode(room)}</h2>
      <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label="Close details" onClick={onClose}><Icon name="close" /></button>
    </header>
    <div className="inspector-body">
      <BridgeConnectionSection room={room} onPairingChanged={onRefresh} />
      <BridgeDevices room={room} onOpenDevice={onOpenDevice} />
      <BridgeWorkspaces room={room} members={roomMembers(room)} peers={roomPeers(room)} />
      <BridgePermissions room={room} onOpenSettings={onOpenSettings} />
      <ShowDeviceIdentity room={room} />
      <section className="inspector-section" aria-label="Burn">
        <h3>Burn this Bridge</h3>
        <p className="small muted">Ends the Bridge on this device for good. Files you saved stay where they are.</p>
        <div><button type="button" className="btn btn-danger btn-sm" onClick={() => setConfirmBurn(true)}><Icon name="flame" small />Burn Bridge…</button></div>
      </section>
    </div>
    {confirmBurn ? <Dialog
      title={`Burn Bridge ${bridgeCode(room)}?`}
      onClose={() => { if (!burning) setConfirmBurn(false); }}
      footer={<><button type="button" className="btn" disabled={burning} onClick={() => setConfirmBurn(false)}>Cancel</button><button type="button" className="btn btn-danger-solid" disabled={burning} onClick={() => { setBurning(true); void onBurn(room).finally(() => setBurning(false)); }}>{burning ? "Burning…" : "Burn Bridge"}</button></>}
    >
      <p>This permanently removes the Bridge content, membership, authority, and current-session state from this device. Explicitly saved user-owned files remain outside Bridge storage.</p>
      <p className="small muted">This cannot be undone. To work with this device again, pair a new Bridge.</p>
    </Dialog> : null}
  </aside>;
}

/** Members of this Bridge. Inspection only: devices join a Bridge only by
 * pairing. Each opens its Device Details, where checks run on request. */
export function BridgeDevices({ room, onOpenDevice }: { room: RoomInfo; onOpenDevice: (hostRef: string | null) => void }) {
  const peers = useMemo(() => roomMembers(room), [room]);
  const [nodeList, setNodeList] = useState<BridgeNodeListProjectionV1 | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getBridgeNodeListProjection(room.id)
      .then((projection) => { if (!cancelled) setNodeList(projection); })
      .catch(() => { if (!cancelled) setNodeList(null); });
    return () => { cancelled = true; };
  }, [room.id]);

  const rows = nodeList
    ? nodeList.nodes.map((node) => {
      const isLocal = node.hostRef === nodeList.links[0]?.sourceHostRef || node.deviceProfile !== undefined && node.deviceProfile !== null;
      const state = isLocal ? "connected" : node.currentSession?.liveness ?? "disconnected";
      return { key: node.hostRef, hostRef: isLocal ? null : node.hostRef, name: isLocal ? "This device" : node.displayName ?? "Bridge device", state };
    })
    : [{ key: "local", hostRef: null, name: "This device", state: "connected" }, ...peers.map((peer) => ({ key: peer.peerSessionId, hostRef: peer.hostRef ?? null, name: peer.displayName, state: peer.liveness as string }))];

  return <section className="inspector-section" aria-label="Devices">
    <h3>Devices</h3>
    <div className="card"><div className="list">
      {rows.map((row) => <ListRow
        key={row.key}
        leading={<Dot tone={row.state === "connected" ? "ok" : row.state === "reconnecting" ? "warn" : "neutral"} />}
        title={row.name}
        meta={row.hostRef === null ? "You · details in Settings → This device" : row.state === "connected" ? "Connected now" : `Member · ${row.state}`}
        side={<Icon name="chevron" small />}
        onClick={() => onOpenDevice(row.hostRef)}
      />)}
    </div></div>
    {peers.length === 0 ? <p className="small muted">No other device has joined yet.</p> : null}
    <p className="xsmall subtle">Devices join a Bridge only by pairing. Open a device for its connection, a speed test, what it can run, and trust.</p>
  </section>;
}

/** What else this device lets the other devices of this Bridge do here,
 * outside its shared workspaces, with Revoke beside each: finding files,
 * and unattended runs in a folder given by its path. */
function BridgePermissions({ room, onOpenSettings }: { room: RoomInfo; onOpenSettings: () => void }) {
  const [unattended, setUnattended] = useState<UnattendedGrant[]>([]);
  const [folders, setFolders] = useState<AuthorizedSearchFolder[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const [nextUnattended, nextShares, nextFolders] = await Promise.all([
      listUnattendedGrants().catch(() => []),
      listBridgeWorkspaceShares(room.id).catch(() => []),
      listAuthorizedSearchFolders().catch(() => []),
    ]);
    // Runs in a shared folder are shown, and revoked, with that folder.
    const shared = new Set(nextShares.filter((share) => share.state === "active" && share.agentRequests).map((share) => share.workspace));
    setUnattended(nextUnattended.filter((grant) => grant.bridgeId === room.id && grant.state === "active" && !shared.has(grant.workspace)));
    setFolders(nextFolders.filter((folder) => folder.bridgeId === room.id));
  }
  useEffect(() => {
    void refresh();
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [room.id]); // eslint-disable-line react-hooks/exhaustive-deps

  async function revoke(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await refresh(); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  const none = unattended.length === 0 && folders.length === 0;
  return <section className="inspector-section" aria-label="Also allowed on this device">
    <h3>Also allowed on this device</h3>
    {none
      ? <p className="small muted">Nothing else. Outside the workspaces above, other devices of this Bridge cannot search folders here, and every Codex run they ask for waits for your Accept.</p>
      : <div className="card"><div className="list">
        {unattended.map((grant) => <ListRow key={grant.grantId} title="Start Codex without asking" meta={`${grant.deviceLabel} · ${grant.workspace} · ${grant.runs} of ${grant.maxRuns} runs used · ${expiryLabel(grant.expiresAt)}`} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void revoke(() => revokeUnattendedGrant(grant.grantId))}>Revoke</button>} />)}
        {folders.map((folder) => <ListRow key={folder.folderId} title={`Find files in ${folder.displayName}`} meta={folder.unavailable ? "Moved or replaced; not searched" : "Any device on this Bridge, by file name"} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void revoke(() => revokeAuthorizedSearchFolder(folder.folderId))}>Stop sharing</button>} />)}
      </div></div>}
    <div><button type="button" className="btn btn-link small" onClick={onOpenSettings}>All Bridges: Settings → Permissions</button></div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
  </section>;
}

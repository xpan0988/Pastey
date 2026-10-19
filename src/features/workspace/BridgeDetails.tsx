import { useEffect, useMemo, useState } from "react";
import {
  getBridgeNodeListProjection,
  listAuthorizedSearchFolders,
  listSessionReadGrants,
  listUnattendedGrants,
  revokeAuthorizedSearchFolder,
  revokeSessionReadGrant,
  revokeUnattendedGrant,
  type AuthorizedSearchFolder,
  type SessionReadGrant,
  type UnattendedGrant,
} from "../../lib/tauri";
import type { BridgeNodeListProjectionV1, RoomInfo } from "../../lib/types";
import { expiryLabel } from "../../lib/unattendedAccess";
import { BridgeConnectionSection, ShowDeviceIdentity } from "./BridgeNetworkPanel";
import { Dialog, Dot, Icon, ListRow } from "./ui";
import { bridgeCode, roomMembers } from "./workspaceViewModel";

/** Everything about the current Bridge that is not the work itself:
 * connection, members, what this device allows over it, and Burn. */
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

/** What this device lets the other devices of this Bridge do here, with
 * Revoke beside each. Permissions are created only in Settings, by this
 * device's own user. */
function BridgePermissions({ room, onOpenSettings }: { room: RoomInfo; onOpenSettings: () => void }) {
  const [unattended, setUnattended] = useState<UnattendedGrant[]>([]);
  const [reading, setReading] = useState<SessionReadGrant[]>([]);
  const [folders, setFolders] = useState<AuthorizedSearchFolder[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const [nextUnattended, nextReading, nextFolders] = await Promise.all([
      listUnattendedGrants().catch(() => []),
      listSessionReadGrants().catch(() => []),
      listAuthorizedSearchFolders().catch(() => []),
    ]);
    setUnattended(nextUnattended.filter((grant) => grant.bridgeId === room.id && grant.state === "active"));
    setReading(nextReading.filter((grant) => grant.bridgeId === room.id && grant.state === "active"));
    setFolders(nextFolders.filter((folder) => folder.bridgeId === room.id));
  }
  useEffect(() => { void refresh(); }, [room.id]); // eslint-disable-line react-hooks/exhaustive-deps

  async function revoke(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await refresh(); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  const none = unattended.length === 0 && reading.length === 0 && folders.length === 0;
  return <section className="inspector-section" aria-label="Allowed on this device">
    <h3>Allowed on this device</h3>
    {none
      ? <p className="small muted">Nothing. Over this Bridge, other devices cannot search folders here, read sessions here, or start Codex here without your Accept.</p>
      : <div className="card"><div className="list">
        {unattended.map((grant) => <ListRow key={grant.grantId} title="Start Codex without asking" meta={`${grant.deviceLabel} · ${grant.workspace} · ${grant.runs} of ${grant.maxRuns} runs used · ${expiryLabel(grant.expiresAt)}`} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void revoke(() => revokeUnattendedGrant(grant.grantId))}>Revoke</button>} />)}
        {reading.map((grant) => <ListRow key={grant.grantId} title="Read Agent sessions" meta={`${grant.deviceLabel} · ${grant.workspace} · ${grant.shareExcerpts ? "statuses and excerpts" : "statuses only"} · ${expiryLabel(grant.expiresAt)}`} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void revoke(() => revokeSessionReadGrant(grant.grantId))}>Revoke</button>} />)}
        {folders.map((folder) => <ListRow key={folder.folderId} title={`Find files in ${folder.displayName}`} meta={folder.unavailable ? "Moved or replaced; not searched" : "Any device on this Bridge, by file name"} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void revoke(() => revokeAuthorizedSearchFolder(folder.folderId))}>Stop sharing</button>} />)}
      </div></div>}
    <div><button type="button" className="btn btn-link small" onClick={onOpenSettings}>Manage permissions in Settings</button></div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
  </section>;
}

import { useMemo, useState } from "react";
import type { TransferQueueItem } from "../../lib/transferScheduler";
import type { FileTransferProgressEvent, RoomInfo, RoomItem } from "../../lib/types";
import { MessageCard, TransferMessage } from "./BridgeWorkspace";
import { bridgeCode, formatBytes, formatClock, roomPeers } from "./workspaceViewModel";

export function ActivityScreen({ items, transfers, queueItems }: { items: RoomItem[]; transfers: FileTransferProgressEvent[]; queueItems: TransferQueueItem[] }) {
  const activeTransfers = transfers.filter((transfer) => !["completed", "failed", "cancelled", "burned", "interrupted"].includes(transfer.status));
  const activeQueue = queueItems.filter((item) => ["queued", "preparing", "sending"].includes(item.status));
  const recentItems = items.slice(0, 8);
  return (
    <section className="v2-screen">
      <header className="v2-screen-header"><div><h1>Activity</h1><p>What has happened in this Bridge.</p></div><button type="button" className="v2-button" disabled>Open receiving folder</button></header>
      <div className="v2-screen-body v2-activity-body">
        <ActivityGroup label="Now">
          {activeTransfers.map((transfer) => <TransferMessage key={transfer.transfer_id} name={transfer.file_name} status={transfer.status} />)}
          {activeQueue.map((item) => <TransferMessage key={item.id} name={item.displayName ?? item.path} status={item.status} />)}
          {activeTransfers.length === 0 && activeQueue.length === 0 ? <EmptyRow text="No transfer is active." /> : null}
        </ActivityGroup>
        <ActivityGroup label="Pending"><EmptyRow text="No task is waiting for review or Host admission in this renderer." /></ActivityGroup>
        <ActivityGroup label="Recent">
          {recentItems.map((item) => <MessageCard key={item.id} item={item} />)}
          {recentItems.length === 0 ? <EmptyRow text="No recent Bridge activity." /> : null}
        </ActivityGroup>
      </div>
    </section>
  );
}

function ActivityGroup({ label, children }: { label: string; children: React.ReactNode }) {
  return <section className="v2-activity-group"><h2>{label}</h2><div>{children}</div></section>;
}

function EmptyRow({ text }: { text: string }) {
  return <div className="v2-empty-row"><i className="v2-dot" /><span>{text}</span></div>;
}

export function DevicesScreen({ room, onAddDevice }: { room: RoomInfo | null; onAddDevice: () => void }) {
  const peers = useMemo(() => roomPeers(room), [room]);
  return (
    <section className="v2-screen">
      <header className="v2-screen-header"><div><h1>Bridge devices</h1><p>Current members and connection state.</p></div><button type="button" className="v2-button primary" onClick={onAddDevice}>＋ Add device</button></header>
      <div className="v2-screen-body v2-devices-body">
        <h2>Connected</h2>
        <div className="v2-device-list">
          {room ? <DeviceRow name="This device" meta="Local Host" detail="Managed Agent, transfer, and Developer Mode are separate" connected /> : null}
          {peers.map((peer) => <DeviceRow key={peer.peerSessionId} name={peer.displayName} meta="Current-session Bridge member" detail="Capability projection unavailable" connected={peer.liveness === "connected"} />)}
          {!room ? <EmptyRow text="Device state is unavailable until the Pastey Host is connected." /> : null}
        </div>
        <h2>Add another device</h2>
        <section className="v2-nearby-card"><strong>Nearby devices</strong><p>Open Pastey on another computer on the same LAN, or join with an 8-digit Bridge code.</p><div><button type="button" className="v2-button" disabled>Scan nearby</button><button type="button" className="v2-button" onClick={onAddDevice}>Join with code</button></div></section>
      </div>
    </section>
  );
}

function DeviceRow({ name, meta, detail, connected }: { name: string; meta: string; detail: string; connected: boolean }) {
  return <article className="v2-device-row"><div className="v2-device-icon" /><div><strong>{name}</strong><small>{meta}</small><p>{detail}</p></div><i className={`v2-dot ${connected ? "connected" : ""}`} /></article>;
}

export function DeviceDetailPanel({ room, onDeveloper }: { room: RoomInfo | null; onDeveloper: () => void }) {
  const peers = useMemo(() => roomPeers(room), [room]);
  const selected = peers[0] ?? null;
  return (
    <aside className="v2-context-panel v2-device-detail-panel">
      <p className="v2-eyebrow">Current Bridge</p>
      <div className="v2-context-status"><span><i className={`v2-dot ${room?.peer_connected ? "connected" : ""}`} /> {room?.peer_connected ? "Connected" : room ? "Waiting" : "Unavailable"}</span><small>{room ? bridgeCode(room) : "No Bridge selected"}</small></div>
      <div className="v2-context-section"><small>Devices</small><strong>{room ? peers.length + 1 : 0} current members</strong></div>
      {selected ? <section className="v2-device-detail-card"><strong>{selected.displayName}</strong><small>Current-session member</small><hr /><label>Managed Agent</label><p>Availability not renderer-exposed</p><label>Transfer</label><p>{selected.liveness === "connected" ? "Connected route available" : "Unavailable"}</p><label>Developer Mode</label><p>Availability not renderer-exposed</p></section> : <p className="v2-context-empty">Select a connected device to view renderer-safe details.</p>}
      <button type="button" className="v2-button v2-full-button" disabled={!selected} onClick={onDeveloper}>Open Developer Mode</button>
    </aside>
  );
}

export function NewBridgeScreen({ onCreate, onJoin }: { onCreate: () => Promise<void>; onJoin: (code: string) => Promise<void> }) {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  async function run(action: () => Promise<void>) {
    setBusy(true); setMessage(null);
    try { await action(); } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); } finally { setBusy(false); }
  }
  return (
    <section className="v2-screen">
      <header className="v2-screen-header"><div><h1>New Bridge</h1><p>Connect another device, then keep the work scoped to this Bridge.</p></div></header>
      <div className="v2-screen-body v2-new-bridge-body">
        <h2>Nearby</h2>
        <div className="v2-nearby-empty"><p>Nearby discovery results are not exposed on this screen.</p><button type="button" className="v2-button primary" disabled={busy} onClick={() => void run(onCreate)}>Start Bridge</button></div>
        <h2>Join manually</h2>
        <section className="v2-join-card"><strong>Enter an 8-digit Bridge code</strong><small>Session permission ends when the Bridge is burned, left, or reconnected.</small><div><input value={code} onChange={(event) => setCode(event.target.value.replace(/\D/g, "").slice(0, 8))} placeholder="4829 1736" aria-label="Bridge code" /><button type="button" className="v2-button primary" disabled={code.length !== 8 || busy} onClick={() => void run(() => onJoin(code))}>Join</button></div></section>
        {message ? <p className="v2-error">{message}</p> : null}
      </div>
    </section>
  );
}

export function InboxScreen({ items, inboxDir }: { items: RoomItem[]; inboxDir?: string | null }) {
  const received = items.filter((item) => item.direction === "incoming");
  const [filter, setFilter] = useState<"all" | "text" | "images" | "files">("all");
  const filtered = received.filter((item) => filter === "all" || filter === "text" ? filter === "all" || item.payload_type === "text" : item.payload_type === "file");
  return (
    <section className="v2-screen v2-inbox-screen">
      <header className="v2-screen-header"><div><h1>Inbox</h1><p>Received text, images, and files across Bridges.</p></div><button type="button" className="v2-button" disabled>Open folder</button></header>
      <div className="v2-filterbar">{(["all", "text", "images", "files"] as const).map((value) => <button key={value} type="button" className={filter === value ? "active" : ""} onClick={() => setFilter(value)}>{value[0].toUpperCase() + value.slice(1)} <span>{value === "all" ? received.length : value === "text" ? received.filter((item) => item.payload_type === "text").length : received.filter((item) => item.payload_type === "file").length}</span></button>)}</div>
      <div className="v2-screen-body v2-inbox-body"><h2>Today</h2>{filtered.map((item) => <MessageCard key={item.id} item={item} />)}{filtered.length === 0 ? <EmptyRow text="No received items in this view." /> : null}</div>
      <aside className="v2-inbox-context"><p className="v2-eyebrow">Inbox</p><h2>{received.length} items</h2><p>{inboxDir ?? "Receiving folder not configured"}</p><hr /><p>Session-only items disappear when their Bridge ends. Saved user-owned files remain.</p><hr /><p>Items retain their source Bridge; Pastey does not imply shared storage.</p></aside>
    </section>
  );
}

export function ItemMetadata({ item }: { item: RoomItem }) {
  return <span>{formatClock(item.created_at)} · {formatBytes(item.size_bytes)}</span>;
}

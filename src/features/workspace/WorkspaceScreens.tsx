import { useEffect, useMemo, useState } from "react";
import type { TransferQueueItem } from "../../lib/transferScheduler";
import type { FileTransferProgressEvent, NearbyDevice, RoomInfo, RoomItem } from "../../lib/types";
import { MessageCard, TransferMessage } from "./BridgeWorkspace";
import { bridgeCode, formatBytes, formatClock, roomPeers } from "./workspaceViewModel";

export function ActivityScreen({ items, transfers, queueItems, onRevealInFolder }: { items: RoomItem[]; transfers: FileTransferProgressEvent[]; queueItems: TransferQueueItem[]; onRevealInFolder?: (path: string) => Promise<void> }) {
  const activeTransfers = transfers.filter((transfer) => !["completed", "failed", "cancelled", "burned", "interrupted"].includes(transfer.status));
  const activeQueue = queueItems.filter((item) => ["queued", "preparing", "sending"].includes(item.status));
  const recentItems = items.slice(0, 8);
  const firstSavedPath = items.find((item) => item.direction === "incoming" && item.saved_path)?.saved_path ?? null;
  return (
    <section className="v2-screen">
      <header className="v2-screen-header"><div><h1>Activity</h1><p>What has happened in this Bridge.</p></div><button type="button" className="v2-button" disabled={!firstSavedPath || !onRevealInFolder} onClick={() => { if (firstSavedPath && onRevealInFolder) void onRevealInFolder(firstSavedPath); }}>Open receiving folder</button></header>
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
        <section className="v2-nearby-card"><strong>Nearby devices</strong><p>Open Pastey on another computer on the same LAN, or join with an 8-digit Bridge code.</p><div><button type="button" className="v2-button" onClick={onAddDevice}>Find nearby devices</button><button type="button" className="v2-button" onClick={onAddDevice}>Join with code</button></div></section>
      </div>
    </section>
  );
}

function DeviceRow({ name, meta, detail, connected }: { name: string; meta: string; detail: string; connected: boolean }) {
  return <article className="v2-device-row"><div className="v2-device-icon" /><div><strong>{name}</strong><small>{meta}</small><p>{detail}</p></div><i className={`v2-dot ${connected ? "connected" : ""}`} /></article>;
}

export function NewBridgeScreen({
  onCreate,
  onJoin,
  onListNearby,
  onJoinNearby,
  nearbyDiscoveryAvailable,
}: {
  onCreate: () => Promise<void>;
  onJoin: (code: string) => Promise<void>;
  onListNearby: () => Promise<NearbyDevice[]>;
  onJoinNearby: (deviceId: string) => Promise<void>;
  nearbyDiscoveryAvailable: boolean;
}) {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [nearbyDevices, setNearbyDevices] = useState<NearbyDevice[]>([]);
  const [nearbyMessage, setNearbyMessage] = useState("Looking for nearby Pastey devices…");

  useEffect(() => {
    if (!nearbyDiscoveryAvailable) {
      setNearbyMessage("Nearby discovery is available in the Pastey desktop app.");
      return;
    }
    let cancelled = false;
    let inFlight = false;

    async function loadNearby() {
      if (inFlight) return;
      inFlight = true;
      try {
        const devices = await onListNearby();
        if (cancelled) return;
        setNearbyDevices(devices);
        setNearbyMessage(devices.length === 0 ? "No nearby devices found. Keep Pastey open on the other device." : "");
      } catch {
        if (cancelled) return;
        setNearbyDevices([]);
        setNearbyMessage("Pastey cannot see nearby devices on this network.");
      } finally {
        inFlight = false;
      }
    }

    void loadNearby();
    const interval = window.setInterval(() => void loadNearby(), 2_000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [nearbyDiscoveryAvailable, onListNearby]);

  async function run(action: () => Promise<void>) {
    setBusy(true); setMessage(null);
    try { await action(); } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); } finally { setBusy(false); }
  }
  return (
    <section className="v2-screen">
      <header className="v2-screen-header"><div><h1>New Bridge</h1><p>Connect another device, then keep the work scoped to this Bridge.</p></div></header>
      <div className="v2-screen-body v2-new-bridge-body">
        <h2>Nearby</h2>
        <div className="v2-nearby-list">
          {nearbyDevices.map((device) => {
            const ready = device.availability === "Available" && device.compatible;
            return <article key={device.device_id} className="v2-nearby-device"><div className="v2-device-icon" /><div><strong>{device.display_name}</strong><small>{nearbyDeviceSummary(device)}</small><span className={`v2-nearby-status ${ready ? "ready" : ""}`}><i className={`v2-dot ${ready ? "connected" : ""}`} />{device.compatible ? device.availability : "Update needed"}</span></div><button type="button" className="v2-button primary" disabled={busy || !ready} onClick={() => void run(() => onJoinNearby(device.device_id))}>Join</button></article>;
          })}
          {nearbyDevices.length === 0 ? <div className="v2-nearby-empty"><p>{nearbyMessage}</p><button type="button" className="v2-button" disabled={busy} onClick={() => void run(onCreate)}>Create with code</button></div> : null}
        </div>
        <h2>Join manually</h2>
        <section className="v2-join-card"><strong>Enter an 8-digit Bridge code</strong><small>Session permission ends when the Bridge is burned or the current session is replaced.</small><div><input value={code} onChange={(event) => setCode(event.target.value.replace(/\D/g, "").slice(0, 8))} placeholder="4829 1736" aria-label="Bridge code" /><button type="button" className="v2-button primary" disabled={code.length !== 8 || busy} onClick={() => void run(() => onJoin(code))}>Join</button></div></section>
        {message ? <p className="v2-error">{message}</p> : null}
      </div>
    </section>
  );
}

function nearbyDeviceSummary(device: NearbyDevice): string {
  const platform = device.platform.trim() || "Nearby device";
  const seen = device.last_seen_seconds_ago <= 3
    ? "Online"
    : `Seen ${Math.max(0, Math.round(device.last_seen_seconds_ago))}s ago`;
  return [platform, device.app_version ? `Pastey ${device.app_version}` : null, seen].filter(Boolean).join(" · ");
}

export function InboxScreen({ items, inboxDir, onRevealInFolder }: { items: RoomItem[]; inboxDir?: string | null; onRevealInFolder?: (path: string) => Promise<void> }) {
  const received = items.filter((item) => item.direction === "incoming");
  const folderTarget = inboxDir ?? received.find((item) => item.saved_path)?.saved_path ?? null;
  const [filter, setFilter] = useState<"all" | "text" | "images" | "files">("all");
  const filtered = received.filter((item) => filter === "all" || filter === "text" ? filter === "all" || item.payload_type === "text" : item.payload_type === "file");
  return (
    <section className="v2-screen v2-inbox-screen">
      <header className="v2-screen-header"><div><h1>Inbox</h1><p>Received text, images, and files across Bridges.</p></div><button type="button" className="v2-button" disabled={!folderTarget || !onRevealInFolder} onClick={() => { if (folderTarget && onRevealInFolder) void onRevealInFolder(folderTarget); }}>Open folder</button></header>
      <div className="v2-filterbar">{(["all", "text", "images", "files"] as const).map((value) => <button key={value} type="button" className={filter === value ? "active" : ""} onClick={() => setFilter(value)}>{value[0].toUpperCase() + value.slice(1)} <span>{value === "all" ? received.length : value === "text" ? received.filter((item) => item.payload_type === "text").length : received.filter((item) => item.payload_type === "file").length}</span></button>)}</div>
      <div className="v2-screen-body v2-inbox-body"><h2>Today</h2>{filtered.map((item) => <MessageCard key={item.id} item={item} />)}{filtered.length === 0 ? <EmptyRow text="No received items in this view." /> : null}</div>
    </section>
  );
}

export function ItemMetadata({ item }: { item: RoomItem }) {
  return <span>{formatClock(item.created_at)} · {formatBytes(item.size_bytes)}</span>;
}

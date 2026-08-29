import { useEffect, useMemo, useRef, useState } from "react";
import type { TransferQueueItem } from "../../lib/transferScheduler";
import type { FileTransferProgressEvent, NearbyDevice, RoomInfo, RoomItem } from "../../lib/types";
import { MessageCard, TransferMessage } from "./BridgeWorkspace";
import { formatBytes, formatClock, roomMembers, uniqueNearbyDevices } from "./workspaceViewModel";

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

export function DevicesScreen({ room }: { room: RoomInfo | null }) {
  const peers = useMemo(() => roomMembers(room), [room]);
  return (
    <section className="v2-screen">
      <header className="v2-screen-header"><div><h1>Bridge devices</h1><p>Known members and their current connection state. Device admission is not supported from this view.</p></div></header>
      <div className="v2-screen-body v2-devices-body">
        <h2>Current Bridge</h2>
        <div className="v2-device-list">
          {room ? <DeviceRow name="This device" meta="Local Host" detail="This logical Bridge remains local until Burn." state="connected" /> : null}
          {peers.map((peer) => <DeviceRow key={peer.peerSessionId} name={peer.displayName} meta="Current Bridge member" detail={peer.liveness === "connected" ? "Exact current session is routeable." : "Logical membership is retained; the old session is not routeable."} state={peer.liveness} />)}
          {!room ? <EmptyRow text="Select a Bridge to inspect its devices." /> : null}
          {room && peers.length === 0 ? <EmptyRow text="No remote device has joined this Bridge yet." /> : null}
        </div>
        <p className="v2-developer-footnote">Nearby and code-based Bridge creation belong to New Bridge. Pastey does not currently add another Host to an existing Bridge.</p>
      </div>
    </section>
  );
}

function DeviceRow({ name, meta, detail, state }: { name: string; meta: string; detail: string; state: string }) {
  return <article className="v2-device-row"><div className="v2-device-icon" /><div><strong>{name}</strong><small>{meta} · {state}</small><p>{detail}</p></div><i className={`v2-dot ${state === "connected" ? "connected" : state === "reconnecting" ? "pending" : ""}`} /></article>;
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
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const busyRef = useRef(false);
  const [message, setMessage] = useState<string | null>(null);
  const [nearbyDevices, setNearbyDevices] = useState<NearbyDevice[]>([]);
  const [nearbyMessage, setNearbyMessage] = useState("Looking for nearby Pastey devices…");

  async function loadNearby(cancelled?: () => boolean) {
    if (!nearbyDiscoveryAvailable) {
      setNearbyMessage("Nearby discovery is available in the Pastey desktop app.");
      return;
    }
    try {
      const devices = await onListNearby();
      if (cancelled?.()) return;
      const unique = uniqueNearbyDevices(devices);
      setNearbyDevices(unique);
      setNearbyMessage(unique.length === 0 ? "No nearby devices found. Keep Pastey open on the other device." : "");
    } catch {
      if (cancelled?.()) return;
      setNearbyDevices([]);
      setNearbyMessage("Pastey cannot see nearby devices on this network.");
    }
  }

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;
    const refresh = async () => {
      if (inFlight) return;
      inFlight = true;
      try { await loadNearby(() => cancelled); } finally { inFlight = false; }
    };

    void refresh();
    const interval = window.setInterval(() => void refresh(), 2_000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [nearbyDiscoveryAvailable, onListNearby]);

  async function run(actionName: string, action: () => Promise<void>) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusyAction(actionName); setMessage(null);
    try { await action(); } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); } finally { busyRef.current = false; setBusyAction(null); }
  }
  return (
    <section className="v2-screen">
      <header className="v2-screen-header"><div><h1>New Bridge</h1><p>Connect another device, then keep the work scoped to this Bridge.</p></div></header>
      <div className="v2-screen-body v2-new-bridge-body">
        <h2>Nearby</h2>
        <div className="v2-nearby-list">
          {nearbyDevices.map((device) => {
            const ready = device.availability === "Available" && device.compatible;
            return <article key={device.device_id} className="v2-nearby-device"><div className="v2-device-icon" /><div><strong>{device.display_name}</strong><small>{nearbyDeviceSummary(device)}</small><span className={`v2-nearby-status ${ready ? "ready" : ""}`}><i className={`v2-dot ${ready ? "connected" : ""}`} />{device.compatible ? device.availability : "Update needed"}</span></div><button type="button" className="v2-button primary" disabled={busyAction !== null || !ready} onClick={() => void run(`nearby:${device.device_id}`, () => onJoinNearby(device.device_id))}>{busyAction === `nearby:${device.device_id}` ? "Waiting…" : "Join"}</button></article>;
          })}
          {nearbyDevices.length === 0 ? <div className="v2-nearby-empty"><p>{nearbyMessage}</p><button type="button" className="v2-button" disabled={busyAction !== null} onClick={() => void run("refresh", async () => { await loadNearby(); })}>{busyAction === "refresh" ? "Refreshing…" : "Refresh"}</button></div> : null}
        </div>
        <h2>Join manually</h2>
        <section className="v2-join-card"><strong>Enter an 8-digit Bridge code</strong><small>Session permission ends when the Bridge is burned or the current session is replaced.</small><div><input value={code} onChange={(event) => setCode(event.target.value.replace(/\D/g, "").slice(0, 8))} placeholder="4829 1736" aria-label="Bridge code" /><button type="button" className="v2-button primary" disabled={code.length !== 8 || busyAction !== null} onClick={() => void run("join", () => onJoin(code))}>{busyAction === "join" ? "Joining…" : "Join"}</button></div></section>
        <h2>Create with code</h2>
        <section className="v2-join-card"><strong>Create an empty Bridge</strong><small>A new Bridge and 8-digit code are created only after this explicit action.</small><div><button type="button" className="v2-button primary" disabled={busyAction !== null} onClick={() => void run("create", onCreate)}>{busyAction === "create" ? "Creating…" : "Create Bridge"}</button></div></section>
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

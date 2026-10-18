import { peerAddressProblem } from "../../lib/bridgeAddress";
import { useEffect, useRef, useState } from "react";
import type { NearbyDevice, NearbyJoinPending, RoomInfo, RoomItem } from "../../lib/types";
import { MessageCard } from "./BridgeWorkspace";
import { Badge, Dot, EmptyState, Field, Icon, ListRow, Notice, Segmented } from "./ui";
import { bridgeCode, uniqueNearbyDevices } from "./workspaceViewModel";

export function NewBridgeScreen({
  onCreate,
  onJoin,
  onListNearby,
  onJoinNearby,
  onCompleteNearby,
  nearbyDiscoveryAvailable,
  menuButton,
}: {
  onCreate: () => Promise<void>;
  onJoin: (code: string, address?: string) => Promise<void>;
  onListNearby: () => Promise<NearbyDevice[]>;
  onJoinNearby: (deviceId: string) => Promise<NearbyJoinPending | null>;
  onCompleteNearby: (requestId: string, code: string) => Promise<void>;
  nearbyDiscoveryAvailable: boolean;
  menuButton?: React.ReactNode;
}) {
  const [code, setCode] = useState("");
  const [nearbyPending, setNearbyPending] = useState<NearbyJoinPending | null>(null);
  const [nearbyCode, setNearbyCode] = useState("");
  const [remote, setRemote] = useState(false);
  const [address, setAddress] = useState("");
  const addressProblem = remote ? peerAddressProblem(address) : null;
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
    <section className="page">
      <header className="page-header">{menuButton}<div className="page-header-main"><h1>New Bridge</h1><p>Pair this device with one of your other devices.</p></div></header>
      <div className="page-body"><div className="content stack-lg">
        <section className="section">
          <div className="section-head-row"><div className="section-head"><h2>Nearby</h2><p>Pastey devices on this network. A device you paired before connects without a code.</p></div>
            <button type="button" className="btn btn-sm" disabled={busyAction !== null} onClick={() => void run("refresh", async () => { await loadNearby(); })}><Icon name="refresh" small />{busyAction === "refresh" ? "Refreshing…" : "Refresh"}</button></div>
          <div className="card"><div className="list">
            {nearbyDevices.map((device) => {
              // A device paired before may start another Bridge even while Busy.
              const ready = (device.availability === "Available" || device.known === true) && device.compatible;
              return <ListRow
                key={device.device_id}
                leading={<Icon name="device" />}
                title={device.display_name}
                meta={<>{device.known ? "Paired before · " : ""}{nearbyDeviceSummary(device)}</>}
                side={<>{device.compatible ? device.availability === "Available" ? null : <Badge>{device.availability}</Badge> : <Badge tone="warn">Update needed</Badge>}<button type="button" className="btn btn-primary btn-sm" disabled={busyAction !== null || !ready} onClick={() => void run(`nearby:${device.device_id}`, async () => { const pending = await onJoinNearby(device.device_id); setNearbyCode(""); setNearbyPending(pending); })}>{busyAction === `nearby:${device.device_id}` ? (device.known ? "Connecting…" : "Waiting…") : device.known ? "Connect" : "Join"}</button></>}
              />;
            })}
            {nearbyDevices.length === 0 ? <div className="list-empty row"><Dot pulse={nearbyDiscoveryAvailable} />{nearbyMessage}</div> : null}
          </div>
          {nearbyPending ? <div className="add-form">
            <strong>{nearbyPending.deviceName} accepted</strong>
            <p className="small muted">Enter the 8-digit code now shown on {nearbyPending.deviceName}. The code is never sent over the network; it proves both devices are the ones you chose. Both devices then remember each other and connect without a code next time.</p>
            <div className="row"><input className="input code-input" value={nearbyCode} onChange={(event) => setNearbyCode(event.target.value.replace(/\D/g, "").slice(0, 8))} placeholder="4829 1736" aria-label="Code shown on the other device" inputMode="numeric" autoFocus /><button type="button" className="btn btn-primary" disabled={nearbyCode.length !== 8 || busyAction !== null} onClick={() => void run("nearby-code", async () => { await onCompleteNearby(nearbyPending.requestId, nearbyCode); setNearbyPending(null); })}>{busyAction === "nearby-code" ? "Pairing…" : "Pair"}</button></div>
          </div> : null}
          </div>
        </section>

        <section className="section">
          <div className="section-head"><h2>Join with a code</h2><p>Enter the 8-digit code another device shows for its Bridge.</p></div>
          <div className="card"><div className="card-body">
            <div className="row"><input className="input code-input" value={code} onChange={(event) => setCode(event.target.value.replace(/\D/g, "").slice(0, 8))} placeholder="4829 1736" aria-label="Bridge code" inputMode="numeric" /><button type="button" className="btn btn-primary" disabled={code.length !== 8 || busyAction !== null || (remote && addressProblem !== null)} onClick={() => void run("join", () => onJoin(code, remote ? address.trim() : undefined))}>{busyAction === "join" ? "Joining…" : "Join"}</button></div>
            <label className="check"><input type="checkbox" checked={remote} onChange={(event) => setRemote(event.target.checked)} /><span>The other device is on another network</span></label>
            {remote ? <Field label="Its private-network address" hint={<>Its address on a private network you both use, such as Tailscale or WireGuard. The address only says where to look; the code still decides whether you join, and it gives the other device no permissions.{address.trim() && addressProblem ? ` ${addressProblem}` : ""}</>}>
              <input className="input input-mono" value={address} onChange={(event) => setAddress(event.target.value)} placeholder="home-pc.tail1234.ts.net or 100.x.y.z" aria-label="Other device's overlay address" />
            </Field> : null}
            <p className="xsmall subtle">Session permission ends when the Bridge is burned or the current session is replaced.</p>
          </div></div>
        </section>

        <section className="section">
          <div className="section-head"><h2>Show a code here</h2><p>Create an empty Bridge on this device and enter its code on the other one. A new Bridge and 8-digit code are created only after this explicit action.</p></div>
          <div><button type="button" className="btn" disabled={busyAction !== null} onClick={() => void run("create", onCreate)}>{busyAction === "create" ? "Creating…" : "Create Bridge"}</button></div>
        </section>
        {message ? <Notice tone="bad">{message}</Notice> : null}
      </div></div>
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

type InboxFilter = "all" | "text" | "files";

/** Everything received on this device, across Bridges. */
export function InboxScreen({ items, rooms, inboxDir, onRevealInFolder, menuButton }: { items: RoomItem[]; rooms: RoomInfo[]; inboxDir?: string | null; onRevealInFolder?: (path: string) => Promise<void>; menuButton?: React.ReactNode }) {
  const received = items.filter((item) => item.direction === "incoming").sort((left, right) => right.created_at - left.created_at);
  const folderTarget = inboxDir ?? received.find((item) => item.saved_path)?.saved_path ?? null;
  const [filter, setFilter] = useState<InboxFilter>("all");
  const filtered = received.filter((item) => filter === "all" || (filter === "text" ? item.payload_type === "text" : item.payload_type === "file"));
  const count = (value: InboxFilter) => value === "all" ? received.length : received.filter((item) => value === "text" ? item.payload_type === "text" : item.payload_type === "file").length;
  const bridgeName = (roomId: string) => {
    const room = rooms.find((candidate) => candidate.id === roomId);
    return room ? room.peer_device_name ?? `Bridge ${bridgeCode(room)}` : "An ended Bridge";
  };
  return (
    <section className="page">
      <header className="page-header">{menuButton}<div className="page-header-main"><h1>Inbox</h1><p>Text and files received on this device, from every Bridge.</p></div>
        <div className="page-header-actions"><button type="button" className="btn btn-sm" disabled={!folderTarget || !onRevealInFolder} onClick={() => { if (folderTarget && onRevealInFolder) void onRevealInFolder(folderTarget); }}><Icon name="folder" small />Open folder</button></div></header>
      <div className="page-body"><div className="content stack-lg">
        <Segmented label="Show" value={filter} onChange={setFilter} options={(["all", "text", "files"] as const).map((value) => ({ value, label: `${value === "all" ? "All" : value === "text" ? "Text" : "Files"} ${count(value)}` }))} />
        {filtered.length === 0 ? <EmptyState title="Nothing here yet">Text and files other devices send you appear here.</EmptyState> : <div className="stack-lg">
          {filtered.map((item) => <div key={item.id} className="stack" style={{ gap: 4 }}>
            <span className="xsmall subtle">From {bridgeName(item.room_id)}</span>
            <MessageCard item={item} onRevealInFolder={onRevealInFolder} />
          </div>)}
        </div>}
      </div></div>
    </section>
  );
}

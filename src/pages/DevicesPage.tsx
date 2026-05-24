import { useEffect, useRef, useState } from "react";
import { joinRoom, listNearbyDevices, requestNearbyJoin } from "../lib/tauri";
import { formatCode, formatTimestamp } from "../lib/format";
import type { NearbyDevice, RoomInfo } from "../lib/types";

interface DevicesPageProps {
  rooms: RoomInfo[];
  onOpenRoom: (room: RoomInfo) => void;
  shouldFocus: boolean;
}

export function DevicesPage({ rooms, onOpenRoom, shouldFocus }: DevicesPageProps) {
  const [joinCode, setJoinCode] = useState("");
  const [nearbyDevices, setNearbyDevices] = useState<NearbyDevice[]>([]);
  const [busy, setBusy] = useState<"join" | "nearby" | null>(null);
  const [joiningDeviceId, setJoiningDeviceId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nearbyMessage, setNearbyMessage] = useState<string | null>(null);
  const joinInputRef = useRef<HTMLInputElement | null>(null);
  const recentRoom = rooms[0] ?? null;

  useEffect(() => {
    if (shouldFocus) {
      joinInputRef.current?.focus();
      joinInputRef.current?.select();
    }
  }, [shouldFocus]);

  useEffect(() => {
    let cancelled = false;

    async function loadNearby() {
      try {
        const devices = await listNearbyDevices();
        if (cancelled) return;
        setNearbyDevices(devices);
        setNearbyMessage(devices.length === 0 ? "No nearby devices found." : null);
      } catch {
        if (!cancelled) {
          setNearbyDevices([]);
          setNearbyMessage("Pastey cannot see nearby devices on this network.");
        }
      }
    }

    void loadNearby();
    const interval = window.setInterval(() => {
      void loadNearby();
    }, 2000);

    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, []);

  async function handleJoinRoom() {
    setBusy("join");
    setError(null);

    try {
      const room = await joinRoom(joinCode);
      setJoinCode("");
      onOpenRoom(room);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleNearbyJoin(device: NearbyDevice) {
    setBusy("nearby");
    setJoiningDeviceId(device.device_id);
    setError(null);
    setNearbyMessage(`Waiting for ${device.display_name} to approve...`);

    try {
      const room = await requestNearbyJoin(device.device_id);
      setNearbyMessage(null);
      onOpenRoom(room);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setNearbyMessage(networkHelpMessage(message));
    } finally {
      setJoiningDeviceId(null);
      setBusy(null);
    }
  }

  return (
    <div className="page-stack">
      <PageHeader title="Devices" subtitle="Nearby and trusted devices" />

      <section className="page-section">
        <SectionHeader title="Nearby Devices" icon="nearby" />
        {nearbyDevices.length === 0 ? (
          <div className="empty-card">
            <div className="device-visual laptop" aria-hidden="true" />
            <div>
              <strong>No nearby devices yet</strong>
              <p className="muted">{nearbyMessage ?? "Open Pastey on another local device."}</p>
            </div>
          </div>
        ) : (
          <div className="device-card-list">
            {nearbyDevices.map((device) => (
              <DeviceCard
                key={device.device_id}
                device={device}
                joining={joiningDeviceId === device.device_id}
                busy={busy !== null}
                onJoin={handleNearbyJoin}
              />
            ))}
          </div>
        )}
        {nearbyDevices.length > 0 && nearbyMessage ? <p className="section-hint">{nearbyMessage}</p> : null}
      </section>

      <section className="page-section">
        <SectionHeader title="Trusted Devices" icon="shield" />
        <div className="trusted-grid">
          <TrustedDeviceCard name="Trusted devices" detail="Approved devices will appear here." status="Local only" />
          {recentRoom?.peer_device_name ? (
            <TrustedDeviceCard name={recentRoom.peer_device_name} detail="Recent room partner" status="Recent" />
          ) : null}
        </div>
      </section>

      <section className="page-section">
        <div className="join-code-card">
          <div className="join-code-heading">
            <span className="row-icon qr" aria-hidden="true" />
            <div>
            <h3>Join with Code</h3>
            <p className="muted">Enter a code to connect manually.</p>
            </div>
          </div>
          <div className="join-code-controls">
            <input
              ref={joinInputRef}
              inputMode="numeric"
              aria-label="Room code"
              placeholder="4829-1736"
              value={formatCode(joinCode)}
              onChange={(event) => setJoinCode(event.target.value.replace(/[^\d]/g, "").slice(0, 8))}
              onKeyDown={(event) => {
                if (event.key === "Enter" && joinCode.length === 8 && busy === null) {
                  event.preventDefault();
                  void handleJoinRoom();
                }
              }}
            />
            <button className="secondary-button" onClick={handleJoinRoom} disabled={busy !== null || joinCode.length !== 8}>
              {busy === "join" ? "Joining..." : "Join"}
            </button>
          </div>
        </div>
      </section>

      {recentRoom ? (
        <section className="page-section">
          <button className="recent-hint-card" onClick={() => onOpenRoom(recentRoom)}>
            <span className="recent-icon clock" aria-hidden="true" />
            <div className="recent-copy">
              <span className="meta-label">Last connected</span>
              <strong>{recentRoom.peer_device_name ?? formatCode(recentRoom.room_code_display ?? recentRoom.room_code ?? "")}</strong>
              <span className="muted">{formatTimestamp(recentRoom.created_at)}</span>
            </div>
            <span className="time-badge">Recent</span>
          </button>
        </section>
      ) : null}

      {error ? <div className="error-box">{error}</div> : null}
    </div>
  );
}

function DeviceCard({
  device,
  joining,
  busy,
  onJoin
}: {
  device: NearbyDevice;
  joining: boolean;
  busy: boolean;
  onJoin: (device: NearbyDevice) => Promise<void>;
}) {
  const status = device.availability === "Available" && device.compatible ? "Available" : device.compatible ? device.availability : "Update needed";

  return (
    <article className="device-card">
      <div className={`device-visual ${deviceVisualClass(device.platform)}`} aria-hidden="true" />
      <div className="device-card-copy">
        <div>
          <h3>{device.display_name}</h3>
          <div className={`status-line ${status === "Available" ? "ready" : ""}`}>
            <span aria-hidden="true" />
            {status}
          </div>
        </div>
        <p>{deviceSummary(device)}</p>
        <span className="muted">{device.last_seen_seconds_ago <= 2 ? "Ready now" : `Seen ${device.last_seen_seconds_ago}s ago`}</span>
      </div>
      <div className="card-actions">
        <button
          className="primary-button card-action"
          onClick={() => void onJoin(device)}
          disabled={busy || device.availability !== "Available" || !device.compatible}
        >
          {joining ? "Waiting..." : "Join"}
        </button>
        <span className="chevron" aria-hidden="true">
          &gt;
        </span>
      </div>
    </article>
  );
}

function TrustedDeviceCard({ name, detail, status }: { name: string; detail: string; status: string }) {
  return (
    <article className="trusted-device-card">
      <div className="device-visual mini" aria-hidden="true" />
      <div className="trusted-copy">
        <strong>{name}</strong>
        <div className="status-line trusted">
          <span aria-hidden="true" />
          {status}
        </div>
        <p className="muted">{detail}</p>
      </div>
      <span className="chevron" aria-hidden="true">
        &gt;
      </span>
    </article>
  );
}

function PageHeader({ title, subtitle }: { title: string; subtitle: string }) {
  return (
    <header className="page-header">
      <div>
        <h1>{title}</h1>
        <p>{subtitle}</p>
      </div>
      <button className="page-menu-button" aria-label="More options">
        ...
      </button>
    </header>
  );
}

function SectionHeader({ title, icon }: { title: string; icon: "nearby" | "shield" }) {
  return (
    <div className="section-header">
      <span className={`section-icon ${icon}`} aria-hidden="true" />
      <h2>{title}</h2>
    </div>
  );
}

function deviceVisualClass(platform: string): string {
  const normalized = platform.toLowerCase();
  if (normalized.includes("ipad") || normalized.includes("tablet")) return "tablet";
  if (normalized.includes("windows") || normalized.includes("linux") || normalized.includes("desktop")) return "desktop";
  return "laptop";
}

function deviceSummary(device: NearbyDevice): string {
  const pieces = [device.platform];
  if (device.capabilities.includes("large_file")) {
    pieces.push("Large files ready");
  }
  pieces.push(`Pastey ${device.app_version}`);
  return pieces.join(" · ");
}

function networkHelpMessage(message: string): string {
  if (message.includes("rejected")) return "Join request rejected.";
  if (message.includes("timed out")) return "Join request timed out.";
  if (message.includes("No nearby")) return "No nearby devices found.";
  if (message.includes("could not connect")) return "Device found, but Pastey could not connect to it.";
  if (message.includes("block") || message.includes("Firewall")) return "This network may block local device connections.";
  return message;
}

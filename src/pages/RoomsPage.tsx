import { useState } from "react";
import { createRoom, updateConfig } from "../lib/tauri";
import { formatBytes, formatCode, formatTimestamp } from "../lib/format";
import type { AppConfig, FileTransferProgressEvent, RoomInfo } from "../lib/types";

interface RoomsPageProps {
  config: AppConfig;
  rooms: RoomInfo[];
  transfers: FileTransferProgressEvent[];
  onOpenRoom: (room: RoomInfo) => void;
  onConfigChange: (config: AppConfig) => void;
}

export function RoomsPage({ config, rooms, transfers, onOpenRoom, onConfigChange }: RoomsPageProps) {
  const [busy, setBusy] = useState<"create" | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function handleCreateRoom() {
    setBusy("create");
    setError(null);

    try {
      const room = await createRoom();
      onOpenRoom(room);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function saveConfig(next: AppConfig) {
    const saved = await updateConfig(next);
    onConfigChange(saved);
  }

  return (
    <div className="page-stack">
      <header className="page-header">
        <div>
          <h1>Rooms</h1>
          <p>Create and revisit shared spaces</p>
        </div>
        <button className="page-menu-button" aria-label="More options">
          ...
        </button>
      </header>

      <section className="page-section">
        <div className="room-setup-grid">
          <div className="create-room-card">
            <div className="workspace-visual" aria-hidden="true">
              <span />
            </div>
            <div>
              <h2>Create New Room</h2>
              <p className="muted">Start a secure local workspace.</p>
            </div>
            <button className="plus-action-button" onClick={handleCreateRoom} disabled={busy !== null} aria-label="Create New Room">
              {busy === "create" ? "..." : "+"}
            </button>
          </div>

          <div className="received-items-card">
            <div>
              <h2>Received items</h2>
              <p className="muted">Keep successful transfers in Inbox.</p>
            </div>
            <label className="mini-toggle-row">
              <span>Save files to Inbox</span>
              <input
                type="checkbox"
                checked={config.save_received_files_to_inbox}
                onChange={(event) =>
                  void saveConfig({ ...config, save_received_files_to_inbox: event.target.checked })
                }
              />
            </label>
            <label className="mini-toggle-row">
              <span>Save images to Inbox</span>
              <input
                type="checkbox"
                checked={config.save_received_images_to_inbox}
                onChange={(event) =>
                  void saveConfig({ ...config, save_received_images_to_inbox: event.target.checked })
                }
              />
            </label>
          </div>
        </div>
        {error ? <div className="error-box">{error}</div> : null}
      </section>

      <section className="page-section">
        <div className="section-header">
          <span className="section-icon recent" aria-hidden="true" />
          <h2>Recent Rooms</h2>
        </div>
        {rooms.length === 0 ? (
          <div className="empty-card">
            <div className="workspace-visual small" aria-hidden="true">
              <span />
            </div>
            <div>
              <strong>No rooms yet</strong>
              <p className="muted">Create a room when you are ready to share.</p>
            </div>
          </div>
        ) : (
          <div className="room-card-list">
            {rooms.map((room) => (
              <RoomCard key={room.id} room={room} onOpenRoom={onOpenRoom} />
            ))}
          </div>
        )}
      </section>

      <section className="page-section">
        <div className="section-header">
          <span className="section-icon activity" aria-hidden="true" />
          <h2>Recent Activity</h2>
        </div>
        {transfers.length === 0 ? (
          <div className="empty-card compact-empty-card">
            <div>
              <strong>No recent activity</strong>
              <p className="muted">Transfers will appear here while rooms are active.</p>
            </div>
          </div>
        ) : (
          <div className="activity-list">
            {transfers.slice(0, 5).map((transfer) => (
              <ActivityRow key={transfer.transfer_id} transfer={transfer} />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function RoomCard({ room, onOpenRoom }: { room: RoomInfo; onOpenRoom: (room: RoomInfo) => void }) {
  return (
    <article className="room-card">
      <div className="room-card-visual" aria-hidden="true" />
      <div className="room-card-copy">
        <h3>{roomTitle(room)}</h3>
        <p>{roomDevices(room)}</p>
        <div className="room-card-meta">
          <span className={`status-pill ${room.peer_connected ? "ready" : "muted-pill"}`}>{roomStatusLabel(room)}</span>
          <span className="muted" title={formatTimestamp(room.created_at)}>
            Manual burn
          </span>
        </div>
      </div>
      <div className="card-actions">
        <button className="secondary-button card-action" onClick={() => onOpenRoom(room)}>
          Open
        </button>
        <span className="chevron" aria-hidden="true">
          &gt;
        </span>
      </div>
    </article>
  );
}

function ActivityRow({ transfer }: { transfer: FileTransferProgressEvent }) {
  return (
    <div className="activity-row">
      <div className={`file-glyph ${transfer.direction === "incoming" ? "incoming" : "outgoing"}`} aria-hidden="true" />
      <div>
        <strong>{transfer.file_name}</strong>
        <p className="muted">
          {transfer.direction === "incoming" ? "Received" : "Shared"} · {formatBytes(transfer.transferred_bytes || transfer.file_size)}
        </p>
      </div>
      <span className={`status-pill ${transfer.status === "completed" ? "ready" : "muted-pill"}`}>
        {transfer.status}
      </span>
    </div>
  );
}

function roomTitle(room: RoomInfo): string {
  if (room.peer_device_name) return `${room.peer_device_name} Room`;
  return `Room ${formatCode(room.room_code_display ?? room.room_code ?? "").replace("-", " ")}`;
}

function roomDevices(room: RoomInfo): string {
  const local = room.local_role === "creator" ? "This device" : "Joined here";
  return room.peer_device_name ? `${local} · ${room.peer_device_name}` : `${local} · Waiting for another device`;
}

function roomStatusLabel(room: RoomInfo): string {
  if (room.peer_connected) return "Active";
  if (room.peer_burned_at) return "Peer done";
  if (room.status === "peer_left") return "Peer disconnected";
  if (room.status === "burned") return "Burned";
  return "Waiting";
}

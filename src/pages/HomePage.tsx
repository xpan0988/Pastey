import { useEffect, useRef, useState } from "react";
import { createRoom, joinRoom } from "../lib/tauri";
import { formatCode, formatRelativeExpiry, formatTimestamp } from "../lib/format";
import type { AppConfig, RoomInfo } from "../lib/types";

interface HomePageProps {
  config: AppConfig;
  rooms: RoomInfo[];
  onOpenRoom: (room: RoomInfo) => void;
  onShowSettings: () => void;
  shouldFocus: boolean;
}

export function HomePage({
  config,
  rooms,
  onOpenRoom,
  onShowSettings,
  shouldFocus
}: HomePageProps) {
  const [expiryMinutes, setExpiryMinutes] = useState(config.default_expiry_minutes);
  const [joinCode, setJoinCode] = useState("");
  const [busy, setBusy] = useState<"create" | "join" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const joinInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    setExpiryMinutes(config.default_expiry_minutes);
  }, [config.default_expiry_minutes]);

  useEffect(() => {
    if (shouldFocus) {
      joinInputRef.current?.focus();
      joinInputRef.current?.select();
    }
  }, [shouldFocus]);

  async function handleCreateRoom() {
    setBusy("create");
    setError(null);

    try {
      const room = await createRoom(expiryMinutes);
      onOpenRoom(room);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

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

  function roomPillLabel(room: RoomInfo): string {
    if (room.peer_connected) return "Connected";
    if (room.peer_burned_at) return "Peer burned";
    if (room.status === "peer_left") return "Peer left";
    return "Waiting";
  }

  return (
    <div className="stack">
      <section className="panel hero-panel">
        <div className="home-grid">
          <div className="create-room-block">
            <div className="subtle-stack compact-copy">
              <h2>Transfer room</h2>
              <p className="muted">Temporary local transfer room.</p>
            </div>

            <div className="create-controls">
              <button className="primary-button create-button" onClick={handleCreateRoom} disabled={busy !== null}>
                {busy === "create" ? "Creating..." : "Create Room"}
              </button>

              <label className="field inline-field">
                <span>Expiry</span>
                <select value={expiryMinutes} onChange={(event) => setExpiryMinutes(Number(event.target.value))}>
                  <option value={5}>5 min</option>
                  <option value={15}>15 min</option>
                  <option value={60}>1 hour</option>
                  <option value={1440}>24 hours</option>
                </select>
              </label>
            </div>

            <div className="home-status-line">Ready for local transfer</div>
          </div>

          <div className="join-card">
            <label className="field">
              <span>Join room</span>
              <input
                ref={joinInputRef}
                inputMode="numeric"
                placeholder="4829-1736"
                value={joinCode}
                onChange={(event) => setJoinCode(event.target.value.replace(/[^\d]/g, "").slice(0, 8))}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && joinCode.length === 8 && busy === null) {
                    event.preventDefault();
                    void handleJoinRoom();
                  }
                }}
              />
            </label>

            <button
              className="ghost-button"
              onClick={handleJoinRoom}
              disabled={busy !== null || joinCode.length !== 8}
            >
              {busy === "join" ? "Joining..." : "Join Room"}
            </button>

            <button className="text-button" onClick={onShowSettings}>
              Settings
            </button>
          </div>
        </div>

        {error ? <div className="error-box">{error}</div> : null}
      </section>

      <section className="panel subtle-stack">
        <div className="row spread room-list-header">
          <div>
            <h3>Recent rooms</h3>
          </div>
        </div>

        {rooms.length === 0 ? (
          <div className="empty-state">No rooms yet.</div>
        ) : (
          <div className="room-list">
            {rooms.map((room) => (
              <button key={room.id} className="room-list-item" onClick={() => onOpenRoom(room)}>
                <div className="row spread">
                  <strong>{formatCode(room.room_code_display ?? room.room_code)}</strong>
                  <span className={`pill ${room.peer_connected ? "connected" : "waiting"}`}>
                    {roomPillLabel(room)}
                  </span>
                </div>
                <div className="row spread wrap">
                  <span className="muted">{room.local_role === "creator" ? "Created here" : "Joined here"}</span>
                  <span className="muted" title={formatTimestamp(room.expires_at)}>
                    Expires in {formatRelativeExpiry(room.expires_at)}
                  </span>
                </div>
              </button>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

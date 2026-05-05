import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import { copyTextToClipboard, revealInFolder, sendFileToRoom, sendTextToRoom } from "../lib/tauri";
import { formatBytes, formatRelativeExpiry, formatTimestamp } from "../lib/format";
import type { RoomInfo, RoomItem } from "../lib/types";
import { DropZone } from "../components/DropZone";

interface RoomPageProps {
  room: RoomInfo;
  items: RoomItem[];
  onBack: () => void;
  onRefresh: () => Promise<void>;
  onBurn: (roomId: string) => Promise<void>;
  onLeave: (roomId: string) => Promise<void>;
}

export function RoomPage({
  room,
  items,
  onBack,
  onRefresh,
  onBurn,
  onLeave
}: RoomPageProps) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"text" | "file" | "burn" | "leave" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    composerRef.current?.focus();
  }, [room.id]);

  useEffect(() => {
    let cancelled = false;
    const interval = window.setInterval(() => {
      if (!cancelled) {
        void onRefresh();
      }
    }, 2000);

    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [onRefresh, room.id]);

  async function handleSendText() {
    if (!text.trim()) return;
    setBusy("text");
    setError(null);

    try {
      await sendTextToRoom(room.id, text);
      setText("");
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleSendFile(path: string) {
    setBusy("file");
    setError(null);

    try {
      await sendFileToRoom(room.id, path);
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleCopyCode() {
    if (room.room_code) {
      await copyTextToClipboard(room.room_code);
    }
  }

  async function handleBurnRoom() {
    setBusy("burn");
    try {
      await onBurn(room.id);
    } finally {
      setBusy(null);
    }
  }

  async function handleLeaveRoom() {
    setBusy("leave");
    try {
      await onLeave(room.id);
    } finally {
      setBusy(null);
    }
  }

  const headerStatus = room.peer_connected ? "Connected" : room.status === "waiting" ? "Waiting for peer" : room.status;

  return (
    <div className="stack room-shell">
      <section className="panel room-header">
        <div className="row spread wrap">
          <div className="subtle-stack">
            <button className="text-button back-button" onClick={onBack}>
              Back
            </button>
            <h2>{room.room_code_display ?? room.room_code ?? "Room"}</h2>
            <p className="muted">
              {room.local_role === "creator" ? "Share this code once, then keep chatting." : "Joined transfer room."}
            </p>
          </div>

          <div className="header-actions">
            <button className="ghost-button" onClick={handleCopyCode} disabled={!room.room_code}>
              Copy code
            </button>
            <button className="ghost-button" onClick={() => void onRefresh()}>
              Refresh
            </button>
            <button className="ghost-button" onClick={handleLeaveRoom} disabled={busy !== null}>
              {busy === "leave" ? "Leaving..." : "Leave"}
            </button>
            <button className="ghost-button danger" onClick={handleBurnRoom} disabled={busy !== null}>
              {busy === "burn" ? "Burning..." : "Burn Room"}
            </button>
          </div>
        </div>

        <div className="room-meta-grid">
          <div className="meta-card">
            <span className="meta-label">Connection</span>
            <strong>{headerStatus}</strong>
            <span className="muted">{room.peer_device_name ?? "No peer device yet"}</span>
          </div>
          <div className="meta-card">
            <span className="meta-label">Expiry</span>
            <strong title={formatTimestamp(room.expires_at)}>{formatRelativeExpiry(room.expires_at)}</strong>
            <span className="muted">Encrypted room cleanup stays local.</span>
          </div>
        </div>
      </section>

      <section className="panel chat-panel">
        <div className="message-list">
          {items.length === 0 ? <div className="empty-state">No messages yet. Send text, a file, or an image.</div> : null}
          {items.map((item) => (
            <MessageRow key={item.id} item={item} onCopyText={copyTextToClipboard} onReveal={revealInFolder} />
          ))}
        </div>
      </section>

      <section className="panel composer-panel">
        <label className="field">
          <span>Message</span>
          <textarea
            ref={composerRef}
            rows={3}
            placeholder={room.peer_connected ? "Type a message and press Enter to send." : "Waiting for the other device to join this room."}
            value={text}
            disabled={!room.peer_connected || busy !== null}
            onChange={(event) => setText(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                void handleSendText();
              }
            }}
          />
        </label>

        <div className="row gap wrap">
          <button
            className="primary-button"
            onClick={handleSendText}
            disabled={!room.peer_connected || busy !== null || !text.trim()}
          >
            {busy === "text" ? "Sending..." : "Send"}
          </button>
          <span className="muted">Press Enter to send. Use Shift + Enter for a new line.</span>
        </div>

        <DropZone onPick={handleSendFile} />

        {error ? <div className="error-box">{error}</div> : null}
      </section>
    </div>
  );
}

interface MessageRowProps {
  item: RoomItem;
  onCopyText: (text: string) => Promise<void>;
  onReveal: (path: string) => Promise<void>;
}

function MessageRow({ item, onCopyText, onReveal }: MessageRowProps) {
  const isOutgoing = item.direction === "outgoing";
  const imagePreview = useMemo(() => {
    if (!item.saved_path || !item.mime_type?.startsWith("image/")) return null;
    return convertFileSrc(item.saved_path);
  }, [item.mime_type, item.saved_path]);

  return (
    <div className={isOutgoing ? "message-row outgoing" : "message-row incoming"}>
      <div className={item.payload_type === "text" ? "message-bubble" : "file-card"}>
        <div className="message-topline">
          <span>{isOutgoing ? "You" : "Peer"}</span>
          <span>{new Date(item.created_at * 1000).toLocaleTimeString()}</span>
        </div>

        {item.payload_type === "text" ? (
          <>
            <div className="message-text">{item.text}</div>
            {item.text ? (
              <button className="text-button inline-action" onClick={() => void onCopyText(item.text ?? "")}>
                Copy
              </button>
            ) : null}
          </>
        ) : (
          <div className="subtle-stack">
            <strong>{item.display_name ?? "file"}</strong>
            <span className="muted">
              {formatBytes(item.size_bytes)}
              {item.mime_type ? ` • ${item.mime_type}` : ""}
            </span>
            {imagePreview ? <img className="image-preview chat-image" src={imagePreview} alt={item.display_name ?? "Preview"} /> : null}
            {item.saved_path ? (
              <button className="ghost-button inline-ghost" onClick={() => void onReveal(item.saved_path ?? "")}>
                Reveal in folder
              </button>
            ) : null}
          </div>
        )}
      </div>
    </div>
  );
}

import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState, type ClipboardEvent, type DragEvent, type MouseEvent } from "react";
import {
  cancelTransfer,
  copyTextToClipboard,
  deleteTempFile,
  getFileTransferMetadata,
  revealInFolder,
  sendFileToRoom,
  sendTextToRoom,
  writeTempFile
} from "../lib/tauri";
import { FILE_TOO_LARGE_MESSAGE, MAX_FILE_SIZE_BYTES } from "../lib/constants";
import { fileTypeLabel, formatBytes, formatDuration, formatRelativeExpiry, formatSpeed, formatTimestamp } from "../lib/format";
import type { FileTransferProgressEvent, RoomInfo, RoomItem } from "../lib/types";

interface RoomPageProps {
  room: RoomInfo;
  items: RoomItem[];
  transfers: FileTransferProgressEvent[];
  onBack: () => void;
  onRefresh: () => Promise<void>;
  onBurn: (roomId: string) => Promise<void>;
  onLeave: (roomId: string) => Promise<void>;
}

function fileIdentityKey(name: string, size: number, lastModified: number): string {
  return `${name}:${size}:${lastModified}`;
}

export function RoomPage({
  room,
  items,
  transfers,
  onBack,
  onRefresh,
  onBurn,
  onLeave
}: RoomPageProps) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"text" | "file" | "burn" | "leave" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cancellingTransferId, setCancellingTransferId] = useState<string | null>(null);
  const [composerDropActive, setComposerDropActive] = useState(false);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const inFlightFileKeysRef = useRef<Set<string>>(new Set());
  const roomUnavailable = room.status === "burned" || room.status === "expired" || busy === "burn" || busy === "leave";
  const canSend = room.peer_connected && busy === null && !roomUnavailable;

  useEffect(() => {
    composerRef.current?.focus();
  }, [room.id]);

  useEffect(() => {
    if (busy === "burn" || room.status === "burned" || room.status === "expired") {
      return;
    }

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
  }, [busy, onRefresh, room.id, room.status]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    void getCurrentWebview()
      .onDragDropEvent(async (event) => {
        if (event.payload.type === "over") {
          setComposerDropActive(canSend);
          return;
        }

        if (event.payload.type === "drop") {
          setComposerDropActive(false);
          if (!canSend) return;
          for (const path of event.payload.paths) {
            await handleSendFile(path);
          }
          return;
        }

        setComposerDropActive(false);
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => {
      if (unlisten) {
        unlisten();
      }
    };
  }, [canSend, room.id]);

  async function handleSendText() {
    if (!text.trim()) return;
    setBusy("text");
    setError(null);
    setCancellingTransferId(null);

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
    try {
      const metadata = await getFileTransferMetadata(path);
      if (metadata.size_bytes > MAX_FILE_SIZE_BYTES) {
        setError(FILE_TOO_LARGE_MESSAGE);
        return;
      }

      await sendFileMessage({
        path,
        displayName: metadata.display_name,
        mimeType: metadata.mime_type,
        size: metadata.size_bytes,
        fileKey: fileIdentityKey(metadata.display_name, metadata.size_bytes, metadata.modified_ms)
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function handlePickFile(event?: MouseEvent<HTMLButtonElement>) {
    event?.preventDefault();
    event?.stopPropagation();
    if (!canSend) return;
    const selected = await open({
      multiple: false,
      directory: false
    });

    if (typeof selected === "string") {
      await handleSendFile(selected);
    }
  }

  async function sendFileMessage({
    path,
    displayName,
    mimeType,
    size,
    fileKey
  }: {
    path: string;
    displayName?: string;
    mimeType?: string | null;
    size: number;
    fileKey?: string;
  }) {
    const dedupeKey = fileKey ?? fileIdentityKey(displayName ?? path, size, 0);
    const ignoredDuplicate = inFlightFileKeysRef.current.has(dedupeKey);
    if (ignoredDuplicate) {
      return;
    }
    inFlightFileKeysRef.current.add(dedupeKey);

    setBusy("file");
    setError(null);

    try {
      await sendFileToRoom(room.id, path, { displayName, mimeType });
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      inFlightFileKeysRef.current.delete(dedupeKey);
      setBusy(null);
    }
  }

  async function handleSendPastedImage(file: File, fileKey: string) {
    if (file.size > MAX_FILE_SIZE_BYTES) {
      setError(FILE_TOO_LARGE_MESSAGE);
      return;
    }

    let tempPath: string | null = null;
    try {
      const buffer = await file.arrayBuffer();
      tempPath = await writeTempFile(file.name, Array.from(new Uint8Array(buffer)));
      await sendFileMessage({
        path: tempPath,
        displayName: file.name,
        mimeType: file.type || "image/png",
        size: file.size,
        fileKey
      });
    } finally {
      if (tempPath) {
        void deleteTempFile(tempPath);
      }
    }
  }

  async function handleComposerPaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    const imageItem = Array.from(event.clipboardData.items).find((item) => item.type.startsWith("image/"));
    if (!imageItem) {
      return;
    }

    event.preventDefault();
    event.stopPropagation();

    if (!canSend) {
      return;
    }

    const clipboardFile = imageItem.getAsFile();
    if (!clipboardFile) {
      setError("Unable to read image from clipboard.");
      return;
    }

    const mimeType = imageItem.type || clipboardFile.type || "image/png";
    const dedupeKey = fileIdentityKey(clipboardFile.name || "clipboard-image", clipboardFile.size, clipboardFile.lastModified);
    const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
    const screenshotFile = new File([clipboardFile], `screenshot_${timestamp}.png`, {
      type: mimeType,
      lastModified: Date.now()
    });

    await handleSendPastedImage(screenshotFile, dedupeKey);
  }

  function handleComposerDragOver(event: DragEvent<HTMLDivElement>) {
    event.preventDefault();
    event.stopPropagation();
  }

  function handleComposerDrop(event: DragEvent<HTMLDivElement>) {
    event.preventDefault();
    event.stopPropagation();
  }

  async function handleCopyCode() {
    if (room.room_code) {
      await copyTextToClipboard(room.room_code);
    }
  }

  async function handleBurnRoom() {
    setError(null);
    setBusy("burn");
    try {
      await onBurn(room.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleLeaveRoom() {
    setError(null);
    setBusy("leave");
    try {
      await onLeave(room.id);
    } finally {
      setBusy(null);
    }
  }

  async function handleCancelTransfer(transferId: string) {
    setCancellingTransferId(transferId);
    setError(null);
    try {
      await cancelTransfer(transferId);
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCancellingTransferId(null);
    }
  }

  const peerStateMessage = room.peer_burned_at
    ? "Peer burned room. Your local items stay here until you burn this room or it expires."
    : room.status === "peer_left"
      ? "Peer left this room. Sending is disabled until a new connection exists."
      : null;

  const headerStatus = room.peer_connected
    ? "Connected"
    : room.peer_burned_at
      ? "Peer burned room"
      : room.status === "peer_left"
        ? "Peer left"
        : "Waiting for peer";

  return (
    <div className="stack room-shell">
      <section className="panel room-header">
        <div className="row spread wrap">
          <div className="subtle-stack room-title-block">
            <h2>{room.room_code_display ?? room.room_code ?? "Room"}</h2>
            <p className="muted">
              {room.local_role === "creator" ? "Share this code once, then transfer." : "Joined transfer room."}
            </p>
          </div>

          <div className="header-actions">
            <button className="ghost-button back-button" onClick={onBack}>
              Back
            </button>
            <button className="ghost-button" onClick={handleCopyCode} disabled={!room.room_code}>
              Copy code
            </button>
            <button className="ghost-button" onClick={() => void onRefresh()}>
              Refresh
            </button>
            <button className="ghost-button" onClick={handleLeaveRoom} disabled={busy !== null}>
              {busy === "leave" ? "Leaving..." : "Leave"}
            </button>
            <button className="ghost-button danger" onClick={handleBurnRoom} disabled={busy !== null || room.status === "burned"}>
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

        {peerStateMessage ? <div className="error-box">{peerStateMessage}</div> : null}
      </section>

      <section className="panel chat-panel">
        {transfers.length > 0 ? (
          <div className="transfer-list">
            {transfers.map((transfer) => (
              <TransferCard
                key={transfer.transfer_id}
                transfer={transfer}
                cancelling={cancellingTransferId === transfer.transfer_id}
                onCancel={handleCancelTransfer}
              />
            ))}
          </div>
        ) : null}

        <div className="message-list">
          {items.length === 0 ? <div className="empty-state">No messages yet. Send text, a file, or an image.</div> : null}
          {items.map((item) => (
            <MessageRow key={item.id} item={item} onCopyText={copyTextToClipboard} onReveal={revealInFolder} />
          ))}
        </div>
      </section>

      <section className="panel composer-panel">
        <div
          className={`composer-row ${composerDropActive ? "drop-active" : ""}`}
          onDragOver={handleComposerDragOver}
          onDrop={handleComposerDrop}
        >
          <button
            className="plus-button"
            onClick={(event) => void handlePickFile(event)}
            disabled={!canSend}
            title="Add file"
            aria-label="Add file"
          >
            +
          </button>
          <textarea
            ref={composerRef}
            rows={1}
            placeholder={
              room.peer_connected
                ? "Message"
                : room.status === "burned"
                  ? "Room burned"
                : room.peer_burned_at
                  ? "Peer burned this room. Burn locally when you're done."
                  : room.status === "peer_left"
                    ? "Peer left this room."
                    : "Waiting for the other device to join this room."
            }
            value={text}
            disabled={!canSend}
            onChange={(event) => setText(event.target.value)}
            onPaste={(event) => {
              void handleComposerPaste(event);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                void handleSendText();
              }
            }}
          />
          <button
            className="primary-button"
            onClick={handleSendText}
            disabled={!canSend || !text.trim()}
          >
            {busy === "text" ? "Sending..." : "Send"}
          </button>
        </div>

        {error ? <div className="error-box">{error}</div> : null}
      </section>
    </div>
  );
}

interface TransferCardProps {
  transfer: FileTransferProgressEvent;
  cancelling: boolean;
  onCancel: (transferId: string) => Promise<void>;
}

function TransferCard({ transfer, cancelling, onCancel }: TransferCardProps) {
  const percent = transfer.file_size > 0 ? Math.min(100, (transfer.transferred_bytes / transfer.file_size) * 100) : 0;
  const canCancel = transfer.status === "pending" || transfer.status === "transferring";
  const statusLabel = transferStatusLabel(transfer.status);

  return (
    <div className={`transfer-card ${transfer.status}`}>
      <div className="row spread gap">
        <div className="subtle-stack tight">
          <strong>{transfer.file_name}</strong>
          <span className="muted">
            {formatBytes(transfer.transferred_bytes)} / {formatBytes(transfer.file_size)}
            {" • "}
            {Math.round(percent)}%
          </span>
        </div>
        {canCancel ? (
          <button className="ghost-button compact-button" onClick={() => void onCancel(transfer.transfer_id)} disabled={cancelling}>
            {cancelling ? "Cancelling..." : "Cancel"}
          </button>
        ) : (
          <span className={`pill ${transfer.status}`}>{statusLabel}</span>
        )}
      </div>
      <div className="progress-track">
        <div className="progress-fill" style={{ width: `${percent}%` }} />
      </div>
      <div className="transfer-stats">
        <span>{formatSpeed(transfer.current_speed_bps)}</span>
        <span>Avg {formatSpeed(transfer.average_speed_bps)}</span>
        <span>ETA {formatDuration(transfer.eta_seconds)}</span>
      </div>
      {transfer.error_message ? <div className="transfer-error">{transfer.error_message}</div> : null}
    </div>
  );
}

function transferStatusLabel(status: FileTransferProgressEvent["status"]): string {
  switch (status) {
    case "transferring":
      return "Transferring";
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Transfer cancelled";
    case "burned":
      return "Room burned";
    case "interrupted":
      return "Transfer interrupted";
    default:
      return "Pending";
  }
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
              {" • "}
              {fileTypeLabel(item.display_name, item.mime_type)}
              {item.status === "cancelled" ? " • cancelled" : ""}
              {item.status === "interrupted" ? " • interrupted" : ""}
              {item.status === "failed" ? " • failed" : ""}
            </span>
            {item.error_message ? <span className="transfer-error">{item.error_message}</span> : null}
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

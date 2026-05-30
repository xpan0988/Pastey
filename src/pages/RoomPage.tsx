import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState, type ClipboardEvent, type DragEvent, type MouseEvent } from "react";
import {
  cancelTransfer,
  copyTextToClipboard,
  revealInFolder,
  sendTextToRoom,
  writeTempFile
} from "../lib/tauri";
import { FILE_TOO_LARGE_MESSAGE, MAX_FILE_SIZE_BYTES } from "../lib/constants";
import { fileTypeLabel, formatBytes, formatDuration, formatSpeed, formatTimestamp } from "../lib/format";
import { fileIdentityKey, type RoomTransferQueueView, type TransferQueueBatch, type TransferQueueInput, type TransferQueueItem } from "../lib/transferScheduler";
import type { FileTransferProgressEvent, RoomInfo, RoomItem } from "../lib/types";

interface RoomPageProps {
  room: RoomInfo;
  items: RoomItem[];
  transfers: FileTransferProgressEvent[];
  queue: RoomTransferQueueView;
  onBack: () => void;
  onRefresh: () => Promise<void>;
  onBurn: (roomId: string) => Promise<void>;
  onEnqueueFiles: (roomId: string, paths: string[]) => void;
  onEnqueueTransferInputs: (roomId: string, inputs: TransferQueueInput[]) => void;
  onCancelQueueItem: (itemId: string) => Promise<void>;
  onCancelQueueBatch: (batchId: string) => Promise<void>;
}

export function RoomPage({
  room,
  items,
  transfers,
  queue,
  onBack,
  onRefresh,
  onBurn,
  onEnqueueFiles,
  onEnqueueTransferInputs,
  onCancelQueueItem,
  onCancelQueueBatch
}: RoomPageProps) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"text" | "burn" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cancellingTransferId, setCancellingTransferId] = useState<string | null>(null);
  const [composerDropActive, setComposerDropActive] = useState(false);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const roomUnavailable = room.status === "burned" || busy === "burn";
  const canSend = room.peer_connected && busy === null && !roomUnavailable;

  useEffect(() => {
    composerRef.current?.focus();
  }, [room.id]);

  useEffect(() => {
    if (busy === "burn" || room.status === "burned") {
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
          if (event.payload.paths.length > 0) {
            onEnqueueFiles(room.id, event.payload.paths);
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

  async function handlePickFile(event?: MouseEvent<HTMLButtonElement>) {
    event?.preventDefault();
    event?.stopPropagation();
    if (!canSend) return;
    const selected = await open({
      multiple: true,
      directory: false
    });

    if (typeof selected === "string") {
      onEnqueueFiles(room.id, [selected]);
      return;
    }

    if (Array.isArray(selected) && selected.length > 0) {
      onEnqueueFiles(room.id, selected);
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
      onEnqueueTransferInputs(room.id, [{
        path: tempPath,
        displayName: file.name,
        mimeType: file.type || "image/png",
        sizeBytes: file.size,
        dedupeKey: fileKey,
        deleteWhenDone: true
      }]);
    } catch (err) {
      if (tempPath) {
        setError("Unable to queue image from clipboard.");
      } else {
        setError(err instanceof Error ? err.message : String(err));
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

  async function handleCancelTransfer(transferId: string) {
    setCancellingTransferId(transferId);
    setError(null);
    try {
      await cancelTransfer(transferId, {
        source: "transfer-card"
      });
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCancellingTransferId(null);
    }
  }

  async function handleCancelQueueItem(itemId: string) {
    setError(null);
    try {
      await onCancelQueueItem(itemId);
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleCancelQueueBatch(batchId: string) {
    setError(null);
    try {
      await onCancelQueueBatch(batchId);
      await onRefresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  const peerStateMessage = room.peer_burned_at
    ? "Peer burned room. Saved Inbox files stay on this device; burn locally when you're done."
    : room.status === "peer_left"
      ? "Peer disconnected. Sending is disabled until a new connection exists."
      : null;

  const headerStatus = room.peer_connected
    ? "Connected"
    : room.peer_burned_at
      ? "Peer burned room"
      : room.status === "peer_left"
        ? "Peer disconnected"
        : "Waiting for peer to join";

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
            <span className="meta-label">Lifecycle</span>
            <strong title={formatTimestamp(room.created_at)}>Manual burn</strong>
            <span className="muted">Burn clears room state, not saved Inbox files.</span>
          </div>
        </div>

        {peerStateMessage ? <div className="error-box">{peerStateMessage}</div> : null}
      </section>

      <section className="panel chat-panel">
        {queue.items.length > 0 ? (
          <QueuePanel
            queue={queue}
            onCancelItem={handleCancelQueueItem}
            onCancelBatch={handleCancelQueueBatch}
          />
        ) : null}

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
                    ? "Peer disconnected."
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

interface QueuePanelProps {
  queue: RoomTransferQueueView;
  onCancelItem: (itemId: string) => Promise<void>;
  onCancelBatch: (batchId: string) => Promise<void>;
}

function QueuePanel({ queue, onCancelItem, onCancelBatch }: QueuePanelProps) {
  const activeBatch = queue.batches.find((batch) => batch.status === "running") ?? queue.batches[queue.batches.length - 1];
  const panelItems = activeBatch
    ? activeBatch.itemIds
      .map((itemId) => queue.items.find((item) => item.id === itemId))
      .filter((item): item is TransferQueueItem => Boolean(item))
    : queue.items;
  const activeItems = panelItems.filter((item) => item.status === "preparing" || item.status === "sending");
  const activeCount = activeItems.length;
  const completedCount = panelItems.filter((item) => item.status === "completed").length;
  const failedCount = panelItems.filter((item) => item.status === "failed").length;
  const queuedCount = panelItems.filter((item) => item.status === "queued").length;
  const cancelledCount = panelItems.filter((item) => item.status === "cancelled").length;
  const visibleActiveItems = activeItems.slice(0, 4);
  const remainingActiveCount = Math.max(0, activeCount - visibleActiveItems.length);
  const visibleItems = panelItems
    .filter((item) => item.status === "queued" || item.status === "failed")
    .slice(0, 4);

  return (
    <div className="queue-panel">
      <div className="row spread gap">
        <div className="subtle-stack tight">
          <span className="meta-label">Transfer queue</span>
          <strong>{queueStatusLabel(activeBatch)}</strong>
          <span className="muted">
            {panelItems.length} files · {activeCount} active · {queuedCount} queued · {completedCount} done · {failedCount} failed · {cancelledCount} cancelled
          </span>
        </div>
        {activeBatch?.status === "running" ? (
          <button className="ghost-button compact-button" onClick={() => void onCancelBatch(activeBatch.id)}>
            Cancel batch
          </button>
        ) : null}
      </div>

      {activeCount > 0 ? (
        <div className="queue-active">
          <div className="row spread gap">
            <span className="muted">{activeCount === 1 ? "Active transfer" : `${activeCount} active transfers`}</span>
            {remainingActiveCount > 0 ? <span className="muted">+{remainingActiveCount} more</span> : null}
          </div>
          <div className="queue-items">
            {visibleActiveItems.map((item) => (
              <QueueItemRow key={item.id} item={item} onCancelItem={onCancelItem} />
            ))}
          </div>
        </div>
      ) : null}

      {visibleItems.length > 0 ? (
        <div className="queue-items">
          {visibleItems.map((item) => (
            <QueueItemRow key={item.id} item={item} onCancelItem={onCancelItem} />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function QueueItemRow({
  item,
  onCancelItem
}: {
  item: TransferQueueItem;
  onCancelItem: (itemId: string) => Promise<void>;
}) {
  const canCancel = item.status === "queued" || item.status === "preparing" || item.status === "sending";

  return (
    <div className={`queue-item ${item.status}`}>
      <div className="subtle-stack tight">
        <strong>{item.displayName ?? fileNameFromPath(item.path)}</strong>
        <span className="muted">
          {queueItemStatusLabel(item)}
          {typeof item.sizeBytes === "number" ? ` · ${formatBytes(item.sizeBytes)}` : ""}
        </span>
        {item.errorMessage ? <span className="transfer-error">{item.errorMessage}</span> : null}
      </div>
      {canCancel ? (
        <button className="ghost-button compact-button" onClick={() => void onCancelItem(item.id)}>
          Cancel
        </button>
      ) : null}
    </div>
  );
}

function queueStatusLabel(batch?: TransferQueueBatch): string {
  if (!batch) return "No active batch";
  switch (batch.status) {
    case "completed":
      return "Batch completed";
    case "completed_with_errors":
      return "Batch completed with errors";
    case "cancelled":
      return "Batch cancelled";
    default:
      return "Batch running";
  }
}

function queueItemStatusLabel(item: TransferQueueItem): string {
  switch (item.status) {
    case "preparing":
      return "Preparing";
    case "sending":
      return item.cancelRequested ? "Cancelling" : "Sending";
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Cancelled";
    default:
      return "Queued";
  }
}

function fileNameFromPath(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? "file";
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

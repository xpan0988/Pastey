import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState, type ClipboardEvent, type DragEvent, type MouseEvent } from "react";
import {
  cancelTransfer,
  copyTextToClipboard,
  markBridgePeerPairingRotationRequired,
  pairBridgePeer,
  revealInFolder,
  revokeBridgePeerPairing,
  sendTextToRoom,
  writeTempFile
} from "../lib/tauri";
import {
  deriveBridgeRoutingStateForRoom,
  enqueueTransferInputsWithBridgeRoute,
  routeStateLabel,
  sendTextToRoomWithBridgeRoute,
} from "../lib/bridgeRoutingRuntime";
import { bridgePeerSessionId, formatBridgeRouteErrorForUser, type BridgeRoute } from "../lib/bridgeRouting";
import { legacyRoomToBridgePeerCollection } from "../lib/bridgeRoomAdapter";
import { findBridgePeerBySessionId, getRouteableBridgePeers, type BridgePeerSession } from "../lib/bridgePeers";
import { FILE_TOO_LARGE_MESSAGE, MAX_FILE_SIZE_BYTES } from "../lib/constants";
import { fileTypeLabel, formatBytes, formatDuration, formatSpeed, formatTimestamp } from "../lib/format";
import { fileIdentityKey, type RoomTransferQueueView, type TransferQueueBatch, type TransferQueueInput, type TransferQueueItem } from "../lib/transferScheduler";
import type { FileTransferProgressEvent, RoomBridgePeerInfo, RoomInfo, RoomItem } from "../lib/types";
import { AiSlotPreview } from "../components/AiSlotPreview";

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
  onEnqueueCandidatePayloadHandoff: (roomId: string, input: TransferQueueInput) => boolean;
  onCancelQueueItem: (itemId: string) => Promise<void>;
  onCancelQueueBatch: (batchId: string) => Promise<void>;
  agentBridgeEnabled: boolean;
}

type BridgeTargetSelectionMode = "selected_peer" | "selected_peers" | "broadcast_bridge";

export function RoomPage({
  room,
  items,
  transfers,
  queue,
  onBack,
  onRefresh,
  onBurn,
  onEnqueueTransferInputs,
  onEnqueueCandidatePayloadHandoff,
  onCancelQueueItem,
  onCancelQueueBatch,
  agentBridgeEnabled
}: RoomPageProps) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"text" | "burn" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cancellingTransferId, setCancellingTransferId] = useState<string | null>(null);
  const [pairingPeerId, setPairingPeerId] = useState<string | null>(null);
  const [composerDropActive, setComposerDropActive] = useState(false);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const roomUnavailable = room.status === "burned" || busy === "burn";
  const bridgeRouteState = useMemo(() => deriveBridgeRoutingStateForRoom(room), [room]);
  const bridgePeerCollection = useMemo(() => {
    try {
      return legacyRoomToBridgePeerCollection(room);
    } catch {
      return null;
    }
  }, [room]);
  const routeablePeers = useMemo(
    () => bridgePeerCollection ? [...getRouteableBridgePeers(bridgePeerCollection)] : [],
    [bridgePeerCollection],
  );
  const [targetMode, setTargetMode] = useState<BridgeTargetSelectionMode>("selected_peer");
  const [selectedPeerIds, setSelectedPeerIds] = useState<string[]>([]);
  const selectedBridgeRoute = useMemo(
    () => buildSelectedBridgeRoute(bridgePeerCollection?.bridgeSessionId ?? `legacy-room:${room.id}`, routeablePeers, targetMode, selectedPeerIds),
    [bridgePeerCollection?.bridgeSessionId, room.id, routeablePeers, selectedPeerIds, targetMode],
  );
  const selectedTargetCount = selectedBridgeRoute
    ? resolvedPeersForRoute(selectedBridgeRoute, routeablePeers).length
    : 0;
  const canSend = room.peer_connected && selectedBridgeRoute !== null && busy === null && !roomUnavailable;
  const bridgeRouteError = bridgeRouteState.status === "ready_selected_peer"
    ? null
    : formatBridgeRouteErrorForUser(bridgeRouteState.errors[0] ?? routeStateLabel(bridgeRouteState));
  const currentSessionPeerCount = room.peers?.length ?? 0;
  const routeablePeerCount = routeablePeers.length;
  const unavailablePeerCount = Math.max(0, currentSessionPeerCount - routeablePeerCount);
  const roomPeerBySessionId = useMemo(() => {
    const peers = new Map<string, RoomBridgePeerInfo>();
    for (const peer of room.peers ?? []) {
      peers.set(peer.peerSessionId, peer);
    }
    return peers;
  }, [room.peers]);

  useEffect(() => {
    if (routeablePeers.length === 0) {
      setSelectedPeerIds([]);
      return;
    }
    setSelectedPeerIds((current) => {
      const routeableIds = new Set(routeablePeers.map((peer) => peer.peerSessionId));
      const next = current.filter((peerId) => routeableIds.has(bridgePeerSessionId(peerId)));
      return next.length > 0 ? next : [routeablePeers[0].peerSessionId];
    });
  }, [routeablePeers]);

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
          if (!canSend) {
            console.info("[pastey queue] event=file_drop_rejected reason=send_disabled room_id=%s status=%s peer_connected=%s busy=%s", room.id, room.status, room.peer_connected, busy ?? "none");
            if (bridgeRouteError) setError(bridgeRouteError);
            return;
          }
          if (event.payload.paths.length > 0) {
            console.info("[pastey queue] event=file_drop_received room_id=%s file_count=%d", room.id, event.payload.paths.length);
            try {
              enqueueSelectedRouteFiles(event.payload.paths);
            } catch (err) {
              setError(formatBridgeRouteErrorForUser(err));
            }
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
      if (!selectedBridgeRoute) {
        throw new Error("No routeable Bridge peer is available for this send.");
      }
      await sendTextToRoomWithBridgeRoute(room, text, sendTextToRoom, selectedBridgeRoute);
      setText("");
      await onRefresh();
    } catch (err) {
      setError(formatBridgeRouteErrorForUser(err));
    } finally {
      setBusy(null);
    }
  }

  async function handlePickFile(event?: MouseEvent<HTMLButtonElement>) {
    event?.preventDefault();
    event?.stopPropagation();
    if (!canSend) {
      console.info("[pastey queue] event=file_input_rejected reason=send_disabled room_id=%s status=%s peer_connected=%s busy=%s", room.id, room.status, room.peer_connected, busy ?? "none");
      if (bridgeRouteError) setError(bridgeRouteError);
      return;
    }
    const selected = await open({
      multiple: true,
      directory: false
    });

    if (typeof selected === "string") {
      console.info("[pastey queue] event=file_input_selected room_id=%s file_count=1", room.id);
      try {
        enqueueSelectedRouteFiles([selected]);
      } catch (err) {
        setError(formatBridgeRouteErrorForUser(err));
      }
      return;
    }

    if (Array.isArray(selected) && selected.length > 0) {
      console.info("[pastey queue] event=file_input_selected room_id=%s file_count=%d", room.id, selected.length);
      try {
        enqueueSelectedRouteFiles(selected);
      } catch (err) {
        setError(formatBridgeRouteErrorForUser(err));
      }
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
      const inputs = transferInputsForSelectedRoute([{
        path: tempPath,
        displayName: file.name,
        mimeType: file.type || "image/png",
        sizeBytes: file.size,
        dedupeKey: fileKey,
        deleteWhenDone: true
      }], "pasted_image");
      enqueueTransferInputsWithBridgeRoute(room, inputs, "pasted_image", onEnqueueTransferInputs, selectedBridgeRoute ?? undefined);
    } catch (err) {
      if (tempPath) {
        setError("Unable to queue image from clipboard.");
      } else {
        setError(formatBridgeRouteErrorForUser(err));
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
      if (bridgeRouteError) setError(bridgeRouteError);
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

  function enqueueSelectedRouteFiles(paths: string[]) {
    const inputs = transferInputsForSelectedRoute(paths.map((path) => ({ path })), "file");
    enqueueTransferInputsWithBridgeRoute(room, inputs, "file", onEnqueueTransferInputs, selectedBridgeRoute ?? undefined);
  }

  function transferInputsForSelectedRoute(
    inputs: TransferQueueInput[],
    contentKind: "file" | "image" | "pasted_image",
  ): TransferQueueInput[] {
    if (!selectedBridgeRoute) {
      throw new Error("No routeable Bridge peer is available for this send.");
    }
    const peers = resolvedPeersForRoute(selectedBridgeRoute, routeablePeers);
    if (peers.length === 0) {
      throw new Error("No routeable Bridge peer is available for this send.");
    }
    const operationId = `bridge-queue:${room.id}:${Date.now()}:${Math.random().toString(36).slice(2)}`;
    return inputs.flatMap((input) => peers.map((peer) => ({
      ...input,
      bridgeRoute: {
        bridgeSessionId: selectedBridgeRoute.bridgeSessionId,
        target: {
          kind: "selected_peer",
          peerSessionId: peer.peerSessionId,
        },
      },
      bridgeOperationId: operationId,
      bridgeTargetKind: selectedBridgeRoute.target.kind,
      bridgeContentKind: contentKind,
      targetPeerSessionId: peer.peerSessionId,
      targetPeerDisplayName: peer.displayName,
      targetCount: peers.length,
    })));
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

  async function handlePairPeer(peer: BridgePeerSession) {
    setError(null);
    setPairingPeerId(peer.peerSessionId);
    try {
      await pairBridgePeer(room.id, peer.peerSessionId, peer.displayName);
      await onRefresh();
    } catch (err) {
      setError(formatBridgeRouteErrorForUser(err));
    } finally {
      setPairingPeerId(null);
    }
  }

  async function handleRevokePeerPairing(peer: BridgePeerSession) {
    setError(null);
    setPairingPeerId(peer.peerSessionId);
    try {
      await revokeBridgePeerPairing(room.id, peer.peerSessionId);
      await onRefresh();
    } catch (err) {
      setError(formatBridgeRouteErrorForUser(err));
    } finally {
      setPairingPeerId(null);
    }
  }

  async function handleMarkPairingRotationRequired(peer: BridgePeerSession) {
    setError(null);
    setPairingPeerId(peer.peerSessionId);
    try {
      await markBridgePeerPairingRotationRequired(room.id, peer.peerSessionId);
      await onRefresh();
    } catch (err) {
      setError(formatBridgeRouteErrorForUser(err));
    } finally {
      setPairingPeerId(null);
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
            <span className="muted">{routeStateLabel(bridgeRouteState)}</span>
            <span className="muted">
              {currentSessionPeerCount > 0
                ? `${routeablePeerCount}/${currentSessionPeerCount} current-session peers routeable`
                : "No current-session peers yet"}
            </span>
          </div>
          <div className="meta-card">
            <span className="meta-label">Lifecycle</span>
            <strong title={formatTimestamp(room.created_at)}>Manual burn</strong>
            <span className="muted">Burn clears room state, not saved Inbox files.</span>
          </div>
        </div>

        {peerStateMessage ? <div className="error-box">{peerStateMessage}</div> : null}
      </section>

      {agentBridgeEnabled ? (
        <details className="advanced-diagnostics-shell" data-testid="room-advanced-diagnostics">
          <summary>Advanced diagnostics</summary>
          <AiSlotPreview
            key={`${room.id}:${room.peer_connected}:${room.peer_device_name ?? "none"}`}
            room={room}
            onEnqueueCandidatePayloadHandoff={(input) => onEnqueueCandidatePayloadHandoff(room.id, input)}
          />
        </details>
      ) : null}

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
        <div className="subtle-stack tight">
          <div className="row spread gap">
            <span className="meta-label">Bridge target</span>
            <span className="muted">
              {routeablePeerCount} routeable · {selectedTargetCount} selected
            </span>
          </div>
          <div className="row gap wrap">
            <button
              className={targetMode === "selected_peer" ? "primary-button compact-button" : "ghost-button compact-button"}
              onClick={() => setTargetMode("selected_peer")}
              disabled={routeablePeerCount === 0}
            >
              Peer
            </button>
            <button
              className={targetMode === "selected_peers" ? "primary-button compact-button" : "ghost-button compact-button"}
              onClick={() => setTargetMode("selected_peers")}
              disabled={routeablePeerCount < 2}
            >
              Peers
            </button>
            <button
              className={targetMode === "broadcast_bridge" ? "primary-button compact-button" : "ghost-button compact-button"}
              onClick={() => setTargetMode("broadcast_bridge")}
              disabled={routeablePeerCount === 0}
            >
              Broadcast
            </button>
          </div>
          {targetMode === "broadcast_bridge" ? (
            <span className="muted">Broadcast will send to {routeablePeerCount} current-session routeable peers.</span>
          ) : (
            <div className="row gap wrap">
              {routeablePeers.map((peer) => {
                const checked = selectedPeerIds.includes(peer.peerSessionId);
                const roomPeer = roomPeerBySessionId.get(peer.peerSessionId);
                const isPairingBusy = pairingPeerId === peer.peerSessionId;
                return (
                  <div key={peer.peerSessionId} className="row gap wrap">
                    <label className="muted">
                      <input
                        type={targetMode === "selected_peer" ? "radio" : "checkbox"}
                        checked={checked}
                        onChange={() => {
                          if (targetMode === "selected_peer") {
                            setSelectedPeerIds([peer.peerSessionId]);
                          } else {
                            setSelectedPeerIds((current) => checked
                              ? current.filter((peerId) => peerId !== peer.peerSessionId)
                              : [...current, peer.peerSessionId]);
                          }
                        }}
                      />
                      {" "}
                      {peer.displayName}
                    </label>
                    <span className="muted">{pairedPeerStatusLabel(roomPeer)}</span>
                    {roomPeer?.durableIdentityId ? (
                      <>
                        <button
                          type="button"
                          className="ghost-button compact-button"
                          onClick={() => void handleMarkPairingRotationRequired(peer)}
                          disabled={isPairingBusy || roomPeer.pairingRotationState === "rotation_required"}
                        >
                          Rotation required
                        </button>
                        <button
                          type="button"
                          className="ghost-button compact-button"
                          onClick={() => void handleRevokePeerPairing(peer)}
                          disabled={isPairingBusy}
                        >
                          Revoke pairing
                        </button>
                      </>
                    ) : (
                      <button
                        type="button"
                        className="ghost-button compact-button"
                        onClick={() => void handlePairPeer(peer)}
                        disabled={isPairingBusy}
                      >
                        Pair
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
          {unavailablePeerCount > 0 ? (
            <span className="muted">{unavailablePeerCount} current-session peer{unavailablePeerCount === 1 ? "" : "s"} unavailable or route-expired.</span>
          ) : null}
        </div>
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

function buildSelectedBridgeRoute(
  bridgeSessionId: string,
  routeablePeers: readonly BridgePeerSession[],
  targetMode: BridgeTargetSelectionMode,
  selectedPeerIds: readonly string[],
): BridgeRoute | null {
  if (targetMode === "broadcast_bridge") {
    return routeablePeers.length > 0
      ? { bridgeSessionId, target: { kind: "broadcast_bridge", explicit: true } }
      : null;
  }
  const routeableIds = new Set(routeablePeers.map((peer) => peer.peerSessionId));
  const selectedIds = selectedPeerIds
    .map((peerId) => bridgePeerSessionId(peerId))
    .filter((peerId) => routeableIds.has(peerId));
  if (targetMode === "selected_peer") {
    const peerSessionId = selectedIds[0] ?? routeablePeers[0]?.peerSessionId;
    return peerSessionId
      ? { bridgeSessionId, target: { kind: "selected_peer", peerSessionId } }
      : null;
  }
  return selectedIds.length >= 2
    ? { bridgeSessionId, target: { kind: "selected_peers", peerSessionIds: selectedIds } }
    : null;
}

function resolvedPeersForRoute(route: BridgeRoute, routeablePeers: readonly BridgePeerSession[]): BridgePeerSession[] {
  if (route.target.kind === "broadcast_bridge") {
    return [...routeablePeers];
  }
  if (route.target.kind === "selected_peer") {
    const peer = findBridgePeerBySessionId({
      bridgeSessionId: route.bridgeSessionId,
      peers: routeablePeers,
    }, route.target.peerSessionId);
    return peer ? [peer] : [];
  }
  return route.target.peerSessionIds
    .map((peerSessionId) => findBridgePeerBySessionId({
      bridgeSessionId: route.bridgeSessionId,
      peers: routeablePeers,
    }, peerSessionId))
    .filter((peer): peer is BridgePeerSession => Boolean(peer));
}

function pairedPeerStatusLabel(peer?: RoomBridgePeerInfo): string {
  if (!peer?.durableIdentityId) {
    return "known device not paired";
  }
  const parts = ["paired"];
  if (peer.pairingRotationState === "rotation_required") {
    parts.push("rotation required");
  } else if (peer.pairingRotationState === "rotation_deferred") {
    parts.push("rotation deferred");
  } else if (peer.pairingRotationState === "rotation_unsupported") {
    parts.push("rotation unsupported");
  }
  const fingerprint = peer.pairingPublicKeyFingerprint?.slice(0, 18);
  if (fingerprint) {
    parts.push(`fingerprint ${fingerprint}`);
  }
  return parts.join(" · ");
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
  const bridgeOperations = bridgeQueueOperationSummaries(panelItems);
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
          {bridgeOperations.map((operation) => (
            <span key={operation.operationId} className="muted">
              {operation.label}: {operation.completed}/{operation.total} delivered
              {operation.failed > 0 ? ` · ${operation.failed} failed` : ""}
              {operation.cancelled > 0 ? ` · ${operation.cancelled} cancelled` : ""}
            </span>
          ))}
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
          {item.targetPeerDisplayName ? ` · to ${item.targetPeerDisplayName}` : ""}
          {item.targetCount && item.targetCount > 1 ? ` · ${item.targetCount} targets` : ""}
          {typeof item.sizeBytes === "number" ? ` · ${formatBytes(item.sizeBytes)}` : ""}
        </span>
        {item.agentBridgeMetadata?.origin === "agent_bridge_candidate_payload" ? (
          <span className="muted">Queued from approved candidate payload request.</span>
        ) : null}
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

function bridgeQueueOperationSummaries(items: readonly TransferQueueItem[]): Array<{
  operationId: string;
  label: string;
  total: number;
  completed: number;
  failed: number;
  cancelled: number;
}> {
  const groups = new Map<string, TransferQueueItem[]>();
  for (const item of items) {
    if (!item.bridgeOperationId) continue;
    groups.set(item.bridgeOperationId, [...(groups.get(item.bridgeOperationId) ?? []), item]);
  }
  return [...groups.entries()].map(([operationId, operationItems]) => ({
    operationId,
    label: operationItems[0]?.bridgeTargetKind === "broadcast_bridge"
      ? "Broadcast"
      : operationItems[0]?.bridgeTargetKind === "selected_peers"
        ? "Selected peers"
        : "Selected peer",
    total: operationItems.length,
    completed: operationItems.filter((item) => item.status === "completed").length,
    failed: operationItems.filter((item) => item.status === "failed").length,
    cancelled: operationItems.filter((item) => item.status === "cancelled").length,
  }));
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

function bridgeSendOperationLabel(status: string, targetCount: number): string {
  switch (status) {
    case "completed":
      return targetCount === 1 ? "Delivered to selected peer" : `Delivered to ${targetCount} peers`;
    case "partial":
      return `Partially delivered to ${targetCount} peers`;
    case "failed":
      return `Delivery failed for ${targetCount} peer${targetCount === 1 ? "" : "s"}`;
    case "cancelled":
      return "Delivery cancelled";
    case "unsupported":
      return "Delivery target unsupported";
    default:
      return targetCount === 1 ? "Delivery pending" : `Delivery pending for ${targetCount} peers`;
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
            {item.bridge_send_operation ? (
              <span className="muted">{bridgeSendOperationLabel(item.bridge_send_operation.aggregateStatus, item.bridge_send_operation.outcomes.length)}</span>
            ) : null}
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
            {item.bridge_send_operation ? (
              <span className="muted">{bridgeSendOperationLabel(item.bridge_send_operation.aggregateStatus, item.bridge_send_operation.outcomes.length)}</span>
            ) : null}
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

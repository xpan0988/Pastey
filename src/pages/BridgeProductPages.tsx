import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState, type ClipboardEvent, type ReactNode } from "react";
import {
  copyTextToClipboard,
  approveBridgePlan,
  createDirectFileTransferBridgePlan,
  createFileSearchBridgePlan,
  createFileTransformBridgePlan,
  proposeBridgePlanTransformFallback,
  getDeviceProfile,
  getRoomControlSessionContext,
  joinRoom,
  listBridgePlanWorkspace,
  listReceivedRoomControlEvents,
  listNearbyDevices,
  localTransformAvailability,
  requestNearbyJoin,
  revealInFolder,
  refreshSelectedPeerCapabilities,
  bindBridgePlanToSession,
  startBridgePlanAttempt,
  selectBridgePlanSearchCandidate,
  selectedPeerTransformAvailability,
  sendTextToRoom,
  type SelectedPeerTransformAvailability,
  withdrawBridgePlanRevision,
  writeTempFile,
} from "../lib/tauri";
import {
  bridgeRoutePayload,
  enqueueTransferInputsWithBridgeRoute,
  sendTextToRoomWithBridgeRoute,
} from "../lib/bridgeRoutingRuntime";
import {
  bridgePeerSessionId,
  bridgeRouteErrorCodeFromMessage,
  formatBridgeRouteErrorForUser,
  type BridgeRoute,
} from "../lib/bridgeRouting";
import { legacyRoomToBridgePeerCollection } from "../lib/bridgeRoomAdapter";
import {
  OperationTimeline,
  type OperationTimelineRow,
  type OperationTimelineStatus,
  type OperationTimelineStep,
} from "../components/OperationTimeline";
import {
  findBridgePeerBySessionId,
  getRouteableBridgePeers,
  type BridgePeerSession,
} from "../lib/bridgePeers";
import {
  bridgePollingIntervalMs,
  reconcileSelectedPeerIds,
} from "../lib/agentBridge/bridgeDetailPolling";
import {
  SAFE_SEARCH_SCOPES,
  addPrimitive,
  canAddPrimitive,
  initialTransformExecutionDevice,
  manualBridgePlanInput,
  moveBlock,
  newSearchBlock,
  objectFlow,
  removeBlock,
  updateSearchBlock,
  type ComposerBlock,
  type ComposerDevice,
  type DerivedPipelineTransferBlock,
  type SafeSearchScope,
  type TransformAvailability,
  type TransformExecutorCapabilities,
} from "../lib/bridgePlanComposer";
import { FILE_TOO_LARGE_MESSAGE, MAX_FILE_SIZE_BYTES } from "../lib/constants";
import { formatCode, formatTimestamp } from "../lib/format";
import {
  bridgePlanSearchCandidateMode,
  candidateMetadata,
  parseBridgePlanSearchCandidates,
  parseBridgePlanSearchTerminalResult,
  selectedBridgePlanCandidateId,
  terminalSearchPresentation,
  type BridgePlanSearchCandidate,
} from "../lib/bridgePlanSearchResults";
import type { TransferQueueInput, TransferQueueItem } from "../lib/transferScheduler";
import type {
  FileTransferProgressEvent,
  DeviceProfile,
  JoinRequestPrompt,
  NearbyDevice,
  RoomControlSessionContext,
  ReceivedRoomControlEvent,
  RoomInfo,
  RoomItem,
} from "../lib/types";

type PrimaryRoute = "bridge" | "activity" | "devices" | "settings";
type BridgeTargetSelectionMode = "selected_peer" | "selected_peers" | "broadcast_bridge";
const BRIDGE_PLAN_REQUIRES_ONE_SELECTED_DEVICE = "Ask Bridge requires one selected device.";

function bridgePlanControlErrorMessage(error: unknown, action: "review" | "decision"): string {
  if (bridgeRouteErrorCodeFromMessage(error)) {
    return formatBridgeRouteErrorForUser(error);
  }
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  if (message.includes("selected device session changed")) {
    return "The selected device reconnected. Refresh the Bridge, select its current session, and send a new review request.";
  }
  if (message.includes("Room control session") || message.includes("Room session is unavailable")) {
    return "The Bridge session is no longer active on one device. Reopen the active Bridge and try again.";
  }
  if (message.includes("Peer is unavailable") || message.includes("delivery failed") || message.includes("delivery timed out")) {
    return "The selected device is not reachable for Bridge review. Confirm that both devices are connected to the same active Bridge, then try again.";
  }
  if (message.includes("review_expired")) {
    return action === "review"
      ? "This review request expired before the selected device could accept it. Create a new plan and send it again."
      : "This review decision arrived after the request expired. Create a new plan and send a new review request.";
  }
  if (message.includes("review_session_mismatch")) {
    return "The selected device reconnected or the Bridge session changed. Refresh the Bridge, select its current session, and create a new plan.";
  }
  if (message.includes("review_revision_hash_mismatch") || message.includes("review_step_digest_mismatch")) {
    return action === "review"
      ? "The selected device rejected a mismatched immutable plan. Refresh the Bridge and create a new plan."
      : "The requester rejected a mismatched immutable plan decision. Refresh the Bridge and send a new review request.";
  }
  if (message.includes("review_unknown_approval")) {
    return action === "review"
      ? "The selected device no longer has this approved review request. Create a new plan and send it again."
      : "The requester no longer has this approved review request. Refresh the Bridge and send a new review request.";
  }
  if (message.includes("review_payload_invalid")) {
    return action === "review"
      ? "The selected device rejected an invalid review request. Refresh the Bridge and create a new plan."
      : "The requester rejected an invalid review decision. Refresh the Bridge and send a new review request.";
  }
  if (message.includes("event validation failed") || message.includes("Bridge Plan review not found") || message.includes("receiver review")) {
    return action === "review"
      ? "The selected device could not validate this review request. Refresh the Bridge and create a new plan."
      : "The requester could not validate this decision. Refresh the Bridge and send a new review request.";
  }
  return action === "review"
    ? "Pastey could not send the plan for review. Refresh the Bridge and try again."
    : "The plan decision could not be sent. Refresh the Bridge and try again.";
}

function bridgePlanSearchErrorMessage(error: unknown): string {
  if (bridgeRouteErrorCodeFromMessage(error)) return formatBridgeRouteErrorForUser(error);
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  if (message.includes("no_searchable_scopes")) {
    return "No approved folder is available to search on this device. Check the reviewed Downloads, Desktop, Documents, or Pastey Shared folder, then start a new approved attempt.";
  }
  if (message.includes("Invalid file candidate filename")) {
    return "This approved Search has an invalid filename request. Create a new plan using a filename only.";
  }
  if (message.includes("search_timeout")) {
    return "The approved Search timed out. Narrow the filename or reviewed locations, then start a new approved attempt.";
  }
  if (message.includes("delivery failed") || message.includes("delivery timed out")) {
    return "Search finished locally but the result could not be delivered. Confirm the requester is still connected, then start a new approved attempt.";
  }
  if (message.includes("Search execution grant") || message.includes("Bridge Plan attempt missing")) {
    return "This approved Search is no longer available on this device. Refresh the Bridge and start a new approved attempt.";
  }
  return "The approved Search could not be completed. Check the reviewed folders and Bridge connection, then start a new approved attempt.";
}

function bridgePlanTransferErrorMessage(error: unknown): string {
  if (bridgeRouteErrorCodeFromMessage(error)) return formatBridgeRouteErrorForUser(error);
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  if (message.includes("attempt_missing") || message.includes("attempt_correlation_mismatch")) {
    return "The requester could not confirm this approved Transfer attempt. Create a new approved plan and try again.";
  }
  if (message.includes("candidate_changed")) {
    return "The selected file changed after Search completed. Start a new approved plan to search it again.";
  }
  if (message.includes("candidate_unavailable") || message.includes("candidate is unavailable")) {
    return "The selected Search result is no longer available on this device. Start a new approved plan.";
  }
  if (message.includes("route_unavailable") || message.includes("route target") || message.includes("routeable")) {
    return "The requesting device is no longer reachable for Transfer. Reconnect it to this Bridge and start a new approved plan.";
  }
  if (message.includes("outgoing_item_failed")) {
    return "Pastey could not prepare the approved file for Transfer. Start a new approved plan.";
  }
  if (message.includes("file_send_failed")) {
    return "The approved file could not be sent to the requesting device. Confirm the Bridge connection and start a new approved plan.";
  }
  if (message.includes("result_delivery_failed")) {
    return "Transfer finished locally but its result could not be delivered. Confirm the Bridge connection before starting a new approved plan.";
  }
  return "The approved Transfer could not be completed. Check the Bridge connection and start a new approved plan.";
}

interface BridgePageProps {
  rooms: RoomInfo[];
  roomItems: RoomItem[];
  queueItems: TransferQueueItem[];
  onCreateBridge: () => Promise<void>;
  onOpenBridge: (room: RoomInfo) => void;
  onJoinBridge: (code: string) => Promise<void>;
  onSelectView: (view: PrimaryRoute) => void;
}

export function BridgePage({
  rooms,
  roomItems,
  queueItems,
  onCreateBridge,
  onOpenBridge,
  onJoinBridge,
  onSelectView,
}: BridgePageProps) {
  const [joinOpen, setJoinOpen] = useState(false);
  const [joinCode, setJoinCode] = useState("");
  const [busy, setBusy] = useState<"create" | "join" | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const activeRooms = rooms.filter((room) => room.status !== "burned");

  async function handleCreateBridge() {
    setBusy("create");
    setMessage(null);
    try {
      await onCreateBridge();
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleJoinBridge() {
    if (joinCode.length !== 8) return;
    setBusy("join");
    setMessage(null);
    try {
      await onJoinBridge(joinCode);
      setJoinCode("");
      setJoinOpen(false);
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="product-page bridge-page" aria-label="Bridge">
      <ProductHeader
        title="Bridge"
        subtitle="Send anything between your devices."
      />

      <div className="primary-action-row">
        <button type="button" className="primary-button large-action" disabled={busy !== null} onClick={() => void handleCreateBridge()}>
          + Create Bridge
        </button>
        <button type="button" className="secondary-button large-action" onClick={() => setJoinOpen((open) => !open)}>
          Join with code
        </button>
        <button type="button" className="secondary-button large-action" onClick={() => onSelectView("devices")}>
          Find nearby devices
        </button>
      </div>

      {joinOpen ? (
        <Card className="join-inline-card">
          <div>
            <strong>Enter an 8-digit code</strong>
            <p className="muted">Ask the other device for its code.</p>
            <p className="muted">Joining allows the other current Bridge device to run reviewed, bounded Pastey tasks for this session only. It does not grant file-system, shell, or durable device control.</p>
          </div>
          <div className="join-code-controls compact">
            <input
              inputMode="numeric"
              aria-label="Bridge code"
              placeholder="4829 1736"
              value={formatCode(joinCode)}
              onChange={(event) => setJoinCode(event.target.value.replace(/[^\d]/g, "").slice(0, 8))}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void handleJoinBridge();
                }
              }}
            />
            <button type="button" className="primary-button" disabled={busy !== null || joinCode.length !== 8} onClick={() => void handleJoinBridge()}>
              {busy === "join" ? "Joining..." : "Join"}
            </button>
          </div>
        </Card>
      ) : null}

      {message ? <div className="error-box">{message}</div> : null}

      <section className="page-section">
        <div className="section-row">
          <h2>Your Bridges</h2>
        </div>
        {activeRooms.length === 0 ? (
          <Card className="bridge-start-card">
            <span className="bridge-status-icon waiting" aria-hidden="true" />
            <div>
              <h2>Start a Bridge</h2>
              <p className="muted">Send text, links, images, and files directly between your devices.</p>
            </div>
            <div className="button-row">
              <button type="button" className="primary-button" disabled={busy !== null} onClick={() => void handleCreateBridge()}>
                Create Bridge
              </button>
              <button type="button" className="secondary-button" onClick={() => setJoinOpen(true)}>
                Join with code
              </button>
              <button type="button" className="secondary-button" onClick={() => onSelectView("devices")}>
                Find nearby devices
              </button>
            </div>
          </Card>
        ) : (
          <div className="bridge-card-list">
            {activeRooms.map((room) => (
              <BridgeListCard
                key={room.id}
                room={room}
                lastActivity={lastActivityForBridge(room, roomItems, queueItems)}
                onOpen={() => onOpenBridge(room)}
              />
            ))}
          </div>
        )}
      </section>
    </section>
  );
}

function BridgeListCard({ room, lastActivity, onOpen }: { room: RoomInfo; lastActivity: string; onOpen: () => void }) {
  const code = bridgeCode(room);
  const members = bridgeMemberSummary(room);
  const status = bridgeStatus(room);

  async function copyCode() {
    await copyTextToClipboard(code);
  }

  return (
    <article className="bridge-list-card">
      <span className={`bridge-status-icon ${status.tone}`} aria-hidden="true" />
      <div className="bridge-card-code">
        <strong>{code}</strong>
        <StatusChip tone={status.tone}>{status.label}</StatusChip>
      </div>
      <div className="bridge-card-members">
        <strong>{members.title}</strong>
        <span>{members.detail}</span>
      </div>
      <div className="bridge-card-activity">
        <span>Last activity</span>
        <strong>{lastActivity}</strong>
      </div>
      <div className="bridge-card-actions">
        <button type="button" className="primary-button" onClick={onOpen}>
          Open
        </button>
        <button type="button" className="secondary-button" onClick={() => void copyCode()}>
          Copy code
        </button>
      </div>
    </article>
  );
}

interface BridgeDetailPageProps {
  room: RoomInfo;
  items: RoomItem[];
  transfers: FileTransferProgressEvent[];
  queueItems: TransferQueueItem[];
  askBridgeBetaEnabled: boolean;
  onBack: () => void;
  onRefresh: () => Promise<void>;
  onLeaveOrBurn: (room: RoomInfo, action: "leave" | "burn") => Promise<void>;
  onEnqueueTransferInputs: (roomId: string, inputs: TransferQueueInput[]) => void;
  onOpenActivity: () => void;
}

export function BridgeDetailPage({
  room,
  items,
  transfers,
  queueItems,
  askBridgeBetaEnabled,
  onBack,
  onRefresh,
  onLeaveOrBurn,
  onEnqueueTransferInputs,
  onOpenActivity,
}: BridgeDetailPageProps) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"send" | "files" | "close" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dropActive, setDropActive] = useState(false);
  const [targetMode, setTargetMode] = useState<BridgeTargetSelectionMode>("selected_peer");
  const [selectedPeerIds, setSelectedPeerIds] = useState<string[]>([]);
  const [controlSession, setControlSession] = useState<RoomControlSessionContext | null>(null);
  const controlSessionRef = useRef<RoomControlSessionContext | null>(null);
  const refreshInFlightRef = useRef(false);
  const refreshBridgeControlInboxRef = useRef<() => Promise<void>>(async () => {});
  const [bridgePlanInboxBatch, setBridgePlanInboxBatch] = useState<ReceivedRoomControlEvent[]>([]);
  const [localDeviceProfile, setLocalDeviceProfile] = useState<DeviceProfile | null>(null);
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const enqueueDroppedFilesRef = useRef<(paths: string[]) => void>(() => {});
  const routeablePeers = useRouteablePeers(room);
  const remotePeers = useMemo(
    () => routeablePeers.filter((peer) => peer.isLocalSelf !== true),
    [routeablePeers],
  );
  const selectedRoute = useMemo(
    () => buildSelectedBridgeRoute(bridgeSessionId(room), remotePeers, targetMode, selectedPeerIds),
    [room.id, remotePeers, selectedPeerIds, targetMode],
  );
  const selectedPeers = useMemo(
    () => selectedRoute ? resolvedPeersForRoute(selectedRoute, remotePeers) : [],
    [selectedRoute, remotePeers],
  );
  const selectedSinglePeer = selectedRoute?.target.kind === "selected_peer" ? selectedPeers[0] ?? null : null;
  const canSend = room.status === "active" && room.peer_connected && selectedRoute !== null && selectedPeers.length > 0 && busy === null;

  useEffect(() => {
    let cancelled = false;
    void getDeviceProfile({ forceRefresh: false })
      .then((profile) => {
        if (!cancelled) setLocalDeviceProfile(profile);
      })
      .catch(() => {
        if (!cancelled) setLocalDeviceProfile(null);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    setSelectedPeerIds((current) => {
      const next = reconcileSelectedPeerIds(
        current,
        remotePeers.map((peer) => peer.peerSessionId),
      );
      return next === current ? current : [...next];
    });
  }, [remotePeers]);

  useEffect(() => {
    composerRef.current?.focus();
  }, [room.id]);

  useEffect(() => {
    let cancelled = false;
    if (!room.peer_connected || room.status !== "active") {
      applyControlSession(null);
      return;
    }
    void getRoomControlSessionContext(room.id)
      .then((session) => {
        if (!cancelled) applyControlSession(session);
      })
      .catch((err) => {
        if (!cancelled) {
          applyControlSession(null);
          setError(err instanceof Error ? err.message : String(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [room.id, room.peer_connected, room.status]);

  const roomControlPollingActive = true;
  refreshBridgeControlInboxRef.current = refreshBridgeControlInbox;

  useEffect(() => {
    if (!controlSession) return;
    let cancelled = false;
    const refresh = () => {
      if (!cancelled) void refreshBridgeControlInboxRef.current();
    };
    refresh();
    const intervalMs = bridgePollingIntervalMs(roomControlPollingActive);
    const interval = intervalMs === null ? null : window.setInterval(refresh, intervalMs);
    window.addEventListener("focus", refresh);
    return () => {
      cancelled = true;
      if (interval !== null) window.clearInterval(interval);
      window.removeEventListener("focus", refresh);
    };
  }, [controlSession, roomControlPollingActive]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void getCurrentWebview().onDragDropEvent((event) => {
      if (cancelled) return;
      if (event.payload.type === "over") {
        setDropActive(canSend);
        return;
      }
      if (event.payload.type === "drop") {
        setDropActive(false);
        if (event.payload.paths.length > 0) {
          enqueueDroppedFilesRef.current(event.payload.paths);
        }
        return;
      }
      setDropActive(false);
    }).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [room.id]);

  async function handleSendText() {
    const trimmed = text.trim();
    if (!trimmed) return;
    setBusy("send");
    setError(null);
    try {
      if (!selectedRoute) throw new Error("Select a connected device before sending.");
      await sendTextToRoomWithBridgeRoute(room, trimmed, sendTextToRoom, selectedRoute);
      setText("");
      await onRefresh();
    } catch (err) {
      setError(formatBridgeRouteErrorForUser(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleChooseFiles() {
    setBusy("files");
    setError(null);
    try {
      const selected = await open({ multiple: true, directory: false });
      const paths = typeof selected === "string" ? [selected] : Array.isArray(selected) ? selected : [];
      if (paths.length > 0) enqueueSelectedRouteFiles(paths, "file");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function handlePaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    const imageItem = Array.from(event.clipboardData.items).find((item) => item.type.startsWith("image/"));
    const file = imageItem?.getAsFile();
    if (!file) return;
    event.preventDefault();
    setError(null);
    try {
      if (file.size > MAX_FILE_SIZE_BYTES) {
        throw new Error(FILE_TOO_LARGE_MESSAGE);
      }
      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      const extension = file.type.includes("png") ? "png" : file.type.includes("jpeg") ? "jpg" : "img";
      const displayName = file.name?.trim() || `pasted-image-${Date.now()}.${extension}`;
      const path = await writeTempFile(displayName, bytes);
      enqueueSelectedRouteFiles([path], "pasted_image", [{
        path,
        displayName,
        mimeType: file.type || "image/png",
        sizeBytes: file.size,
        modifiedMs: file.lastModified || Date.now(),
        deleteWhenDone: true,
      }]);
    } catch (err) {
      setError(formatBridgeRouteErrorForUser(err));
    }
  }

  function enqueueSelectedRouteFiles(
    paths: string[],
    contentKind: "file" | "image" | "pasted_image",
    preparedInputs?: TransferQueueInput[],
  ) {
    if (!selectedRoute || selectedPeers.length === 0 || !canSend) {
      setError("Select a connected device before sending.");
      return;
    }
    const inputs = transferInputsForSelectedRoute(
      preparedInputs ?? paths.map((path) => ({ path })),
      selectedRoute,
      selectedPeers,
      room.id,
      contentKind,
    );
    enqueueTransferInputsWithBridgeRoute(room, inputs, contentKind, onEnqueueTransferInputs, selectedRoute);
  }

  enqueueDroppedFilesRef.current = (paths) => enqueueSelectedRouteFiles(paths, "file");

  async function copyCode() {
    await copyTextToClipboard(bridgeCode(room));
  }

  async function handleLeaveOrBurn() {
    const action = connectedRemoteMembers(room).length > 0 ? "leave" : "burn";
    const code = bridgeCode(room);
    const confirmed = window.confirm(action === "leave"
      ? `Leave this Bridge?\n\nOther devices are still connected. This device will leave Bridge ${code}. Received files on this device will stay.`
      : `Burn this Bridge?\n\nThis is the last device in Bridge ${code}. Pastey will delete local Bridge state from this device. Received files will stay in your receiving folder.`);
    if (!confirmed) return;
    setBusy("close");
    setError(null);
    try {
      await onLeaveOrBurn(room, action);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  function applyControlSession(nextSession: RoomControlSessionContext | null) {
    const previous = controlSessionRef.current;
    if (previous?.localSessionRef !== nextSession?.localSessionRef || previous?.peerSessionRef !== nextSession?.peerSessionRef) {
      setBridgePlanInboxBatch([]);
    }
    controlSessionRef.current = nextSession;
    setControlSession(nextSession);
  }

  async function refreshBridgeControlInbox() {
    const currentSession = controlSessionRef.current;
    if (!currentSession) {
      setError("Ask Bridge requires an active selected-peer Bridge session.");
      return;
    }
    if (refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    try {
      const events = await listReceivedRoomControlEvents(currentSession.roomId);
      const bridgePlanEvents = events.filter((event) => event.kind.startsWith("bridge_plan."));
      if (bridgePlanEvents.length > 0) {
        setBridgePlanInboxBatch((current) => {
          const byId = new Map(current.map((event) => [event.eventId, event]));
          bridgePlanEvents.forEach((event) => byId.set(event.eventId, event));
          return [...byId.values()].slice(-64);
        });
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      refreshInFlightRef.current = false;
    }
  }

  const status = bridgeStatus(room);
  const recent = recentActivityRows(room, items, transfers, queueItems).slice(0, 3);

  return (
    <section className="product-page bridge-detail-page" aria-label="Bridge detail">
      <div className="detail-back-row">
        <button type="button" className="text-button" onClick={onBack}>
          &larr; Bridges
        </button>
      </div>
      <header className="bridge-detail-header">
        <div>
          <h1>Bridge {bridgeCode(room)}</h1>
          <div className="detail-title-row">
            <StatusChip tone={status.tone}>{status.label}</StatusChip>
            <span className="muted">{bridgeSubtitle(room)}</span>
          </div>
        </div>
        <div className="button-row">
          <button type="button" className="secondary-button" onClick={() => void copyCode()}>
            Copy code
          </button>
          <button type="button" className="danger-button" disabled={busy === "close"} onClick={() => void handleLeaveOrBurn()}>
            Burn Bridge
          </button>
        </div>
      </header>

      <section className="members-strip" aria-label="Members">
        <MemberChip title={localDeviceLabel(localDeviceProfile)} subtitle={localDeviceSubtitle(localDeviceProfile)} you />
        {remotePeers.length === 0 ? <span className="muted">No connected members yet.</span> : null}
        {remotePeers.map((peer) => (
          <MemberChip key={peer.peerSessionId} title={remotePeerDisplayName(peer, room)} subtitle={remotePeerSubtitle(peer)} />
        ))}
      </section>

      <Card className={`send-anything-card ${dropActive ? "drop-active" : ""}`}>
        <div className="send-card-heading">
          <div>
            <h2>Send anything</h2>
            <p className="muted">{targetSummary(selectedRoute, selectedPeers)}</p>
          </div>
          <TargetSelector
            peers={remotePeers}
            targetMode={targetMode}
            selectedPeerIds={selectedPeerIds}
            onModeChange={setTargetMode}
            onSelectedPeerIdsChange={setSelectedPeerIds}
          />
        </div>
        <textarea
          ref={composerRef}
          value={text}
          onChange={(event) => setText(event.target.value)}
          onPaste={(event) => void handlePaste(event)}
          placeholder="Paste text, links, images, or drop files here..."
          aria-label="Send anything"
        />
        <div className="send-composer-actions">
          <button type="button" className="secondary-button" disabled={!canSend || busy !== null} onClick={() => void handleChooseFiles()}>
            + Files
          </button>
          <div className="composer-status">
            {!canSend ? <span>Select a connected device to send.</span> : null}
            {error ? <span className="danger-text">{error}</span> : null}
          </div>
          <button type="button" className="primary-button" disabled={!canSend || !text.trim()} onClick={() => void handleSendText()}>
            {busy === "send" ? "Sending..." : "Send"}
          </button>
        </div>
      </Card>

      <BridgePlanSenderPanel
        enabled={askBridgeBetaEnabled}
        room={room}
        localDeviceProfile={localDeviceProfile}
        selectedPeer={selectedSinglePeer}
        route={selectedRoute}
        inboxEvents={bridgePlanInboxBatch}
      />

      <BridgePlanReceiverPanel
        inboxEvents={bridgePlanInboxBatch}
      />

      <BridgePlanWorkspacePanel room={room} />

      <Card className="recent-activity-card">
        <div className="section-row">
          <h2>Recent activity</h2>
          <button type="button" className="text-button" onClick={onOpenActivity}>View all activity</button>
        </div>
        {recent.length === 0 ? <p className="muted">Nothing yet for this Bridge.</p> : null}
        {recent.map((row) => (
          <ActivityRow key={row.id} row={row} compact />
        ))}
      </Card>
    </section>
  );
}

function BridgePlanReceiverPanel({
  inboxEvents,
}: {
  inboxEvents: readonly ReceivedRoomControlEvent[];
}) {
  const active = inboxEvents.some((event) => event.kind.startsWith("bridge_plan."));
  if (!active) return null;

  return (
    <Card className="ask-bridge-card" aria-label="Received Ask Bridge plan">
      <div className="section-row">
        <div>
          <h2>Ask Bridge</h2>
          <p className="muted">Running bounded tasks from the current Bridge session. Progress is observational; Stop or Burn remains available from Bridge controls.</p>
        </div>
      </div>
      <div className="request-file-preview"><h3>Executing automatically</h3><p>Pastey derives and consumes each eligible bounded step authority on this Host. This device does not approve or start individual plan steps.</p></div>
    </Card>
  );
}

function BridgePlanSenderPanel({
  enabled,
  room,
  localDeviceProfile,
  selectedPeer,
  route,
  inboxEvents,
}: {
  enabled: boolean;
  room: RoomInfo;
  localDeviceProfile: DeviceProfile | null;
  selectedPeer: BridgePeerSession | null;
  route: BridgeRoute | null;
  inboxEvents: readonly ReceivedRoomControlEvent[];
}) {
  const [blocks, setBlocks] = useState<ComposerBlock[]>([newSearchBlock()]);
  const localTransformHostLabel = localDeviceProfile?.device_name?.trim() || localDeviceLabel(localDeviceProfile);
  const selectedTransformHostLabel = selectedPeer?.displayName ?? "Selected device";
  const [transformCapabilities, setTransformCapabilities] = useState<TransformExecutorCapabilities>(() => ({
    requesting_device: unknownTransformAvailability("This device", "Checking this device capability…"),
    selected_device: unknownTransformAvailability("Selected device", "Checking selected device capability…"),
  }));
  const [revisionId, setRevisionId] = useState<string | null>(null);
  const [approvalId, setApprovalId] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState<"plan" | "review" | "start" | null>(null);
  const [approvalState, setApprovalState] = useState<string | null>(null);
  const [resultSummary, setResultSummary] = useState<string | null>(null);
  const [attemptId, setAttemptId] = useState<string | null>(null);
  const [selectedCandidateId, setSelectedCandidateId] = useState<string | null>(null);
  const [hasTransform, setHasTransform] = useState(false);
  const [directTransfer, setDirectTransfer] = useState(false);
  const selectedPeerRoute = route?.target.kind === "selected_peer" ? route : null;
  const canPlan = enabled && Boolean(selectedPeer && selectedPeerRoute);
  const safeSearchCandidates = useMemo(
    () => inboxEvents.flatMap(parseBridgePlanSearchCandidates).filter((entry) => !attemptId || entry.attemptId === attemptId),
    [attemptId, inboxEvents],
  );
  const terminalSearchResult = useMemo(
    () => inboxEvents
      .flatMap((event) => {
        const result = parseBridgePlanSearchTerminalResult(event);
        return result ? [result] : [];
      })
      .find((entry) => entry.attemptId === attemptId) ?? null,
    [attemptId, inboxEvents],
  );
  const candidateMode = bridgePlanSearchCandidateMode(blocks.map((step) => step.primitive));
  const transformExecutor = blocks.find((step): step is Extract<ComposerBlock, { primitive: "Transform" }> => step.primitive === "Transform")?.executionDevice ?? "requesting_device";
  const transformAvailability = transformCapabilities[transformExecutor];
  const visibleObjectFlow = objectFlow(blocks).visibleBlocks;
  const derivedPipelineHandoffs = visibleObjectFlow.filter((step): step is DerivedPipelineTransferBlock => step.primitive === "Transfer" && "derived" in step);
  const composerDeviceLabel = (device: ComposerDevice) => device === "requesting_device" ? localTransformHostLabel : selectedTransformHostLabel;
  const reviewSearch = blocks.find((step) => step.primitive === "Search");
  const reviewTransfer = blocks.find((step) => step.primitive === "Transfer");
  const failedTransformAttemptIds = useMemo(
    () => new Set(inboxEvents.flatMap(parseFailedBridgePlanTransform)),
    [inboxEvents],
  );

  useEffect(() => {
    if (revisionId && !approvalId) {
      void withdrawBridgePlanRevision(room.id, revisionId).catch(() => undefined);
    }
    setBlocks([newSearchBlock()]);
    setRevisionId(null);
    setApprovalId(null);
    setApprovalState(null);
    setResultSummary(null);
    setAttemptId(null);
    setSelectedCandidateId(null);
    setDirectTransfer(false);
    setMessage(null);
  }, [room.id, selectedPeer?.peerSessionId]);

  useEffect(() => {
    let cancelled = false;
    setTransformCapabilities({
      requesting_device: unknownTransformAvailability(localTransformHostLabel, "Checking this device capability…"),
      selected_device: unknownTransformAvailability(selectedTransformHostLabel, "Checking selected device capability…"),
    });
    if (!selectedPeerRoute || !selectedPeer) return;
    let refreshing = false;
    const refresh = async () => {
      if (refreshing) return;
      refreshing = true;
      try {
        const local = localTransformAvailability()
          .then((observation) => {
            if (!cancelled) setTransformCapabilities((current) => ({
              ...current,
              requesting_device: transformAvailabilityFromObservation(observation, localTransformHostLabel),
            }));
          })
          .catch(() => {
            if (!cancelled) setTransformCapabilities((current) => ({
              ...current,
              requesting_device: unknownTransformAvailability(localTransformHostLabel, "This device capability is unknown."),
            }));
          });
        const remote = (async () => {
          try {
            await refreshSelectedPeerCapabilities(room.id, bridgeRoutePayload(selectedPeerRoute, "pastey-bridge-control-route-v1"));
            for (let attempt = 0; attempt < 5 && !cancelled; attempt += 1) {
              const observation = await selectedPeerTransformAvailability(room.id);
              if (cancelled) return;
              setTransformCapabilities((current) => ({
                ...current,
                selected_device: transformAvailabilityFromObservation(observation, selectedTransformHostLabel),
              }));
              if (observation.status !== "unknown") return;
              await new Promise((resolve) => window.setTimeout(resolve, 300));
            }
          } catch {
            if (!cancelled) setTransformCapabilities((current) => ({
              ...current,
              selected_device: unknownTransformAvailability(selectedTransformHostLabel, "Selected device capability is unknown."),
            }));
          }
        })();
        await Promise.all([local, remote]);
      } finally {
        refreshing = false;
      }
    };
    void refresh();
    const interval = window.setInterval(() => { void refresh(); }, 60_000);
    return () => { cancelled = true; window.clearInterval(interval); };
  }, [localTransformHostLabel, room.id, selectedPeer?.peerSessionId, selectedPeerRoute, selectedTransformHostLabel]);

  function editBlocks(next: ComposerBlock[]) {
    if (approvalId) {
      setMessage("The approved revision is already running. Its execution devices cannot be changed.");
      return;
    }
    if (revisionId) {
      const staleRevisionId = revisionId;
      void withdrawBridgePlanRevision(room.id, staleRevisionId).catch((error) => {
        setMessage(error instanceof Error ? error.message : "Pastey could not withdraw the stale plan revision.");
      });
      setRevisionId(null);
      setApprovalId(null);
      setApprovalState(null);
      setAttemptId(null);
      setSelectedCandidateId(null);
      setDirectTransfer(false);
      setMessage("Plan semantics changed. Review creates a new immutable revision before approval.");
    }
    setBlocks(next);
  }

  useEffect(() => {
    const hasDraftTransform = blocks.some((step) => step.primitive === "Transform");
    if (!revisionId || approvalId || !hasDraftTransform || transformAvailability.status !== "unavailable") return;
    const staleRevisionId = revisionId;
    setRevisionId(null);
    setMessage(`${transformAvailability.hostLabel} can no longer execute the reviewed Transform. Review a new revision after choosing an Available executor.`);
    void withdrawBridgePlanRevision(room.id, staleRevisionId).catch(() => undefined);
  }, [approvalId, blocks, revisionId, room.id, transformAvailability.hostLabel, transformAvailability.status]);

  useEffect(() => {
    if (!approvalId) return;
    let cancelled = false;
    const refresh = async () => {
      try {
        const workspace = await listBridgePlanWorkspace(room.id);
        if (cancelled) return;
        const approval = workspace.approvals
          .map(parseBridgePlanApproval)
          .find((entry): entry is { approvalId: string; state: string } => entry?.approvalId === approvalId);
        if (approval) setApprovalState(approval.state);
        const attemptIds = new Set(
          workspace.attempts
            .map(parseBridgePlanAttempt)
            .filter((entry): entry is { approvalId: string; attemptId: string } => entry?.approvalId === approvalId)
            .map((entry) => entry.attemptId),
        );
        const currentAttemptIds = [...attemptIds];
        const latestAttemptId = currentAttemptIds.length > 0 ? currentAttemptIds[currentAttemptIds.length - 1] : null;
        setAttemptId(latestAttemptId);
        const result = workspace.results
          .map(parseBridgePlanResult)
          .find((entry): entry is { attemptId: string; summary: string } => Boolean(entry && attemptIds.has(entry.attemptId)));
        if (result) setResultSummary(result.summary);
      } catch (error) {
        if (!cancelled) setMessage(error instanceof Error ? error.message : "Could not refresh the plan status.");
      }
    };
    let timeout: number | null = null;
    const poll = () => {
      void refresh().finally(() => {
        if (!cancelled) timeout = window.setTimeout(poll, 2_000);
      });
    };
    poll();
    return () => {
      cancelled = true;
      if (timeout !== null) window.clearTimeout(timeout);
    };
  }, [approvalId, room.id]);

  useEffect(() => {
    if (!terminalSearchResult) return;
    const presentation = terminalSearchPresentation(terminalSearchResult);
    setApprovalState(presentation.status);
    setResultSummary(presentation.summary);
    setMessage(null);
  }, [terminalSearchResult]);

  async function createPlan() {
    if (!canPlan || !selectedPeerRoute) {
      setMessage(BRIDGE_PLAN_REQUIRES_ONE_SELECTED_DEVICE);
      return;
    }
    const composed = manualBridgePlanInput(blocks, transformCapabilities);
    if (!composed.value) { setMessage(composed.error ?? "Complete the bounded plan fields."); return; }
    setBusy("plan");
    setMessage(null);
    try {
      const plan = composed.value;
      const supportsTransform = Boolean(plan.transformIntent);
      const supportsTransfer = Boolean(plan.transferDestination);
      if (supportsTransform && !plan.transformExecutionDevice) throw new Error("Choose an explicit Transform execution device.");
      const workspace = plan.transformIntent && plan.transformExecutionDevice
        ? await createFileTransformBridgePlan({
          roomId: room.id,
          originalUserGoal: plan.originalUserGoal,
          filenameHint: plan.filenameHint,
          extensions: plan.extensions,
          safeScopes: plan.safeScopes,
          transferToRequester: plan.transferDestination === "requesting_device",
          transferDestination: plan.transferDestination === "pastey_shared" ? "selected_device" : "requesting_device",
          transformIntent: plan.transformIntent,
          transformExecutionDevice: plan.transformExecutionDevice,
        })
        : await createFileSearchBridgePlan({
          roomId: room.id,
          originalUserGoal: plan.originalUserGoal,
          filenameHint: plan.filenameHint,
          extensions: plan.extensions,
          safeScopes: plan.safeScopes,
          transferToRequester: plan.transferDestination === "requesting_device",
          transferDestination: plan.transferDestination === "pastey_shared" ? "selected_device" : "requesting_device",
        });
      const revision = workspace.revisions.map(parseBridgePlanRevision).filter((entry): entry is { revisionId: string; state: string } => entry?.state === "available").pop();
      if (!revision) throw new Error("Pastey did not return the durable Search plan.");
      setRevisionId(revision.revisionId);
      setApprovalId(null);
      setApprovalState(null);
      setResultSummary(null);
      setAttemptId(null);
      setSelectedCandidateId(null);
      setHasTransform(supportsTransform);
      setDirectTransfer(false);
      setMessage(supportsTransform
        ? supportsTransfer ? "Plan ready. Review the complete Search, Transform, and Transfer plan before sending it to the selected device." : "Plan ready. Review the complete Search and Transform plan before sending it to the selected device."
        : supportsTransfer ? "Plan ready. Review the complete Search and Transfer plan before sending it to the selected device." : "Plan ready. Review the complete Search plan before sending it to the selected device.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not create the Search plan.");
    } finally {
      setBusy(null);
    }
  }

  async function requestReview() {
    if (!revisionId || !selectedPeerRoute) return;
    if (blocks.some((step) => step.primitive === "Transform") && (transformAvailability.status !== "available" || !transformAvailability.available)) {
      setMessage(transformAvailability.reason);
      return;
    }
    setBusy("review");
    setMessage(null);
    try {
      const nextApprovalId = `bridge-plan-approval-${crypto.randomUUID()}`;
      await approveBridgePlan(revisionId, nextApprovalId);
      await bindBridgePlanToSession(
        nextApprovalId,
        bridgeRoutePayload(selectedPeerRoute, "pastey-bridge-control-route-v1"),
      );
      setApprovalId(nextApprovalId);
      const nextAttemptId = `bridge-plan-attempt-${crypto.randomUUID()}`;
      await startBridgePlanAttempt(
        nextApprovalId,
        nextAttemptId,
        bridgeRoutePayload(selectedPeerRoute, "pastey-bridge-control-route-v1"),
      );
      setAttemptId(nextAttemptId);
      setApprovalState("running");
      setMessage("Approved plan started. Pastey is waiting only for a bounded Search result selection if needed.");
    } catch (error) {
      setMessage(bridgePlanControlErrorMessage(error, "review"));
    } finally {
      setBusy(null);
    }
  }

  async function createDirectTransferPlan() {
    if (!canPlan) return;
    setBusy("plan");
    setMessage(null);
    try {
      const selected = await open({ multiple: false, directory: false });
      if (typeof selected !== "string") return;
      const workspace = await createDirectFileTransferBridgePlan({
        roomId: room.id,
        originalUserGoal: "Transfer one selected local file to the selected device.",
        sourcePath: selected,
      });
      const revision = workspace.revisions.map(parseBridgePlanRevision).filter((entry): entry is { revisionId: string; state: string } => entry?.state === "available").pop();
      if (!revision) throw new Error("Pastey did not return the direct Transfer plan.");
      setBlocks([{ primitive: "Transfer", destination: "pastey_shared", landingMode: "final_delivery" }]);
      setRevisionId(revision.revisionId);
      setApprovalId(null); setApprovalState(null); setAttemptId(null); setSelectedCandidateId(null);
      setHasTransform(false); setDirectTransfer(true);
      setMessage("Plan ready. Review the complete Transfer plan before sending it to the selected device.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not create the direct Transfer plan.");
    } finally { setBusy(null); }
  }

  async function selectCandidate(candidateId: string) {
    if (!attemptId || !selectedPeerRoute) return;
    setBusy("start");
    setMessage(null);
    try {
      await selectBridgePlanSearchCandidate(
        room.id,
        attemptId,
        candidateId,
        bridgeRoutePayload(selectedPeerRoute, "pastey-bridge-control-route-v1"),
      );
      setSelectedCandidateId(candidateId);
      setMessage("Candidate selected. Pastey is continuing the approved object flow…");
    } catch (error) {
      setMessage(bridgePlanTransferErrorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  async function proposeTransformFallback() {
    if (!revisionId) return;
    setBusy("plan");
    setMessage(null);
    try {
      const workspace = await proposeBridgePlanTransformFallback(revisionId);
      const revision = workspace.revisions.map(parseBridgePlanRevision).filter((entry): entry is { revisionId: string; state: string } => entry?.state === "available").pop();
      if (!revision) throw new Error("Pastey did not create the revised plan.");
      setRevisionId(revision.revisionId);
      setApprovalId(null); setApprovalState(null); setAttemptId(null); setSelectedCandidateId(null); setHasTransform(false);
      setMessage("A new unapproved alternative removed the unavailable processing step. Review it again before sending it to the selected device.");
    } catch (error) { setMessage(error instanceof Error ? error.message : "Pastey could not create a revised plan."); }
    finally { setBusy(null); }
  }

  if (!enabled) return null;
  return (
    <Card className="ask-bridge-card">
      <div className="section-row">
        <div>
          <h2>Ask Bridge</h2>
          <p className="muted">Create one complete, reviewable plan for the selected device. Review &amp; Run gives the current Bridge session one bounded task to execute.</p>
        </div>
      </div>
      <div className="button-row">
        <button type="button" className="secondary-button" disabled={!canAddPrimitive(blocks, "Search")} onClick={() => editBlocks(addPrimitive(blocks, "Search").blocks)}>+ Search</button>
        <button type="button" className="secondary-button" disabled={Boolean(approvalId) || !canAddPrimitive(blocks, "Transform")} onClick={() => editBlocks(addPrimitive(blocks, "Transform", initialTransformExecutionDevice(transformCapabilities)).blocks)}>+ Transform</button>
        <button type="button" className="secondary-button" disabled={!canAddPrimitive(blocks, "Transfer")} onClick={() => editBlocks(addPrimitive(blocks, "Transfer").blocks)}>+ Transfer</button>
        <button type="button" className="secondary-button" disabled={!canPlan || busy !== null} onClick={() => void createDirectTransferPlan()}>
          Transfer local file
        </button>
      </div>
      <div className="request-file-preview" data-testid="ask-bridge-block-composer">
        {blocks.map((block, index) => (
          <section key={`${block.primitive}-${index}`} className="bridge-plan-block">
            <div className="section-row"><h3>{index + 1}. {block.primitive}</h3><div className="button-row"><button type="button" className="text-button" disabled={index === 0} onClick={() => { const next = moveBlock(blocks, index, index - 1); editBlocks(next.blocks); setMessage(next.error); }}>↑</button><button type="button" className="text-button" disabled={index === blocks.length - 1} onClick={() => { const next = moveBlock(blocks, index, index + 1); editBlocks(next.blocks); setMessage(next.error); }}>↓</button><button type="button" className="text-button" onClick={() => { const next = removeBlock(blocks, index); editBlocks(next.blocks); setMessage(next.error); }}>Remove</button></div></div>
            {block.primitive === "Search" ? <div className="bridge-plan-block-fields"><label>Device<input value={selectedPeer?.displayName ?? "Selected device"} readOnly /></label><label>Look in<select aria-label="Reviewed Search scope" value={block.safeScopes[0] ?? "downloads"} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Search" ? updateSearchBlock(entry, { safeScopes: [event.target.value as SafeSearchScope] }) : entry))}>{SAFE_SEARCH_SCOPES.map((scope) => <option key={scope.value} value={scope.value}>{scope.label}</option>)}</select></label><label>File name<input aria-label="Search filename" value={block.filenameHint} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Search" ? updateSearchBlock(entry, { filenameHint: event.target.value }) : entry))} placeholder="Funding Statement.pdf" /></label><label>Type<input aria-label="Search extension" value={block.extension} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Search" ? updateSearchBlock(entry, { extension: event.target.value }) : entry))} placeholder="pdf" /></label></div> : null}
            {block.primitive === "Transform" ? <div className="bridge-plan-block-fields"><p>Process file: <strong>Extract readable text</strong></p><label>Process on:<select aria-label="Transform execution device" disabled={Boolean(approvalId)} value={block.executionDevice} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Transform" ? { ...entry, executionDevice: event.target.value as ComposerDevice } : entry))}><option value="requesting_device">{localTransformHostLabel} — {transformCapabilityStatusLabel(transformCapabilities.requesting_device)}</option><option value="selected_device">{selectedTransformHostLabel} — {transformCapabilityStatusLabel(transformCapabilities.selected_device)}</option></select></label><div aria-label="Transform executor capabilities">{(["requesting_device", "selected_device"] as const).map((device) => { const capability = transformCapabilities[device]; return <p key={device} className={capability.status === "unknown" ? "muted" : capability.available ? "success-text" : "danger-text"}><strong>{composerDeviceLabel(device)}</strong>: {transformCapabilityStatusLabel(capability)}{capability.reason ? ` — ${capability.reason}` : ""}</p>; })}</div><p>Runs on: {transformAvailability.hostLabel}</p><p className={transformAvailability.status === "unknown" ? "muted" : transformAvailability.available ? "success-text" : "danger-text"}>Availability: {transformCapabilityStatusLabel(transformAvailability)}{transformAvailability.reason ? ` — ${transformAvailability.reason}` : ""}</p>{transformAvailability.acceptedInputMediaTypes?.length ? <p className="muted">Accepted input types: {transformAvailability.acceptedInputMediaTypes.join(", ")}. Candidate compatibility is rechecked by the execution Host.</p> : null}</div> : null}
            {block.primitive === "Transfer" ? <div className="bridge-plan-block-fields"><label>Send result to:<select aria-label="Transfer destination" value={block.destination} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Transfer" ? { ...entry, destination: event.target.value as "requesting_device" | "selected_device" | "pastey_shared" } : entry))}><option value="requesting_device">This device</option><option value="selected_device">Selected device</option><option value="pastey_shared">Pastey Shared on selected device</option></select></label><p className="muted">Final delivery creates a user-visible result. Required processing handoffs are shown in the reviewed plan and stay private.</p></div> : null}
          </section>
        ))}
        {derivedPipelineHandoffs.map((handoff, index) => (
          <section key={`pipeline-handoff-${index}`} className="bridge-plan-block" aria-label="Derived Pipeline handoff">
            <h3>Transfer for processing</h3>
            <p><strong>{composerDeviceLabel(handoff.source)} → {composerDeviceLabel(handoff.destination)}</strong></p>
            <p>Pipeline handoff</p>
            <p className="muted">Private intermediate transfer. Required for processing; it does not create an Inbox or Pastey Shared delivery.</p>
          </section>
        ))}
      </div>
      <div className="button-row"><button type="button" className="primary-button" disabled={!canPlan || busy !== null || directTransfer || Boolean(approvalId) || (blocks.some((step) => step.primitive === "Transform") && (transformAvailability.status !== "available" || !transformAvailability.available))} onClick={() => void createPlan()}>{busy === "plan" ? "Building…" : "Review plan"}</button></div>
      {!canPlan ? <p className="muted">Select one connected device to create a plan.</p> : null}
      {revisionId ? (
        <div className="request-file-preview" data-testid="ask-bridge-plan-preview">
          <h3>Review plan</h3>
          <p>{directTransfer ? "Transfer the one local file you chose to the selected device after your single Review & Run approval." : blocks.some((step) => step.primitive === "Transform") ? "Search the selected device’s reviewed locations, let you choose one bounded result, then process it with the reviewed capability." : blocks.some((step) => step.primitive === "Transfer") ? "Search the selected device’s reviewed locations, let you choose one bounded result, then transfer it to the approved destination." : "Search the selected device’s reviewed locations for matching files and return a bounded summary."}</p>
          {!directTransfer && reviewSearch?.primitive === "Search" ? (
            <p className="muted">
              Search: {reviewSearch.filenameHint} {reviewSearch.extension ? `(${reviewSearch.extension.toUpperCase()})` : ""} in {reviewSearch.safeScopes.join(", ")}.
              {reviewTransfer?.primitive === "Transfer" ? ` Destination: ${reviewTransfer.destination === "requesting_device" ? "requesting device" : "selected device Pastey Shared"}.` : ""}
            </p>
          ) : null}
          {!directTransfer ? (
            <div aria-label="Reviewed object flow">
              {visibleObjectFlow.map((step, index) => {
                if (step.primitive === "Search") return <p key={`review-flow-${index}`}><strong>Search @ {composerDeviceLabel(step.executionDevice)}</strong></p>;
                if (step.primitive === "Transform") return <p key={`review-flow-${index}`}><strong>Transform @ {composerDeviceLabel(step.executionDevice)}</strong><br /><span className="muted">Extract readable text</span></p>;
                if ("derived" in step) return <p key={`review-flow-${index}`}><strong>PipelineHandoff</strong><br />{composerDeviceLabel(step.source)} → {composerDeviceLabel(step.destination)}<br /><span className="muted">Required for processing · Private intermediate transfer</span></p>;
                return <p key={`review-flow-${index}`}><strong>Final Transfer → {step.destination === "requesting_device" ? localTransformHostLabel : selectedTransformHostLabel}</strong></p>;
              })}
            </div>
          ) : null}
          {!approvalId ? (
            <button type="button" className="primary-button" disabled={busy !== null || (blocks.some((step) => step.primitive === "Transform") && (transformAvailability.status !== "available" || !transformAvailability.available))} onClick={() => void requestReview()}>
              {busy === "review" ? "Starting…" : "Review & Run"}
            </button>
          ) : null}
        </div>
      ) : null}
      {approvalState === "running" ? <p className="muted">Search is running on the selected device.</p> : null}
      {resultSummary && terminalSearchResult?.candidateCount !== 0 ? <p className="success-text">{resultSummary}</p> : null}
      {terminalSearchResult?.candidateCount === 0 ? (
        <div className="request-file-preview">
          <h3>Search results</h3>
          <p className="success-text">Search completed with no matching files.</p>
        </div>
      ) : null}
      {safeSearchCandidates.length > 0 && (candidateMode === "result" || !selectedCandidateId) ? (
        <div className="candidate-card-list">
          <h3>{candidateMode === "selectable" ? "Choose a file for the approved next step" : "Search results on the selected device"}</h3>
          {candidateMode === "result" ? <p className="muted">These matching files remain on the selected device.</p> : null}
          {safeSearchCandidates.map((candidate) => (
            <SearchCandidateCard
              key={candidate.candidateId}
              candidate={candidate}
              selectable={candidateMode === "selectable"}
              disabled={busy !== null}
              onSelect={candidateMode === "selectable" ? () => void selectCandidate(selectedBridgePlanCandidateId(candidate)) : undefined}
            />
          ))}
        </div>
      ) : null}
      {hasTransform && attemptId && failedTransformAttemptIds.has(attemptId) ? (
        <div className="request-file-preview"><h3>Processing unavailable</h3><p>The selected device could not perform the approved Transform for this file. Create a new plan revision without that step; both devices must review it again.</p><button type="button" className="secondary-button" disabled={busy !== null} onClick={() => void proposeTransformFallback()}>{busy === "plan" ? "Preparing…" : "Create revised plan"}</button></div>
      ) : null}
      {message ? <p className="muted" role="status">{message}</p> : null}
    </Card>
  );
}

function unknownTransformAvailability(hostLabel: string, reason: string): TransformAvailability {
  return {
    intent: "extract readable text",
    status: "unknown",
    available: false,
    reason,
    hostLabel,
  };
}

function transformAvailabilityFromObservation(
  observation: SelectedPeerTransformAvailability,
  hostLabel: string,
): TransformAvailability {
  return {
    intent: "extract readable text",
    status: observation.status,
    available: observation.available,
    reason: observation.reason,
    hostLabel,
    acceptedInputMediaTypes: observation.acceptedInputMediaTypes,
    outputMediaType: observation.outputMediaType,
  };
}

function transformCapabilityStatusLabel(capability: TransformAvailability): string {
  if (capability.status === "unknown") return "Unknown";
  return capability.available ? "Available" : "Unavailable";
}

function SearchCandidateCard({
  candidate,
  selectable,
  disabled,
  onSelect,
}: {
  candidate: BridgePlanSearchCandidate;
  selectable: boolean;
  disabled: boolean;
  onSelect?: () => void;
}) {
  const metadata = candidateMetadata(candidate);
  const contents = <>
    <strong>{candidate.displayName}</strong>
    <span>{metadata.size} · {metadata.fileType}</span>
    {metadata.redactedLocation ? <small>Location: {metadata.redactedLocation}</small> : null}
    {metadata.modifiedAt ? <small>Modified: {metadata.modifiedAt}</small> : null}
    <small>{metadata.matchReason}{metadata.confidence ? ` · ${metadata.confidence} confidence` : ""}</small>
  </>;
  return selectable ? (
    <button type="button" className="candidate-metadata-card" disabled={disabled} onClick={onSelect}>
      {contents}
    </button>
  ) : (
    <article className="candidate-metadata-card search-result-card" data-testid="bridge-plan-search-result-card">
      {contents}
    </article>
  );
}

function BridgePlanWorkspacePanel({ room }: { room: RoomInfo }) {
  const [workspace, setWorkspace] = useState<{
    plans: Array<{ description: string; state: string }>;
    activity: string[];
    results: string[];
  }>({ plans: [], activity: [], results: [] });
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const records = await listBridgePlanWorkspace(room.id);
      setWorkspace({
        plans: records.revisions.map(parseBridgePlanWorkspaceRevision).filter((entry): entry is { description: string; state: string } => entry !== null),
        activity: records.activities.map(parseBridgePlanActivity).filter((entry): entry is string => entry !== null).slice(-8),
        results: records.results.map(parseBridgePlanResult).filter((entry): entry is { attemptId: string; summary: string } => entry !== null).map((entry) => entry.summary).slice(-8),
      });
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Pastey could not load Bridge Plan history.");
    }
  };

  useEffect(() => {
    void refresh();
  }, [room.id]);

  if (workspace.plans.length === 0 && workspace.activity.length === 0 && workspace.results.length === 0 && !error) return null;
  return (
    <Card className="ask-bridge-card">
      <div className="section-row">
        <div>
          <h2>Plan history</h2>
          <p className="muted">Plan history stays with this Bridge until it is burned.</p>
        </div>
        <button type="button" className="text-button" onClick={() => void refresh()}>Refresh</button>
      </div>
      {workspace.plans.map((plan, index) => <p key={`${plan.description}-${index}`}>{plan.description} <span className="muted">({formatBridgePlanState(plan.state)})</span></p>)}
      {workspace.activity.map((entry, index) => <p className="muted" key={`${entry}-${index}`}>{entry}</p>)}
      {workspace.results.map((entry, index) => <p className="success-text" key={`${entry}-${index}`}>{entry}</p>)}
      {error ? <p className="danger-text">{error}</p> : null}
    </Card>
  );
}

function parseBridgePlanWorkspaceRevision(value: unknown): { description: string; state: string } | null {
  if (!isRecord(value) || !isRecord(value.revision) || !isRecord(value.revision.presentation) || typeof value.revision.presentation.natural_language_plan !== "string" || typeof value.state !== "string") return null;
  return { description: value.revision.presentation.natural_language_plan, state: value.state };
}

function parseBridgePlanActivity(value: unknown): string | null {
  return isRecord(value) && typeof value.summary === "string" ? value.summary : null;
}

function formatBridgePlanState(state: string): string {
  return state.replace(/_/g, " ");
}

function parseBridgePlanRevision(value: unknown): { revisionId: string; state: string } | null {
  if (!isRecord(value) || !isRecord(value.revision) || typeof value.revision.revision_id !== "string" || typeof value.state !== "string") return null;
  return { revisionId: value.revision.revision_id, state: value.state };
}

function parseBridgePlanApproval(value: unknown): { approvalId: string; state: string } | null {
  if (!isRecord(value) || !isRecord(value.approval) || typeof value.approval.approval_id !== "string" || typeof value.state !== "string") return null;
  return { approvalId: value.approval.approval_id, state: value.state };
}

function parseBridgePlanAttempt(value: unknown): { approvalId: string; attemptId: string } | null {
  if (!isRecord(value) || !isRecord(value.attempt) || typeof value.attempt.approval_id !== "string" || typeof value.attempt.attempt_id !== "string") return null;
  return { approvalId: value.attempt.approval_id, attemptId: value.attempt.attempt_id };
}

function parseBridgePlanResult(value: unknown): { attemptId: string; summary: string } | null {
  if (!isRecord(value) || typeof value.attempt_id !== "string" || typeof value.summary !== "string") return null;
  return { attemptId: value.attempt_id, summary: value.summary };
}

function parseFailedBridgePlanTransform(event: ReceivedRoomControlEvent): string[] {
  if (event.kind !== "bridge_plan.step_failed") return [];
  const payload = roomControlEventPayload(event.event);
  return payload?.stepId === "transform" && typeof payload.attemptId === "string" ? [payload.attemptId] : [];
}

function roomControlEventPayload(event: unknown): Record<string, unknown> | null {
  if (!isRecord(event) || !isRecord(event.payload)) return null;
  return event.payload;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

interface ActivityPageProps {
  rooms: RoomInfo[];
  roomItems: RoomItem[];
  transfers: FileTransferProgressEvent[];
  queueItems: TransferQueueItem[];
}

export function ActivityPage({ rooms, roomItems, transfers, queueItems }: ActivityPageProps) {
  const rows = activityRows(rooms, roomItems, transfers, queueItems);
  const firstSavedPath = roomItems.find((item) => item.direction === "incoming" && item.saved_path)?.saved_path ?? null;
  const groups = [
    { title: "Now", rows: rows.filter((row) => row.group === "now") },
    { title: "Pending", rows: rows.filter((row) => row.group === "pending") },
    { title: "Received", rows: rows.filter((row) => row.group === "received") },
    { title: "Sent", rows: rows.filter((row) => row.group === "sent") },
    { title: "Failed", rows: rows.filter((row) => row.group === "failed") },
  ];

  return (
    <section className="product-page activity-page" aria-label="Activity">
      <ProductHeader
        title="Activity"
        subtitle="Track what's happening across your Bridges."
        action={(
          <button
            type="button"
            className="secondary-button"
            disabled={!firstSavedPath}
            title={firstSavedPath ? "Open receiving folder" : "No received files yet."}
            onClick={() => {
              if (firstSavedPath) void revealInFolder(firstSavedPath);
            }}
          >
            Open receiving folder
          </button>
        )}
      />
      {rows.length === 0 ? (
        <Card className="bridge-start-card">
          <h2>No activity yet</h2>
          <p className="muted">Sent and received items will appear here as they happen.</p>
        </Card>
      ) : null}
      {groups.map((group) => group.rows.length > 0 ? (
        <section key={group.title} className="activity-group">
          <h2>{group.title}</h2>
          <div className="activity-stream">
            {group.rows.map((row) => <ActivityRow key={row.id} row={row} />)}
          </div>
        </section>
      ) : null)}
    </section>
  );
}

interface DevicesProductPageProps {
  rooms: RoomInfo[];
  activeBridgeRoomId: string;
  shouldFocus: boolean;
  onOpenBridge: (room: RoomInfo) => void;
  onJoinBridge: (code: string) => Promise<void>;
  onConnectionJoined: (room: RoomInfo) => void;
}

export function DevicesProductPage({
  rooms,
  activeBridgeRoomId,
  shouldFocus,
  onOpenBridge,
  onJoinBridge,
  onConnectionJoined,
}: DevicesProductPageProps) {
  const [nearbyDevices, setNearbyDevices] = useState<NearbyDevice[]>([]);
  const [joinCode, setJoinCode] = useState("");
  const [busy, setBusy] = useState<"nearby" | "join" | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const joinInputRef = useRef<HTMLInputElement | null>(null);
  const activeBridge = activeBridgeRoomId ? rooms.find((room) => room.id === activeBridgeRoomId) ?? null : null;

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
        if (!cancelled) {
          setNearbyDevices(devices);
          setMessage(devices.length === 0 ? "No nearby devices found." : null);
        }
      } catch {
        if (!cancelled) {
          setNearbyDevices([]);
          setMessage("Pastey cannot see nearby devices on this network.");
        }
      }
    }
    void loadNearby();
    const interval = window.setInterval(() => void loadNearby(), 2000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, []);

  async function handleNearbyJoin(device: NearbyDevice) {
    setBusy("nearby");
    setMessage(`Waiting for ${device.display_name} to approve...`);
    try {
      const room = await requestNearbyJoin(device.device_id);
      setMessage(null);
      onConnectionJoined(room);
    } catch (err) {
      setMessage(networkHelpMessage(err instanceof Error ? err.message : String(err)));
    } finally {
      setBusy(null);
    }
  }

  async function handleJoinBridge() {
    if (joinCode.length !== 8) return;
    setBusy("join");
    setMessage(null);
    try {
      await onJoinBridge(joinCode);
      setJoinCode("");
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  const knownRooms = rooms.filter((room) => room.peer_device_name || (room.peers?.length ?? 0) > 0);

  return (
    <section className="product-page devices-page" aria-label="Devices">
      <ProductHeader title="Devices" subtitle="Connect and manage the devices you use most." />

      <section className="page-section">
        <h2>Nearby</h2>
        <div className="simple-list-card">
          {nearbyDevices.length === 0 ? <p className="muted">{message ?? "Open Pastey on another local device."}</p> : null}
          {nearbyDevices.map((device) => (
            <div key={device.device_id} className="simple-device-row">
              <div>
                <strong>{device.display_name}</strong>
                <span className="muted">{nearbyDeviceSystemSummary(device)}</span>
                <span className={`status-line ${device.availability === "Available" && device.compatible ? "ready" : ""}`}>
                  <span aria-hidden="true" />
                  {device.compatible ? device.availability : "Update needed"}
                </span>
              </div>
              <button
                type="button"
                className="primary-button"
                disabled={busy !== null || device.availability !== "Available" || !device.compatible}
                onClick={() => void handleNearbyJoin(device)}
              >
                {activeBridge ? `Add to ${bridgeCode(activeBridge)}` : "Start Bridge"}
              </button>
            </div>
          ))}
        </div>
      </section>

      <section className="page-section">
        <h2>Previously connected</h2>
        <div className="simple-list-card">
          {knownRooms.length === 0 ? <p className="muted">Known devices will appear here after you connect.</p> : null}
          {knownRooms.map((room) => (
            <div key={room.id} className="simple-device-row">
              <div>
                <strong>{room.peer_device_name ?? bridgeMemberSummary(room).title}</strong>
                <span className="muted">{room.peer_connected ? "Available now" : `Last used ${formatTimestamp(room.created_at)}`}</span>
              </div>
              <button type="button" className={room.peer_connected ? "primary-button" : "secondary-button"} onClick={() => onOpenBridge(room)}>
                Open Bridge
              </button>
            </div>
          ))}
        </div>
      </section>

      <section className="page-section">
        <h2>Join manually</h2>
        <Card className="manual-join-card">
          <div>
            <strong>Enter an 8-digit code</strong>
            <p className="muted">Ask the other device for its code.</p>
            <p className="muted">Joining allows reviewed bounded Pastey tasks in this current Bridge session only; Burn, leave, restart, or reconnect removes that session permission.</p>
          </div>
          <div className="join-code-controls compact">
            <input
              ref={joinInputRef}
              inputMode="numeric"
              aria-label="Bridge code"
              placeholder="4829 1736"
              value={formatCode(joinCode)}
              onChange={(event) => setJoinCode(event.target.value.replace(/[^\d]/g, "").slice(0, 8))}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void handleJoinBridge();
                }
              }}
            />
            <button type="button" className="primary-button" disabled={busy !== null || joinCode.length !== 8} onClick={() => void handleJoinBridge()}>
              {busy === "join" ? "Joining..." : "Join"}
            </button>
          </div>
        </Card>
        {message ? <p className="muted">{message}</p> : null}
      </section>
    </section>
  );
}

function TargetSelector({
  peers,
  targetMode,
  selectedPeerIds,
  onModeChange,
  onSelectedPeerIdsChange,
}: {
  peers: BridgePeerSession[];
  targetMode: BridgeTargetSelectionMode;
  selectedPeerIds: string[];
  onModeChange: (mode: BridgeTargetSelectionMode) => void;
  onSelectedPeerIdsChange: (ids: string[]) => void;
}) {
  return (
    <div className="target-selector">
      <label className="field-label">
        <span>To</span>
        <select
          value={targetMode === "broadcast_bridge" ? "broadcast_bridge" : targetMode === "selected_peers" ? "selected_peers" : selectedPeerIds[0] ?? ""}
          disabled={peers.length === 0}
          onChange={(event) => {
            const value = event.target.value;
            if (value === "broadcast_bridge" || value === "selected_peers") {
              onModeChange(value);
              return;
            }
            onModeChange("selected_peer");
            onSelectedPeerIdsChange(value ? [value] : []);
          }}
        >
          {peers.map((peer) => <option key={peer.peerSessionId} value={peer.peerSessionId}>{peer.displayName}</option>)}
          {peers.length > 1 ? <option value="selected_peers">Selected devices</option> : null}
          {peers.length > 0 ? <option value="broadcast_bridge">All connected members</option> : null}
        </select>
      </label>
      {targetMode === "selected_peers" ? (
        <div className="target-checkboxes">
          {peers.map((peer) => (
            <label key={peer.peerSessionId}>
              <input
                type="checkbox"
                checked={selectedPeerIds.includes(peer.peerSessionId)}
                onChange={(event) => onSelectedPeerIdsChange(event.target.checked
                  ? [...selectedPeerIds, peer.peerSessionId]
                  : selectedPeerIds.filter((id) => id !== peer.peerSessionId))}
              />
              <span>{peer.displayName}</span>
            </label>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function useRouteablePeers(room: RoomInfo): BridgePeerSession[] {
  const cacheRef = useRef<{ identity: string; peers: BridgePeerSession[] } | null>(null);
  const identity = bridgeRoomRoutingIdentity(room);
  if (cacheRef.current?.identity === identity) return cacheRef.current.peers;
  const peers = (() => {
    try {
      return [...getRouteableBridgePeers(legacyRoomToBridgePeerCollection(room))];
    } catch {
      return [];
    }
  })();
  cacheRef.current = { identity, peers };
  return peers;
}

function bridgeRoomRoutingIdentity(room: RoomInfo): string {
  return JSON.stringify({
    id: room.id,
    status: room.status,
    localRole: room.local_role,
    peerDeviceName: room.peer_device_name ?? null,
    peerConnected: room.peer_connected,
    peerBurnedAt: room.peer_burned_at ?? null,
    peers: (room.peers ?? []).map((peer) => ({
      peerSessionId: peer.peerSessionId,
      displayName: peer.displayName ?? null,
      joinMethod: peer.joinMethod,
      liveness: peer.liveness,
      connected: peer.connected,
    })),
  });
}

function buildSelectedBridgeRoute(
  bridgeSessionId: string,
  routeablePeers: readonly BridgePeerSession[],
  targetMode: BridgeTargetSelectionMode,
  selectedPeerIds: readonly string[],
): BridgeRoute | null {
  if (targetMode === "broadcast_bridge") {
    return routeablePeers.length > 0 ? { bridgeSessionId, target: { kind: "broadcast_bridge", explicit: true } } : null;
  }
  const routeableIds = new Set(routeablePeers.map((peer) => peer.peerSessionId));
  const selectedIds = selectedPeerIds
    .map((peerId) => bridgePeerSessionId(peerId))
    .filter((peerId) => routeableIds.has(peerId));
  if (targetMode === "selected_peer") {
    const peerSessionId = selectedIds[0] ?? routeablePeers[0]?.peerSessionId;
    return peerSessionId ? { bridgeSessionId, target: { kind: "selected_peer", peerSessionId } } : null;
  }
  return selectedIds.length >= 2
    ? { bridgeSessionId, target: { kind: "selected_peers", peerSessionIds: selectedIds } }
    : null;
}

function resolvedPeersForRoute(route: BridgeRoute, routeablePeers: readonly BridgePeerSession[]): BridgePeerSession[] {
  if (route.target.kind === "broadcast_bridge") return [...routeablePeers];
  if (route.target.kind === "selected_peer") {
    const peer = findBridgePeerBySessionId({ bridgeSessionId: route.bridgeSessionId, peers: routeablePeers }, route.target.peerSessionId);
    return peer ? [peer] : [];
  }
  return route.target.peerSessionIds
    .map((peerSessionId) => findBridgePeerBySessionId({ bridgeSessionId: route.bridgeSessionId, peers: routeablePeers }, peerSessionId))
    .filter((peer): peer is BridgePeerSession => Boolean(peer));
}

function transferInputsForSelectedRoute(
  inputs: TransferQueueInput[],
  selectedBridgeRoute: BridgeRoute,
  selectedRoutePeers: readonly BridgePeerSession[],
  bridgeId: string,
  contentKind: "file" | "image" | "pasted_image",
): TransferQueueInput[] {
  const operationId = `bridge-queue:${bridgeId}:${Date.now()}:${Math.random().toString(36).slice(2)}`;
  return inputs.flatMap((input) => selectedRoutePeers.map((peer) => ({
    ...input,
    bridgeRoute: {
      bridgeSessionId: selectedBridgeRoute.bridgeSessionId,
      target: { kind: "selected_peer", peerSessionId: peer.peerSessionId },
    },
    bridgeOperationId: operationId,
    bridgeTargetKind: selectedBridgeRoute.target.kind,
    bridgeContentKind: contentKind,
    targetPeerSessionId: peer.peerSessionId,
    targetPeerDisplayName: peer.displayName,
    targetCount: selectedRoutePeers.length,
  })));
}

interface ActivityListRow {
  id: string;
  group: "now" | "pending" | "received" | "sent" | "failed";
  title: string;
  detail: string;
  bridge: string;
  status: string;
  tone: "success" | "neutral" | "warning" | "danger";
  progress?: number;
  savedPath?: string | null;
  previewText?: string;
  fullText?: string;
  copyLabel?: string;
}

function recentActivityRows(room: RoomInfo, items: RoomItem[], transfers: FileTransferProgressEvent[], queueItems: TransferQueueItem[]): ActivityListRow[] {
  return activityRows([room], items.filter((item) => item.room_id === room.id), transfers.filter((transfer) => transfer.room_id === room.id), queueItems.filter((item) => item.roomId === room.id));
}

function activityRows(
  rooms: RoomInfo[],
  roomItems: RoomItem[],
  transfers: FileTransferProgressEvent[],
  queueItems: TransferQueueItem[],
): ActivityListRow[] {
  const roomById = new Map(rooms.map((room) => [room.id, room]));
  const transferRows = transfers.map((transfer): ActivityListRow => ({
    id: `transfer:${transfer.transfer_id}`,
    group: transfer.status === "failed" ? "failed" : transfer.status === "completed" ? transfer.direction === "incoming" ? "received" : "sent" : "now",
    title: transfer.direction === "incoming" ? `Receiving ${transfer.file_name}` : `Sending ${transfer.file_name}`,
    detail: transfer.direction === "incoming" ? "From device" : "To device",
    bridge: bridgeCode(roomById.get(transfer.room_id)),
    status: transferStatusLabel(transfer.status, transfer.direction),
    tone: transfer.status === "failed" ? "danger" : transfer.status === "completed" ? "success" : "neutral",
    progress: transfer.file_size > 0 ? Math.min(100, Math.round((transfer.transferred_bytes / transfer.file_size) * 100)) : undefined,
  }));
  const queueRows = queueItems
    .filter((item) => item.status !== "completed")
    .map((item): ActivityListRow => ({
      id: `queue:${item.id}`,
      group: item.status === "failed" ? "failed" : item.status === "queued" || item.status === "preparing" ? "pending" : "now",
      title: item.targetPeerDisplayName ? `Waiting for ${item.targetPeerDisplayName}` : queueItemTitle(item),
      detail: item.displayName ?? fileNameFromPath(item.path),
      bridge: bridgeCode(roomById.get(item.roomId)),
      status: queueStatusLabel(item.status),
      tone: item.status === "failed" ? "danger" : item.status === "queued" || item.status === "preparing" ? "warning" : "neutral",
    }));
  const itemRows = roomItems.map((item): ActivityListRow => {
    const fullText = item.payload_type === "text" ? item.text ?? "" : "";
    return {
      id: `item:${item.id}`,
      group: item.status === "failed" ? "failed" : item.direction === "incoming" ? "received" : "sent",
      title: item.direction === "incoming" ? `You received ${itemTitle(item)}` : `You sent ${itemTitle(item)}`,
      detail: item.direction === "incoming" ? "From device" : "To device",
      bridge: bridgeCode(roomById.get(item.room_id)),
      status: roomItemStatusLabel(item.status),
      tone: item.status === "failed" ? "danger" : "success",
      savedPath: item.saved_path,
      previewText: fullText ? contentPreview(fullText) : undefined,
      fullText: fullText || undefined,
      copyLabel: fullText ? "Copy full text" : undefined,
    };
  });
  return [...transferRows, ...queueRows, ...itemRows].sort((a, b) => a.id < b.id ? 1 : -1);
}

function ActivityRow({ row, compact = false }: { row: ActivityListRow; compact?: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const hasFullText = Boolean(row.fullText);
  const previewText = row.previewText ?? row.fullText ?? "";
  const fullText = row.fullText ?? "";
  const canExpand = hasFullText && previewText !== fullText;
  return (
    <article className={`activity-row ${compact ? "compact" : ""}`}>
      <div>
        <strong>{row.title}</strong>
        <span className="muted">{row.detail} - Bridge {row.bridge}</span>
        {hasFullText ? (
          <pre className={`activity-full-text ${expanded ? "expanded" : ""}`}>
            {expanded ? fullText : previewText}
          </pre>
        ) : null}
        {hasFullText ? (
          <div className="button-row activity-content-actions">
            {canExpand ? (
              <button type="button" className="text-button" onClick={() => setExpanded((current) => !current)}>
                {expanded ? "View preview" : "View full"}
              </button>
            ) : null}
            <button type="button" className="text-button" onClick={() => void copyTextToClipboard(fullText)}>
              {row.copyLabel ?? "Copy"}
            </button>
          </div>
        ) : null}
        {typeof row.progress === "number" ? (
          <div className="activity-progress" aria-label={`${row.progress}%`}>
            <span style={{ width: `${row.progress}%` }} />
          </div>
        ) : null}
      </div>
      <div className="activity-row-actions">
        <StatusChip tone={row.tone}>{row.status}</StatusChip>
        {row.savedPath ? (
          <button type="button" className="secondary-button compact-button" onClick={() => void revealInFolder(row.savedPath as string)}>
            Reveal
          </button>
        ) : null}
      </div>
    </article>
  );
}

function ProductHeader({ title, subtitle, action }: { title: string; subtitle: string; action?: ReactNode }) {
  return (
    <header className="product-header">
      <div>
        <h1>{title}</h1>
        <p>{subtitle}</p>
      </div>
      {action}
    </header>
  );
}

function Card({ className = "", children }: { className?: string; children: ReactNode }) {
  return <section className={`summary-card ${className}`.trim()}>{children}</section>;
}

function StatusChip({ tone, children }: { tone: "success" | "neutral" | "warning" | "danger"; children: ReactNode }) {
  return <span className={`status-chip ${tone}`}>{children}</span>;
}

function FullValue({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <strong>{label}</strong>
      <span title={value}>{value}</span>
    </div>
  );
}

function MemberChip({ title, subtitle, you = false }: { title: string; subtitle: string; you?: boolean }) {
  return (
    <article className="member-chip">
      <span className="member-device-icon" aria-hidden="true" />
      <div>
        <strong>{title}</strong>
        <span className="muted">{subtitle}</span>
      </div>
      {you ? <StatusChip tone="neutral">You</StatusChip> : null}
    </article>
  );
}

function bridgeCode(room?: RoomInfo | null): string {
  return room ? formatCode(room.room_code_display ?? room.room_code ?? room.id.slice(0, 8)) : "Unknown";
}

function bridgeSessionId(room: RoomInfo): string {
  try {
    return legacyRoomToBridgePeerCollection(room).bridgeSessionId;
  } catch {
    return `legacy-room:${room.id}`;
  }
}

function bridgeStatus(room: RoomInfo): { label: string; tone: "success" | "neutral" | "warning" | "danger" } {
  if (room.status === "burned") return { label: "Offline", tone: "neutral" };
  if (room.status === "peer_left") return { label: "Peer left", tone: "neutral" };
  if (room.peer_connected) return { label: "Connected", tone: "success" };
  return { label: "Waiting for peer", tone: "warning" };
}

function bridgeMemberSummary(room: RoomInfo): { title: string; detail: string } {
  const remoteNames = connectedRemoteMembers(room).map((peer) => peer.displayName?.trim()).filter(Boolean) as string[];
  if (remoteNames.length === 0 && room.peer_device_name) {
    return { title: `${room.peer_device_name} - 1 member`, detail: "Recent" };
  }
  if (remoteNames.length <= 1) {
    return { title: `${remoteNames[0] ?? "No devices yet"} - ${remoteNames.length || 0} member${remoteNames.length === 1 ? "" : "s"}`, detail: remoteNames.length ? "Current Bridge member" : "Waiting for a device" };
  }
  const shown = remoteNames.slice(0, 2).join(", ");
  const extra = remoteNames.length > 2 ? `, +${remoteNames.length - 2} more` : "";
  return { title: `${remoteNames.length} members - ${shown}${extra}`, detail: "Current Bridge members" };
}

function connectedRemoteMembers(room: RoomInfo) {
  return (room.peers ?? []).filter((peer) => peer.connected && peer.liveness === "connected");
}

function bridgeSubtitle(room: RoomInfo): string {
  const members = connectedRemoteMembers(room);
  if (members.length === 1) return `Connected to ${members[0].displayName ?? room.peer_device_name ?? "device"}`;
  if (members.length > 1) return `${members.length} members connected`;
  return room.peer_device_name ? `Recent device: ${room.peer_device_name}` : "Waiting for another device";
}

function targetSummary(route: BridgeRoute | null, peers: BridgePeerSession[]): string {
  if (!route || peers.length === 0) return "To: choose a connected device";
  if (route.target.kind === "broadcast_bridge") return "To: all connected members";
  if (route.target.kind === "selected_peers") return `To: ${peers.length} selected devices`;
  return `To: ${peers[0]?.displayName ?? "selected device"}`;
}

function localDeviceLabel(profile: DeviceProfile | null): string {
  const platform = normalizePlatform(profile?.platform);
  if (platform === "macos") return "This Mac";
  if (platform === "linux") return "This Linux device";
  if (platform === "windows") return "This Windows device";
  return "This device";
}

function localDeviceSubtitle(profile: DeviceProfile | null): string {
  if (!profile) return "This device";
  return [formatPlatformLabel(profile.platform), profile.arch].filter(Boolean).join(" · ") || "This device";
}

function remotePeerDisplayName(peer: BridgePeerSession, room: RoomInfo): string {
  const label = peer.displayName?.trim() || room.peer_device_name?.trim();
  return label && !isLocalOnlyDeviceLabel(label) ? label : "Nearby device";
}

function remotePeerSubtitle(peer: BridgePeerSession): string {
  return peer.liveness === "connected" ? "Connected" : peer.liveness || "Current session";
}

function nearbyDeviceSystemSummary(device: NearbyDevice): string {
  const parts = [
    formatPlatformLabel(device.platform) ?? "Nearby device",
    device.app_version ? `Pastey ${device.app_version}` : null,
    device.last_seen_seconds_ago <= 3 ? "Online" : `Last seen ${Math.max(0, Math.round(device.last_seen_seconds_ago))}s ago`,
  ];
  return parts.filter(Boolean).join(" · ");
}

function normalizePlatform(value?: string | null): "macos" | "linux" | "windows" | "unknown" {
  const normalized = value?.trim().toLowerCase() ?? "";
  if (normalized === "macos" || normalized === "darwin" || normalized === "mac") return "macos";
  if (normalized === "linux") return "linux";
  if (normalized === "windows" || normalized === "win32") return "windows";
  return "unknown";
}

function formatPlatformLabel(value?: string | null): string | null {
  const platform = normalizePlatform(value);
  if (platform === "macos") return "macOS";
  if (platform === "linux") return "Linux";
  if (platform === "windows") return "Windows";
  const raw = value?.trim();
  return raw || null;
}

function isLocalOnlyDeviceLabel(label: string): boolean {
  return /^this (mac|linux device|windows device|device)$/i.test(label.trim());
}

function lastActivityForBridge(room: RoomInfo, items: RoomItem[], queueItems: TransferQueueItem[]): string {
  const latestItem = items.filter((item) => item.room_id === room.id).sort((a, b) => b.created_at - a.created_at)[0];
  const latestQueue = queueItems.filter((item) => item.roomId === room.id).sort((a, b) => b.updatedAt - a.updatedAt)[0];
  const latest = Math.max(latestItem?.created_at ?? 0, latestQueue?.updatedAt ?? 0, room.created_at);
  return latest ? formatTimestamp(latest) : "Recent";
}

function itemTitle(item: RoomItem): string {
  if (item.display_name?.trim()) return item.display_name;
  if (item.text?.trim()) return contentPreview(item.text, 80);
  return item.payload_type === "text" ? "text" : "file";
}

function contentPreview(value: string, limit = 160): string {
  const trimmed = value.trim();
  if (trimmed.length <= limit) return trimmed;
  return `${trimmed.slice(0, limit)}...`;
}

function queueItemTitle(item: TransferQueueItem): string {
  return item.status === "queued" || item.status === "preparing" ? `Waiting to send ${item.displayName ?? fileNameFromPath(item.path)}` : `Sending ${item.displayName ?? fileNameFromPath(item.path)}`;
}

function fileNameFromPath(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

function transferStatusLabel(status: FileTransferProgressEvent["status"], direction: FileTransferProgressEvent["direction"]): string {
  if (status === "transferring") return direction === "incoming" ? "Receiving" : "Sending";
  if (status === "completed") return direction === "incoming" ? "Received" : "Sent";
  if (status === "failed") return "Failed";
  if (status === "cancelled") return "Cancelled";
  return "Waiting";
}

function queueStatusLabel(status: TransferQueueItem["status"]): string {
  if (status === "queued" || status === "preparing") return "Waiting";
  if (status === "sending") return "Sending";
  if (status === "failed") return "Failed";
  if (status === "cancelled") return "Cancelled";
  return "Sent";
}

function roomItemStatusLabel(status: RoomItem["status"]): string {
  if (status === "received") return "Received";
  if (status === "sent" || status === "created") return "Sent";
  if (status === "failed") return "Failed";
  if (status === "cancelled") return "Cancelled";
  return "Waiting";
}

function networkHelpMessage(message: string): string {
  if (message.toLowerCase().includes("timed out") || message.toLowerCase().includes("timeout")) {
    return "That device did not respond. Make sure Pastey is open on both devices.";
  }
  return message;
}

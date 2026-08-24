import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { lazy, Suspense, useEffect, useMemo, useRef, useState, type ClipboardEvent, type ReactNode } from "react";
import {
  copyTextToClipboard,
  acceptDeveloperTerminal,
  approveBridgePlan,
  createComposedFileBridgePlan,
  createDirectFileTransferBridgePlan,
  getDeviceProfile,
  getDeveloperTerminalWorkspace,
  getRoomControlSessionContext,
  joinRoom,
  listBridgePlanWorkspace,
  listReceivedRoomControlEvents,
  listNearbyDevices,
  requestNearbyJoin,
  requestDeveloperTerminal,
  resizeDeveloperTerminal,
  revealInFolder,
  bindBridgePlanToSession,
  startBridgePlanAttempt,
  selectBridgePlanSearchCandidate,
  sendTextToRoom,
  sendDeveloperTerminalInput,
  closeDeveloperTerminal,
  denyDeveloperTerminal,
  enterDeveloperMode,
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
  dependencyError,
  insertRequiredTransfer,
  manualBridgePlanInput,
  moveBlock,
  newSearchBlock,
  removeBlock,
  requiredTransferForConsumer,
  updateSearchBlock,
  type ComposerBlock,
  type ComposerDevice,
  type SafeSearchScope,
} from "../lib/bridgePlanComposer";
import { FILE_TOO_LARGE_MESSAGE, MAX_FILE_SIZE_BYTES } from "../lib/constants";
import {
  OrderedTerminalInputWriter,
  TerminalInputBackpressureError,
} from "../lib/developerTerminalFrontend";
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
  DeveloperModeUiSession,
  DeveloperTerminalSession,
  DeveloperTerminalWorkspace,
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
const DeveloperTerminalViewport = lazy(() => import("../components/DeveloperTerminalViewport"));

function bridgePlanControlErrorMessage(error: unknown): string {
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
    return "This approved plan binding expired before the selected device could accept it. Create a new plan and send it again.";
  }
  if (message.includes("review_session_mismatch")) {
    return "The selected device reconnected or the Bridge session changed. Refresh the Bridge, select its current session, and create a new plan.";
  }
  if (message.includes("review_revision_hash_mismatch") || message.includes("review_step_digest_mismatch")) {
    return "The selected device rejected a mismatched immutable plan. Refresh the Bridge and create a new plan.";
  }
  if (message.includes("review_unknown_approval")) {
    return "The selected device no longer has this approved plan binding. Create a new plan and send it again.";
  }
  if (message.includes("review_payload_invalid")) {
    return "The selected device rejected an invalid plan binding. Refresh the Bridge and create a new plan.";
  }
  if (message.includes("event validation failed") || message.includes("Bridge Plan review not found") || message.includes("remote plan binding")) {
    return "The selected device could not validate this approved plan binding. Refresh the Bridge and create a new plan.";
  }
  return "Pastey could not send the approved plan binding. Refresh the Bridge and try again.";
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
  const [developerTerminalWorkspace, setDeveloperTerminalWorkspace] = useState<DeveloperTerminalWorkspace>({ pendingRequests: [], sessions: [] });
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
      setDeveloperTerminalWorkspace({ pendingRequests: [], sessions: [] });
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
      const [events, terminalWorkspace] = await Promise.all([
        listReceivedRoomControlEvents(currentSession.roomId),
        getDeveloperTerminalWorkspace(currentSession.roomId),
      ]);
      setDeveloperTerminalWorkspace(terminalWorkspace);
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

      <DeveloperModePanel
        room={room}
        peers={remotePeers}
        workspace={developerTerminalWorkspace}
        onWorkspaceChange={setDeveloperTerminalWorkspace}
      />

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

function DeveloperModePanel({
  room,
  peers,
  workspace,
  onWorkspaceChange,
}: {
  room: RoomInfo;
  peers: readonly BridgePeerSession[];
  workspace: DeveloperTerminalWorkspace;
  onWorkspaceChange: (workspace: DeveloperTerminalWorkspace) => void;
}) {
  const [uiSession, setUiSession] = useState<DeveloperModeUiSession | null>(null);
  const [selectedPeerId, setSelectedPeerId] = useState<string>(peers[0]?.peerSessionId ?? "");
  const [error, setError] = useState<string | null>(null);
  const terminalInputWriterRef = useRef<OrderedTerminalInputWriter | null>(null);
  const activeController = workspace.sessions.find(
    (session) => session.role === "controller" && session.state === "active",
  );
  const currentController = activeController ?? workspace.sessions.find(
    (session) => session.role === "controller" && session.state === "awaiting_admission",
  );
  const controlledHostName = peers.find(
    (peer) => peer.peerSessionId === selectedPeerId,
  )?.displayName ?? "Current linked Host";

  useEffect(() => {
    if (currentController) return;
    if (!peers.some((peer) => peer.peerSessionId === selectedPeerId)) {
      setSelectedPeerId(peers[0]?.peerSessionId ?? "");
    }
  }, [currentController, peers, selectedPeerId]);

  useEffect(() => {
    terminalInputWriterRef.current?.cancel();
    terminalInputWriterRef.current = null;
    if (!activeController || !uiSession) return;
    const writer = new OrderedTerminalInputWriter(
      (frame) => sendDeveloperTerminalInput(
        activeController.terminalSessionId,
        uiSession.token,
        frame,
      ),
      (cause) => setError(cause instanceof Error ? cause.message : String(cause)),
    );
    terminalInputWriterRef.current = writer;
    return () => {
      writer.cancel();
      if (terminalInputWriterRef.current === writer) terminalInputWriterRef.current = null;
    };
  }, [activeController?.terminalSessionId, uiSession?.token]);

  async function activate() {
    setError(null);
    try {
      setUiSession(await enterDeveloperMode(room.id));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function requestOpen() {
    if (!uiSession || !selectedPeerId) return;
    setError(null);
    try {
      onWorkspaceChange(await requestDeveloperTerminal(room.id, selectedPeerId, uiSession.token));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function decide(sessionId: string, accepted: boolean) {
    if (!uiSession) return;
    setError(null);
    try {
      if (accepted) {
        await acceptDeveloperTerminal(room.id, sessionId, uiSession.token, 100, 30);
      } else {
        await denyDeveloperTerminal(sessionId, uiSession.token);
      }
      onWorkspaceChange(await getDeveloperTerminalWorkspace(room.id));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function close(session: DeveloperTerminalSession) {
    if (!uiSession) return;
    terminalInputWriterRef.current?.cancel();
    setError(null);
    try {
      await closeDeveloperTerminal(session.terminalSessionId, uiSession.token);
      onWorkspaceChange(await getDeveloperTerminalWorkspace(room.id));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function sendTerminalInput(bytes: number[]) {
    if (!terminalInputWriterRef.current) return;
    try {
      terminalInputWriterRef.current.enqueue(bytes);
    } catch (cause) {
      setError(cause instanceof TerminalInputBackpressureError
        ? cause.message
        : cause instanceof Error ? cause.message : String(cause));
    }
  }

  function sendTerminalResize(cols: number, rows: number) {
    if (!activeController || !uiSession) return;
    void resizeDeveloperTerminal(activeController.terminalSessionId, uiSession.token, cols, rows)
      .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)));
  }

  return (
    <Card className="developer-mode-card" aria-label="Developer Mode">
      <div className="section-row">
        <div>
          <h2>Developer Mode</h2>
          <p className="muted">Broad manual control of one current linked Host. This is separate from Ask Bridge and Agent authority.</p>
        </div>
        {!uiSession ? (
          <button type="button" className="danger-button" onClick={() => void activate()}>Enter Developer Mode</button>
        ) : <StatusChip tone="warning">Human-authorized</StatusChip>}
      </div>
      <p className="developer-warning">Commands run with the Pastey Host user's normal privileges. Terminal effects do not create managed revisions.</p>
      {uiSession && !currentController ? (
        <div className="developer-controls">
          <select value={selectedPeerId} onChange={(event) => setSelectedPeerId(event.target.value)} aria-label="Developer terminal target Host">
            {peers.map((peer) => <option key={peer.peerSessionId} value={peer.peerSessionId}>{peer.displayName}</option>)}
          </select>
          <button type="button" className="secondary-button" disabled={!selectedPeerId} onClick={() => void requestOpen()}>Request terminal</button>
        </div>
      ) : null}
      {workspace.pendingRequests.map((pending) => (
        <div key={pending.terminalSessionId} className="developer-admission">
          <div><strong>Terminal access requested</strong><p className="muted">A current Bridge peer requests broad manual shell control.</p></div>
          <div className="button-row">
            <button type="button" className="secondary-button" disabled={!uiSession} onClick={() => void decide(pending.terminalSessionId, false)}>Deny</button>
            <button type="button" className="danger-button" disabled={!uiSession} onClick={() => void decide(pending.terminalSessionId, true)}>Allow terminal</button>
          </div>
        </div>
      ))}
      {currentController ? (
        <div key={currentController.terminalSessionId} className="developer-terminal-session">
          <div className="section-row">
            <div className="developer-terminal-identity">
              <p><strong>Connected Host: {controlledHostName}</strong></p>
              <p className="muted">Shell: {currentController.environmentLabel ?? "Waiting for Host"} · Status: {currentController.state.replace(/_/g, " ")}</p>
            </div>
            <button type="button" className="danger-button" onClick={() => void close(currentController)}>Close</button>
          </div>
          {currentController.state === "active" ? (
            <Suspense fallback={<div className="developer-terminal-waiting">Opening terminal emulator…</div>}>
              <DeveloperTerminalViewport
                roomId={room.id}
                terminalSessionId={currentController.terminalSessionId}
                environmentLabel={currentController.environmentLabel}
                output={currentController.output}
                outputSequence={currentController.outputSequence}
                onInput={sendTerminalInput}
                onResize={sendTerminalResize}
              />
            </Suspense>
          ) : (
            <div className="developer-terminal-waiting">Waiting for the remote Host to allow this terminal…</div>
          )}
        </div>
      ) : null}
      {error ? <p className="danger-text">{error}</p> : null}
    </Card>
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
  const [revisionId, setRevisionId] = useState<string | null>(null);
  const [approvalId, setApprovalId] = useState<string | null>(null);
  const [attemptId, setAttemptId] = useState<string | null>(null);
  const [selectedCandidateId, setSelectedCandidateId] = useState<string | null>(null);
  const [approvalState, setApprovalState] = useState<string | null>(null);
  const [resultSummary, setResultSummary] = useState<string | null>(null);
  const [directTransfer, setDirectTransfer] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState<"plan" | "review" | "start" | null>(null);

  const localHostLabel = localDeviceProfile?.device_name?.trim() || localDeviceLabel(localDeviceProfile);
  const selectedHostLabel = selectedPeer?.displayName ?? "Selected device";
  const selectedPeerRoute = route?.target.kind === "selected_peer" ? route : null;
  const canPlan = enabled && Boolean(selectedPeer && selectedPeerRoute);
  const composerDeviceLabel = (device: ComposerDevice) => device === "requesting_device" ? localHostLabel : selectedHostLabel;
  const candidateMode = bridgePlanSearchCandidateMode(blocks.map((step) => step.primitive));
  const safeSearchCandidates = useMemo(
    () => inboxEvents.flatMap(parseBridgePlanSearchCandidates).filter((entry) => !attemptId || entry.attemptId === attemptId),
    [attemptId, inboxEvents],
  );
  const terminalSearchResult = useMemo(
    () => inboxEvents.flatMap((event) => {
      const result = parseBridgePlanSearchTerminalResult(event);
      return result ? [result] : [];
    }).find((entry) => entry.attemptId === attemptId) ?? null,
    [attemptId, inboxEvents],
  );

  useEffect(() => {
    if (revisionId && !approvalId) void withdrawBridgePlanRevision(room.id, revisionId).catch(() => undefined);
    setBlocks([newSearchBlock()]);
    setRevisionId(null);
    setApprovalId(null);
    setAttemptId(null);
    setSelectedCandidateId(null);
    setApprovalState(null);
    setResultSummary(null);
    setDirectTransfer(false);
    setMessage(null);
  }, [room.id, selectedPeer?.peerSessionId]);

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
        const attempts = workspace.attempts
          .map(parseBridgePlanAttempt)
          .filter((entry): entry is { approvalId: string; attemptId: string } => entry?.approvalId === approvalId);
        const latest = attempts[attempts.length - 1]?.attemptId ?? null;
        if (latest) setAttemptId(latest);
        const attemptIds = new Set(attempts.map((entry) => entry.attemptId));
        const result = workspace.results
          .map(parseBridgePlanResult)
          .find((entry): entry is { attemptId: string; summary: string } => Boolean(entry && attemptIds.has(entry.attemptId)));
        if (result) setResultSummary(result.summary);
      } catch (error) {
        if (!cancelled) setMessage(error instanceof Error ? error.message : "Could not refresh the plan status.");
      }
    };
    const timer = window.setInterval(() => void refresh(), 2_000);
    void refresh();
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [approvalId, room.id]);

  useEffect(() => {
    if (!terminalSearchResult) return;
    const presentation = terminalSearchPresentation(terminalSearchResult);
    setApprovalState(presentation.status);
    setResultSummary(presentation.summary);
  }, [terminalSearchResult]);

  function editBlocks(next: ComposerBlock[]) {
    if (approvalId) {
      setMessage("This immutable revision has already been approved. Create a new Plan to change its semantics.");
      return;
    }
    if (revisionId) {
      void withdrawBridgePlanRevision(room.id, revisionId).catch(() => undefined);
      setRevisionId(null);
      setMessage("Plan semantics changed. Review creates a new immutable revision.");
    }
    setBlocks(next);
    const error = dependencyError(next);
    if (error) setMessage(error);
  }

  async function createPlan() {
    if (!canPlan || !selectedPeerRoute) {
      setMessage(BRIDGE_PLAN_REQUIRES_ONE_SELECTED_DEVICE);
      return;
    }
    const composed = manualBridgePlanInput(blocks);
    if (!composed.value) {
      setMessage(composed.error ?? "Complete the Plan fields.");
      return;
    }
    setBusy("plan");
    setMessage(null);
    try {
      const workspace = await createComposedFileBridgePlan({
        roomId: room.id,
        originalUserGoal: composed.value.originalUserGoal,
        blocks: composed.value.blocks,
      });
      const revision = workspace.revisions
        .map(parseBridgePlanRevision)
        .filter((entry): entry is { revisionId: string; state: string } => entry?.state === "available")
        .pop();
      if (!revision) throw new Error("Pastey did not return the immutable Plan revision.");
      setRevisionId(revision.revisionId);
      setApprovalId(null);
      setAttemptId(null);
      setApprovalState(null);
      setResultSummary(null);
      setSelectedCandidateId(null);
      setDirectTransfer(false);
      const frameworkOnly = blocks.some((step) => step.primitive === "Transform" || step.primitive === "Execute");
      setMessage(frameworkOnly
        ? "Plan ready. Transform and Execute intent are reviewable framework steps; execution is not yet available."
        : "Plan ready. Review the complete Search and Transfer flow.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not create the Plan.");
    } finally {
      setBusy(null);
    }
  }

  async function reviewAndRun() {
    if (!revisionId || !selectedPeerRoute) return;
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
      setMessage("Approved Plan started. Pastey will continue executable Search and Transfer steps automatically.");
    } catch (error) {
      setApprovalState("execution unavailable");
      setMessage(bridgePlanControlErrorMessage(error));
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
      const revision = workspace.revisions
        .map(parseBridgePlanRevision)
        .filter((entry): entry is { revisionId: string; state: string } => entry?.state === "available")
        .pop();
      if (!revision) throw new Error("Pastey did not return the direct Transfer Plan.");
      setBlocks([{ primitive: "Transfer", source: "requesting_device", destination: "selected_device", landingMode: "final_delivery" }]);
      setRevisionId(revision.revisionId);
      setApprovalId(null);
      setAttemptId(null);
      setApprovalState(null);
      setSelectedCandidateId(null);
      setDirectTransfer(true);
      setMessage("Plan ready. Review the explicit Transfer.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not create the direct Transfer Plan.");
    } finally {
      setBusy(null);
    }
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
      setMessage("Candidate selected. Pastey is continuing the approved executable flow.");
    } catch (error) {
      setMessage(bridgePlanControlErrorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  return (
    <Card className="ask-bridge-card" aria-label="Ask Bridge Composer">
      <div className="section-row">
        <div>
          <h2>Block Composer</h2>
          <p className="muted">Search finds. Transform records modification intent. Transfer moves. Execute records execution intent.</p>
        </div>
        <button type="button" className="secondary-button" disabled={!canPlan || busy !== null || Boolean(approvalId)} onClick={() => void createDirectTransferPlan()}>
          Direct Transfer
        </button>
      </div>
      <p className="muted">Transform and Execute are framework-defined but not executable until the future Agent layer exists. Pastey never inserts hidden movement.</p>
      <div className="button-row" aria-label="Add Plan primitive">
        {(["Search", "Transform", "Transfer", "Execute"] as const).map((primitive) => (
          <button
            key={primitive}
            type="button"
            className="secondary-button"
            disabled={Boolean(approvalId) || !canAddPrimitive(blocks, primitive)}
            onClick={() => {
              const next = addPrimitive(blocks, primitive);
              if (next.error) setMessage(next.error);
              else editBlocks(next.blocks);
            }}
          >
            + {primitive}
          </button>
        ))}
      </div>

      <div className="bridge-plan-block-list">
        {blocks.map((block, index) => (
          <section className="bridge-plan-block" key={`${block.primitive}-${index}`}>
            <div className="section-row">
              <h3>{index + 1}. {block.primitive}</h3>
              <div className="button-row">
                <button type="button" className="text-button" disabled={index === 0 || Boolean(approvalId)} onClick={() => { const next = moveBlock(blocks, index, index - 1); if (next.error) setMessage(next.error); else editBlocks(next.blocks); }}>Up</button>
                <button type="button" className="text-button" disabled={index === blocks.length - 1 || Boolean(approvalId)} onClick={() => { const next = moveBlock(blocks, index, index + 1); if (next.error) setMessage(next.error); else editBlocks(next.blocks); }}>Down</button>
                <button type="button" className="text-button" disabled={Boolean(approvalId)} onClick={() => { const next = removeBlock(blocks, index); if (next.error) setMessage(next.error); else editBlocks(next.blocks); }}>Remove</button>
              </div>
            </div>

            {block.primitive === "Search" ? (
              <div className="bridge-plan-block-fields">
                <p>Find an object on <strong>{selectedHostLabel}</strong>. Search does not modify or move it.</p>
                <label>Locations<select multiple aria-label="Search locations" value={block.safeScopes} onChange={(event) => {
                  const safeScopes = [...event.currentTarget.selectedOptions].map((option) => option.value as SafeSearchScope);
                  editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Search" ? updateSearchBlock(entry, { safeScopes }) : entry));
                }}>{SAFE_SEARCH_SCOPES.map((scope) => <option key={scope.value} value={scope.value}>{scope.label}</option>)}</select></label>
                <label>File name or description<input aria-label="Search filename" value={block.filenameHint} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Search" ? updateSearchBlock(entry, { filenameHint: event.target.value }) : entry))} /></label>
                <label>Optional extension<input aria-label="Search extension" value={block.extension} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Search" ? updateSearchBlock(entry, { extension: event.target.value }) : entry))} /></label>
              </div>
            ) : null}

            {block.primitive === "Transform" ? (
              <div className="bridge-plan-block-fields">
                <p><strong>Modify the selected object.</strong> The intent advances the same logical object from revision {block.targetRevision.revision} to {block.targetRevision.revision + 1} without moving it.</p>
                <label>Modify on<select aria-label="Transform execution device" value={block.executionDevice} disabled={Boolean(approvalId)} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Transform" ? { ...entry, executionDevice: event.target.value as ComposerDevice } : entry))}><option value="requesting_device">{localHostLabel}</option><option value="selected_device">{selectedHostLabel}</option></select></label>
                {requiredTransferForConsumer(blocks, index) ? <button type="button" className="secondary-button" onClick={() => { const next = insertRequiredTransfer(blocks, index); editBlocks(next.blocks); setMessage(next.error ?? "Explicit PipelinePrivate Transfer inserted for review."); }}>Insert required Transfer</button> : null}
                <label>Modification intent<textarea aria-label="Modification intent" maxLength={1024} value={block.modificationIntent} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Transform" ? { ...entry, modificationIntent: event.target.value } : entry))} placeholder="Change the retry behavior to use exponential backoff." /></label>
                <p className="muted">Framework defined · execution not yet available. No patch format or mutation worker is selected by Pastey Core.</p>
              </div>
            ) : null}

            {block.primitive === "Transfer" ? (
              <div className="bridge-plan-block-fields">
                <p>From: <strong>{composerDeviceLabel(block.source)}</strong></p>
                <label>Transfer mode<select aria-label="Transfer landing mode" value={block.landingMode} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Transfer" ? { ...entry, landingMode: event.target.value as "pipeline_handoff" | "final_delivery" } : entry))}><option value="final_delivery">Final delivery</option><option value="pipeline_handoff">Private pipeline handoff</option></select></label>
                <label>Move to<select aria-label="Transfer destination" value={block.destination} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Transfer" ? { ...entry, destination: event.target.value as "requesting_device" | "selected_device" | "pastey_shared" } : entry))}><option value="requesting_device">{localHostLabel}</option><option value="selected_device">{selectedHostLabel}</option>{block.landingMode === "final_delivery" && block.source === "selected_device" ? <option value="pastey_shared">Pastey Shared on {selectedHostLabel}</option> : null}</select></label>
                <p className="muted">PipelinePrivate is used only for this explicit intermediate Transfer.</p>
              </div>
            ) : null}

            {block.primitive === "Execute" ? (
              <div className="bridge-plan-block-fields">
                <p><strong>Authorize execution intent for logical revision {block.targetRevision.revision}.</strong> Execute does not move the object or select a runtime.</p>
                <label>Execute on<select aria-label="Execute device" value={block.executionDevice} disabled={Boolean(approvalId)} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Execute" ? { ...entry, executionDevice: event.target.value as ComposerDevice } : entry))}><option value="requesting_device">{localHostLabel}</option><option value="selected_device">{selectedHostLabel}</option></select></label>
                {requiredTransferForConsumer(blocks, index) ? <button type="button" className="secondary-button" onClick={() => { const next = insertRequiredTransfer(blocks, index); editBlocks(next.blocks); setMessage(next.error ?? "Explicit PipelinePrivate Transfer inserted for review."); }}>Insert required Transfer</button> : null}
                <label>Execution intent<textarea aria-label="Execution intent" maxLength={1024} value={block.executionIntent} onChange={(event) => editBlocks(blocks.map((entry, current) => current === index && entry.primitive === "Execute" ? { ...entry, executionIntent: event.target.value } : entry))} placeholder="Run or validate the modified object and report the result." /></label>
                <p className="danger-text"><strong>Framework defined · execution not yet available.</strong> Pastey Core does not choose a runtime, shell, process, or containment policy.</p>
              </div>
            ) : null}
          </section>
        ))}
      </div>

      <button type="button" className="primary-button" disabled={!canPlan || busy !== null || directTransfer || Boolean(approvalId)} onClick={() => void createPlan()}>
        {busy === "plan" ? "Building…" : "Review plan"}
      </button>

      {revisionId ? (
        <div className="request-file-preview" data-testid="ask-bridge-plan-preview">
          <h3>Review plan</h3>
          {blocks.map((step, index) => {
            if (step.primitive === "Search") return <p key={index}><strong>Search @ {composerDeviceLabel(step.executionDevice)}</strong><br />{step.filenameHint}</p>;
            if (step.primitive === "Transform") return <p key={index}><strong>Transform @ {composerDeviceLabel(step.executionDevice)}</strong><br />Modify selected object revision {step.targetRevision.revision} → {step.targetRevision.revision + 1}<br />Intent: “{step.modificationIntent}”<br /><span className="muted">Execution unavailable until Agent integration.</span></p>;
            if (step.primitive === "Execute") return <p key={index}><strong>Execute @ {composerDeviceLabel(step.executionDevice)}</strong><br />Consume selected object revision {step.targetRevision.revision}<br />Intent: “{step.executionIntent}”<br /><span className="danger-text">Execution unavailable; no runtime is selected.</span></p>;
            return <p key={index}><strong>{step.landingMode === "pipeline_handoff" ? "Transfer · PipelinePrivate" : "Transfer · Final delivery"}</strong><br />{composerDeviceLabel(step.source)} → {step.destination === "pastey_shared" ? `Pastey Shared on ${selectedHostLabel}` : composerDeviceLabel(step.destination)}</p>;
          })}
          {!approvalId ? <button type="button" className="primary-button" disabled={busy !== null} onClick={() => void reviewAndRun()}>{busy === "review" ? "Starting…" : "Review & Run"}</button> : null}
        </div>
      ) : null}

      {approvalState ? <p className="muted">Plan state: {approvalState}</p> : null}
      {resultSummary ? <p className="success-text">{resultSummary}</p> : null}
      {terminalSearchResult?.candidateCount === 0 ? <div className="request-file-preview"><h3>Search results</h3><p className="success-text">Search completed with no matching files.</p></div> : null}
      {safeSearchCandidates.length > 0 && (candidateMode === "result" || !selectedCandidateId) ? (
        <div className="candidate-card-list">
          <h3>{candidateMode === "selectable" ? "Choose a file for the approved next step" : "Search results on the selected device"}</h3>
          {safeSearchCandidates.map((candidate) => <SearchCandidateCard key={candidate.candidateId} candidate={candidate} selectable={candidateMode === "selectable"} disabled={busy !== null} onSelect={candidateMode === "selectable" ? () => void selectCandidate(selectedBridgePlanCandidateId(candidate)) : undefined} />)}
        </div>
      ) : null}
      {!canPlan ? <p className="muted">Select one connected device to create a Plan.</p> : null}
      {message ? <p className="muted" role="status">{message}</p> : null}
    </Card>
  );
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
        results: records.results.map(parseBridgePlanResult).filter((entry): entry is NonNullable<ReturnType<typeof parseBridgePlanResult>> => entry !== null).map((entry) => entry.summary).slice(-8),
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
  if (room.status === "peer_left") return { label: "Local only", tone: "neutral" };
  if (room.peer_connected) return { label: "Connected", tone: "success" };
  return { label: "Waiting for peer", tone: "warning" };
}

function bridgeMemberSummary(room: RoomInfo): { title: string; detail: string } {
  const remoteNames = currentRemoteMembers(room).map((peer) => peer.displayName?.trim()).filter(Boolean) as string[];
  if (remoteNames.length === 0 && room.status === "peer_left") {
    return { title: "Local-only Bridge - 1 member", detail: "This device remains" };
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

function currentRemoteMembers(room: RoomInfo) {
  return (room.peers ?? []).filter((peer) => !["left", "stale", "expired"].includes(peer.liveness));
}

function bridgeSubtitle(room: RoomInfo): string {
  const connected = connectedRemoteMembers(room);
  if (connected.length === 1) return `Connected to ${connected[0].displayName ?? room.peer_device_name ?? "device"}`;
  if (connected.length > 1) return `${connected.length} members connected`;
  const current = currentRemoteMembers(room);
  if (current.length > 0) return `${current.length} current member${current.length === 1 ? "" : "s"} disconnected`;
  return room.status === "peer_left" ? "No remote Bridge members" : "Waiting for another device";
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

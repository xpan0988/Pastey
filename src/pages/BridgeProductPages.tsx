import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState, type ClipboardEvent, type ReactNode } from "react";
import {
  copyTextToClipboard,
  executeFileCandidateSearchCapability,
  executeHelloStdoutCapability,
  getDeviceProfile,
  getRoomControlSessionContext,
  joinRoom,
  listReceivedRoomControlEvents,
  listNearbyDevices,
  requestNearbyJoin,
  revealInFolder,
  resolveCandidatePayloadCapability,
  sendRoomControlEvent,
  sendTextToRoom,
  writeTempFile,
} from "../lib/tauri";
import {
  assertCapabilityEventHasSelectedPeerRoute,
  bridgeRoutePayload,
  enqueueTransferInputsWithBridgeRoute,
  sendTextToRoomWithBridgeRoute,
} from "../lib/bridgeRoutingRuntime";
import {
  bridgePeerSessionId,
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
  createRoomControlProductRegistry,
  registerOutboundCapabilityPreview,
  routeRoomControlInboxEvents,
  type RoomControlProductRegistry,
} from "../lib/agentBridge/roomControlProductRegistry";
import {
  bridgePollingIntervalMs,
  reconcileSelectedPeerIds,
} from "../lib/agentBridge/bridgeDetailPolling";
import {
  buildCandidatePayloadExecutionRequest,
  buildCandidatePayloadWorkflowPayloadPreview,
  buildFileCandidateExecutionRequest,
  buildHelloPeerStdoutProductPreview,
  buildHelloStdoutExecutionRequest,
  buildPeerConsentStatusEvent,
  buildSessionBoundCapabilityPreviewControlEvent,
  confirmCandidatePayloadWorkflowSearch,
  createCandidatePayloadWorkflow,
  createControlQueueState,
  createIdleRoomControlSendState,
  createPeerConsentConsumptionState,
  createPeerConsentSessionState,
  denyPeerCapability,
  markCandidatePayloadWorkflowPayloadPendingConsent,
  markCandidatePayloadWorkflowPayloadAllowed,
  markCandidatePayloadWorkflowSearchAllowed,
  startCandidatePayloadWorkflowFromSearchAdvisory,
  receiveCandidatePayloadWorkflowHandoffResult,
  receiveCandidatePayloadWorkflowSearchResult,
  allowPeerCapabilityOnce,
  applyInboundPeerStatusToOutboundQueue,
  enqueueInboundRoomControlEvents,
  enqueueRoomControlEvent,
  evaluatePeerCapabilityPreview,
  executeInboundCandidatePayloadRequest,
  executeInboundFileCandidateRequest,
  executeInboundHelloStdoutRequest,
  formatHelloPeerStdoutProductResult,
  HELLO_PEER_DEMO_ACTION_LABEL,
  HELLO_PEER_DEMO_DESCRIPTION,
  HELLO_PEER_LIFECYCLE_STEPS,
  HELLO_PEER_REQUIRES_ONE_SELECTED_DEVICE,
  markControlQueueItemStatus,
  matchExecutionResultToRequest,
  processNextControlQueueItem,
  roomControlSessionIdentity,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityExecutionResultRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type CandidatePayloadHandoffResult,
  type CandidatePayloadLocalResolution,
  type CandidatePayloadWorkflow,
  type ControlQueueItem,
  type ControlQueueState,
  type PeerConsentBinding,
  type PeerConsentRecord,
  type PeerConsentSessionState,
  type PeerConsentConsumptionState,
  type RoomControlEvent,
  type RoomControlSendState,
} from "../lib/agentBridge";
import {
  buildCapabilityRequestPreviewEnvelope,
  buildMockAiContextSnapshot,
  buildMockFileCandidatePlan,
  type CandidatePayloadExecutionRequest,
  type CandidatePayloadExecutionResult,
  type CandidatePayloadRequest,
  type FileCandidateExecutionResult,
  type FileCandidateRequest,
} from "../lib/ai";
import { FILE_TOO_LARGE_MESSAGE, MAX_FILE_SIZE_BYTES } from "../lib/constants";
import { formatBytes, formatCode, formatTimestamp } from "../lib/format";
import type { TransferQueueInput, TransferQueueItem } from "../lib/transferScheduler";
import type {
  FileTransferProgressEvent,
  DeviceProfile,
  JoinRequestPrompt,
  NearbyDevice,
  RoomControlSessionContext,
  RoomInfo,
  RoomItem,
} from "../lib/types";

type PrimaryRoute = "bridge" | "activity" | "devices" | "settings";
type BridgeTargetSelectionMode = "selected_peer" | "selected_peers" | "broadcast_bridge";
type SafeSearchScope = "downloads" | "desktop" | "documents" | "pastey_shared";
type HelloPeerDemoStatus =
  | "idle"
  | "preview_ready"
  | "peer_requested"
  | "awaiting_peer"
  | "peer_approved"
  | "denied"
  | "executing"
  | "completed"
  | "failed";

interface HelloPeerProductState {
  status: HelloPeerDemoStatus;
  message: string | null;
  steps: string[];
  preview: CapabilityPreviewRoomControlEvent | null;
  peerReview: HelloPeerReviewState | null;
  senderAck: CapabilityPreviewAckRoomControlEvent | null;
  result: CapabilityExecutionResultRoomControlEvent | null;
}

interface HelloPeerReviewState {
  queueId: string;
  event: CapabilityPreviewRoomControlEvent;
  binding: PeerConsentBinding;
  record?: PeerConsentRecord;
}

type RequestFileStatus =
  | "idle"
  | "search_preview_ready"
  | "waiting_search_approval"
  | "awaiting_peer"
  | "search_denied"
  | "candidates_found"
  | "payload_request_sent"
  | "payload_denied"
  | "handoff_queued"
  | "failed";

interface RequestFileProductState {
  status: RequestFileStatus;
  message: string | null;
  steps: string[];
  searchPreview: CapabilityPreviewRoomControlEvent | null;
  payloadPreview: CapabilityPreviewRoomControlEvent | null;
  peerReview: RequestFileReviewState | null;
  latestResult: CapabilityExecutionResultRoomControlEvent | null;
}

interface RequestFileReviewState {
  queueId: string;
  event: CapabilityPreviewRoomControlEvent;
  binding: PeerConsentBinding;
}

const REQUEST_FILE_REQUIRES_ONE_SELECTED_DEVICE = "Request file requires one selected device.";
const REQUEST_FILE_LIFECYCLE_STEPS = [
  "Search prepared",
  "Host validated safe scopes",
  "You confirmed",
  "Peer approved search",
  "Peer denied search",
  "Candidates returned",
  "Candidate selected",
  "Payload request sent",
  "Peer approved transfer",
  "Peer denied transfer",
  "Handoff queued",
  "Transfer completed",
] as const;

const SAFE_SEARCH_SCOPES: Array<{ value: SafeSearchScope; label: string }> = [
  { value: "downloads", label: "Downloads" },
  { value: "desktop", label: "Desktop" },
  { value: "documents", label: "Documents" },
  { value: "pastey_shared", label: "Pastey Shared" },
];

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
  onEnqueueCandidatePayloadHandoff: (roomId: string, input: TransferQueueInput) => boolean;
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
  onEnqueueCandidatePayloadHandoff,
  onOpenActivity,
}: BridgeDetailPageProps) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"send" | "files" | "close" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dropActive, setDropActive] = useState(false);
  const [targetMode, setTargetMode] = useState<BridgeTargetSelectionMode>("selected_peer");
  const [selectedPeerIds, setSelectedPeerIds] = useState<string[]>([]);
  const [requestOpen, setRequestOpen] = useState(false);
  const [askOpen, setAskOpen] = useState(false);
  const [helloSession, setHelloSession] = useState<RoomControlSessionContext | null>(null);
  const helloSessionRef = useRef<RoomControlSessionContext | null>(null);
  const [helloQueue, setHelloQueue] = useState<ControlQueueState>(createControlQueueState);
  const helloQueueRef = useRef<ControlQueueState>(helloQueue);
  const roomControlRegistryRef = useRef<RoomControlProductRegistry>(createRoomControlProductRegistry());
  const refreshInFlightRef = useRef(false);
  const refreshBridgeControlInboxRef = useRef<() => Promise<void>>(async () => {});
  const helloPumpInFlightRef = useRef(false);
  const [requestInboxBatch, setRequestInboxBatch] = useState<RoomControlEvent[]>([]);
  const [requestPollingActive, setRequestPollingActive] = useState(false);
  const [helloSendState, setHelloSendState] = useState<RoomControlSendState>(createIdleRoomControlSendState);
  const [helloPeerConsentSession, setHelloPeerConsentSession] =
    useState<PeerConsentSessionState>(createPeerConsentSessionState);
  const [helloPeerConsentRecords, setHelloPeerConsentRecords] = useState<PeerConsentRecord[]>([]);
  const [helloConsumptionState, setHelloConsumptionState] =
    useState<PeerConsentConsumptionState>(createPeerConsentConsumptionState);
  const [helloFlow, setHelloFlow] = useState<HelloPeerProductState>(() => createHelloPeerProductState());
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
  const canRunHelloPeerDemo = Boolean(askBridgeBetaEnabled && selectedSinglePeer && helloSession);

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
      applyHelloSession(null);
      return;
    }
    void getRoomControlSessionContext(room.id)
      .then((session) => {
        if (!cancelled) applyHelloSession(session);
      })
      .catch((err) => {
        if (!cancelled) {
          applyHelloSession(null);
          setHelloFlow((current) => ({
            ...current,
            status: "failed",
            message: err instanceof Error ? err.message : String(err),
          }));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [room.id, room.peer_connected, room.status]);

  const helloPollingActive = askBridgeBetaEnabled && !isHelloPeerTerminal(helloFlow.status) && helloFlow.status !== "idle" && helloFlow.status !== "preview_ready";
  const roomControlPollingActive = helloPollingActive || requestPollingActive;
  refreshBridgeControlInboxRef.current = refreshBridgeControlInbox;

  useEffect(() => {
    if (!helloSession) return;
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
  }, [helloSession, roomControlPollingActive]);

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

  function applyHelloSession(nextSession: RoomControlSessionContext | null) {
    const previous = helloSessionRef.current;
    if (roomControlSessionIdentity(previous) !== roomControlSessionIdentity(nextSession)) {
      const freshQueue = createControlQueueState();
      helloQueueRef.current = freshQueue;
      setHelloQueue(freshQueue);
      setHelloSendState(createIdleRoomControlSendState());
      setHelloPeerConsentSession(createPeerConsentSessionState());
      setHelloPeerConsentRecords([]);
      setHelloConsumptionState(createPeerConsentConsumptionState());
      setHelloFlow(createHelloPeerProductState());
      roomControlRegistryRef.current = createRoomControlProductRegistry();
      setRequestInboxBatch([]);
    }
    helloSessionRef.current = nextSession;
    setHelloSession(nextSession);
  }

  function applyHelloQueue(nextQueue: ControlQueueState) {
    helloQueueRef.current = nextQueue;
    setHelloQueue(nextQueue);
  }

  function registerProductPreview(
    event: CapabilityPreviewRoomControlEvent,
    owner: "hello_peer" | "request_file",
  ) {
    roomControlRegistryRef.current = registerOutboundCapabilityPreview(
      roomControlRegistryRef.current,
      event,
      owner,
    );
  }

  function prepareHelloPeerDemo() {
    if (!askBridgeBetaEnabled) {
      setHelloFlow((current) => ({
        ...current,
        status: "failed",
        message: "Enable Labs in Settings to use Ask Bridge Beta.",
      }));
      return;
    }
    if (!selectedSinglePeer || !helloSession || selectedRoute?.target.kind !== "selected_peer") {
      setHelloFlow((current) => ({
        ...current,
        status: "idle",
        message: HELLO_PEER_REQUIRES_ONE_SELECTED_DEVICE,
      }));
      return;
    }
    const preview = buildHelloPeerStdoutProductPreview(helloSession);
    if (!preview.ok) {
      setHelloFlow((current) => ({
        ...current,
        status: "failed",
        message: preview.errors.join(" "),
      }));
      return;
    }
    setHelloFlow({
      status: "preview_ready",
      message: `Ready to ask ${selectedSinglePeer.displayName} for stdout.`,
      steps: ["Plan prepared", "Host validated"],
      preview: preview.preview.previewEvent,
      peerReview: null,
      senderAck: null,
      result: null,
    });
  }

  async function confirmHelloPeerDemo() {
    if (!helloFlow.preview) {
      prepareHelloPeerDemo();
      return;
    }
    registerProductPreview(helloFlow.preview, "hello_peer");
    const queued = enqueueRoomControlEvent(helloQueueRef.current, helloFlow.preview, "outbound");
    if (!queued.ok) {
      setHelloFlow((current) => ({ ...current, status: "failed", message: queued.errors.join(" ") }));
      return;
    }
    setHelloFlow((current) => ({
      ...current,
      status: "peer_requested",
      message: "Hello Peer request is being sent to the selected device.",
      steps: mergeHelloSteps(current.steps, ["You confirmed", "Peer requested"]),
    }));
    applyHelloQueue(queued.state);
    await pumpHelloPeerQueue();
  }

  async function refreshBridgeControlInbox() {
    const currentSession = helloSessionRef.current;
    if (!currentSession) {
      setHelloFlow((current) => ({
        ...current,
        status: "failed",
        message: "Hello Peer requires an active Bridge session.",
      }));
      return;
    }
    if (refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    try {
      const events = await listReceivedRoomControlEvents(currentSession.roomId);
      const routed = routeRoomControlInboxEvents(
        roomControlRegistryRef.current,
        events.map((event) => event.event),
        {
          expectedRoomRef: currentSession.roomId,
          expectedSourceDeviceRef: currentSession.peerSessionRef,
          expectedTargetPeerRef: currentSession.localSessionRef,
        },
      );
      roomControlRegistryRef.current = routed.registry;
      if (routed.requestFile.length > 0) {
        setRequestInboxBatch((current) =>
          [...current, ...routed.requestFile].slice(-256)
        );
      }
      if (routed.helloPeer.length === 0) return;
      const integrated = enqueueInboundRoomControlEvents(
        helloQueueRef.current,
        routed.helloPeer,
        {
          expectedRoomRef: currentSession.roomId,
          expectedSourceDeviceRef: currentSession.peerSessionRef,
          expectedTargetPeerRef: currentSession.localSessionRef,
        },
      );
      applyHelloQueue(integrated.state);
      await pumpHelloPeerQueue();
    } catch (err) {
      setHelloFlow((current) => ({
        ...current,
        status: "failed",
        message: err instanceof Error ? err.message : String(err),
      }));
    } finally {
      refreshInFlightRef.current = false;
    }
  }

  async function decideHelloPeerReview(decision: "allow_once" | "deny") {
    if (!helloFlow.peerReview) {
      setHelloFlow((current) => ({
        ...current,
        message: "No Hello Peer request is waiting for this device.",
      }));
      return;
    }
    const now = new Date();
    const decisionResult = decision === "allow_once"
      ? allowPeerCapabilityOnce(helloFlow.peerReview.binding, helloPeerConsentSession, { now })
      : denyPeerCapability(helloFlow.peerReview.binding, helloPeerConsentSession, { now });
    if (!decisionResult.ok) {
      setHelloFlow((current) => ({ ...current, status: "failed", message: decisionResult.errors.join(" ") }));
      return;
    }
    const statusEvent = buildPeerConsentStatusEvent(helloFlow.peerReview.event, decisionResult.record, { now });
    if (!statusEvent.ok) {
      setHelloFlow((current) => ({ ...current, status: "failed", message: statusEvent.errors.join(" ") }));
      return;
    }
    const reviewed = markControlQueueItemStatus(
      helloQueueRef.current,
      helloFlow.peerReview.queueId,
      decision === "allow_once" ? "allowed_once" : "denied",
      {
        now,
        reason: decision === "allow_once"
          ? "Allowed once for this exact Hello Peer request."
          : "Denied by receiver.",
      },
    );
    if (!reviewed.ok) {
      setHelloFlow((current) => ({ ...current, status: "failed", message: reviewed.errors.join(" ") }));
      return;
    }
    const outbound = enqueueRoomControlEvent(reviewed.state, statusEvent.event, "outbound", { now });
    if (!outbound.ok) {
      setHelloFlow((current) => ({ ...current, status: "failed", message: outbound.errors.join(" ") }));
      return;
    }
    setHelloPeerConsentSession(decisionResult.state);
    setHelloPeerConsentRecords((records) => [...records, decisionResult.record]);
    setHelloFlow((current) => ({
      ...current,
      status: decision === "allow_once" ? "peer_approved" : "denied",
      message: decision === "allow_once"
        ? "Allowed once. Pastey will run the fixed hello runtime after the sender requests execution."
        : "Denied. Nothing will run.",
      peerReview: null,
      steps: decision === "allow_once"
        ? mergeHelloSteps(current.steps, ["Peer approved"])
        : mergeHelloSteps(current.steps, ["Peer denied"]),
    }));
    applyHelloQueue(outbound.state);
    await pumpHelloPeerQueue();
  }

  async function pumpHelloPeerQueue() {
    if (helloPumpInFlightRef.current) return;
    helloPumpInFlightRef.current = true;
    try {
      let nextQueue = helloQueueRef.current;
      for (let index = 0; index < 8; index += 1) {
        const result = await processNextControlQueueItem(
          nextQueue,
          (event) => sendHelloPeerControlEvent(event),
          {
            onState: (state) => {
              nextQueue = state;
              applyHelloQueue(state);
            },
            onSendState: setHelloSendState,
          },
        );
        nextQueue = result.state;
        applyHelloQueue(nextQueue);
        if (!result.ok) {
          if (result.action !== "no_selectable_item") {
            setHelloFlow((current) => ({ ...current, status: "failed", message: result.message }));
          }
          return;
        }
        if (result.action === "selected_inbound") {
          nextQueue = await handleHelloPeerInbound(nextQueue, result.item);
          applyHelloQueue(nextQueue);
          if (result.item.event.kind === "capability_preview") return;
        } else if (result.item.event.kind === "capability_preview") {
          setHelloFlow((current) => ({
            ...current,
            status: "awaiting_peer",
            message: "Request sent. Waiting for the selected device to allow once or deny.",
            steps: mergeHelloSteps(current.steps, ["Peer requested"]),
          }));
        } else if (result.item.event.kind === "capability_execution_result") {
          setHelloFlow((current) => ({
            ...current,
            status: "completed",
            message: "Result returned to the requesting device.",
            steps: mergeHelloSteps(current.steps, ["Result returned"]),
          }));
        }
      }
    } finally {
      helloPumpInFlightRef.current = false;
    }
  }

  async function handleHelloPeerInbound(
    state: ControlQueueState,
    item: ControlQueueItem,
  ): Promise<ControlQueueState> {
    if (!helloSession) return state;

    if (item.event.kind === "capability_preview") {
      const previewEvent = item.event as CapabilityPreviewRoomControlEvent;
      if (previewEvent.payload.request.capability !== "runtime.hello_stdout") {
        const invalid = markControlQueueItemStatus(state, item.queueId, "invalid", {
          reason: "Ask Bridge Beta product flow accepts only runtime.hello_stdout.",
        });
        setHelloFlow((current) => ({
          ...current,
          status: "failed",
          message: "Unknown capability request rejected.",
        }));
        return invalid.state;
      }
      const policy = evaluatePeerCapabilityPreview(previewEvent, {
        roomRef: helloSession.roomId,
        sourceDeviceRef: helloSession.peerSessionRef,
        targetPeerRef: helloSession.localSessionRef,
        session: helloPeerConsentSession,
      });
      if (policy.status === "rejected") {
        const invalid = markControlQueueItemStatus(state, item.queueId, "invalid", {
          reason: policy.errors.join(" ").slice(0, 512),
        });
        setHelloFlow((current) => ({
          ...current,
          status: "failed",
          message: policy.errors.join(" "),
        }));
        return invalid.state;
      }
      const awaiting = markControlQueueItemStatus(state, item.queueId, "awaiting_peer_decision", {
        reason: "Receiver PolicyGate accepted this exact Hello Peer preview.",
      });
      if (!awaiting.ok) {
        setHelloFlow((current) => ({ ...current, status: "failed", message: awaiting.errors.join(" ") }));
        return state;
      }
      setHelloFlow((current) => ({
        ...current,
        status: "awaiting_peer",
        message: "This device received a Hello Peer request.",
        peerReview: {
          queueId: item.queueId,
          event: previewEvent,
          binding: policy.binding,
        },
        steps: mergeHelloSteps(current.steps, ["Peer requested"]),
      }));
      setAskOpen(true);
      return awaiting.state;
    }

    if (item.event.kind === "capability_preview_ack" || item.event.kind === "capability_preview_deny") {
      const decisionEvent = item.event;
      const applied = applyInboundPeerStatusToOutboundQueue(state, decisionEvent);
      if (!applied.ok) {
        setHelloFlow((current) => ({
          ...current,
          status: "failed",
          message: applied.errors.join(" "),
        }));
        return state;
      }
      const marked = markControlQueueItemStatus(
        applied.state,
        item.queueId,
        decisionEvent.payload.status,
        { reason: "Peer returned a Hello Peer decision." },
      );
      if (!marked.ok) {
        setHelloFlow((current) => ({
          ...current,
          status: "failed",
          message: marked.errors.join(" "),
        }));
        return state;
      }
      if (decisionEvent.kind === "capability_preview_deny") {
        setHelloFlow((current) => ({
          ...current,
          status: "denied",
          message: "The selected device denied the Hello Peer request.",
          steps: mergeHelloSteps(current.steps, ["Peer denied"]),
        }));
        return marked.state;
      }
      const ackEvent = decisionEvent as CapabilityPreviewAckRoomControlEvent;
      if (!ackEvent.payload.consent) return marked.state;
      const requestItem = marked.state.outbound.find(
        (candidate): candidate is ControlQueueItem & { event: CapabilityPreviewRoomControlEvent } =>
          candidate.event.kind === "capability_preview"
          && candidate.event.eventId === ackEvent.payload.consent?.sourcePreviewEventId,
      );
      if (!requestItem) {
        setHelloFlow((current) => ({
          ...current,
          status: "failed",
          message: "The exact Hello Peer preview for this Allow once response is unavailable.",
        }));
        return marked.state;
      }
      const execution = buildHelloStdoutExecutionRequest(requestItem.event, ackEvent);
      if (!execution.ok) {
        setHelloFlow((current) => ({ ...current, status: "failed", message: execution.errors.join(" ") }));
        return marked.state;
      }
      const queued = enqueueRoomControlEvent(marked.state, execution.event, "outbound");
      if (!queued.ok) {
        setHelloFlow((current) => ({ ...current, status: "failed", message: queued.errors.join(" ") }));
        return marked.state;
      }
      setHelloFlow((current) => ({
        ...current,
        status: "peer_approved",
        message: "The selected device allowed once. Requesting the fixed runtime now.",
        senderAck: ackEvent,
        steps: mergeHelloSteps(current.steps, ["Peer approved"]),
      }));
      return queued.state;
    }

    if (item.event.kind === "capability_execute_request") {
      return executeHelloPeerInboundRequest(state, item as ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent });
    }

    if (item.event.kind === "capability_execution_result") {
      const resultEvent = item.event;
      const requestItem = state.outbound.find(
        (candidate): candidate is ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent } =>
          candidate.event.kind === "capability_execute_request"
          && matchExecutionResultToRequest(resultEvent, candidate.event),
      );
      const matched = requestItem
        ? markControlQueueItemStatus(state, requestItem.queueId, resultEvent.payload.status === "succeeded" ? "execution_succeeded" : "execution_rejected", {
            reason: resultEvent.payload.status === "succeeded"
              ? "Peer returned the bounded Hello Peer result."
              : `Peer returned ${resultEvent.payload.errorCode ?? resultEvent.payload.status}.`,
          })
        : { ok: false as const, state, errors: ["No matching Hello Peer execution request was found."] };
      const inbound = markControlQueueItemStatus(
        matched.ok ? matched.state : state,
        item.queueId,
        matched.ok ? "execution_succeeded" : "invalid",
        { reason: matched.ok ? "Matched bounded Hello Peer result." : matched.errors.join(" ") },
      );
      const formatted = formatHelloPeerStdoutProductResult(resultEvent);
      setHelloFlow((current) => ({
        ...current,
        status: formatted ? "completed" : "failed",
        message: formatted ? formatted.title : resultEvent.payload.errorCode ?? resultEvent.payload.status,
        result: resultEvent,
        steps: formatted ? mergeHelloSteps(current.steps, ["Result returned"]) : current.steps,
      }));
      return inbound.state;
    }

    return state;
  }

  async function executeHelloPeerInboundRequest(
    state: ControlQueueState,
    item: ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent },
  ): Promise<ControlQueueState> {
    if (!helloSession) return state;
    if (item.event.payload.capability !== "runtime.hello_stdout") {
      const invalid = markControlQueueItemStatus(state, item.queueId, "invalid", {
        reason: "Ask Bridge Beta product flow executes only runtime.hello_stdout.",
      });
      return invalid.state;
    }
    const consent = helloPeerConsentRecords.find(
      (record) => record.binding.consentId === item.event.payload.consentId,
    );
    setHelloFlow((current) => ({
      ...current,
      status: "executing",
      message: "Running Pastey's fixed hello runtime.",
    }));
    const execution = await executeInboundHelloStdoutRequest(
      item.event,
      consent,
      helloConsumptionState,
      executeHelloStdoutCapability,
      {
        roomRef: helloSession.roomId,
        sourceDeviceRef: helloSession.peerSessionRef,
        targetPeerRef: helloSession.localSessionRef,
      },
    );
    const completed = markControlQueueItemStatus(
      state,
      item.queueId,
      execution.result.status === "succeeded" ? "execution_consumed" : "execution_rejected",
      {
        reason: execution.result.status === "succeeded"
          ? "Exact one-time consent consumed. Hello Peer demo executed once."
          : `Execution request rejected: ${execution.result.errorCode ?? execution.result.status}.`,
      },
    );
    if (!completed.ok) {
      setHelloFlow((current) => ({ ...current, status: "failed", message: completed.errors.join(" ") }));
      return state;
    }
    const outbound = enqueueRoomControlEvent(completed.state, execution.resultEvent, "outbound");
    if (!outbound.ok) {
      setHelloFlow((current) => ({ ...current, status: "failed", message: outbound.errors.join(" ") }));
      return completed.state;
    }
    setHelloConsumptionState(execution.state);
    setHelloFlow((current) => ({
      ...current,
      status: execution.result.status === "succeeded" ? "executing" : "failed",
      message: execution.result.status === "succeeded"
        ? "Runtime executed. Returning result to the sender."
        : `Execution request rejected: ${execution.result.errorCode ?? execution.result.status}.`,
      steps: execution.result.status === "succeeded" ? mergeHelloSteps(current.steps, ["Runtime executed"]) : current.steps,
    }));
    return outbound.state;
  }

  async function sendHelloPeerControlEvent(event: RoomControlEvent) {
    const currentSession = helloSessionRef.current;
    if (!currentSession) {
      throw new Error("Hello Peer requires an active selected-peer Bridge session.");
    }
    const route = assertCapabilityEventHasSelectedPeerRoute(currentSession, event);
    return sendRoomControlEvent(
      currentSession.roomId,
      event,
      bridgeRoutePayload(route, "pastey-bridge-control-route-v1"),
    );
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

      <div className="secondary-action-grid">
        <button type="button" className="bridge-action-panel" onClick={() => setRequestOpen((open) => !open)}>
          <strong>Request file</strong>
          <span>Ask the other device to send you a file.</span>
        </button>
        <button type="button" className={`bridge-action-panel ${askBridgeBetaEnabled ? "" : "disabled"}`} onClick={() => setAskOpen((open) => !open)}>
          <strong>Ask Bridge Beta</strong>
          <span>{HELLO_PEER_DEMO_DESCRIPTION}</span>
        </button>
      </div>

      <div hidden={!requestOpen}>
        <RequestFilePanel
          room={room}
          selectedPeer={selectedSinglePeer}
          route={selectedRoute}
          queueItems={queueItems}
          transfers={transfers}
          onEnqueueCandidatePayloadHandoff={onEnqueueCandidatePayloadHandoff}
          onIncomingReview={() => setRequestOpen(true)}
          inboxEvents={requestInboxBatch}
          onRefresh={() => void refreshBridgeControlInbox()}
          onPollingActiveChange={setRequestPollingActive}
          onRegisterPreview={(event) => registerProductPreview(event, "request_file")}
        />
      </div>

      {askOpen ? (
        <HelloPeerDemoPanel
          enabled={askBridgeBetaEnabled}
          selectedPeer={selectedSinglePeer}
          selectedRoute={selectedRoute}
          canRun={canRunHelloPeerDemo}
          flow={helloFlow}
          sendState={helloSendState}
          onPrepare={prepareHelloPeerDemo}
          onConfirm={() => void confirmHelloPeerDemo()}
          onCancel={() => setHelloFlow(createHelloPeerProductState())}
          onRefresh={() => void refreshBridgeControlInbox()}
          onAllowOnce={() => void decideHelloPeerReview("allow_once")}
          onDeny={() => void decideHelloPeerReview("deny")}
        />
      ) : null}

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

function HelloPeerDemoPanel({
  enabled,
  selectedPeer,
  selectedRoute,
  canRun,
  flow,
  sendState,
  onPrepare,
  onConfirm,
  onCancel,
  onRefresh,
  onAllowOnce,
  onDeny,
}: {
  enabled: boolean;
  selectedPeer: BridgePeerSession | null;
  selectedRoute: BridgeRoute | null;
  canRun: boolean;
  flow: HelloPeerProductState;
  sendState: RoomControlSendState;
  onPrepare: () => void;
  onConfirm: () => void;
  onCancel: () => void;
  onRefresh: () => void;
  onAllowOnce: () => void;
  onDeny: () => void;
}) {
  const requiresOnePeer = !selectedPeer || selectedRoute?.target.kind !== "selected_peer";
  const result = flow.result ? formatHelloPeerStdoutProductResult(flow.result) : null;
  return (
    <Card className="hello-peer-panel">
      <div className="section-row">
        <div>
          <h2>Ask Bridge Beta</h2>
          <p className="muted">{HELLO_PEER_DEMO_DESCRIPTION}</p>
        </div>
        <StatusChip tone={enabled && selectedPeer ? "success" : "neutral"}>
          {enabled ? "Beta" : "Disabled"}
        </StatusChip>
      </div>

      <div className="hello-peer-action-row">
        <div>
          <strong>{HELLO_PEER_DEMO_ACTION_LABEL}</strong>
          <p className="muted">
            {enabled
              ? requiresOnePeer
                ? HELLO_PEER_REQUIRES_ONE_SELECTED_DEVICE
                : `Selected device: ${selectedPeer.displayName}.`
              : "Enable Labs in Settings to use Ask Bridge Beta."}
          </p>
        </div>
        <button type="button" className="primary-button" disabled={!canRun} onClick={onPrepare}>
          {HELLO_PEER_DEMO_ACTION_LABEL}
        </button>
      </div>

      {flow.status === "preview_ready" && flow.preview ? (
        <div className="hello-peer-preview" role="status">
          <span className="agent-bridge-status-label">Confirmation preview</span>
          <strong>Run Pastey's built-in hello runtime on {selectedPeer?.displayName ?? "the selected device"}</strong>
          <span className="muted">Capability: runtime.hello_stdout. Expected stdout: hello peer.</span>
          <div className="button-row">
            <button type="button" className="primary-button" onClick={onConfirm}>Confirm and send</button>
            <button type="button" className="secondary-button" onClick={onCancel}>Cancel</button>
          </div>
        </div>
      ) : null}

      {flow.peerReview ? (
        <div className="hello-peer-preview" data-testid="bridge-hello-peer-review">
          <span className="agent-bridge-status-label">Hello Peer request</span>
          <strong>Allow this device to run Pastey's built-in hello runtime once?</strong>
          <span className="muted">Expected stdout: hello peer. This does not grant shell, paths, environment, or network access.</span>
          <div className="button-row">
            <button type="button" className="primary-button" onClick={onAllowOnce}>Allow once</button>
            <button type="button" className="secondary-button" onClick={onDeny}>Deny</button>
          </div>
        </div>
      ) : null}

      <div className="hello-peer-status-row">
        <button type="button" className="secondary-button" onClick={onRefresh}>
          Check for updates
        </button>
        <span className="muted">
          {sendState.status === "sending"
            ? "Sending..."
            : sendState.status === "accepted"
              ? "Latest request delivered."
              : sendState.status === "rejected"
                ? sendState.message
                : flow.message ?? "Ready."}
        </span>
      </div>

      <OperationTimeline
        label="Hello Peer lifecycle"
        steps={buildOperationTimelineSteps(HELLO_PEER_LIFECYCLE_STEPS, flow.steps)}
      />

      {result ? (
        <div className="hello-peer-result" data-testid="bridge-hello-peer-result">
          <strong>{result.title}</strong>
          <pre>{`stdout: ${result.stdout}`}</pre>
          <div className="button-row">
            <span>exitCode: {result.exitCode}</span>
            <button type="button" className="secondary-button compact-button" onClick={() => void copyTextToClipboard(result.stdout)}>
              Copy stdout
            </button>
          </div>
        </div>
      ) : null}
    </Card>
  );
}

function RequestFilePanel({
  room,
  selectedPeer,
  route,
  queueItems,
  transfers,
  onEnqueueCandidatePayloadHandoff,
  onIncomingReview,
  inboxEvents,
  onRefresh,
  onPollingActiveChange,
  onRegisterPreview,
}: {
  room: RoomInfo;
  selectedPeer: BridgePeerSession | null;
  route: BridgeRoute | null;
  queueItems: TransferQueueItem[];
  transfers: FileTransferProgressEvent[];
  onEnqueueCandidatePayloadHandoff: (roomId: string, input: TransferQueueInput) => boolean;
  onIncomingReview: () => void;
  inboxEvents: readonly RoomControlEvent[];
  onRefresh: () => void;
  onPollingActiveChange: (active: boolean) => void;
  onRegisterPreview: (event: CapabilityPreviewRoomControlEvent) => void;
}) {
  const [query, setQuery] = useState("");
  const [scopes, setScopes] = useState<SafeSearchScope[]>(["downloads", "desktop", "documents", "pastey_shared"]);
  const [workflow, setWorkflow] = useState<CandidatePayloadWorkflow>(() => createCandidatePayloadWorkflow());
  const workflowRef = useRef<CandidatePayloadWorkflow>(workflow);
  const [selectedCandidateId, setSelectedCandidateId] = useState("");
  const [session, setSession] = useState<RoomControlSessionContext | null>(null);
  const sessionRef = useRef<RoomControlSessionContext | null>(null);
  const [queue, setQueue] = useState<ControlQueueState>(createControlQueueState);
  const queueRef = useRef<ControlQueueState>(queue);
  const pumpInFlightRef = useRef(false);
  const [sendState, setSendState] = useState<RoomControlSendState>(createIdleRoomControlSendState);
  const [peerConsentSession, setPeerConsentSession] =
    useState<PeerConsentSessionState>(createPeerConsentSessionState);
  const [peerConsentRecords, setPeerConsentRecords] = useState<PeerConsentRecord[]>([]);
  const [consumptionState, setConsumptionState] =
    useState<PeerConsentConsumptionState>(createPeerConsentConsumptionState);
  const [flow, setFlow] = useState<RequestFileProductState>(() => createRequestFileProductState());
  const candidates = workflow.snapshot.candidates ?? [];
  const requiresOnePeer = !selectedPeer || route?.target.kind !== "selected_peer";
  const canRequestSearch = Boolean(!requiresOnePeer && session && query.trim() && scopes.length > 0);
  const canConfirmSearch = Boolean(flow.searchPreview && flow.status === "search_preview_ready");
  const canRequestFile = Boolean(!requiresOnePeer && session && selectedCandidateId && workflow.snapshot.state === "candidate_selection_required");
  const selectedCandidate = candidates.find((candidate) => candidate.candidateId === selectedCandidateId) ?? null;
  const relatedQueueItem = selectedCandidateId
    ? queueItems.find((item) => item.agentBridgeMetadata?.candidateId === selectedCandidateId) ?? null
    : null;
  const relatedTransfer = relatedQueueItem
    ? transfers.find((transfer) => transfer.queue_item_id === relatedQueueItem.id || transfer.item_id === relatedQueueItem.id) ?? null
    : null;
  const displaySteps = requestFileStepsWithTransfer(flow.steps, relatedQueueItem, relatedTransfer);

  useEffect(() => {
    workflowRef.current = workflow;
  }, [workflow]);

  useEffect(() => {
    let cancelled = false;
    if (!selectedPeer || room.status !== "active" || !room.peer_connected) {
      applySession(null);
      return;
    }
    void getRoomControlSessionContext(room.id)
      .then((nextSession) => {
        if (!cancelled) applySession(nextSession);
      })
      .catch((err) => {
        if (!cancelled) {
          applySession(null);
          setFlow((current) => ({
            ...current,
            status: "failed",
            message: err instanceof Error ? err.message : String(err),
          }));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [room.id, room.status, room.peer_connected, selectedPeer?.peerSessionId]);

  useEffect(() => {
    onPollingActiveChange(isRequestFilePollingActive(flow.status));
    return () => onPollingActiveChange(false);
  }, [flow.status, onPollingActiveChange]);

  useEffect(() => {
    const currentSession = sessionRef.current;
    if (!currentSession || inboxEvents.length === 0) return;
    const integrated = enqueueInboundRoomControlEvents(
      queueRef.current,
      inboxEvents,
      {
        expectedRoomRef: currentSession.roomId,
        expectedSourceDeviceRef: currentSession.peerSessionRef,
        expectedTargetPeerRef: currentSession.localSessionRef,
      },
    );
    applyQueue(integrated.state);
    void pumpRequestFileQueue();
  }, [inboxEvents, session]);

  function toggleScope(scope: SafeSearchScope) {
    setScopes((current) => current.includes(scope)
      ? current.filter((candidate) => candidate !== scope)
      : [...current, scope]);
  }

  function applySession(nextSession: RoomControlSessionContext | null) {
    const previous = sessionRef.current;
    if (roomControlSessionIdentity(previous) !== roomControlSessionIdentity(nextSession)) {
      const freshQueue = createControlQueueState();
      queueRef.current = freshQueue;
      setQueue(freshQueue);
      setSendState(createIdleRoomControlSendState());
      setPeerConsentSession(createPeerConsentSessionState());
      setPeerConsentRecords([]);
      setConsumptionState(createPeerConsentConsumptionState());
      const fresh = createCandidatePayloadWorkflow();
      workflowRef.current = fresh;
      setWorkflow(fresh);
      setSelectedCandidateId("");
      setFlow(createRequestFileProductState());
    }
    sessionRef.current = nextSession;
    setSession(nextSession);
  }

  function applyWorkflow(nextWorkflow: CandidatePayloadWorkflow) {
    workflowRef.current = nextWorkflow;
    setWorkflow(nextWorkflow);
  }

  function applyQueue(nextQueue: ControlQueueState) {
    queueRef.current = nextQueue;
    setQueue(nextQueue);
  }

  function handlePrepareSearch() {
    if (requiresOnePeer || !selectedPeer || !session) {
      setFlow((current) => ({
        ...current,
        status: "idle",
        message: REQUEST_FILE_REQUIRES_ONE_SELECTED_DEVICE,
      }));
      return;
    }
    const prepared = prepareCandidateSearchWorkflow(query, scopes, session.peerSessionRef);
    const started = startCandidatePayloadWorkflowFromSearchAdvisory(createCandidatePayloadWorkflow(), prepared.plan, prepared.context);
    const confirmed = started.ok
      ? confirmCandidatePayloadWorkflowSearch(started.workflow, {
          sourceDeviceRef: session.localSessionRef,
        })
      : null;
    if (!started.ok) {
      applyWorkflow(started.workflow);
      setFlow((current) => ({
        ...current,
        status: "failed",
        message: started.errors.join(" "),
      }));
      return;
    }
    if (!confirmed?.ok) {
      applyWorkflow(confirmed?.workflow ?? started.workflow);
      setFlow((current) => ({
        ...current,
        status: "failed",
        message: confirmed?.errors.join(" ") ?? "Search preview could not be confirmed.",
      }));
      return;
    }
    const preview = buildRequestFilePreviewEvent(confirmed.request, session);
    if (!preview.ok) {
      applyWorkflow(confirmed.workflow);
      setFlow((current) => ({ ...current, status: "failed", message: preview.errors.join(" ") }));
      return;
    }
    applyWorkflow(confirmed.workflow);
    setSelectedCandidateId("");
    setFlow({
      status: "search_preview_ready",
      message: "Search preview ready. Confirm before asking the selected device.",
      steps: ["Search prepared", "Host validated safe scopes"],
      searchPreview: preview.event,
      payloadPreview: null,
      peerReview: null,
      latestResult: null,
    });
  }

  async function handleConfirmSearch() {
    if (!flow.searchPreview) {
      handlePrepareSearch();
      return;
    }
    onRegisterPreview(flow.searchPreview);
    const queued = enqueueRoomControlEvent(queueRef.current, flow.searchPreview, "outbound");
    if (!queued.ok) {
      setFlow((current) => ({ ...current, status: "failed", message: queued.errors.join(" ") }));
      return;
    }
    setFlow((current) => ({
      ...current,
      status: "waiting_search_approval",
      message: "Waiting for the selected device to approve metadata search.",
      steps: mergeRequestFileSteps(current.steps, ["You confirmed"]),
    }));
    applyQueue(queued.state);
    await pumpRequestFileQueue();
  }

  function handleSelectCandidate(candidateId: string) {
    setSelectedCandidateId(candidateId);
    setFlow((current) => ({
      ...current,
      message: "Candidate selected. Request selected file when ready.",
      steps: mergeRequestFileSteps(current.steps, ["Candidate selected"]),
    }));
  }

  async function handleRequestSelectedFile() {
    if (requiresOnePeer || !selectedPeer || !session || !selectedCandidateId) {
      setFlow((current) => ({
        ...current,
        status: current.status,
        message: selectedCandidateId ? REQUEST_FILE_REQUIRES_ONE_SELECTED_DEVICE : "Choose candidate first.",
      }));
      return;
    }
    const prepared = prepareCandidateSearchWorkflow(query || "selected file", scopes, session.peerSessionRef);
    const preview = buildCandidatePayloadWorkflowPayloadPreview(
      workflowRef.current,
      { candidateId: selectedCandidateId, selectedByUser: true },
      prepared.context,
      { sourceDeviceRef: session.localSessionRef },
    );
    if (!preview.ok) {
      applyWorkflow(preview.workflow);
      setFlow((current) => ({ ...current, status: "failed", message: preview.errors.join(" ") }));
      return;
    }
    const previewEvent = buildRequestFilePreviewEvent(preview.request, session);
    if (!previewEvent.ok) {
      applyWorkflow(preview.workflow);
      setFlow((current) => ({ ...current, status: "failed", message: previewEvent.errors.join(" ") }));
      return;
    }
    const pending = markCandidatePayloadWorkflowPayloadPendingConsent(preview.workflow);
    if (!pending.ok) {
      applyWorkflow(pending.workflow);
      setFlow((current) => ({ ...current, status: "failed", message: pending.errors.join(" ") }));
      return;
    }
    onRegisterPreview(previewEvent.event);
    const queued = enqueueRoomControlEvent(queueRef.current, previewEvent.event, "outbound");
    if (!queued.ok) {
      applyWorkflow(pending.workflow);
      setFlow((current) => ({ ...current, status: "failed", message: queued.errors.join(" ") }));
      return;
    }
    applyWorkflow(pending.workflow);
    setFlow((current) => ({
      ...current,
      status: "payload_request_sent",
      message: "Payload request sent. Waiting for second receiver consent.",
      payloadPreview: previewEvent.event,
      steps: mergeRequestFileSteps(current.steps, ["Candidate selected", "Payload request sent"]),
    }));
    applyQueue(queued.state);
    await pumpRequestFileQueue();
  }

  async function decideRequestFileReview(decision: "allow_once" | "deny") {
    if (!flow.peerReview) {
      setFlow((current) => ({ ...current, message: "No Request file approval is waiting on this device." }));
      return;
    }
    const now = new Date();
    const decisionResult = decision === "allow_once"
      ? allowPeerCapabilityOnce(flow.peerReview.binding, peerConsentSession, { now })
      : denyPeerCapability(flow.peerReview.binding, peerConsentSession, { now });
    if (!decisionResult.ok) {
      setFlow((current) => ({ ...current, status: "failed", message: decisionResult.errors.join(" ") }));
      return;
    }
    const statusEvent = buildPeerConsentStatusEvent(flow.peerReview.event, decisionResult.record, { now });
    if (!statusEvent.ok) {
      setFlow((current) => ({ ...current, status: "failed", message: statusEvent.errors.join(" ") }));
      return;
    }
    const reviewed = markControlQueueItemStatus(
      queueRef.current,
      flow.peerReview.queueId,
      decision === "allow_once" ? "allowed_once" : "denied",
      {
        now,
        reason: decision === "allow_once"
          ? "Allowed once for this exact Request file capability."
          : "Denied by receiver.",
      },
    );
    if (!reviewed.ok) {
      setFlow((current) => ({ ...current, status: "failed", message: reviewed.errors.join(" ") }));
      return;
    }
    const outbound = enqueueRoomControlEvent(reviewed.state, statusEvent.event, "outbound", { now });
    if (!outbound.ok) {
      setFlow((current) => ({ ...current, status: "failed", message: outbound.errors.join(" ") }));
      return;
    }
    setPeerConsentSession(decisionResult.state);
    setPeerConsentRecords((records) => [...records, decisionResult.record]);
    const deniedStep = flow.peerReview.binding.capability === "transfer.request_candidate_payload"
      ? "Peer denied transfer"
      : "Peer denied search";
    setFlow((current) => ({
      ...current,
      status: decision === "allow_once"
        ? current.peerReview?.binding.capability === "transfer.request_candidate_payload"
          ? "payload_request_sent"
          : "waiting_search_approval"
        : current.peerReview?.binding.capability === "transfer.request_candidate_payload"
          ? "payload_denied"
          : "search_denied",
      message: decision === "allow_once" ? "Allowed once. Waiting for the sender's execution request." : "Denied. Nothing will run.",
      peerReview: null,
      steps: decision === "allow_once" ? current.steps : mergeRequestFileSteps(current.steps, [deniedStep]),
    }));
    applyQueue(outbound.state);
    await pumpRequestFileQueue();
  }

  async function pumpRequestFileQueue() {
    if (pumpInFlightRef.current) return;
    pumpInFlightRef.current = true;
    try {
      let nextQueue = queueRef.current;
      for (let index = 0; index < 10; index += 1) {
        const result = await processNextControlQueueItem(
          nextQueue,
          (event) => sendRequestFileControlEvent(event),
          {
            onState: (state) => {
              nextQueue = state;
              applyQueue(state);
            },
            onSendState: setSendState,
          },
        );
        nextQueue = result.state;
        applyQueue(nextQueue);
        if (!result.ok) {
          if (result.action !== "no_selectable_item") {
            setFlow((current) => ({ ...current, status: "failed", message: result.message }));
          }
          return;
        }
        if (result.action === "selected_inbound") {
          nextQueue = await handleRequestFileInbound(nextQueue, result.item);
          applyQueue(nextQueue);
          if (result.item.event.kind === "capability_preview") return;
        }
      }
    } finally {
      pumpInFlightRef.current = false;
    }
  }

  async function handleRequestFileInbound(
    state: ControlQueueState,
    item: ControlQueueItem,
  ): Promise<ControlQueueState> {
    if (!session) return state;

    if (item.event.kind === "capability_preview") {
      const previewEvent = item.event as CapabilityPreviewRoomControlEvent;
      const capability = previewEvent.payload.request.capability;
      if (capability !== "filesystem.find_file_candidates" && capability !== "transfer.request_candidate_payload") {
        const invalid = markControlQueueItemStatus(state, item.queueId, "invalid", {
          reason: "Request file accepts only file-candidate search or candidate-payload requests.",
        });
        setFlow((current) => ({ ...current, status: "failed", message: "Unknown capability request rejected." }));
        return invalid.state;
      }
      const policy = evaluatePeerCapabilityPreview(previewEvent, {
        roomRef: session.roomId,
        sourceDeviceRef: session.peerSessionRef,
        targetPeerRef: session.localSessionRef,
        session: peerConsentSession,
      });
      if (policy.status === "rejected") {
        const invalid = markControlQueueItemStatus(state, item.queueId, "invalid", {
          reason: policy.errors.join(" ").slice(0, 512),
        });
        setFlow((current) => ({ ...current, status: "failed", message: policy.errors.join(" ") }));
        return invalid.state;
      }
      const awaiting = markControlQueueItemStatus(state, item.queueId, "awaiting_peer_decision", {
        reason: "Receiver PolicyGate accepted this exact Request file preview.",
      });
      if (!awaiting.ok) {
        setFlow((current) => ({ ...current, status: "failed", message: awaiting.errors.join(" ") }));
        return state;
      }
      setFlow((current) => ({
        ...current,
        status: "awaiting_peer",
        message: capability === "transfer.request_candidate_payload"
          ? "This device received a selected-candidate payload request."
          : "This device received a metadata-only search request.",
        peerReview: {
          queueId: item.queueId,
          event: previewEvent,
          binding: policy.binding,
        },
      }));
      onIncomingReview();
      return awaiting.state;
    }

    if (item.event.kind === "capability_preview_ack" || item.event.kind === "capability_preview_deny") {
      const decisionEvent = item.event;
      const applied = applyInboundPeerStatusToOutboundQueue(state, decisionEvent);
      if (!applied.ok) {
        setFlow((current) => ({ ...current, status: "failed", message: applied.errors.join(" ") }));
        return state;
      }
      const marked = markControlQueueItemStatus(
        applied.state,
        item.queueId,
        decisionEvent.payload.status,
        { reason: "Peer returned a Request file decision." },
      );
      if (!marked.ok) {
        setFlow((current) => ({ ...current, status: "failed", message: marked.errors.join(" ") }));
        return state;
      }
      const decisionPreview = marked.state.outbound.find(
        (candidate): candidate is ControlQueueItem & { event: CapabilityPreviewRoomControlEvent } =>
          candidate.event.kind === "capability_preview" &&
          candidate.event.payload.request.requestId === decisionEvent.payload.requestId,
      );
      const decisionCapability = decisionPreview?.event.payload.request.capability;
      if (decisionEvent.kind === "capability_preview_deny") {
        const deniedStep = decisionCapability === "transfer.request_candidate_payload"
          ? "Peer denied transfer"
          : "Peer denied search";
        setFlow((current) => ({
          ...current,
          status: decisionCapability === "transfer.request_candidate_payload" ? "payload_denied" : "search_denied",
          message: decisionCapability === "transfer.request_candidate_payload"
            ? "The selected device denied the payload request."
            : "The selected device denied the metadata search.",
          steps: mergeRequestFileSteps(current.steps, [deniedStep]),
        }));
        return marked.state;
      }
      const ackEvent = decisionEvent as CapabilityPreviewAckRoomControlEvent;
      if (!ackEvent.payload.consent) return marked.state;
      const requestItem = marked.state.outbound.find(
        (candidate): candidate is ControlQueueItem & { event: CapabilityPreviewRoomControlEvent } =>
          candidate.event.kind === "capability_preview"
          && candidate.event.eventId === ackEvent.payload.consent?.sourcePreviewEventId,
      );
      if (!requestItem) {
        setFlow((current) => ({ ...current, status: "failed", message: "The exact Request file preview for this Allow once response is unavailable." }));
        return marked.state;
      }
      const capability = requestItem.event.payload.request.capability;
      const execution = capability === "filesystem.find_file_candidates"
        ? buildFileCandidateExecutionRequest(requestItem.event, ackEvent)
        : capability === "transfer.request_candidate_payload"
          ? buildCandidatePayloadExecutionRequest(requestItem.event, ackEvent)
          : { ok: false as const, errors: ["Request file received an unsupported capability acknowledgement."] };
      if (!execution.ok) {
        setFlow((current) => ({ ...current, status: "failed", message: execution.errors.join(" ") }));
        return marked.state;
      }
      const queued = enqueueRoomControlEvent(marked.state, execution.event, "outbound");
      if (!queued.ok) {
        setFlow((current) => ({ ...current, status: "failed", message: queued.errors.join(" ") }));
        return marked.state;
      }
      if (capability === "filesystem.find_file_candidates") {
        const allowed = markCandidatePayloadWorkflowSearchAllowed(workflowRef.current);
        if (allowed.ok) applyWorkflow(allowed.workflow);
      } else {
        const allowed = markCandidatePayloadWorkflowPayloadAllowed(workflowRef.current);
        if (allowed.ok) applyWorkflow(allowed.workflow);
      }
      setFlow((current) => ({
        ...current,
        status: capability === "filesystem.find_file_candidates" ? "waiting_search_approval" : "payload_request_sent",
        message: capability === "filesystem.find_file_candidates"
          ? "Peer approved search. Running metadata-only search."
          : "Peer approved transfer. Requesting handoff.",
        steps: capability === "filesystem.find_file_candidates"
          ? mergeRequestFileSteps(current.steps, ["Peer approved search"])
          : mergeRequestFileSteps(current.steps, ["Peer approved transfer"]),
      }));
      return queued.state;
    }

    if (item.event.kind === "capability_execute_request") {
      const capability = item.event.payload.capability;
      if (capability === "filesystem.find_file_candidates") {
        return executeRequestFileSearch(state, item as ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent });
      }
      if (capability === "transfer.request_candidate_payload") {
        return executeRequestFilePayload(state, item as ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent });
      }
    }

    if (item.event.kind === "capability_execution_result") {
      return receiveRequestFileResult(state, item as ControlQueueItem & { event: CapabilityExecutionResultRoomControlEvent });
    }

    return state;
  }

  async function executeRequestFileSearch(
    state: ControlQueueState,
    item: ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent },
  ): Promise<ControlQueueState> {
    if (!session) return state;
    const consent = peerConsentRecords.find((record) => record.binding.consentId === item.event.payload.consentId);
    const execution = await executeInboundFileCandidateRequest(
      item.event,
      consent,
      consumptionState,
      executeFileCandidateSearchCapability,
      {
        roomRef: session.roomId,
        sourceDeviceRef: session.peerSessionRef,
        targetPeerRef: session.localSessionRef,
      },
    );
    return enqueueRequestFileExecutionResult(state, item, execution.state, execution.resultEvent, execution.result.status === "completed"
      ? "Exact search consent consumed. Metadata-only candidates returned."
      : `Search rejected: ${execution.result.errorCode ?? execution.result.status}.`);
  }

  async function executeRequestFilePayload(
    state: ControlQueueState,
    item: ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent },
  ): Promise<ControlQueueState> {
    if (!session) return state;
    const consent = peerConsentRecords.find((record) => record.binding.consentId === item.event.payload.consentId);
    const execution = await executeInboundCandidatePayloadRequest(
      item.event,
      consent,
      consumptionState,
      resolveCandidatePayloadCapability,
      enqueueCandidatePayloadHandoff,
      {
        roomRef: session.roomId,
        sourceDeviceRef: session.peerSessionRef,
        targetPeerRef: session.localSessionRef,
      },
    );
    return enqueueRequestFileExecutionResult(state, item, execution.state, execution.resultEvent, execution.result.status === "handoff_queued"
      ? "Exact payload consent consumed. Handoff queued."
      : `Payload request rejected: ${execution.result.errorCode ?? execution.result.status}.`);
  }

  function enqueueRequestFileExecutionResult(
    state: ControlQueueState,
    item: ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent },
    nextConsumption: PeerConsentConsumptionState,
    resultEvent: CapabilityExecutionResultRoomControlEvent,
    reason: string,
  ): ControlQueueState {
    const completed = markControlQueueItemStatus(
      state,
      item.queueId,
      resultEvent.payload.status === "completed" || resultEvent.payload.status === "handoff_queued"
        ? "execution_consumed"
        : "execution_rejected",
      { reason },
    );
    if (!completed.ok) {
      setFlow((current) => ({ ...current, status: "failed", message: completed.errors.join(" ") }));
      return state;
    }
    const outbound = enqueueRoomControlEvent(completed.state, resultEvent, "outbound");
    if (!outbound.ok) {
      setFlow((current) => ({ ...current, status: "failed", message: outbound.errors.join(" ") }));
      return completed.state;
    }
    setConsumptionState(nextConsumption);
    setFlow((current) => ({
      ...current,
      status: resultEvent.payload.status === "handoff_queued"
        ? "handoff_queued"
        : resultEvent.payload.status === "completed"
          ? "idle"
          : "failed",
      message: resultEvent.payload.status === "handoff_queued"
        ? "Handoff queued. Existing transfer pipeline owns progress and completion."
        : resultEvent.payload.status === "completed"
          ? "Metadata-only search result returned to the requesting device."
          : `Request file execution failed: ${resultEvent.payload.errorCode ?? resultEvent.payload.status}.`,
      latestResult: resultEvent,
    }));
    return outbound.state;
  }

  function receiveRequestFileResult(
    state: ControlQueueState,
    item: ControlQueueItem & { event: CapabilityExecutionResultRoomControlEvent },
  ): ControlQueueState {
    const resultEvent = item.event;
    const requestItem = state.outbound.find(
      (candidate): candidate is ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent } =>
        candidate.event.kind === "capability_execute_request"
        && matchExecutionResultToRequest(resultEvent, candidate.event),
    );
    const resultStatus = resultEvent.payload.status === "completed" || resultEvent.payload.status === "handoff_queued"
      ? "execution_succeeded"
      : resultEvent.payload.status === "already_consumed"
        ? "already_consumed"
        : resultEvent.payload.status === "failed"
          ? "execution_failed"
          : "execution_rejected";
    const matched = requestItem
      ? markControlQueueItemStatus(state, requestItem.queueId, resultStatus, {
          reason: resultStatus === "execution_succeeded"
            ? "Peer returned Request file result."
            : `Peer returned ${resultEvent.payload.errorCode ?? resultEvent.payload.status}.`,
        })
      : { ok: false as const, state, errors: ["No matching Request file execution request was found."] };
    const inbound = markControlQueueItemStatus(
      matched.ok ? matched.state : state,
      item.queueId,
      matched.ok ? resultStatus : "invalid",
      { reason: matched.ok ? "Matched Request file result." : matched.errors.join(" ") },
    );

    if ("capability" in resultEvent.payload && resultEvent.payload.capability === "filesystem.find_file_candidates") {
      const received = receiveCandidatePayloadWorkflowSearchResult(
        workflowRef.current,
        resultEvent.payload as FileCandidateExecutionResult,
      );
      applyWorkflow(received.workflow);
      setFlow((current) => ({
        ...current,
        status: received.ok ? "candidates_found" : "failed",
        message: received.ok
          ? `${received.workflow.snapshot.candidates?.length ?? 0} redacted candidate(s) returned.`
          : received.errors.join(" "),
        latestResult: resultEvent,
        steps: received.ok ? mergeRequestFileSteps(current.steps, ["Candidates returned"]) : current.steps,
      }));
    } else if ("capability" in resultEvent.payload && resultEvent.payload.capability === "transfer.request_candidate_payload") {
      const received = receiveCandidatePayloadWorkflowHandoffResult(
        workflowRef.current,
        resultEvent.payload as CandidatePayloadExecutionResult,
      );
      applyWorkflow(received.workflow);
      setFlow((current) => ({
        ...current,
        status: received.ok ? "handoff_queued" : "failed",
        message: received.ok
          ? "Handoff queued. Existing transfer pipeline owns progress and completion."
          : received.errors.join(" "),
        latestResult: resultEvent,
        steps: received.ok ? mergeRequestFileSteps(current.steps, ["Handoff queued"]) : current.steps,
      }));
    }
    return inbound.state;
  }

  async function enqueueCandidatePayloadHandoff(
    request: CandidatePayloadExecutionRequest,
    resolution: CandidatePayloadLocalResolution,
  ): Promise<CandidatePayloadHandoffResult> {
    if (!session || request.capability !== "transfer.request_candidate_payload" || !resolution.receiverLocalSource) {
      return { queued: false, errorCode: "unsupported_route" };
    }
    const modifiedMs = typeof resolution.modifiedAt === "string" ? Date.parse(resolution.modifiedAt) : Number.NaN;
    if (
      resolution.candidateKind !== "filesystem_file" ||
      typeof resolution.sizeBytes !== "number" ||
      !Number.isFinite(modifiedMs)
    ) {
      return { queued: false, errorCode: "handoff_failed" };
    }
    const targetPeerSessionId = session.peerRouteRef ?? session.peerSessionRef;
    const queued = onEnqueueCandidatePayloadHandoff(room.id, {
      path: resolution.receiverLocalSource,
      displayName: resolution.displayName ?? request.candidateDisplayName,
      mimeType: "application/octet-stream",
      sizeBytes: resolution.sizeBytes,
      modifiedMs,
      dedupeKey: [
        "agent-bridge-candidate-payload",
        request.sourceRequestId,
        request.candidateId,
        request.candidateKind,
        resolution.sizeBytes,
        modifiedMs,
      ].join(":"),
      bridgeRoute: {
        bridgeSessionId: `legacy-room:${session.roomId}`,
        target: {
          kind: "selected_peer",
          peerSessionId: bridgePeerSessionId(targetPeerSessionId),
        },
      },
      bridgeOperationId: `agent-bridge-candidate:${request.requestId}:${request.executionId}`,
      bridgeTargetKind: "selected_peer",
      bridgeContentKind: "file",
      targetPeerSessionId,
      targetPeerDisplayName: selectedPeer?.displayName ?? room.peer_device_name ?? "selected peer",
      targetCount: 1,
      agentBridgeMetadata: {
        origin: "agent_bridge_candidate_payload",
        label: "Agent Bridge candidate payload request",
        note: "Queued from approved candidate payload request.",
        sourceCapability: "filesystem.find_file_candidates",
        requestCapability: "transfer.request_candidate_payload",
        sourceRequestId: request.sourceRequestId,
        candidateId: request.candidateId,
        candidateKind: "filesystem_file",
        candidateDisplayName: resolution.displayName ?? request.candidateDisplayName,
        requestedByPeerRef: request.sourceDeviceRef,
        approvedByPeerRef: request.targetPeerRef,
        consentId: request.consentId,
        agentBridgeRequestId: request.requestId,
        handoffCreatedAt: new Date().toISOString(),
        sizeBytes: resolution.sizeBytes,
        extension: resolution.extension,
        mimeFamily: resolution.mimeFamily,
      },
    });
    return queued ? { queued: true } : { queued: false, errorCode: "handoff_failed" };
  }

  async function sendRequestFileControlEvent(event: RoomControlEvent) {
    const currentSession = sessionRef.current;
    if (!currentSession) {
      throw new Error("Request file requires an active selected-peer Bridge session.");
    }
    const selectedRoute = assertCapabilityEventHasSelectedPeerRoute(currentSession, event);
    return sendRoomControlEvent(
      currentSession.roomId,
      event,
      bridgeRoutePayload(selectedRoute, "pastey-bridge-control-route-v1"),
    );
  }

  return (
    <Card className="request-file-panel">
      <div className="section-row">
        <div>
          <h2>Request file</h2>
          <p className="muted">{selectedPeer && !requiresOnePeer ? `From ${selectedPeer.displayName}` : REQUEST_FILE_REQUIRES_ONE_SELECTED_DEVICE}</p>
        </div>
        <StatusChip tone={selectedPeer && !requiresOnePeer ? "success" : "neutral"}>{selectedPeer && !requiresOnePeer ? "Selected" : "Choose device"}</StatusChip>
      </div>
      <div className="find-search-grid">
        <label className="field-label">
          <span>What should the device look for?</span>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="notes, screenshot, invoice..." />
        </label>
        <div className="safe-scope-card">
          <span className="field-label-text">Safe locations</span>
          <div className="scope-chip-grid">
            {SAFE_SEARCH_SCOPES.map((scope) => (
              <button
                key={scope.value}
                type="button"
                className={`scope-chip ${scopes.includes(scope.value) ? "checked" : ""}`}
                onClick={() => toggleScope(scope.value)}
              >
                <span className="scope-chip-check" aria-hidden="true" />
                <span>{scope.label}</span>
              </button>
            ))}
          </div>
        </div>
      </div>
      <div className="button-row">
        <button type="button" className="primary-button" disabled={!canRequestSearch} onClick={handlePrepareSearch}>
          Search selected device
        </button>
        <button type="button" className="secondary-button" disabled={!canRequestFile} onClick={() => void handleRequestSelectedFile()}>
          Request selected file
        </button>
      </div>
      {canConfirmSearch ? (
        <div className="request-file-preview" role="status">
          <span className="agent-bridge-status-label">Search preview</span>
          <strong>Search selected device</strong>
          <span className="muted">Metadata only. Safe scopes: {scopes.map((scope) => SAFE_SEARCH_SCOPES.find((entry) => entry.value === scope)?.label ?? scope).join(", ")}.</span>
          <div className="button-row">
            <button type="button" className="primary-button" onClick={() => void handleConfirmSearch()}>Confirm search preview</button>
            <button type="button" className="secondary-button" onClick={() => setFlow(createRequestFileProductState())}>Cancel</button>
          </div>
        </div>
      ) : null}
      {flow.peerReview ? (
        <div className="request-file-preview" data-testid="request-file-peer-review">
          <span className="agent-bridge-status-label">Approval needed</span>
          <strong>{flow.peerReview.binding.capability === "transfer.request_candidate_payload" ? "Allow selected candidate handoff once?" : "Allow metadata search once?"}</strong>
          <span className="muted">
            {flow.peerReview.binding.capability === "transfer.request_candidate_payload"
              ? "This is separate from search consent. Pastey revalidates the selected candidate before queue handoff."
              : "Search returns redacted metadata only and does not authorize transfer."}
          </span>
          <div className="button-row">
            <button type="button" className="primary-button" onClick={() => void decideRequestFileReview("allow_once")}>Allow once</button>
            <button type="button" className="secondary-button" onClick={() => void decideRequestFileReview("deny")}>Deny</button>
          </div>
        </div>
      ) : null}
      {candidates.length > 0 ? (
        <div className="candidate-card-list">
          <h3>Choose candidate</h3>
          {candidates.map((candidate) => (
            <button
              key={candidate.candidateId}
              type="button"
              className={`candidate-metadata-card ${selectedCandidateId === candidate.candidateId ? "selected" : ""}`}
              onClick={() => handleSelectCandidate(candidate.candidateId)}
            >
              <strong>{candidate.candidateDisplayName}</strong>
              <span>{formatBytes(candidate.sizeBytes)} - {candidate.extension || candidate.mimeFamily}</span>
              <small>{candidate.matchReason}</small>
            </button>
          ))}
        </div>
      ) : null}
      <div className="request-file-status-row">
        <button type="button" className="secondary-button" onClick={onRefresh}>
          Check for updates
        </button>
        <span className="muted">
          {sendState.status === "sending"
            ? "Sending capability event..."
            : sendState.status === "accepted"
              ? "Latest capability event delivered."
              : sendState.status === "rejected"
                ? sendState.message
                : flow.message ?? (requiresOnePeer ? REQUEST_FILE_REQUIRES_ONE_SELECTED_DEVICE : "Ready.")}
        </span>
      </div>
      <OperationTimeline
        label="Request file lifecycle"
        steps={buildOperationTimelineSteps(REQUEST_FILE_LIFECYCLE_STEPS, displaySteps)}
        rows={requestFileLifecycleRows(flow, selectedCandidate, relatedQueueItem, relatedTransfer)}
      />
      <details className="request-file-advanced-details">
        <summary>Advanced details</summary>
        <div className="agent-bridge-definition-list">
          <FullValue label="Search capability" value={workflow.snapshot.search?.capability ?? "Not prepared"} />
          <FullValue label="Search request" value={workflow.searchRequest?.requestId ?? "None"} />
          <FullValue label="Payload capability" value={workflow.snapshot.payload?.capability ?? "Not prepared"} />
          <FullValue label="Payload request" value={workflow.payloadRequest?.requestId ?? "None"} />
          <FullValue label="Selected candidate" value={selectedCandidateId || "None"} />
        </div>
      </details>
    </Card>
  );
}

function buildRequestFilePreviewEvent(
  request: FileCandidateRequest | CandidatePayloadRequest,
  session: RoomControlSessionContext,
): { ok: true; event: CapabilityPreviewRoomControlEvent } | { ok: false; errors: string[] } {
  const envelope = buildCapabilityRequestPreviewEnvelope(request, {
    roomRef: session.roomId,
    sourceDeviceRef: session.localSessionRef,
    targetPeerRef: session.peerSessionRef,
  });
  if (!envelope.ok) {
    return { ok: false, errors: envelope.errors };
  }
  const preview = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, session);
  if (!preview.ok || preview.event.kind !== "capability_preview") {
    return { ok: false, errors: preview.ok ? ["Request file preview builder produced the wrong event kind."] : preview.errors };
  }
  return { ok: true, event: preview.event };
}

function createRequestFileProductState(): RequestFileProductState {
  return {
    status: "idle",
    message: null,
    steps: [],
    searchPreview: null,
    payloadPreview: null,
    peerReview: null,
    latestResult: null,
  };
}

function mergeRequestFileSteps(
  current: readonly string[],
  next: readonly string[],
): string[] {
  return [...new Set([...current, ...next].filter((step) =>
    (REQUEST_FILE_LIFECYCLE_STEPS as readonly string[]).includes(step)
  ))];
}

function isRequestFileTerminal(status: RequestFileStatus): boolean {
  return status === "search_denied"
    || status === "payload_denied"
    || status === "handoff_queued"
    || status === "failed";
}

function isRequestFilePollingActive(status: RequestFileStatus): boolean {
  return !isRequestFileTerminal(status) && (
    status === "waiting_search_approval"
    || status === "awaiting_peer"
    || status === "payload_request_sent"
  );
}

function requestFileStepsWithTransfer(
  current: readonly string[],
  queueItem: TransferQueueItem | null,
  transfer: FileTransferProgressEvent | null,
): string[] {
  const next = [...current];
  if (queueItem) next.push("Handoff queued");
  if (queueItem?.status === "completed" || transfer?.status === "completed") {
    next.push("Transfer completed");
  }
  return mergeRequestFileSteps([], next);
}

function buildOperationTimelineSteps(
  orderedLabels: readonly string[],
  completedLabels: readonly string[],
): OperationTimelineStep[] {
  return orderedLabels
    .filter((label) => completedLabels.includes(label))
    .map((label, index, visibleLabels) => ({
      id: normalizeOperationTimelineId(label),
      label,
      status: index === visibleLabels.length - 1 ? "active" : "complete",
    }));
}

function requestFileLifecycleRows(
  flow: RequestFileProductState,
  candidate: NonNullable<CandidatePayloadWorkflow["snapshot"]["candidates"]>[number] | null,
  queueItem: TransferQueueItem | null,
  transfer: FileTransferProgressEvent | null,
): OperationTimelineRow[] {
  const rows: OperationTimelineRow[] = [];
  if (flow.status === "waiting_search_approval" || flow.status === "payload_request_sent" || flow.status === "awaiting_peer") {
    rows.push(operationTimelineRow("Waiting for approval", "active", flow.message ?? "Receiver consent is required."));
  }
  if (flow.status === "candidates_found") {
    rows.push(operationTimelineRow("Candidates found", "complete", flow.message ?? "Redacted metadata returned."));
  }
  if (flow.status === "search_denied" || flow.status === "payload_denied") {
    rows.push(operationTimelineRow("Denied", "denied", flow.message ?? "The selected device denied the request."));
  }
  if (flow.status === "handoff_queued" || queueItem) {
    rows.push(operationTimelineRow(
      "Handoff queued",
      "complete",
      candidate ? `${candidate.candidateDisplayName} is in the existing transfer queue.` : "Existing transfer pipeline owns progress.",
    ));
  }
  if (queueItem?.status === "sending" || transfer?.status === "transferring") {
    rows.push(operationTimelineRow(
      "Transfer started",
      "active",
      transfer ? `${formatBytes(transfer.transferred_bytes)} of ${formatBytes(transfer.file_size)}` : queueItem?.displayName ?? "Sending.",
    ));
  }
  if (queueItem?.status === "completed" || transfer?.status === "completed") {
    rows.push(operationTimelineRow("Transfer complete", "complete", queueItem?.displayName ?? transfer?.file_name ?? "Completed."));
  }
  if (flow.status === "failed" || queueItem?.status === "failed" || transfer?.status === "failed") {
    rows.push(operationTimelineRow("Failed", "failed", flow.message ?? queueItem?.errorMessage ?? transfer?.error_message ?? "Request file failed."));
  }
  return rows;
}

function operationTimelineRow(
  label: string,
  status: OperationTimelineStatus,
  detail: string,
): OperationTimelineRow {
  return {
    id: `${normalizeOperationTimelineId(label)}:${normalizeOperationTimelineId(detail)}`,
    label,
    status,
    detail,
  };
}

function normalizeOperationTimelineId(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "operation";
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

  const trustedRooms = rooms.filter((room) => room.peer_device_name || (room.peers?.length ?? 0) > 0);

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
        <h2>Trusted devices</h2>
        <div className="simple-list-card">
          {trustedRooms.length === 0 ? <p className="muted">Known devices will appear here after you connect.</p> : null}
          {trustedRooms.map((room) => (
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

function createHelloPeerProductState(): HelloPeerProductState {
  return {
    status: "idle",
    message: null,
    steps: [],
    preview: null,
    peerReview: null,
    senderAck: null,
    result: null,
  };
}

function mergeHelloSteps(
  current: readonly string[],
  next: readonly string[],
): string[] {
  return [...new Set([...current, ...next].filter((step) =>
    (HELLO_PEER_LIFECYCLE_STEPS as readonly string[]).includes(step)
  ))];
}

function isHelloPeerTerminal(status: HelloPeerDemoStatus): boolean {
  return status === "completed" || status === "denied" || status === "failed";
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

function prepareCandidateSearchWorkflow(searchQuery: string, safeScopes: SafeSearchScope[], targetPeerRef: string) {
  const plan = buildMockFileCandidatePlan();
  const proposedInput = plan.proposedInput ?? {};
  const query = typeof proposedInput.query === "object" && proposedInput.query !== null ? proposedInput.query : {};
  const context = buildMockAiContextSnapshot();
  return {
    plan: {
      ...plan,
      title: "Request file search",
      explanation: "Ask the selected device to look in safe locations.",
      references: [{ kind: "peer" as const, ref: targetPeerRef }],
      proposedInput: {
        ...proposedInput,
        targetPeerRef,
        query: {
          ...query,
          rawUserRequest: searchQuery,
          filenameHint: searchQuery.trim(),
          searchMode: "filename_metadata_only",
        },
        scopePolicy: {
          allowedScopes: safeScopes,
          allowFullDisk: false,
          includeFileContents: false,
          includeAbsolutePaths: false,
          includeHiddenFiles: false,
        },
        safety: {
          returnRedactedPaths: true,
          noAutoTransfer: true,
          requireReceiverConsent: true,
          selectedPeerOnly: true,
        },
      },
    },
    context: {
      ...context,
      peers: [{
        peerRef: targetPeerRef,
        visible: true,
        trusted: true,
        capabilities: ["filesystem.find_file_candidates", "transfer.request_candidate_payload"],
      }],
    },
  };
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

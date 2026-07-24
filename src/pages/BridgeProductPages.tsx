import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState, type ClipboardEvent, type ReactNode } from "react";
import {
  copyTextToClipboard,
  approveBridgePlan,
  bridgePlanReceiverReviewStatus,
  createDirectFileTransferBridgePlan,
  createFileSearchBridgePlan,
  createFileTransformBridgePlan,
  proposeBridgePlanTransformFallback,
  decideBridgePlanReview,
  executeBridgePlanSearchAttempt,
  executeDirectBridgePlanTransferAttempt,
  executeBridgePlanTransferAttempt,
  executeBridgePlanTransformAttempt,
  getDeviceProfile,
  getRoomControlSessionContext,
  joinRoom,
  listBridgePlanWorkspace,
  listReceivedRoomControlEvents,
  listNearbyDevices,
  requestNearbyJoin,
  revealInFolder,
  sendBridgePlanReviewRequest,
  startBridgePlanAttempt,
  startBridgePlanTransferAttempt,
  startBridgePlanTransformAttempt,
  selectBridgePlanSearchCandidate,
  sendTextToRoom,
  writeTempFile,
} from "../lib/tauri";
import {
  bridgeRoutePayload,
  enqueueTransferInputsWithBridgeRoute,
  sendTextToRoomWithBridgeRoute,
} from "../lib/bridgeRoutingRuntime";
import { bridgePeerSessionId, formatBridgeRouteErrorForUser, type BridgeRoute } from "../lib/bridgeRouting";
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
import { useAgentBridgeRuntimeConfig } from "../lib/agentBridge";
import {
  buildMockAiContextSnapshot,
  CloudOpenAICompatibleProvider,
  CLOUD_STRICT_AI_CONTEXT_POLICY,
  generateMockAskBridgeNaturalV1Plan,
  isSupportedBridgePlanSubmission,
  validateAskBridgeNaturalV1Plan,
  type AskBridgeNaturalV1Plan,
  type AiGenerateResult,
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
  ReceivedRoomControlEvent,
  RoomInfo,
  RoomItem,
} from "../lib/types";

type PrimaryRoute = "bridge" | "activity" | "devices" | "settings";
type BridgeTargetSelectionMode = "selected_peer" | "selected_peers" | "broadcast_bridge";
type SafeSearchScope = "downloads" | "desktop" | "documents" | "pastey_shared";

const SAFE_SEARCH_SCOPES: Array<{ value: SafeSearchScope; label: string }> = [
  { value: "downloads", label: "Downloads" },
  { value: "desktop", label: "Desktop" },
  { value: "documents", label: "Documents" },
  { value: "pastey_shared", label: "Pastey Shared" },
];
const BRIDGE_PLAN_REQUIRES_ONE_SELECTED_DEVICE = "Ask Bridge requires one selected device.";

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
  const bridgeConfig = useAgentBridgeRuntimeConfig();

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
        config={bridgeConfig}
        room={room}
        selectedPeer={selectedSinglePeer}
        route={selectedRoute}
        inboxEvents={bridgePlanInboxBatch}
      />

      <BridgePlanReceiverPanel
        room={room}
        route={selectedRoute}
        inboxEvents={bridgePlanInboxBatch}
        onRefresh={() => void refreshBridgeControlInbox()}
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

interface ReviewedBridgePlan {
  approvalId: string;
  description: string;
}

interface StartedBridgePlanAttempt {
  approvalId: string;
  attemptId: string;
}

interface StartedBridgePlanTransfer {
  approvalId: string;
  attemptId: string;
  requesterDirect: boolean;
}
interface StartedBridgePlanTransform {
  approvalId: string;
  attemptId: string;
}

function BridgePlanReceiverPanel({
  room,
  route,
  inboxEvents,
  onRefresh,
}: {
  room: RoomInfo;
  route: BridgeRoute | null;
  inboxEvents: readonly ReceivedRoomControlEvent[];
  onRefresh: () => void;
}) {
  const [decisions, setDecisions] = useState<Record<string, "allow" | "deny">>({});
  const [runningAttempts, setRunningAttempts] = useState<Record<string, "running" | "completed" | "failed">>({});
  const [message, setMessage] = useState<string | null>(null);
  const reviewedPlans = useMemo(
    () => inboxEvents.flatMap(parseReviewedBridgePlan),
    [inboxEvents],
  );
  const startedAttempts = useMemo(
    () => inboxEvents.flatMap(parseStartedBridgePlanAttempt),
    [inboxEvents],
  );
  const startedTransfers = useMemo(
    () => inboxEvents.flatMap(parseStartedBridgePlanTransfer),
    [inboxEvents],
  );
  const startedTransforms = useMemo(
    () => inboxEvents.flatMap(parseStartedBridgePlanTransform),
    [inboxEvents],
  );
  const singlePeerRoute = route?.target.kind === "selected_peer" ? route : null;

  useEffect(() => {
    setDecisions({});
    setRunningAttempts({});
    setMessage(null);
  }, [room.id]);

  useEffect(() => {
    let cancelled = false;
    void Promise.all(reviewedPlans.map(async (plan) => ({
      approvalId: plan.approvalId,
      decision: await bridgePlanReceiverReviewStatus(room.id, plan.approvalId),
    }))).then((statuses) => {
      if (cancelled) return;
      setDecisions((current) => {
        const next = { ...current };
        statuses.forEach(({ approvalId, decision }) => {
          if (decision) next[approvalId] = decision;
        });
        return next;
      });
    }).catch(() => {
      // An absent local decision is a safe pending state; the Rust decision
      // command remains authoritative when the user acts.
    });
    return () => {
      cancelled = true;
    };
  }, [reviewedPlans, room.id]);

  async function decide(plan: ReviewedBridgePlan, allow: boolean) {
    if (!singlePeerRoute) {
      setMessage("Select the requesting device before reviewing this plan.");
      return;
    }
    setMessage(null);
    try {
      await decideBridgePlanReview(
        room.id,
        plan.approvalId,
        allow,
        bridgeRoutePayload(singlePeerRoute, "pastey-bridge-control-route-v1"),
      );
      setDecisions((current) => ({ ...current, [plan.approvalId]: allow ? "allow" : "deny" }));
      setMessage(allow ? "Plan approved. Waiting for the requester to start it." : "Plan denied. No search will run.");
      onRefresh();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "The plan decision could not be sent.");
    }
  }

  async function runSearch(attempt: StartedBridgePlanAttempt) {
    if (!singlePeerRoute) {
      setMessage("Select the requesting device before running this plan.");
      return;
    }
    setRunningAttempts((current) => ({ ...current, [attempt.attemptId]: "running" }));
    setMessage("Searching the approved locations on this device…");
    try {
      await executeBridgePlanSearchAttempt(
        room.id,
        attempt.attemptId,
        bridgeRoutePayload(singlePeerRoute, "pastey-bridge-control-route-v1"),
      );
      setRunningAttempts((current) => ({ ...current, [attempt.attemptId]: "completed" }));
      setMessage("Search complete. The requester can see the result summary.");
      onRefresh();
    } catch (error) {
      setRunningAttempts((current) => ({ ...current, [attempt.attemptId]: "failed" }));
      setMessage(error instanceof Error ? error.message : "The approved search could not be completed.");
    }
  }

  async function runTransfer(attempt: StartedBridgePlanTransfer) {
    if (!singlePeerRoute) {
      setMessage("Select the requesting device before completing this transfer.");
      return;
    }
    setRunningAttempts((current) => ({ ...current, [`transfer:${attempt.attemptId}`]: "running" }));
    setMessage("Transferring the selected file to the requesting device…");
    try {
      await executeBridgePlanTransferAttempt(
        room.id,
        attempt.attemptId,
        bridgeRoutePayload(singlePeerRoute, "pastey-bridge-control-route-v1"),
      );
      setRunningAttempts((current) => ({ ...current, [`transfer:${attempt.attemptId}`]: "completed" }));
      setMessage("Transfer complete.");
      onRefresh();
    } catch (error) {
      setRunningAttempts((current) => ({ ...current, [`transfer:${attempt.attemptId}`]: "failed" }));
      setMessage(error instanceof Error ? error.message : "The approved transfer could not be completed.");
    }
  }

  async function runTransform(attempt: StartedBridgePlanTransform) {
    if (!singlePeerRoute) { setMessage("Select the requesting device before processing this file."); return; }
    setRunningAttempts((current) => ({ ...current, [`transform:${attempt.attemptId}`]: "running" }));
    setMessage("Processing the selected file with the approved local capability…");
    try {
      await executeBridgePlanTransformAttempt(room.id, attempt.attemptId, bridgeRoutePayload(singlePeerRoute, "pastey-bridge-control-route-v1"));
      setRunningAttempts((current) => ({ ...current, [`transform:${attempt.attemptId}`]: "completed" }));
      setMessage("Transform complete. The generated result remains on this device."); onRefresh();
    } catch (error) {
      setRunningAttempts((current) => ({ ...current, [`transform:${attempt.attemptId}`]: "failed" }));
      setMessage(error instanceof Error ? error.message : "The approved Transform could not be completed.");
    }
  }

  const approvedAttempts = startedAttempts.filter((attempt) => decisions[attempt.approvalId] === "allow");
  const approvedTransfers = startedTransfers.filter((attempt) => decisions[attempt.approvalId] === "allow");
  const approvedTransforms = startedTransforms.filter((attempt) => decisions[attempt.approvalId] === "allow");
  if (reviewedPlans.length === 0 && approvedAttempts.length === 0 && approvedTransfers.length === 0 && approvedTransforms.length === 0 && !message) return null;

  return (
    <Card className="ask-bridge-card" aria-label="Received Ask Bridge plan">
      <div className="section-row">
        <div>
          <h2>Ask Bridge</h2>
          <p className="muted">Plans from the selected device need your review before Pastey searches this device.</p>
        </div>
      </div>
      {reviewedPlans.map((plan) => {
        const decision = decisions[plan.approvalId];
        return (
          <div className="request-file-preview" key={plan.approvalId}>
            <h3>Review plan</h3>
            <p>{plan.description}</p>
            {!decision ? (
              <div className="button-row">
                <button type="button" className="secondary-button" disabled={!singlePeerRoute} onClick={() => void decide(plan, false)}>
                  Deny
                </button>
                <button type="button" className="primary-button" disabled={!singlePeerRoute} onClick={() => void decide(plan, true)}>
                  Allow plan
                </button>
              </div>
            ) : (
              <p className={decision === "allow" ? "success-text" : "danger-text"}>
                {decision === "allow" ? "Approved on this device." : "Denied on this device."}
              </p>
            )}
          </div>
        );
      })}
      {approvedAttempts.map((attempt) => {
        const status = runningAttempts[attempt.attemptId];
        return (
          <div className="request-file-preview" key={attempt.attemptId}>
            <h3>Approved plan ready</h3>
            <p>Run the approved search on this device. Pastey will search only the reviewed locations and return a summary.</p>
            {status === "completed" ? <p className="success-text">Search complete.</p> : null}
            {status === "failed" ? <p className="danger-text">Search did not complete. Start a new approved attempt to try again.</p> : null}
            {status !== "completed" ? (
              <button type="button" className="primary-button" disabled={!singlePeerRoute || status === "running"} onClick={() => void runSearch(attempt)}>
                {status === "running" ? "Searching…" : "Run search"}
              </button>
            ) : null}
          </div>
        );
      })}
      {approvedTransfers.map((attempt) => {
        const status = runningAttempts[`transfer:${attempt.attemptId}`];
        return (
          <div className="request-file-preview" key={`transfer-${attempt.attemptId}`}>
            <h3>{attempt.requesterDirect ? "Approved incoming transfer" : "Approved transfer ready"}</h3>
            <p>{attempt.requesterDirect ? "The requesting device is transferring its reviewed local file to this device." : "Transfer the file selected by the requester from the reviewed search results."}</p>
            {status === "completed" ? <p className="success-text">Transfer complete.</p> : null}
            {status === "failed" ? <p className="danger-text">Transfer did not complete. Start a new plan to try again.</p> : null}
            {!attempt.requesterDirect && status !== "completed" ? (
              <button type="button" className="primary-button" disabled={!singlePeerRoute || status === "running"} onClick={() => void runTransfer(attempt)}>
                {status === "running" ? "Transferring…" : "Transfer selected file"}
              </button>
            ) : null}
          </div>
        );
      })}
      {approvedTransforms.map((attempt) => {
        const status = runningAttempts[`transform:${attempt.attemptId}`];
        return <div className="request-file-preview" key={`transform-${attempt.attemptId}`}><h3>Approved transform ready</h3><p>Process the requester-selected file with the reviewed local capability.</p>{status === "completed" ? <p className="success-text">Transform complete; the result remains local.</p> : null}{status === "failed" ? <p className="danger-text">Transform did not complete. Start a new approved plan to try again.</p> : null}{status !== "completed" ? <button type="button" className="primary-button" disabled={!singlePeerRoute || status === "running"} onClick={() => void runTransform(attempt)}>{status === "running" ? "Processing…" : "Process selected file"}</button> : null}</div>;
      })}
      {message ? <p className="muted" role="status">{message}</p> : null}
    </Card>
  );
}

function BridgePlanSenderPanel({
  enabled,
  config,
  room,
  selectedPeer,
  route,
  inboxEvents,
}: {
  enabled: boolean;
  config: ReturnType<typeof useAgentBridgeRuntimeConfig>;
  room: RoomInfo;
  selectedPeer: BridgePeerSession | null;
  route: BridgeRoute | null;
  inboxEvents: readonly ReceivedRoomControlEvent[];
}) {
  const [input, setInput] = useState("");
  const [advisory, setAdvisory] = useState<AskBridgeNaturalV1Plan | null>(null);
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
  const transformedAttemptIds = useMemo(
    () => new Set(inboxEvents.flatMap(parseCompletedBridgePlanTransform)),
    [inboxEvents],
  );
  const failedTransformAttemptIds = useMemo(
    () => new Set(inboxEvents.flatMap(parseFailedBridgePlanTransform)),
    [inboxEvents],
  );

  useEffect(() => {
    setAdvisory(null);
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

  async function createPlan() {
    if (!canPlan || !selectedPeerRoute) {
      setMessage(BRIDGE_PLAN_REQUIRES_ONE_SELECTED_DEVICE);
      return;
    }
    const userGoal = input.trim();
    if (!userGoal) {
      setMessage("Describe the file you want to search for.");
      return;
    }
    setBusy("plan");
    setMessage(null);
    try {
      const generated = config.providerKind === "cloud" && config.cloudBaseUrl.trim() && config.cloudModel.trim() && config.cloudApiKey.trim()
        ? await generateCloudNaturalPlan(userGoal, config)
        : await generateMockAskBridgeNaturalV1Plan(userGoal);
      const validation = validateAskBridgeNaturalV1Plan(generated.parsedPlan);
      if (!validation.valid || validation.value.status !== "supported") {
        setAdvisory(null);
        setMessage(validation.valid ? validation.value.unsupportedReason ?? "This plan is not supported." : validation.errors.join(" "));
        return;
      }
      const search = validation.value.steps.find((step) => step.primitive === "Search");
      const transform = validation.value.steps.find((step) => step.primitive === "Transform");
      const transfer = validation.value.steps.find((step) => step.primitive === "Transfer");
      const transferToRequester = Boolean(transfer
        && transfer.primitive === "Transfer"
        && transfer.destination === "requesting_device");
      const supportsTransfer = Boolean(
        transfer
        && transfer.primitive === "Transfer"
        && (transfer.destination === "requesting_device" || transfer.destination === "selected_device")
        && (validation.value.steps.length === 2 || (validation.value.steps.length === 3 && Boolean(transform))),
      );
      const supportsTransform = Boolean(transform
        && transform.primitive === "Transform"
        && (validation.value.steps.length === 2 || (validation.value.steps.length === 3 && supportsTransfer)));
      if (!search || !isSupportedBridgePlanSubmission(validation.value) || (!supportsTransfer && validation.value.steps.length !== 1 && !supportsTransform)) {
        setAdvisory(validation.value);
        setMessage("Pastey can currently run Search, Search followed by Transfer to the requesting device, and supported readable-text Transform plans. Other reviewed combinations are not available yet.");
        return;
      }
      const workspace = supportsTransform
        ? await createFileTransformBridgePlan({
          roomId: room.id,
          originalUserGoal: userGoal,
          filenameHint: search.filenameHint,
          extensions: search.extensions,
          safeScopes: search.safeScopes,
          transferToRequester: supportsTransfer,
          transferDestination: transfer?.primitive === "Transfer" && transfer.destination === "selected_device" ? "selected_device" : "requesting_device",
          transformIntent: transform?.primitive === "Transform" ? transform.intent : "process the selected file",
        })
        : await createFileSearchBridgePlan({
          roomId: room.id,
          originalUserGoal: userGoal,
          filenameHint: search.filenameHint,
          extensions: search.extensions,
          safeScopes: search.safeScopes,
          transferToRequester: supportsTransfer,
          transferDestination: transfer?.primitive === "Transfer" && transfer.destination === "selected_device" ? "selected_device" : "requesting_device",
        });
      const revision = workspace.revisions.map(parseBridgePlanRevision).filter((entry): entry is { revisionId: string; state: string } => entry?.state === "available").pop();
      if (!revision) throw new Error("Pastey did not return the durable Search plan.");
      setAdvisory(validation.value);
      setRevisionId(revision.revisionId);
      setApprovalId(null);
      setApprovalState(null);
      setResultSummary(null);
      setHasTransform(supportsTransform);
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
    setBusy("review");
    setMessage(null);
    try {
      const nextApprovalId = `bridge-plan-approval-${crypto.randomUUID()}`;
      await approveBridgePlan(revisionId, nextApprovalId, true);
      await sendBridgePlanReviewRequest(
        nextApprovalId,
        bridgeRoutePayload(selectedPeerRoute, "pastey-bridge-control-route-v1"),
      );
      setApprovalId(nextApprovalId);
      setApprovalState("awaiting_receiver");
      setMessage("Waiting for the selected device to review the complete plan.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not send the plan for review.");
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
      setAdvisory({ schemaVersion: "ask-bridge-natural-v1", title: "Transfer a file", status: "supported", requiresUserConfirmation: true, steps: [{ primitive: "Transfer", destination: "selected_device", object: "selected_file" }] });
      setRevisionId(revision.revisionId);
      setApprovalId(null); setApprovalState(null); setAttemptId(null); setSelectedCandidateId(null);
      setHasTransform(false); setDirectTransfer(true);
      setMessage("Plan ready. Review the complete Transfer plan before sending it to the selected device.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not create the direct Transfer plan.");
    } finally { setBusy(null); }
  }

  async function startAttempt() {
    if (!approvalId || !selectedPeerRoute) return;
    setBusy("start");
    setMessage(null);
    try {
      const attemptId = `bridge-plan-attempt-${crypto.randomUUID()}`;
      await startBridgePlanAttempt(
        approvalId,
        attemptId,
        bridgeRoutePayload(selectedPeerRoute, "pastey-bridge-control-route-v1"),
      );
      setApprovalState("running");
      if (directTransfer) {
        await executeDirectBridgePlanTransferAttempt(room.id, attemptId);
        setMessage("Transfer complete.");
      } else {
        setMessage("The selected device can now run the approved Search.");
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not start the approved plan.");
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
      if (hasTransform) await startBridgePlanTransformAttempt(room.id, attemptId, bridgeRoutePayload(selectedPeerRoute, "pastey-bridge-control-route-v1"));
      else await startBridgePlanTransferAttempt(room.id, attemptId, bridgeRoutePayload(selectedPeerRoute, "pastey-bridge-control-route-v1"));
      setSelectedCandidateId(candidateId);
      setMessage(hasTransform ? "The selected device can now process the chosen file." : "The selected device can now transfer the chosen file.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not start the approved transfer.");
    } finally {
      setBusy(null);
    }
  }

  async function transferGeneratedResult() {
    if (!attemptId || !selectedPeerRoute) return;
    setBusy("start");
    setMessage(null);
    try {
      await startBridgePlanTransferAttempt(room.id, attemptId, bridgeRoutePayload(selectedPeerRoute, "pastey-bridge-control-route-v1"));
      setMessage("The selected device can now transfer the generated result.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not start the approved transfer.");
    } finally { setBusy(null); }
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
          <p className="muted">Create one complete, reviewable plan for the selected device. Pastey never runs it until both devices approve the plan.</p>
        </div>
      </div>
      <textarea
        value={input}
        onChange={(event) => setInput(event.target.value)}
        placeholder="Find my report PDF on this device"
        aria-label="Ask Bridge request"
      />
      <div className="button-row">
        <button type="button" className="primary-button" disabled={!canPlan || busy !== null} onClick={() => void createPlan()}>
          {busy === "plan" ? "Planning…" : "Create plan"}
        </button>
        <button type="button" className="secondary-button" disabled={!canPlan || busy !== null} onClick={() => void createDirectTransferPlan()}>
          Transfer local file
        </button>
      </div>
      {!canPlan ? <p className="muted">Select one connected device to create a plan.</p> : null}
      {advisory && revisionId ? (
        <div className="request-file-preview" data-testid="ask-bridge-plan-preview">
          <h3>Review plan</h3>
          <p>{directTransfer ? "Transfer the one local file you chose to the selected device after both devices approve this plan." : advisory.steps.some((step) => step.primitive === "Transform") ? "Search the selected device’s reviewed locations, let you choose one bounded result, then process it locally with the reviewed capability." : advisory.steps.some((step) => step.primitive === "Transfer") ? "Search the selected device’s reviewed locations, let you choose one bounded result, then transfer it to the approved destination." : "Search the selected device’s reviewed locations for matching files and return a bounded summary."}</p>
          {!approvalId ? (
            <button type="button" className="primary-button" disabled={busy !== null} onClick={() => void requestReview()}>
              {busy === "review" ? "Sending…" : "Approve and send for review"}
            </button>
          ) : approvalState === "valid" ? (
            <button type="button" className="primary-button" disabled={busy !== null} onClick={() => void startAttempt()}>
              {busy === "start" ? "Starting…" : "Start approved plan"}
            </button>
          ) : null}
        </div>
      ) : null}
      {approvalState === "awaiting_receiver" ? <p className="muted">Waiting for receiver review.</p> : null}
      {approvalState === "denied" ? <p className="danger-text">The selected device denied this plan. Create a revised plan to try again.</p> : null}
      {approvalState === "running" ? <p className="muted">Search is running on the selected device.</p> : null}
      {resultSummary ? <p className="success-text">{resultSummary}</p> : null}
      {safeSearchCandidates.length > 0 && advisory?.steps.some((step) => step.primitive === "Transfer" || step.primitive === "Transform") && !selectedCandidateId ? (
        <div className="candidate-card-list">
          <h3>Choose a file for the approved next step</h3>
          {safeSearchCandidates.map((candidate) => (
            <button key={candidate.candidateId} type="button" className="candidate-metadata-card" disabled={busy !== null} onClick={() => void selectCandidate(candidate.candidateId)}>
              <strong>{candidate.displayName}</strong>
              <span>{formatBytes(candidate.sizeBytes)} · {candidate.extension || candidate.mimeFamily}</span>
              <small>{candidate.matchReason}</small>
            </button>
          ))}
        </div>
      ) : null}
      {hasTransform && selectedCandidateId && attemptId && transformedAttemptIds.has(attemptId) && advisory?.steps.some((step) => step.primitive === "Transfer") ? (
        <div className="request-file-preview"><h3>Generated result ready</h3><p>The approved Transform finished on the selected device. Transfer the generated result to the approved destination.</p><button type="button" className="primary-button" disabled={busy !== null} onClick={() => void transferGeneratedResult()}>{busy === "start" ? "Starting…" : "Transfer generated result"}</button></div>
      ) : null}
      {hasTransform && attemptId && failedTransformAttemptIds.has(attemptId) ? (
        <div className="request-file-preview"><h3>Processing unavailable</h3><p>The selected device could not perform the approved Transform for this file. Create a new plan revision without that step; both devices must review it again.</p><button type="button" className="secondary-button" disabled={busy !== null} onClick={() => void proposeTransformFallback()}>{busy === "plan" ? "Preparing…" : "Create revised plan"}</button></div>
      ) : null}
      {message ? <p className="muted" role="status">{message}</p> : null}
    </Card>
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

function parseReviewedBridgePlan(event: ReceivedRoomControlEvent): ReviewedBridgePlan[] {
  if (event.kind !== "bridge_plan.review_request") return [];
  const payload = roomControlEventPayload(event.event);
  const approvalId = typeof payload?.approvalId === "string" ? payload.approvalId : null;
  const revision = isRecord(payload?.revision) ? payload.revision : null;
  const presentation = isRecord(revision?.presentation) ? revision.presentation : null;
  const description = typeof presentation?.natural_language_plan === "string"
    ? presentation.natural_language_plan.trim()
    : "";
  return approvalId && description ? [{ approvalId, description }] : [];
}

function parseStartedBridgePlanAttempt(event: ReceivedRoomControlEvent): StartedBridgePlanAttempt[] {
  if (event.kind !== "bridge_plan.attempt_start") return [];
  const payload = roomControlEventPayload(event.event);
  const approvalId = typeof payload?.approvalId === "string" ? payload.approvalId : null;
  const attemptId = typeof payload?.attemptId === "string" ? payload.attemptId : null;
  return approvalId && attemptId ? [{ approvalId, attemptId }] : [];
}

function parseStartedBridgePlanTransfer(event: ReceivedRoomControlEvent): StartedBridgePlanTransfer[] {
  if (event.kind !== "bridge_plan.transfer_start") return [];
  const payload = roomControlEventPayload(event.event);
  const approvalId = typeof payload?.approvalId === "string" ? payload.approvalId : null;
  const attemptId = typeof payload?.attemptId === "string" ? payload.attemptId : null;
  const transferStep = isRecord(payload?.transferStep) ? payload.transferStep : null;
  const source = isRecord(transferStep?.source) ? transferStep.source : null;
  return approvalId && attemptId
    ? [{ approvalId, attemptId, requesterDirect: source?.kind === "future_user_selection" }]
    : [];
}

function parseStartedBridgePlanTransform(event: ReceivedRoomControlEvent): StartedBridgePlanTransform[] {
  if (event.kind !== "bridge_plan.transform_start") return [];
  const payload = roomControlEventPayload(event.event);
  const approvalId = typeof payload?.approvalId === "string" ? payload.approvalId : null;
  const attemptId = typeof payload?.attemptId === "string" ? payload.attemptId : null;
  return approvalId && attemptId ? [{ approvalId, attemptId }] : [];
}

function parseCompletedBridgePlanTransform(event: ReceivedRoomControlEvent): string[] {
  if (event.kind !== "bridge_plan.step_result") return [];
  const payload = roomControlEventPayload(event.event);
  return payload?.stepId === "transform" && typeof payload.attemptId === "string" ? [payload.attemptId] : [];
}

function parseFailedBridgePlanTransform(event: ReceivedRoomControlEvent): string[] {
  if (event.kind !== "bridge_plan.step_failed") return [];
  const payload = roomControlEventPayload(event.event);
  return payload?.stepId === "transform" && typeof payload.attemptId === "string" ? [payload.attemptId] : [];
}

function parseBridgePlanSearchCandidates(event: ReceivedRoomControlEvent): Array<{
  attemptId: string;
  candidateId: string;
  displayName: string;
  extension: string;
  mimeFamily: string;
  sizeBytes: number;
  matchReason: string;
}> {
  if (event.kind !== "bridge_plan.step_result") return [];
  const payload = roomControlEventPayload(event.event);
  const attemptId = typeof payload?.attemptId === "string" ? payload.attemptId : null;
  const safeResult = isRecord(payload?.safeResult) ? payload.safeResult : null;
  const candidates = Array.isArray(safeResult?.candidates) ? safeResult.candidates : [];
  if (!attemptId) return [];
  return candidates.flatMap((candidate) => {
    if (!isRecord(candidate)
      || typeof candidate.candidateId !== "string"
      || typeof candidate.displayName !== "string"
      || typeof candidate.extension !== "string"
      || typeof candidate.mimeFamily !== "string"
      || typeof candidate.sizeBytes !== "number"
      || typeof candidate.matchReason !== "string") return [];
    return [{
      attemptId,
      candidateId: candidate.candidateId,
      displayName: candidate.displayName,
      extension: candidate.extension,
      mimeFamily: candidate.mimeFamily,
      sizeBytes: candidate.sizeBytes,
      matchReason: candidate.matchReason,
    }];
  });
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

async function generateCloudNaturalPlan(
  userRequest: string,
  config: ReturnType<typeof useAgentBridgeRuntimeConfig>,
): Promise<AiGenerateResult> {
  const provider = new CloudOpenAICompatibleProvider({
    providerId: "pastey-cloud-openai-compatible-natural-v1",
    displayName: "CloudOpenAICompatibleProvider",
    kind: "cloud_openai_compatible",
    apiShape: "openai_compatible_chat",
    baseUrl: config.cloudBaseUrl,
    model: config.cloudModel,
    apiKeyRef: config.cloudApiKey ? "runtime-memory-only" : undefined,
    timeoutMs: 30_000,
    maxOutputTokens: 512,
    enabled: true,
  }, {
    apiKey: config.cloudApiKey,
  });
  const generated = await provider.generate({
    requestId: `ask-bridge-natural-cloud-${Date.now()}`,
    providerId: provider.config.providerId,
    context: buildMockAiContextSnapshot(),
    contextPolicy: CLOUD_STRICT_AI_CONTEXT_POLICY,
    allowedActionKinds: [],
    outputSchema: "ask-bridge-natural-v1",
    userRequest,
  });
  if (!generated.error) return generated;
  const fallback = await generateMockAskBridgeNaturalV1Plan(userRequest);
  return {
    ...fallback,
    error: generated.error,
  };
}

function searchScopesForPlan(plan: AskBridgeNaturalV1Plan | null): SafeSearchScope[] {
  const search = plan?.steps.find((step) => step.primitive === "Search");
  return search?.primitive === "Search" ? search.safeScopes : ["downloads", "desktop", "documents", "pastey_shared"];
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

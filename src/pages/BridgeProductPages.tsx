import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState, type ClipboardEvent, type ReactNode } from "react";
import {
  copyTextToClipboard,
  joinRoom,
  listNearbyDevices,
  requestNearbyJoin,
  revealInFolder,
  sendTextToRoom,
  writeTempFile,
} from "../lib/tauri";
import {
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
  findBridgePeerBySessionId,
  getRouteableBridgePeers,
  type BridgePeerSession,
} from "../lib/bridgePeers";
import {
  buildCandidatePayloadWorkflowPayloadPreview,
  confirmCandidatePayloadWorkflowSearch,
  createCandidatePayloadWorkflow,
  markCandidatePayloadWorkflowPayloadPendingConsent,
  startCandidatePayloadWorkflowFromSearchAdvisory,
  type CandidatePayloadWorkflow,
} from "../lib/agentBridge";
import { buildMockAiContextSnapshot, buildMockFileCandidatePlan } from "../lib/ai";
import { FILE_TOO_LARGE_MESSAGE, MAX_FILE_SIZE_BYTES } from "../lib/constants";
import { formatBytes, formatCode, formatTimestamp } from "../lib/format";
import type { TransferQueueInput, TransferQueueItem } from "../lib/transferScheduler";
import type {
  FileTransferProgressEvent,
  JoinRequestPrompt,
  NearbyDevice,
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
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const routeablePeers = useRouteablePeers(room);
  const remotePeers = routeablePeers.filter((peer) => peer.isLocalSelf !== true);
  const selectedRoute = useMemo(
    () => buildSelectedBridgeRoute(bridgeSessionId(room), remotePeers, targetMode, selectedPeerIds),
    [room.id, remotePeers, selectedPeerIds, targetMode],
  );
  const selectedPeers = selectedRoute ? resolvedPeersForRoute(selectedRoute, remotePeers) : [];
  const selectedSinglePeer = selectedRoute?.target.kind === "selected_peer" ? selectedPeers[0] ?? null : null;
  const canSend = room.status === "active" && room.peer_connected && selectedRoute !== null && selectedPeers.length > 0 && busy === null;

  useEffect(() => {
    if (remotePeers.length === 0) {
      setSelectedPeerIds([]);
      return;
    }
    setSelectedPeerIds((current) => {
      const routeableIds = new Set(remotePeers.map((peer) => peer.peerSessionId));
      const next = current.filter((peerId) => routeableIds.has(bridgePeerSessionId(peerId)));
      return next.length > 0 ? next : [remotePeers[0].peerSessionId];
    });
  }, [remotePeers]);

  useEffect(() => {
    composerRef.current?.focus();
  }, [room.id]);

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
          enqueueSelectedRouteFiles(event.payload.paths, "file");
        }
        return;
      }
      setDropActive(false);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [canSend, selectedRoute, selectedPeers, room.id]);

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
        <MemberChip title="This Mac" subtitle="This device" you />
        {remotePeers.length === 0 ? <span className="muted">No connected members yet.</span> : null}
        {remotePeers.map((peer) => (
          <MemberChip key={peer.peerSessionId} title={peer.displayName} subtitle={memberStatus(peer)} />
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
          <span>Ask the selected device to find, prepare, or send something.</span>
        </button>
      </div>

      {requestOpen ? (
        <RequestFilePanel
          room={room}
          selectedPeer={selectedSinglePeer}
          route={selectedRoute}
          onEnqueueCandidatePayloadHandoff={() => false}
        />
      ) : null}

      {askOpen ? (
        <Card>
          <div className="section-row">
            <div>
              <h2>Ask Bridge Beta</h2>
              <p className="muted">Ask the selected device to find, prepare, or send something.</p>
            </div>
            <StatusChip tone={askBridgeBetaEnabled && selectedSinglePeer ? "success" : "neutral"}>
              {askBridgeBetaEnabled ? "Beta" : "Disabled"}
            </StatusChip>
          </div>
          <p className="muted">
            {askBridgeBetaEnabled
              ? selectedSinglePeer
                ? `Selected device: ${selectedSinglePeer.displayName}.`
                : "Ask Bridge Beta requires one selected device."
              : "Enable Labs in Settings to use Ask Bridge Beta."}
          </p>
        </Card>
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

function RequestFilePanel({
  selectedPeer,
}: {
  room: RoomInfo;
  selectedPeer: BridgePeerSession | null;
  route: BridgeRoute | null;
  onEnqueueCandidatePayloadHandoff: (roomId: string, input: TransferQueueInput) => boolean;
}) {
  const [query, setQuery] = useState("");
  const [scopes, setScopes] = useState<SafeSearchScope[]>(["downloads", "desktop", "documents", "pastey_shared"]);
  const [workflow, setWorkflow] = useState<CandidatePayloadWorkflow>(() => createCandidatePayloadWorkflow());
  const [selectedCandidateId, setSelectedCandidateId] = useState("");
  const [message, setMessage] = useState<string | null>(selectedPeer ? null : "Request file requires one selected device.");
  const candidates = workflow.snapshot.candidates ?? [];
  const canRequestSearch = Boolean(selectedPeer && query.trim() && scopes.length > 0);
  const canRequestFile = Boolean(selectedPeer && selectedCandidateId && workflow.snapshot.state === "candidate_selection_required");

  function toggleScope(scope: SafeSearchScope) {
    setScopes((current) => current.includes(scope)
      ? current.filter((candidate) => candidate !== scope)
      : [...current, scope]);
  }

  function handleRequestSearch() {
    if (!selectedPeer) {
      setMessage("Request file requires one selected device.");
      return;
    }
    const prepared = prepareCandidateSearchWorkflow(query, scopes, selectedPeer.peerSessionId);
    const started = startCandidatePayloadWorkflowFromSearchAdvisory(createCandidatePayloadWorkflow(), prepared.plan, prepared.context);
    const confirmed = started.ok ? confirmCandidatePayloadWorkflowSearch(started.workflow) : null;
    setWorkflow(confirmed?.ok ? confirmed.workflow : started.workflow);
    setSelectedCandidateId("");
    setMessage(confirmed?.ok
      ? "Request search is ready. The selected device must approve before results appear."
      : started.ok
        ? "Request search is ready."
        : started.errors.join(" "));
  }

  function handleRequestSelectedFile() {
    if (!selectedPeer || !selectedCandidateId) {
      setMessage("Choose one result first.");
      return;
    }
    const prepared = prepareCandidateSearchWorkflow(query || "selected file", scopes, selectedPeer.peerSessionId);
    const preview = buildCandidatePayloadWorkflowPayloadPreview(
      workflow,
      { candidateId: selectedCandidateId, selectedByUser: true },
      prepared.context,
    );
    if (!preview.ok) {
      setWorkflow(preview.workflow);
      setMessage(preview.errors.join(" "));
      return;
    }
    const pending = markCandidatePayloadWorkflowPayloadPendingConsent(preview.workflow);
    setWorkflow(pending.ok ? pending.workflow : preview.workflow);
    setMessage("Request selected file is ready. The selected device must approve before sending starts.");
  }

  return (
    <Card className="request-file-panel">
      <div className="section-row">
        <div>
          <h2>Request file</h2>
          <p className="muted">{selectedPeer ? `From ${selectedPeer.displayName}` : "Request file requires one selected device."}</p>
        </div>
        <StatusChip tone={selectedPeer ? "success" : "neutral"}>{selectedPeer ? "Selected" : "Choose device"}</StatusChip>
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
        <button type="button" className="primary-button" disabled={!canRequestSearch} onClick={handleRequestSearch}>
          Request search
        </button>
        <button type="button" className="secondary-button" disabled={!canRequestFile} onClick={handleRequestSelectedFile}>
          Request selected file
        </button>
      </div>
      {candidates.length > 0 ? (
        <div className="candidate-card-list">
          {candidates.map((candidate) => (
            <button
              key={candidate.candidateId}
              type="button"
              className={`candidate-metadata-card ${selectedCandidateId === candidate.candidateId ? "selected" : ""}`}
              onClick={() => setSelectedCandidateId(candidate.candidateId)}
            >
              <strong>{candidate.candidateDisplayName}</strong>
              <span>{formatBytes(candidate.sizeBytes)} - {candidate.extension || candidate.mimeFamily}</span>
              <small>{candidate.matchReason}</small>
            </button>
          ))}
        </div>
      ) : null}
      {message ? <p className="muted">{message}</p> : null}
    </Card>
  );
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

function useRouteablePeers(room: RoomInfo): BridgePeerSession[] {
  return useMemo(() => {
    try {
      return [...getRouteableBridgePeers(legacyRoomToBridgePeerCollection(room))];
    } catch {
      return [];
    }
  }, [room]);
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
  const itemRows = roomItems.map((item): ActivityListRow => ({
    id: `item:${item.id}`,
    group: item.status === "failed" ? "failed" : item.direction === "incoming" ? "received" : "sent",
    title: item.direction === "incoming" ? `You received ${itemTitle(item)}` : `You sent ${itemTitle(item)}`,
    detail: item.direction === "incoming" ? "From device" : "To device",
    bridge: bridgeCode(roomById.get(item.room_id)),
    status: roomItemStatusLabel(item.status),
    tone: item.status === "failed" ? "danger" : "success",
    savedPath: item.saved_path,
  }));
  return [...transferRows, ...queueRows, ...itemRows].sort((a, b) => a.id < b.id ? 1 : -1);
}

function ActivityRow({ row, compact = false }: { row: ActivityListRow; compact?: boolean }) {
  return (
    <article className={`activity-row ${compact ? "compact" : ""}`}>
      <div>
        <strong>{row.title}</strong>
        <span className="muted">{row.detail} - Bridge {row.bridge}</span>
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

function memberStatus(peer: BridgePeerSession): string {
  return peer.liveness === "connected" ? "Connected" : peer.liveness;
}

function lastActivityForBridge(room: RoomInfo, items: RoomItem[], queueItems: TransferQueueItem[]): string {
  const latestItem = items.filter((item) => item.room_id === room.id).sort((a, b) => b.created_at - a.created_at)[0];
  const latestQueue = queueItems.filter((item) => item.roomId === room.id).sort((a, b) => b.updatedAt - a.updatedAt)[0];
  const latest = Math.max(latestItem?.created_at ?? 0, latestQueue?.updatedAt ?? 0, room.created_at);
  return latest ? formatTimestamp(latest) : "Recent";
}

function itemTitle(item: RoomItem): string {
  if (item.display_name?.trim()) return item.display_name;
  if (item.text?.trim()) return item.text.trim().slice(0, 80);
  return item.payload_type === "text" ? "text" : "file";
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

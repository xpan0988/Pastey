import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useMemo, useRef, useState } from "react";
import type { BridgeRoute } from "../../lib/bridgeRouting";
import { sendTextToRoomWithBridgeRoute } from "../../lib/bridgeRoutingRuntime";
import { legacyRoomToBridgePeerCollection } from "../../lib/bridgeRoomAdapter";
import { nativeDecisionsAwaitUser } from "../../lib/nativeDecisions";
import { ownAsyncDisposer } from "../../lib/subscriptionLifecycle";
import { copyTextToClipboard, presentDeviceIdentity, sendTextToRoom } from "../../lib/tauri";
import type { TransferQueueInput, TransferQueueItem } from "../../lib/transferScheduler";
import type { DeveloperModeUiSession, DeveloperTerminalWorkspace, FileTransferProgressEvent, RoomInfo, RoomItem } from "../../lib/types";
import { nativeAgentRecoveryRequiresAction, StatusBadge, useNativeAgentTask, type AgentTaskController } from "./AgentTaskLifecycle";
import { BridgeDetails } from "./BridgeDetails";
import { bridgeStatus } from "./BridgeNetworkPanel";
import { DeveloperModeScreen } from "./DeveloperModeScreen";
import { AgentWorkspace } from "./AgentWorkspace";
import { DeviceDetails } from "./DeviceDetails";
import { RemoteFileFinder } from "./RemoteFileFinder";
import { Dialog, Dot, EmptyState, Icon, MenuButton, Notice, Tabs } from "./ui";
import { FullAccessBanner } from "./FullAccessSettings";
import { bridgeCode, fileName, formatBytes, formatClock, roomMembers, roomPeers } from "./workspaceViewModel";

export type BridgeTab = "share" | "agents";

interface BridgeWorkspaceProps {
  room: RoomInfo | null;
  items: RoomItem[];
  queueItems: TransferQueueItem[];
  transfers: FileTransferProgressEvent[];
  task: AgentTaskController;
  developerMode: boolean;
  developerSession: DeveloperModeUiSession | null;
  developerWorkspace: DeveloperTerminalWorkspace;
  onDeveloperSession: (session: DeveloperModeUiSession) => void;
  onRefreshDeveloper: () => Promise<void>;
  onNewBridge: () => void;
  onRefresh: () => Promise<void>;
  onDeveloper: () => void;
  onBurn: (room: RoomInfo) => Promise<void>;
  onEnqueue: (roomId: string, inputs: TransferQueueInput[]) => void;
  onRevealInFolder: (path: string) => Promise<void>;
  onOpenSettings: (route?: "settings-permissions" | "settings-devices" | "settings-this-device") => void;
  menuButton?: React.ReactNode;
}

export function BridgeWorkspace(props: BridgeWorkspaceProps) {
  if (!props.room) return <EmptyBridge onNewBridge={props.onNewBridge} menuButton={props.menuButton} />;
  return <BridgeView key={props.room.id} {...props} room={props.room} />;
}

function BridgeView({ room, items, queueItems, transfers, task, developerMode, developerSession, developerWorkspace, onDeveloperSession, onRefreshDeveloper, onRefresh, onDeveloper, onBurn, onEnqueue, onRevealInFolder, onOpenSettings, menuButton }: BridgeWorkspaceProps & { room: RoomInfo }) {
  const [tab, setTab] = useState<BridgeTab>("share");
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [deviceOpen, setDeviceOpen] = useState<string | null>(null);
  const peers = useMemo(() => roomPeers(room), [room]);
  const members = useMemo(() => roomMembers(room), [room]);
  const agent = useNativeAgentTask(room.id);
  const status = bridgeStatus(room);
  const title = members.length === 0 ? "Waiting for a device" : members.length === 1 ? members[0].displayName : `${members.length + 1} devices`;
  const agentActive = agent.status?.state === "running" || agent.status?.state === "queued";
  usePresentIdentityToPeers(room.id, peers);
  const agentNeedsYou = nativeAgentRecoveryRequiresAction(agent.status, agent.movement)
    || agent.movement?.state === "awaiting_approval"
    || (!!agent.status && nativeDecisionsAwaitUser(agent.status, !!agent.taskHostRef, Date.now() / 1000));

  return <div className={`bridge${detailsOpen ? " with-inspector" : ""}`}>
    <section className="bridge-main page" aria-label={`Bridge ${bridgeCode(room)}`}>
      <header className="page-header">
        {menuButton}
        <div className="page-header-main">
          <div className="bridge-title">{members.length === 1 && members[0].hostRef ? <button type="button" className="title-button" onClick={() => setDeviceOpen(members[0].hostRef!)} title="Device details"><h1>{title}</h1></button> : <h1>{title}</h1>}<button type="button" className="status-pill" onClick={() => setDetailsOpen(true)} aria-label={`Bridge status: ${status.label}. Open details`}><Dot tone={status.tone} />{status.label}</button></div>
          <p>Bridge <span className="mono">{bridgeCode(room)}</span>{developerMode ? " · Developer Mode" : ""}</p>
        </div>
        <div className="page-header-actions">
          {developerMode ? <button type="button" className="btn btn-sm" onClick={onDeveloper}><Icon name="back" small />Back to Bridge</button> : null}
          <button type="button" className={`btn btn-ghost btn-icon${detailsOpen ? " active" : ""}`} aria-label="Bridge details" title="Bridge details" aria-pressed={detailsOpen} onClick={() => setDetailsOpen(!detailsOpen)}><Icon name="panel" /></button>
          <MenuButton label="More Bridge actions">{(close) => <>
            <button type="button" role="menuitem" disabled={!peers.length || developerMode} onClick={() => { close(); onDeveloper(); }}><Icon name="terminal" /><span>Developer Mode<small>Request a terminal on the other device</small></span></button>
            <button type="button" role="menuitem" onClick={() => { close(); setDetailsOpen(true); }}><Icon name="panel" /><span>Bridge details<small>Connection, devices, Full Access, permissions, Burn</small></span></button>
          </>}</MenuButton>
        </div>
      </header>
      {!developerMode ? <FullAccessBanner room={room} peerName={members.find((member) => member.hostRef)?.displayName ?? "the other device"} onOpenDetails={() => setDetailsOpen(true)} /> : null}
      {developerMode ? <div className="panel-scroll"><div className="panel-inner"><DeveloperModeScreen room={room} session={developerSession} workspace={developerWorkspace} onSession={onDeveloperSession} onRefresh={onRefreshDeveloper} /></div></div> : <>
        <div className="page-tabs">
          <Tabs label="Bridge" value={tab} onChange={setTab} tabs={[
            { value: "share", label: "Share" },
            { value: "agents", label: "Agents", badge: agentNeedsYou ? "!" : agentActive ? <Dot tone="live" pulse /> : undefined },
          ]} />
        </div>
        <div className="page" hidden={tab !== "share"}>
          <ShareView room={room} items={items} queueItems={queueItems} transfers={transfers} peers={peers} task={task} active={tab === "share"} onRefresh={onRefresh} onEnqueue={onEnqueue} onRevealInFolder={onRevealInFolder} />
        </div>
        <div className="page" hidden={tab !== "agents"}><AgentWorkspace room={room} agent={agent} peers={peers} members={members} onOpenDevice={setDeviceOpen} onOpenDetails={() => setDetailsOpen(true)} /></div>
      </>}
    </section>
    {detailsOpen ? <>
      <div className="scrim inspector-scrim" onClick={() => setDetailsOpen(false)} />
      <BridgeDetails room={room} onClose={() => setDetailsOpen(false)} onRefresh={onRefresh} onBurn={onBurn} onOpenSettings={() => onOpenSettings("settings-permissions")} onOpenDevice={(hostRef) => hostRef ? setDeviceOpen(hostRef) : onOpenSettings("settings-this-device")} />
    </> : null}
    {deviceOpen ? <DeviceDetails room={room} hostRef={deviceOpen} onClose={() => setDeviceOpen(null)} onOpenSettings={(route) => { setDeviceOpen(null); onOpenSettings(route); }} /> : null}
  </div>;
}

/** While a Bridge is open here, each connected device of it is shown this
 * device's identity once per connection, so its user can share folders
 * with this device or trust it without a step here. It grants nothing:
 * that device's own user still decides everything. */
function usePresentIdentityToPeers(roomId: string, peers: ReturnType<typeof roomPeers>) {
  const shown = useRef(new Set<string>());
  useEffect(() => {
    for (const peer of peers) {
      if (!peer.hostRef || shown.current.has(peer.peerSessionId)) continue;
      shown.current.add(peer.peerSessionId);
      void presentDeviceIdentity(roomId, peer.hostRef).catch(() => shown.current.delete(peer.peerSessionId));
    }
  }, [peers, roomId]);
}

/** What crossed this Bridge, oldest first, and the composer that sends to
 * one selected device. Find file is the same kind of exchange in the other
 * direction, so it lives here, behind the composer's add menu. */
function ShareView({ room, items, queueItems, transfers, peers, task, active, onRefresh, onEnqueue, onRevealInFolder }: {
  room: RoomInfo;
  items: RoomItem[];
  queueItems: TransferQueueItem[];
  transfers: FileTransferProgressEvent[];
  peers: ReturnType<typeof roomPeers>;
  task: AgentTaskController;
  active: boolean;
  onRefresh: () => Promise<void>;
  onEnqueue: (roomId: string, inputs: TransferQueueInput[]) => void;
  onRevealInFolder: (path: string) => Promise<void>;
}) {
  const [text, setText] = useState("");
  const [selectedPeerId, setSelectedPeerId] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [finding, setFinding] = useState(false);
  const [dragging, setDragging] = useState(false);
  const threadRef = useRef<HTMLDivElement | null>(null);
  // A stale selection is temporarily unavailable until the selection effect
  // adopts the fresh exact peer id. Never route one render through peers[0].
  const selectedPeer = peers.find((peer) => peer.peerSessionId === selectedPeerId) ?? null;

  useEffect(() => {
    if (!peers.some((peer) => peer.peerSessionId === selectedPeerId)) setSelectedPeerId(peers[0]?.peerSessionId ?? "");
  }, [peers, selectedPeerId]);

  const selectedRoute = useMemo<BridgeRoute | null>(() => {
    if (!selectedPeer) return null;
    try {
      return { bridgeSessionId: legacyRoomToBridgePeerCollection(room).bridgeSessionId, target: { kind: "selected_peer", peerSessionId: selectedPeer.peerSessionId } };
    } catch { return null; }
  }, [room, selectedPeer]);

  const thread = useMemo(() => items.filter((item) => item.room_id === room.id).sort((left, right) => left.created_at - right.created_at).slice(-200), [items, room.id]);
  const queued = queueItems.filter((item) => item.roomId === room.id && ["queued", "preparing", "sending"].includes(item.status) && !item.activeTransferId);
  const liveTransfers = transfers.filter((transfer) => transfer.room_id === room.id && !["completed", "failed", "cancelled", "burned", "interrupted"].includes(transfer.status));
  const progressByItem = new Map(liveTransfers.map((transfer) => [transfer.item_id, transfer]));
  const orphanTransfers = liveTransfers.filter((transfer) => !thread.some((item) => item.id === transfer.item_id));

  useEffect(() => {
    const element = threadRef.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [thread.length, queued.length, orphanTransfers.length, active]);

  function enqueuePaths(paths: string[]) {
    if (!selectedRoute || !selectedPeer || !paths.length) return;
    const operationId = `bridge-send:${room.id}:${crypto.randomUUID()}`;
    onEnqueue(room.id, paths.map((path) => ({
      path,
      bridgeRoute: selectedRoute,
      bridgeOperationId: operationId,
      bridgeTargetKind: "selected_peer",
      bridgeContentKind: "file",
      targetPeerSessionId: selectedPeer.peerSessionId,
      targetPeerDisplayName: selectedPeer.displayName,
      targetCount: 1,
    })));
  }

  // Files dropped on the window go to the selected device, exactly like
  // Send files. Only while this view is showing and a device is routeable.
  const enqueueRef = useRef(enqueuePaths);
  enqueueRef.current = enqueuePaths;
  const canDrop = active && !!selectedRoute;
  useEffect(() => {
    if (!canDrop || !("__TAURI_INTERNALS__" in window)) return;
    const dispose = ownAsyncDisposer(getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "enter" || payload.type === "over") setDragging(true);
      else if (payload.type === "leave") setDragging(false);
      else if (payload.type === "drop") { setDragging(false); enqueueRef.current(payload.paths); }
    }));
    return () => { setDragging(false); dispose(); };
  }, [canDrop]);

  async function sendText() {
    if (!selectedRoute || !text.trim() || busy) return;
    setBusy(true); setMessage(null);
    try {
      await sendTextToRoomWithBridgeRoute(room, text.trim(), sendTextToRoom, selectedRoute);
      setText("");
      await onRefresh();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not send this text.");
    } finally { setBusy(false); }
  }

  async function chooseFiles() {
    if (!selectedRoute || !selectedPeer) return;
    try {
      const selected = await open({ multiple: true, directory: false });
      enqueuePaths(typeof selected === "string" ? [selected] : Array.isArray(selected) ? selected : []);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not choose files.");
    }
  }

  const peerName = selectedPeer?.displayName ?? peers[0]?.displayName ?? null;
  const empty = thread.length === 0 && queued.length === 0 && orphanTransfers.length === 0 && !task.status;

  return <>
    <div className="thread" ref={threadRef}>
      <div className="thread-inner">
        {task.status ? <TaskConversation task={task} /> : null}
        {empty ? <EmptyState title={peerName ? `Share with ${peerName}` : "Nothing shared yet"} actions={selectedRoute ? <><button type="button" className="btn" onClick={() => void chooseFiles()}><Icon name="file" />Send files…</button><button type="button" className="btn" onClick={() => setFinding(true)}><Icon name="download" />Get a file…</button></> : null}>
          {peerName ? "Type or paste text below, drop files on this window, or get a file from the other device." : "When the other device connects, you can send it text and files here."}
        </EmptyState> : null}
        {thread.map((item, index) => <ThreadEntry key={item.id} item={item} previous={thread[index - 1]} progress={progressByItem.get(item.id)} onRevealInFolder={onRevealInFolder} />)}
        {orphanTransfers.map((transfer) => <TransferEntry key={transfer.transfer_id} direction={transfer.direction} name={transfer.file_name} detail={transferDetail(transfer)} progress={transfer.file_size > 0 ? transfer.transferred_bytes / transfer.file_size : null} />)}
        {queued.map((item) => <TransferEntry key={item.id} direction="outgoing" name={item.displayName ?? fileName(item.path)} detail={`${item.status === "queued" ? "Queued" : item.status === "preparing" ? "Preparing" : "Sending"}${item.targetPeerDisplayName ? ` to ${item.targetPeerDisplayName}` : ""}`} progress={null} />)}
      </div>
    </div>
    <div className="composer-wrap">
      {!selectedRoute ? <div style={{ maxWidth: "var(--content-w)", margin: "0 auto 8px" }}><Notice tone="neutral">{peers.length === 0 ? "No device is connected to this Bridge right now. Pastey keeps trying; open Bridge details to pair or set an address." : "Choose a device to send to."}</Notice></div> : null}
      <section className={`composer${dragging ? " dragging" : ""}`} aria-label="Send to the other device">
        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void sendText(); } }}
          placeholder={dragging ? "Drop to send…" : peerName ? `Message ${peerName}, or drop files…` : "Waiting for a device…"}
          aria-label="Send message"
          disabled={!selectedRoute}
          rows={2}
        />
        <div className="composer-bar">
          <MenuButton label="Add" icon="plus" align="left" up disabled={!selectedRoute} buttonClassName="btn btn-ghost btn-icon btn-sm">{(close) => <>
            <button type="button" role="menuitem" onClick={() => { close(); void chooseFiles(); }}><Icon name="file" /><span>Send files…<small>Choose files on this device</small></span></button>
            <button type="button" role="menuitem" onClick={() => { close(); setFinding(true); }}><Icon name="download" /><span>Get a file from {peerName ?? "the other device"}…<small>Find one file in folders it shares</small></span></button>
          </>}</MenuButton>
          <span className="composer-target">
            {peers.length > 1
              ? <>To <select className="select select-sm" value={selectedPeerId} onChange={(event) => setSelectedPeerId(event.target.value)} aria-label="Send to">{peers.map((peer) => <option key={peer.peerSessionId} value={peer.peerSessionId}>{peer.displayName}</option>)}</select></>
              : peerName ? <>To {peerName}</> : null}
          </span>
          <span className="spacer" />
          <button type="button" className="btn btn-primary btn-icon send-button" aria-label="Send" disabled={!selectedRoute || busy || !text.trim()} onClick={() => void sendText()}><Icon name="arrowUp" /></button>
        </div>
      </section>
      {message || task.message ? <p className="composer-hint error-text" role="status">{message ?? task.message}</p> : <p className="composer-hint">Enter sends · Shift+Enter adds a line</p>}
    </div>
    {finding ? <Dialog title={`Get a file from ${peerName ?? "the other device"}`} description="Find one file by name in the folders that device shares with this Bridge. Only the file you choose is sent to your Inbox." onClose={() => setFinding(false)}>
      <RemoteFileFinder roomId={room.id} peerName={peerName} route={selectedRoute} />
    </Dialog> : null}
  </>;
}

function sameDay(left: number, right: number): boolean {
  return new Date(left * 1000).toDateString() === new Date(right * 1000).toDateString();
}

function dayLabel(timestampSeconds: number): string {
  const date = new Date(timestampSeconds * 1000);
  const today = new Date();
  if (date.toDateString() === today.toDateString()) return "Today";
  const yesterday = new Date(today.getTime() - 86_400_000);
  if (date.toDateString() === yesterday.toDateString()) return "Yesterday";
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: date.getFullYear() === today.getFullYear() ? undefined : "numeric" }).format(date);
}

function transferDetail(transfer: FileTransferProgressEvent): string {
  const done = formatBytes(transfer.transferred_bytes);
  const total = formatBytes(transfer.file_size);
  return `${transfer.direction === "incoming" ? "Receiving" : "Sending"} · ${done} of ${total}`;
}

const ITEM_STATUS: Record<RoomItem["status"], string> = { created: "Waiting", sent: "Sent", received: "Received", failed: "Failed", cancelled: "Cancelled", interrupted: "Interrupted" };

function ThreadEntry({ item, previous, progress, onRevealInFolder }: { item: RoomItem; previous?: RoomItem; progress?: FileTransferProgressEvent; onRevealInFolder: (path: string) => Promise<void> }) {
  const showDay = !previous || !sameDay(previous.created_at, item.created_at);
  return <>
    {showDay ? <span className="thread-day">{dayLabel(item.created_at)}</span> : null}
    <MessageCard item={item} progress={progress} onRevealInFolder={onRevealInFolder} />
  </>;
}

export function MessageCard({ item, progress, onRevealInFolder }: { item: RoomItem; progress?: FileTransferProgressEvent; onRevealInFolder?: (path: string) => Promise<void> }) {
  const [copied, setCopied] = useState(false);
  const outgoing = item.direction !== "incoming";
  const failed = item.status === "failed" || item.status === "interrupted";
  const meta = <div className="msg-meta">
    <span>{formatClock(item.created_at)}</span>
    {failed || item.status === "cancelled" || item.status === "created" ? <span className={failed ? "error-text xsmall" : ""}>{ITEM_STATUS[item.status]}</span> : null}
    {item.payload_type === "text" && item.text ? <button type="button" onClick={() => void copyTextToClipboard(item.text ?? "").then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1500); }).catch(() => undefined)}>{copied ? "Copied" : "Copy"}</button> : null}
    {item.saved_path && onRevealInFolder ? <button type="button" onClick={() => void onRevealInFolder(item.saved_path!)}>Show in folder</button> : null}
  </div>;
  if (item.payload_type === "text") {
    return <div className={`msg ${outgoing ? "out" : "in"}`}>
      <div className="msg-bubble">{item.text ?? item.display_name ?? "Text"}</div>
      {meta}
      {item.error_message ? <span className="error-text xsmall">{item.error_message}</span> : null}
    </div>;
  }
  const ratio = progress && progress.file_size > 0 ? progress.transferred_bytes / progress.file_size : null;
  return <div className={`msg ${outgoing ? "out" : "in"}`}>
    <div className="file-chip">
      <span className="file-icon" aria-hidden="true"><Icon name="file" /></span>
      <span>
        <strong title={item.display_name ?? undefined}>{item.display_name || "File"}</strong>
        <small>{progress ? transferDetail(progress) : `${formatBytes(item.size_bytes)} · ${ITEM_STATUS[item.status]}`}</small>
        {progress ? <div className={`progress${ratio === null ? " indeterminate" : ""}`}><i style={ratio === null ? undefined : { width: `${Math.round(ratio * 100)}%` }} /></div> : null}
      </span>
    </div>
    {meta}
    {item.error_message ? <span className="error-text xsmall">{item.error_message}</span> : null}
  </div>;
}

export function TransferEntry({ direction, name, detail, progress }: { direction: string; name: string; detail: string; progress: number | null }) {
  return <div className={`msg ${direction === "incoming" ? "in" : "out"}`}>
    <div className="file-chip">
      <span className="file-icon" aria-hidden="true"><Icon name="file" /></span>
      <span>
        <strong>{name}</strong>
        <small>{detail}</small>
        <div className={`progress${progress === null ? " indeterminate" : ""}`}><i style={progress === null ? undefined : { width: `${Math.round(progress * 100)}%` }} /></div>
      </span>
    </div>
  </div>;
}

/** A managed Plan for this Bridge, pinned above the thread while it is
 * open: review, Host readiness, execution and the terminal state Core
 * reports. Nothing here synthesizes a result. */
function TaskConversation({ task }: { task: AgentTaskController }) {
  const status = task.status;
  if (!status || !task.presentation) return null;
  const isReview = status.state === "draft";
  const isTerminal = ["completed", "failed", "interrupted", "cancelled"].includes(status.state);
  return isReview ? <PlanReviewCard task={task} /> : <>
    <PlanExecutionCard task={task} />
    {isTerminal ? <ResultCard task={task} /> : null}
  </>;
}

function PlanReviewCard({ task }: { task: AgentTaskController }) {
  const status = task.status!;
  return <article className="card" aria-label="Plan review">
    <header className="card-header"><div className="stack" style={{ gap: 2 }}><h3>Review this Plan</h3><span className="xsmall subtle">Nothing runs until you approve it.</span></div><StatusBadge tone="pending">Needs review</StatusBadge></header>
    <div className="card-body">
      <dl className="facts"><dt>Steps</dt><dd>{status.totalSteps || "—"}</dd><dt>Devices</dt><dd>{status.totalHosts || "—"}</dd></dl>
      <Notice tone="info">Your approval covers this exact revision only. Each device still admits the Plan before any step runs. Step details stay with the Host and are not shown here.</Notice>
    </div>
    <footer className="card-footer"><button type="button" className="btn" disabled={task.busy !== null} onClick={task.closeRevision}>Close</button><button type="button" className="btn btn-primary" disabled={task.busy !== null} onClick={() => void task.approve()}>{task.busy === "approve" ? "Approving…" : "Approve Plan"}</button></footer>
  </article>;
}

function PlanExecutionCard({ task }: { task: AgentTaskController }) {
  const status = task.status!;
  const canStart = status.state === "approved" && Boolean(status.approvalId);
  const canStop = ["checking_readiness", "preparing", "running"].includes(status.state);
  return <article className="card" aria-label="Plan execution">
    <header className="card-header"><div className="stack" style={{ gap: 2 }}><h3>Plan</h3><span className="xsmall subtle">{task.presentation?.detail}</span></div><StatusBadge tone={task.presentation?.tone ?? "neutral"}>{task.busy === "cancel" ? "Cancelling" : task.presentation?.label}</StatusBadge></header>
    <div className="card-body">
      <dl className="facts">
        <dt>Progress</dt><dd>{task.progress}</dd>
        <dt>Devices ready</dt><dd>{status.readyHosts} of {status.totalHosts}</dd>
        {status.currentStepId ? <><dt>Current step</dt><dd className="mono">{status.currentStepId}</dd></> : null}
        {status.code ? <><dt>Host status</dt><dd className="mono">{status.code}</dd></> : null}
      </dl>
    </div>
    {canStart || canStop ? <footer className="card-footer">
      {canStop ? <button type="button" className="btn btn-danger" disabled={task.busy !== null} onClick={() => void task.cancel()}>{task.busy === "cancel" ? "Cancelling…" : "Stop task"}</button> : null}
      {canStart ? <button type="button" className="btn btn-primary" disabled={task.busy !== null} onClick={() => void task.beginReadiness()}>{task.busy === "start" ? "Preparing…" : "Begin Host readiness"}</button> : null}
    </footer> : null}
  </article>;
}

function ResultCard({ task }: { task: AgentTaskController }) {
  return <Notice tone={task.status?.state === "completed" ? "ok" : task.presentation?.tone === "danger" ? "bad" : "neutral"} title={task.presentation?.label}>
    This is the terminal state the Host reported. Result content is not shown here, and Pastey does not infer success on its own.
  </Notice>;
}

function EmptyBridge({ onNewBridge, menuButton }: { onNewBridge: () => void; menuButton?: React.ReactNode }) {
  return <section className="page">
    {menuButton ? <header className="page-header">{menuButton}</header> : null}
    <div className="page-body" style={{ display: "grid", placeItems: "center" }}>
      <EmptyState title="Connect a device" actions={<button type="button" className="btn btn-primary" onClick={onNewBridge}><Icon name="plus" />New Bridge</button>}>
        A Bridge pairs this device with one of your other devices. Then you can share text and files, run Codex there, and see its Agent sessions.
      </EmptyState>
    </div>
  </section>;
}

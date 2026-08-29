import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import type { BridgeRoute } from "../../lib/bridgeRouting";
import { sendTextToRoomWithBridgeRoute } from "../../lib/bridgeRoutingRuntime";
import { legacyRoomToBridgePeerCollection } from "../../lib/bridgeRoomAdapter";
import { sendTextToRoom } from "../../lib/tauri";
import type { TransferQueueInput, TransferQueueItem } from "../../lib/transferScheduler";
import type { RoomInfo, RoomItem } from "../../lib/types";
import type { AgentTaskController } from "./AgentTaskLifecycle";
import { StatusBadge } from "./AgentTaskLifecycle";
import { bridgeCode, bridgeDeviceCount, fileName, formatBytes, formatClock, roomPeers } from "./workspaceViewModel";

interface BridgeWorkspaceProps {
  room: RoomInfo | null;
  items: RoomItem[];
  queueItems: TransferQueueItem[];
  task: AgentTaskController;
  onCreate: () => void;
  onJoin: () => void;
  onRefresh: () => Promise<void>;
  onDeveloper: () => void;
  onBurn: (room: RoomInfo) => Promise<void>;
  onEnqueue: (roomId: string, inputs: TransferQueueInput[]) => void;
}

export function BridgeWorkspace({ room, items, queueItems, task, onCreate, onJoin, onRefresh, onDeveloper, onBurn, onEnqueue }: BridgeWorkspaceProps) {
  const [composerMode, setComposerMode] = useState<"send" | "task">("send");
  const [text, setText] = useState("");
  const [selectedPeerId, setSelectedPeerId] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const peers = useMemo(() => roomPeers(room), [room]);
  const selectedPeer = peers.find((peer) => peer.peerSessionId === selectedPeerId) ?? peers[0] ?? null;

  useEffect(() => {
    if (!peers.some((peer) => peer.peerSessionId === selectedPeerId)) setSelectedPeerId(peers[0]?.peerSessionId ?? "");
  }, [peers, selectedPeerId]);

  useEffect(() => {
    if (task.status) setComposerMode("task");
  }, [task.status]);

  const selectedRoute = useMemo<BridgeRoute | null>(() => {
    if (!room || !selectedPeer) return null;
    try {
      return { bridgeSessionId: legacyRoomToBridgePeerCollection(room).bridgeSessionId, target: { kind: "selected_peer", peerSessionId: selectedPeer.peerSessionId } };
    } catch { return null; }
  }, [room, selectedPeer]);

  async function sendText() {
    if (!room || !selectedRoute || !text.trim()) return;
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
    if (!room || !selectedRoute || !selectedPeer) return;
    try {
      const selected = await open({ multiple: true, directory: false });
      const paths = typeof selected === "string" ? [selected] : Array.isArray(selected) ? selected : [];
      if (!paths.length) return;
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
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not choose files.");
    }
  }

  async function burnBridge() {
    if (!room) return;
    const confirmed = window.confirm(`Burn Bridge ${bridgeCode(room)}?\n\nThis permanently removes the Bridge content, membership, authority, and current-session state from this device. Explicitly saved user-owned files remain outside Bridge storage.`);
    if (confirmed) await onBurn(room);
  }

  if (!room) return <EmptyBridge onCreate={onCreate} onJoin={onJoin} />;

  const recent = items.filter((item) => item.room_id === room.id).slice(0, 4);
  const queued = queueItems.filter((item) => item.roomId === room.id && ["queued", "preparing", "sending"].includes(item.status));
  const hasManagedTask = task.status !== null;

  return (
    <section className="v2-workspace-page">
      <header className="v2-workspace-header">
        <div><h1>Bridge {bridgeCode(room)}</h1><p>{bridgeDeviceCount(room)} devices · current-session workspace</p></div>
        <div className="v2-bridge-session-actions">
          {task.status && ["checking_readiness", "preparing", "running"].includes(task.status.state)
            ? <button type="button" className="v2-button" disabled={task.busy !== null} onClick={() => void task.cancel()}>{task.busy === "cancel" ? "Cancelling…" : "Stop task"}</button>
            : null}
          <button type="button" className="v2-button danger" onClick={() => void burnBridge()}>Burn</button>
        </div>
      </header>
      <div className="v2-thread-shell">
        <div className="v2-thread">
          {hasManagedTask ? <TaskConversation task={task} /> : (
            <>
              {queued.map((item) => <TransferMessage key={item.id} name={item.displayName ?? fileName(item.path)} status={item.status} />)}
              {recent.map((item) => <MessageCard key={item.id} item={item} />)}
              {recent.length === 0 && queued.length === 0 ? (
                <div className="v2-pastey-message"><small>Pastey</small><p>Pastey is ready. Paste text or images, drop files, or switch to Task when you want Pastey to work across this Bridge.</p></div>
              ) : null}
            </>
          )}
        </div>
        <TaskComposer
          mode={composerMode}
          onMode={setComposerMode}
          text={text}
          onText={setText}
          peers={peers}
          selectedPeerId={selectedPeer?.peerSessionId ?? ""}
          onPeer={setSelectedPeerId}
          disabled={!selectedRoute || busy}
          task={task}
          onFiles={() => void chooseFiles()}
          onSend={() => void sendText()}
          onDeveloper={onDeveloper}
        />
        {message || task.message ? <p className="v2-error" role="status">{message ?? task.message}</p> : null}
      </div>
    </section>
  );
}

function TaskConversation({ task }: { task: AgentTaskController }) {
  const status = task.status;
  if (!status || !task.presentation) return null;
  const isReview = status.state === "draft";
  const isTerminal = ["completed", "failed", "interrupted", "cancelled"].includes(status.state);
  return (
    <>
      <div className="v2-user-message"><small>You</small><p>Managed Agent Task · whole-Plan Bridge scope</p></div>
      <div className="v2-pastey-message"><small>Pastey</small><p>{isReview ? "A bounded Plan Draft is ready for requester review." : task.presentation.detail}</p></div>
      {isReview ? <PlanReviewCard task={task} /> : <PlanExecutionCard task={task} />}
      {isTerminal ? <ResultCard task={task} /> : null}
    </>
  );
}

function PlanReviewCard({ task }: { task: AgentTaskController }) {
  const status = task.status!;
  return (
    <article className="v2-plan-card">
      <header><div><strong>Proposed task</strong><small>Review the bounded Plan before anything runs.</small></div><StatusBadge tone="pending">Review</StatusBadge></header>
      <div className="v2-plan-step"><i className="v2-dot live" /><span><strong>{status.totalSteps || "—"} authored Plan steps</strong><small>PM owns WHAT, WHERE, and ORDER across {status.totalHosts || "the participating"} Hosts.</small></span></div>
      <div className="v2-plan-step unavailable"><i className="v2-dot" /><span><strong>Step topology unavailable</strong><small>The renderer cannot safely name operations or destinations from this status projection. Authored Transfer steps remain explicit in Core.</small></span></div>
      <div className="v2-authority-note"><small>Authority</small><p>Requester approval covers this immutable revision only. Host readiness and admission still follow.</p></div>
      <footer><button type="button" className="v2-button" disabled={task.busy !== null} onClick={task.closeRevision}>Close review</button><button type="button" className="v2-button primary" disabled={task.busy !== null} onClick={() => void task.approve()}>{task.busy === "approve" ? "Approving…" : "Approve Plan"}</button></footer>
    </article>
  );
}

function PlanExecutionCard({ task }: { task: AgentTaskController }) {
  const status = task.status!;
  const canStart = status.state === "approved" && Boolean(status.approvalId);
  return (
    <article className={`v2-execution-card ${task.presentation?.tone ?? "neutral"}`}>
      <header><strong>Task execution</strong><StatusBadge tone={task.presentation?.tone ?? "neutral"}>{task.busy === "cancel" ? "Cancelling" : task.presentation?.label}</StatusBadge></header>
      <div className="v2-plan-step"><i className={`v2-dot ${status.state === "running" ? "live" : status.state === "completed" ? "connected" : "pending"}`} /><span><strong>{status.currentStepId ? `Current step · ${status.currentStepId}` : task.presentation?.label}</strong><small>{task.progress} · {status.readyHosts} of {status.totalHosts} Hosts ready</small></span></div>
      <div className="v2-plan-step unavailable"><i className="v2-dot" /><span><strong>Explicit Transfer / dependency continuation</strong><small>Topology is not renderer-exposed. Pastey will not imply shared storage or hidden movement.</small></span></div>
      {status.code ? <p className="v2-host-code">Host status: {status.code}</p> : null}
      {canStart ? <footer><button type="button" className="v2-button primary" disabled={task.busy !== null} onClick={() => void task.beginReadiness()}>{task.busy === "start" ? "Preparing…" : "Begin Host readiness"}</button></footer> : null}
    </article>
  );
}

function ResultCard({ task }: { task: AgentTaskController }) {
  return (
    <article className={`v2-result-card ${task.presentation?.tone ?? "neutral"}`}>
      <div><i className={`v2-dot ${task.status?.state === "completed" ? "connected" : task.presentation?.tone === "danger" ? "failed" : ""}`} /><span><strong>{task.presentation?.label}</strong><small>Authoritative terminal state reported by Host/Core.</small></span></div>
      <p>Result content is not projected to the renderer. Pastey does not synthesize a result or infer success independently.</p>
    </article>
  );
}

function TaskComposer({ mode, onMode, text, onText, peers, selectedPeerId, onPeer, disabled, task, onFiles, onSend, onDeveloper }: {
  mode: "send" | "task";
  onMode: (mode: "send" | "task") => void;
  text: string;
  onText: (text: string) => void;
  peers: ReturnType<typeof roomPeers>;
  selectedPeerId: string;
  onPeer: (id: string) => void;
  disabled: boolean;
  task: AgentTaskController;
  onFiles: () => void;
  onSend: () => void;
  onDeveloper: () => void;
}) {
  return (
    <section className="v2-composer">
      {mode === "send" ? (
        <textarea value={text} onChange={(event) => onText(event.target.value)} placeholder="Paste text or image, or drop files here…" aria-label="Send message" />
      ) : (
        <div className="v2-task-open">{task.status ? <><strong>Revision {task.status.revisionId}</strong><small>Authoritative lifecycle open · goal and richer topology are not renderer-exposed.</small></> : <><input value={task.revisionInput} onChange={(event) => task.setRevisionInput(event.target.value)} placeholder="Open an existing native-v2 revision ID…" aria-label="Native-v2 revision ID" /><small>Draft origination and PM context projection are not renderer-exposed.</small></>}</div>
      )}
      <div className="v2-composer-controls">
        <button type="button" className="v2-square-button" disabled={mode === "send" && disabled} onClick={mode === "send" ? onFiles : undefined}>＋</button>
        <label className="v2-mode-select"><select value={mode} onChange={(event) => onMode(event.target.value as "send" | "task")}><option value="send">Send</option><option value="task">Task</option></select></label>
        {mode === "send" ? (
          <><label className="v2-target-select"><select value={selectedPeerId} onChange={(event) => onPeer(event.target.value)} disabled={!peers.length}>{peers.map((peer) => <option key={peer.peerSessionId} value={peer.peerSessionId}>{peer.displayName}</option>)}</select></label><button type="button" className="v2-developer-button" onClick={onDeveloper}>Developer Mode</button><button type="button" className="v2-send-button" disabled={disabled || !text.trim()} onClick={onSend}>↑</button></>
        ) : <><span className="v2-plan-scope">Whole Plan · PM selects Hosts</span><button type="button" className="v2-send-button" disabled={task.busy !== null || (!task.status && !task.revisionInput.trim())} onClick={() => void (task.status ? task.refresh() : task.openRevision())}>{task.status ? "↻" : "↑"}</button></>}
      </div>
      <small className="v2-composer-help">{mode === "send" ? "Send mode targets one selected device. Developer Mode is human-only current-session terminal access; it does not grant Agent, Plan, or Execute authority." : "Agent Tasks use the whole Plan scope. Task mode opens an authoritative immutable Draft; it is not permanently bound to the Send destination."}</small>
    </section>
  );
}

export function MessageCard({ item }: { item: RoomItem }) {
  const title = item.display_name || (item.payload_type === "text" ? "Clipboard text" : "Transfer");
  return <article className="v2-message-card"><div><i className={`v2-dot ${item.status === "failed" ? "failed" : "connected"}`} /><strong>{title}</strong><small>{formatClock(item.created_at)} · {item.direction === "incoming" ? "Received" : "Sent"}</small></div>{item.text ? <p>{item.text}</p> : null}{item.size_bytes > 0 ? <small>{formatBytes(item.size_bytes)} · {item.status}</small> : <small>{item.status}</small>}</article>;
}

export function TransferMessage({ name, status }: { name: string; status: string }) {
  return <article className="v2-message-card"><div><i className="v2-dot live" /><strong>{name}</strong><small>{status.replace(/_/g, " ")}</small></div><div className="v2-progress-track"><i /></div></article>;
}

function EmptyBridge({ onCreate, onJoin }: { onCreate: () => void; onJoin: () => void }) {
  return <section className="v2-empty-bridge"><h1>No Bridge selected</h1><p>Create a current-session Bridge, or join one with its 8-digit code.</p><div><button type="button" className="v2-button primary" onClick={onCreate}>New Bridge</button><button type="button" className="v2-button" onClick={onJoin}>Join with code</button></div></section>;
}

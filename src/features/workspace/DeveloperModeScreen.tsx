import { useEffect, useMemo, useRef, useState } from "react";
import { DeveloperTerminalViewport } from "../../components/DeveloperTerminalViewport";
import { OrderedTerminalInputWriter, TerminalInputBackpressureError } from "../../lib/developerTerminalFrontend";
import { closeDeveloperTerminal, enterDeveloperMode, requestDeveloperTerminal, resizeDeveloperTerminal, sendDeveloperTerminalInput } from "../../lib/tauri";
import type { DeveloperModeUiSession, DeveloperTerminalWorkspace, RoomInfo } from "../../lib/types";
import { roomPeers } from "./workspaceViewModel";

interface DeveloperModeScreenProps {
  room: RoomInfo;
  session: DeveloperModeUiSession | null;
  workspace: DeveloperTerminalWorkspace;
  onSession: (session: DeveloperModeUiSession) => void;
  onRefresh: () => Promise<void>;
}

export function DeveloperModeScreen({ room, session, workspace, onSession, onRefresh }: DeveloperModeScreenProps) {
  const peers = useMemo(() => roomPeers(room), [room]);
  const [selectedPeerId, setSelectedPeerId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const inputWriterRef = useRef<OrderedTerminalInputWriter | null>(null);
  const controllers = workspace.sessions.filter((entry) => entry.role === "controller");
  const controller = controllers.find((entry) => entry.state === "active")
    ?? controllers.find((entry) => entry.state === "awaiting_admission")
    ?? controllers.sort((left, right) => right.expiresAt - left.expiresAt)[0]
    ?? null;
  const active = controller && (controller.state === "active" || controller.state === "awaiting_admission") ? controller : null;
  const canRequest = !controller || ["denied", "exited", "closed", "disconnected"].includes(controller.state);

  useEffect(() => {
    if (!peers.some((peer) => peer.peerSessionId === selectedPeerId)) setSelectedPeerId(peers[0]?.peerSessionId ?? "");
  }, [peers, selectedPeerId]);

  useEffect(() => {
    inputWriterRef.current?.cancel();
    inputWriterRef.current = null;
    if (!active || active.state !== "active" || !session) return;
    const writer = new OrderedTerminalInputWriter(
      (frame) => sendDeveloperTerminalInput(active.terminalSessionId, session.token, frame),
      (cause) => setError(cause instanceof Error ? cause.message : String(cause)),
    );
    inputWriterRef.current = writer;
    return () => { writer.cancel(); if (inputWriterRef.current === writer) inputWriterRef.current = null; };
  }, [active?.state, active?.terminalSessionId, session?.token]);

  async function enter() {
    if (busy) return;
    setBusy(true); setError(null);
    try { onSession(await enterDeveloperMode(room.id)); } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); } finally { setBusy(false); }
  }

  async function requestTerminal() {
    if (!session || !selectedPeerId || busy) return;
    setBusy(true); setError(null);
    try {
      await requestDeveloperTerminal(room.id, selectedPeerId, session.token);
      await onRefresh();
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); } finally { setBusy(false); }
  }

  async function close() {
    if (!active || !session || busy) return;
    setBusy(true); setError(null);
    inputWriterRef.current?.cancel();
    try {
      await closeDeveloperTerminal(active.terminalSessionId, session.token);
      await onRefresh();
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); } finally { setBusy(false); }
  }

  function enqueueTerminalInput(bytes: number[]) {
    try { inputWriterRef.current?.enqueue(bytes); } catch (cause) { setError(cause instanceof TerminalInputBackpressureError ? cause.message : cause instanceof Error ? cause.message : String(cause)); }
  }

  return (
    <div className="v2-developer-body">
      <section className="v2-developer-warning"><strong>Developer Mode · elevated capability</strong><p>This terminal is a human-controlled current-session grant inside this Bridge. It is separate from Agent Task, Worker, Execute, Plan authority, and managed object lineage.</p></section>
      {!session ? <section className="v2-developer-empty"><p>Enter Developer Mode before requesting terminal admission from one selected Host.</p><button type="button" className="v2-button primary" disabled={busy || peers.length === 0} onClick={() => void enter()}>{busy ? "Entering…" : "Enter Developer Mode"}</button></section> : null}
      {session && canRequest ? <section className="v2-developer-request"><select value={selectedPeerId} onChange={(event) => setSelectedPeerId(event.target.value)} disabled={busy || peers.length === 0}>{peers.map((peer) => <option key={peer.peerSessionId} value={peer.peerSessionId}>{peer.displayName}</option>)}</select><button type="button" className="v2-button primary" disabled={busy || !selectedPeerId} onClick={() => void requestTerminal()}>{busy ? "Requesting…" : "Request terminal admission"}</button></section> : null}
      {active ? <section className="v2-terminal"><header><strong><i className={`v2-dot ${active.state === "active" ? "connected" : "pending"}`} /> {active.targetHostRef} · {active.environmentLabel ?? "Host shell"}</strong><span>{active.state === "active" ? "Live" : "Awaiting admission"}</span></header>{active.state === "active" && session ? <DeveloperTerminalViewport roomId={room.id} terminalSessionId={active.terminalSessionId} environmentLabel={active.environmentLabel} output={active.output} outputSequence={active.outputSequence} onInput={enqueueTerminalInput} onResize={(cols, rows) => void resizeDeveloperTerminal(active.terminalSessionId, session.token, cols, rows)} /> : <p>Waiting for the target Host’s explicit local admission.</p>}<footer><button type="button" className="v2-button danger" disabled={busy} onClick={() => void close()}>End session</button></footer></section> : controller ? <section className="v2-terminal v2-terminal-unavailable"><header><strong><i className="v2-dot" /> Terminal {controller.state}</strong><span>{controller.state === "denied" ? "Denied" : "Ended"}</span></header><p>{controller.state === "denied" ? "The target Host denied this request. No terminal process was created." : `This exact terminal session ended${controller.terminationReason ? `: ${controller.terminationReason}` : "."}`}</p></section> : <section className="v2-terminal v2-terminal-unavailable"><header><strong><i className="v2-dot" /> Terminal unavailable</strong><span>Not admitted</span></header><p>A terminal appears here only after the selected Host explicitly accepts this current-session request.</p></section>}
      <p className="v2-developer-footnote">End session terminates only the exact terminal session. Burn remains a separate destructive local Bridge action.</p>
      {error ? <p className="v2-error" role="status">{error}</p> : null}
    </div>
  );
}

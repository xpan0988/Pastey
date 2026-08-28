import { useEffect, useMemo, useRef, useState } from "react";
import { DeveloperTerminalViewport } from "../../components/DeveloperTerminalViewport";
import { OrderedTerminalInputWriter, TerminalInputBackpressureError } from "../../lib/developerTerminalFrontend";
import {
  closeDeveloperTerminal,
  enterDeveloperMode,
  getDeveloperTerminalWorkspace,
  requestDeveloperTerminal,
  resizeDeveloperTerminal,
  sendDeveloperTerminalInput,
} from "../../lib/tauri";
import type { DeveloperModeUiSession, DeveloperTerminalWorkspace, RoomInfo } from "../../lib/types";
import { bridgeCode, roomPeers } from "./workspaceViewModel";

export function DeveloperModeScreen({ room }: { room: RoomInfo | null }) {
  const peers = useMemo(() => roomPeers(room), [room]);
  const [session, setSession] = useState<DeveloperModeUiSession | null>(null);
  const [workspace, setWorkspace] = useState<DeveloperTerminalWorkspace>({ pendingRequests: [], sessions: [] });
  const [selectedPeerId, setSelectedPeerId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const inputWriterRef = useRef<OrderedTerminalInputWriter | null>(null);
  const active = workspace.sessions.find((entry) => entry.role === "controller" && (entry.state === "active" || entry.state === "awaiting_admission"));
  const roomId = room?.id ?? null;
  const sessionToken = session?.token ?? null;
  const activeTerminalSessionId = active?.terminalSessionId ?? null;
  const activeState = active?.state ?? null;

  useEffect(() => {
    if (!peers.some((peer) => peer.peerSessionId === selectedPeerId)) setSelectedPeerId(peers[0]?.peerSessionId ?? "");
  }, [peers, selectedPeerId]);

  useEffect(() => {
    if (!roomId || !sessionToken) return;
    const refresh = () => void getDeveloperTerminalWorkspace(roomId).then(setWorkspace).catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)));
    refresh();
    const timer = window.setInterval(refresh, 2_000);
    return () => window.clearInterval(timer);
  }, [roomId, sessionToken]);

  useEffect(() => {
    inputWriterRef.current?.cancel();
    inputWriterRef.current = null;
    if (!activeTerminalSessionId || activeState !== "active" || !sessionToken) return;
    const writer = new OrderedTerminalInputWriter(
      (frame) => sendDeveloperTerminalInput(activeTerminalSessionId, sessionToken, frame),
      (cause) => setError(cause instanceof Error ? cause.message : String(cause)),
    );
    inputWriterRef.current = writer;
    return () => { writer.cancel(); if (inputWriterRef.current === writer) inputWriterRef.current = null; };
  }, [activeState, activeTerminalSessionId, sessionToken]);

  async function enter() {
    if (!room) return;
    try { setSession(await enterDeveloperMode(room.id)); } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  }

  async function requestTerminal() {
    if (!room || !session || !selectedPeerId) return;
    try { setWorkspace(await requestDeveloperTerminal(room.id, selectedPeerId, session.token)); } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  }

  async function close() {
    if (!active || !session) return;
    inputWriterRef.current?.cancel();
    try {
      await closeDeveloperTerminal(active.terminalSessionId, session.token);
      if (room) setWorkspace(await getDeveloperTerminalWorkspace(room.id));
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  }

  function enqueueTerminalInput(bytes: number[]) {
    try { inputWriterRef.current?.enqueue(bytes); } catch (cause) { setError(cause instanceof TerminalInputBackpressureError ? cause.message : cause instanceof Error ? cause.message : String(cause)); }
  }

  return (
    <section className="v2-workspace-page v2-developer-page">
      <header className="v2-workspace-header"><div><h1>Developer Mode</h1><p>{active ? `${active.targetHostRef} · explicitly admitted session` : "Human-only, current-session terminal access"}</p></div>{active ? <button type="button" className="v2-button danger" onClick={() => void close()}>End session</button> : null}</header>
      <div className="v2-developer-body">
        <section className="v2-developer-warning"><strong>Elevated capability</strong><p>This terminal is a human-controlled current-session grant. It is separate from Agent Task, Worker, Execute, Plan authority, and managed object lineage.</p></section>
        {!session ? <section className="v2-developer-empty"><p>Enter Developer Mode before requesting terminal admission from one selected Host.</p><button type="button" className="v2-button primary" disabled={!room} onClick={() => void enter()}>Enter Developer Mode</button></section> : null}
        {session && !active ? <section className="v2-developer-request"><select value={selectedPeerId} onChange={(event) => setSelectedPeerId(event.target.value)}>{peers.map((peer) => <option key={peer.peerSessionId} value={peer.peerSessionId}>{peer.displayName}</option>)}</select><button type="button" className="v2-button primary" disabled={!selectedPeerId} onClick={() => void requestTerminal()}>Request terminal admission</button></section> : null}
        {active ? <section className="v2-terminal"><header><strong><i className={`v2-dot ${active.state === "active" ? "connected" : "pending"}`} /> {active.targetHostRef} · {active.environmentLabel ?? "Host shell"}</strong><span>{active.state === "active" ? "Live" : "Awaiting admission"}</span></header>{active.state === "active" && session ? <DeveloperTerminalViewport roomId={room?.id ?? ""} terminalSessionId={active.terminalSessionId} environmentLabel={active.environmentLabel} output={active.output} outputSequence={active.outputSequence} onInput={enqueueTerminalInput} onResize={(cols, rows) => void resizeDeveloperTerminal(active.terminalSessionId, session.token, cols, rows)} /> : <p>Waiting for the target Host’s human admission.</p>}</section> : <section className="v2-terminal v2-terminal-unavailable"><header><strong><i className="v2-dot" /> Terminal unavailable</strong><span>Not admitted</span></header><p>A terminal appears here only after a human enters Developer Mode and the selected Host admits this current-session request.</p></section>}
        <p className="v2-developer-footnote">Idle Developer Mode termination is not currently renderer-exposed. Closing a live terminal ends that terminal; Burn Bridge is a separate destructive action.</p>
        {error ? <p className="v2-error">{error}</p> : null}
      </div>
    </section>
  );
}

export function DeveloperContextPanel({ room, activeTarget }: { room: RoomInfo | null; activeTarget?: string | null }) {
  return <aside className="v2-context-panel"><p className="v2-eyebrow">Current Bridge</p><div className="v2-context-status"><span><i className={`v2-dot ${room?.peer_connected ? "connected" : ""}`} /> {room?.peer_connected ? "Connected" : room ? "Waiting" : "Unavailable"}</span><small>{room ? bridgeCode(room) : "No Bridge selected"}</small></div><div className="v2-developer-context"><small>Developer session</small><strong>{activeTarget ? `Live on ${activeTarget}` : "No live terminal"}</strong><p>Human terminal grant</p><p>Not Agent / Plan authority</p></div><div className="v2-context-section"><small>Session boundary</small><strong>Human-only, current session</strong><p>Developer Mode admission is never reused as managed Agent authority.</p></div></aside>;
}

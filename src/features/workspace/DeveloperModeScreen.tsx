import { useEffect, useMemo, useRef, useState } from "react";
import { DeveloperTerminalViewport } from "../../components/DeveloperTerminalViewport";
import { OrderedTerminalInputWriter, TerminalInputBackpressureError } from "../../lib/developerTerminalFrontend";
import { closeDeveloperTerminal, enterDeveloperMode, requestDeveloperTerminal, resizeDeveloperTerminal, sendDeveloperTerminalInput } from "../../lib/tauri";
import type { DeveloperModeUiSession, DeveloperTerminalWorkspace, RoomInfo } from "../../lib/types";
import { Dot, Notice } from "./ui";
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
    <div className="stack-lg">
      <div className="panel-intro">
        <h2>Developer Mode</h2>
        <p>A terminal on the other device, controlled by you for this session only. It is separate from Agents, Plans and their permissions, and the other device must accept each request, unless it gave this device Full Access.</p>
      </div>
      {!session ? <section className="card"><div className="card-body"><p className="small muted">Enter Developer Mode, then request a terminal on one device. Nothing opens until that device accepts, or it opens at once if that device gave this one Full Access.</p><div><button type="button" className="btn btn-primary" disabled={busy || peers.length === 0} onClick={() => void enter()}>{busy ? "Entering…" : "Enter Developer Mode"}</button></div></div></section> : null}
      {session && canRequest ? <section className="card"><div className="card-body"><div className="row"><select className="select" value={selectedPeerId} onChange={(event) => setSelectedPeerId(event.target.value)} disabled={busy || peers.length === 0} aria-label="Device">{peers.map((peer) => <option key={peer.peerSessionId} value={peer.peerSessionId}>{peer.displayName}</option>)}</select><button type="button" className="btn btn-primary" disabled={busy || !selectedPeerId} onClick={() => void requestTerminal()}>{busy ? "Requesting…" : "Request terminal"}</button></div></div></section> : null}
      {active ? <section className="terminal"><header className="terminal-header"><strong><Dot tone={active.state === "active" ? "ok" : "warn"} pulse={active.state !== "active"} /> {active.environmentLabel ?? "Shell"}</strong><span className="row"><span className="xsmall">{active.state === "active" ? "Live" : "Waiting for the other device to accept"}</span><button type="button" className="btn btn-danger btn-sm" disabled={busy} onClick={() => void close()}>End session</button></span></header>{active.state === "active" && session ? <DeveloperTerminalViewport roomId={room.id} terminalSessionId={active.terminalSessionId} environmentLabel={active.environmentLabel} output={active.output} outputSequence={active.outputSequence} onInput={enqueueTerminalInput} onResize={(cols, rows) => void resizeDeveloperTerminal(active.terminalSessionId, session.token, cols, rows)} /> : <p className="terminal-empty">Waiting for the other device to accept this request.</p>}</section> : controller ? <Notice tone={controller.state === "denied" ? "warn" : "neutral"} title={controller.state === "denied" ? "Request denied" : "Terminal ended"}>{controller.state === "denied" ? controller.terminationReason === "full_access_revoked" ? "Full Access to the other device ended before this terminal opened. No terminal process remains there." : "The other device denied this request. No terminal process was created." : `This terminal session ended${controller.terminationReason === "host_closed" ? ": the other device closed it." : controller.terminationReason ? `: ${controller.terminationReason}` : "."}`}</Notice> : null}
      <p className="xsmall subtle">End session closes only this terminal. Burn is a separate action in Bridge details.</p>
      {error ? <p className="error-text" role="status">{error}</p> : null}
    </div>
  );
}

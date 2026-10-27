import { bridgeConnectionErrorMessage } from "../../lib/bridgeAddress";
import { useCallback, useEffect, useRef, useState } from "react";
import { useAgentTaskLifecycle } from "./AgentTaskLifecycle";
import { BridgeWorkspace } from "./BridgeWorkspace";
import { developerUiSessionUsable, withEndedDeveloperUiSessions, type DeveloperUiSessionObservation } from "../../lib/developerTerminalFrontend";
import { acceptDeveloperTerminal, acceptNativeInvocationReview, denyDeveloperTerminal, denyNativeInvocationReview, enterDeveloperMode, getDeveloperTerminalWorkspace, listNativeInvocationReviews, type NativeInvocationReview } from "../../lib/tauri";
import type { DeveloperModeUiSession, DeveloperTerminalWorkspace } from "../../lib/types";
import { SettingsScreen, type SettingsRoute } from "./SettingsScreens";
import { Icon, Notice } from "./ui";
import { BridgeNavigation } from "./WorkspaceChrome";
import { InboxScreen, NewBridgeScreen } from "./WorkspaceScreens";
import { type WorkspaceRoute, type WorkspaceV2Props } from "./workspaceTypes";
import { bridgeCode, roomMembers } from "./workspaceViewModel";

const ROUTES: WorkspaceRoute[] = [
  "bridge", "new-bridge", "inbox", "settings",
  "settings-devices", "settings-permissions", "settings-agents", "settings-advanced", "settings-about",
];

function initialRoute(): WorkspaceRoute {
  const requested = new URLSearchParams(window.location.search).get("view") as WorkspaceRoute | null;
  return requested && ROUTES.includes(requested) ? requested : "bridge";
}

export function WorkspaceV2(props: WorkspaceV2Props) {
  const [route, setRoute] = useState<WorkspaceRoute>(initialRoute);
  const [bridgeMode, setBridgeMode] = useState<"normal" | "developer">("normal");
  const [message, setMessage] = useState<string | null>(null);
  const [navOpen, setNavOpen] = useState(false);
  const [developerWorkspaces, setDeveloperWorkspaces] = useState<Record<string, DeveloperTerminalWorkspace>>({});
  const [developerSessions, setDeveloperSessions] = useState<Record<string, DeveloperModeUiSession>>({});
  const [admissionBusy, setAdmissionBusy] = useState<string | null>(null);
  const [invocationReviews, setInvocationReviews] = useState<NativeInvocationReview[]>([]);
  const [reviewBusy, setReviewBusy] = useState<string | null>(null);
  const developerSessionsRef = useRef(developerSessions);
  const task = useAgentTaskLifecycle();
  const activeRoom = props.room;
  const roomIdsKey = props.rooms.filter((room) => room.status !== "burned").map((room) => room.id).sort().join(":");

  const inboxCount = props.activityItems.filter((item) => item.direction === "incoming").length;

  useEffect(() => {
    if (!activeRoom) task.closeRevision();
  }, [activeRoom, task.closeRevision]);

  useEffect(() => {
    developerSessionsRef.current = developerSessions;
  }, [developerSessions]);

  useEffect(() => {
    if (!activeRoom) setBridgeMode("normal");
  }, [activeRoom]);

  useEffect(() => {
    if (!props.focusRequest?.token) return;
    setRoute(props.focusRequest.target === "settings" ? "settings" : "bridge");
    if (props.focusRequest.target !== "settings") setBridgeMode("normal");
  }, [props.focusRequest]);

  const refreshDeveloperWorkspaces = useCallback(async () => {
    if (!props.nearbyDiscoveryAvailable) return;
    const roomIds = roomIdsKey ? roomIdsKey.split(":") : [];
    const settled = await Promise.allSettled(roomIds.map(async (roomId) => {
      const token = developerSessionsRef.current[roomId]?.token;
      return { roomId, token, workspace: await getDeveloperTerminalWorkspace(roomId, token) };
    }));
    const next: Record<string, DeveloperTerminalWorkspace> = {};
    const observed: DeveloperUiSessionObservation[] = [];
    for (const result of settled) {
      if (result.status !== "fulfilled") continue;
      next[result.value.roomId] = result.value.workspace;
      observed.push({ roomId: result.value.roomId, token: result.value.token, uiSessionExpiresAt: result.value.workspace.uiSessionExpiresAt });
    }
    setDeveloperWorkspaces(next);
    // A session that expired, or ended when its Bridge reconnected, stops
    // offering requests here, so Developer Mode can be entered again. Only
    // the exact token asked about is ended: one entered meanwhile stays.
    const sessions = withEndedDeveloperUiSessions(developerSessionsRef.current, observed);
    if (sessions !== developerSessionsRef.current) {
      developerSessionsRef.current = sessions;
      setDeveloperSessions((current) => withEndedDeveloperUiSessions(current, observed));
    }
  }, [props.nearbyDiscoveryAvailable, roomIdsKey]);

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;
    const refresh = async () => {
      if (cancelled || inFlight) return;
      inFlight = true;
      try { await refreshDeveloperWorkspaces(); } finally { inFlight = false; }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [refreshDeveloperWorkspaces]);

  const refreshInvocationReviews = useCallback(async () => {
    try { setInvocationReviews(await listNativeInvocationReviews()); } catch { setInvocationReviews([]); }
  }, []);

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;
    const refresh = async () => {
      if (cancelled || inFlight) return;
      inFlight = true;
      try { await refreshInvocationReviews(); } finally { inFlight = false; }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [refreshInvocationReviews]);

  function navigate(next: WorkspaceRoute) {
    setRoute(next);
    setMessage(null);
    setNavOpen(false);
  }

  async function createBridge() {
    setMessage(null);
    try {
      await props.onCreateBridge();
      setRoute("bridge");
    } catch (error) {
      const next = error instanceof Error ? error.message : "Pastey could not create a Bridge.";
      setMessage(next);
      throw error;
    }
  }

  async function joinBridge(code: string, address?: string) {
    setMessage(null);
    try {
      await props.onJoinBridge(code.replace(/\D/g, ""), address);
      setRoute("bridge");
    } catch (error) {
      setMessage(bridgeConnectionErrorMessage(error, Boolean(address)));
      throw error;
    }
  }

  async function joinNearbyDevice(deviceId: string) {
    setMessage(null);
    try {
      const pending = await props.onJoinNearbyDevice(deviceId);
      if (!pending) setRoute("bridge");
      return pending;
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not join this nearby device.");
      throw error;
    }
  }

  async function completeNearbyJoin(requestId: string, code: string) {
    setMessage(null);
    try {
      await props.onCompleteNearbyJoin(requestId, code.replace(/\D/g, ""));
      setRoute("bridge");
    } catch (error) {
      setMessage(bridgeConnectionErrorMessage(error, false));
      throw error;
    }
  }

  function openBridge(room: NonNullable<typeof activeRoom>) {
    setRoute("bridge");
    setNavOpen(false);
    setBridgeMode("normal");
    void props.onOpenBridge(room);
  }

  async function ensureDeveloperSession(roomId: string): Promise<DeveloperModeUiSession> {
    const existing = developerSessionsRef.current[roomId];
    if (developerUiSessionUsable(existing, Math.floor(Date.now() / 1_000) + 2)) return existing;
    const session = await enterDeveloperMode(roomId);
    developerSessionsRef.current = { ...developerSessionsRef.current, [roomId]: session };
    setDeveloperSessions(developerSessionsRef.current);
    return session;
  }

  const pendingAdmission = Object.entries(developerWorkspaces).flatMap(([roomId, workspace]) => workspace.pendingRequests.map((request) => ({ roomId, request })))[0] ?? null;
  const admissionRoom = pendingAdmission ? props.rooms.find((room) => room.id === pendingAdmission.roomId) ?? null : null;
  const requestingPeer = admissionRoom && pendingAdmission ? roomMembers(admissionRoom).find((peer) => peer.peerSessionId === pendingAdmission.request.requestingPeerSessionId) ?? null : null;

  async function decideTerminalAdmission(accept: boolean) {
    if (!pendingAdmission || admissionBusy) return;
    setAdmissionBusy(pendingAdmission.request.terminalSessionId);
    setMessage(null);
    try {
      const session = await ensureDeveloperSession(pendingAdmission.roomId);
      if (accept) {
        await acceptDeveloperTerminal(pendingAdmission.roomId, pendingAdmission.request.terminalSessionId, session.token, 80, 24);
      } else {
        await denyDeveloperTerminal(pendingAdmission.request.terminalSessionId, session.token);
      }
      await refreshDeveloperWorkspaces();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not complete terminal admission.");
    } finally {
      setAdmissionBusy(null);
    }
  }

  const pendingReview = invocationReviews[0] ?? null;
  const reviewRoom = pendingReview ? props.rooms.find((room) => room.id === pendingReview.bridgeId) ?? null : null;
  const reviewPeer = reviewRoom && pendingReview ? roomMembers(reviewRoom).find((peer) => peer.peerSessionId === pendingReview.requestingPeerSessionId) ?? null : null;

  async function decideInvocationReview(accept: boolean) {
    if (!pendingReview || reviewBusy) return;
    setReviewBusy(pendingReview.taskId);
    setMessage(null);
    try {
      if (accept) await acceptNativeInvocationReview(pendingReview.taskId);
      else await denyNativeInvocationReview(pendingReview.taskId);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not complete Agent admission.");
    } finally {
      await refreshInvocationReviews();
      setReviewBusy(null);
    }
  }

  const menuButton = <button type="button" className="btn btn-ghost btn-icon menu-toggle" aria-label="Show navigation" onClick={() => setNavOpen(true)}><Icon name="menu" /></button>;

  return (
    <div className={`app${navOpen ? " nav-open" : ""}`}>
      <BridgeNavigation route={route} rooms={props.rooms} activeRoom={activeRoom} inboxCount={inboxCount} appVersion={props.config.app_version} onNavigate={navigate} onOpenBridge={openBridge} />
      {navOpen ? <div className="scrim" onClick={() => setNavOpen(false)} /> : null}
      <main className="main">
        {pendingAdmission && admissionRoom || pendingReview && reviewRoom ? <div className="request-stack">
          {pendingAdmission && admissionRoom ? <section className="request" role="alert"><Icon name="terminal" /><div className="request-main"><strong>Developer Mode request</strong><span>{requestingPeer?.displayName ?? "A connected device"} asks to open a terminal on this device over Bridge {bridgeCode(admissionRoom)}.</span></div><div className="request-actions"><button type="button" className="btn" disabled={admissionBusy !== null} onClick={() => void decideTerminalAdmission(false)}>Deny</button><button type="button" className="btn btn-primary" disabled={admissionBusy !== null} onClick={() => void decideTerminalAdmission(true)}>{admissionBusy ? "Responding…" : "Accept"}</button></div></section> : null}
          {pendingReview && reviewRoom ? <section className="request" role="alert"><Icon name="agent" /><div className="request-main"><strong>Agent request</strong><span>{reviewPeer?.displayName ?? "A connected device"} asks to run {pendingReview.capabilityDisplayName} on this device{pendingReview.sharedFolder ? <>, in your shared folder <strong>{pendingReview.sharedFolder}</strong></> : null}, over Bridge {bridgeCode(reviewRoom)}. It runs only if you accept.</span></div><div className="request-actions"><button type="button" className="btn" disabled={reviewBusy !== null} onClick={() => void decideInvocationReview(false)}>Deny</button><button type="button" className="btn btn-primary" disabled={reviewBusy !== null} onClick={() => void decideInvocationReview(true)}>{reviewBusy ? "Responding…" : "Accept"}</button></div></section> : null}
        </div> : null}
        {message ? <div className="main-message"><Notice tone="bad">{message}</Notice></div> : null}
        {route === "bridge" ? <BridgeWorkspace room={activeRoom} items={props.roomItems} queueItems={props.queueItems} transfers={props.transfers} task={task} developerMode={bridgeMode === "developer"} developerSession={activeRoom ? developerSessions[activeRoom.id] ?? null : null} developerWorkspace={activeRoom ? developerWorkspaces[activeRoom.id] ?? { pendingRequests: [], sessions: [] } : { pendingRequests: [], sessions: [] }} onDeveloperSession={(session) => setDeveloperSessions((current) => ({ ...current, [session.roomId]: session }))} onRefreshDeveloper={refreshDeveloperWorkspaces} onNewBridge={() => navigate("new-bridge")} onRefresh={props.onRefreshBridge} onDeveloper={() => setBridgeMode((current) => current === "normal" ? "developer" : "normal")} onBurn={props.onBurnBridge} onEnqueue={props.onEnqueueTransferInputs} onRevealInFolder={props.onRevealInFolder} onOpenSettings={(next) => navigate(next ?? "settings-permissions")} menuButton={menuButton} /> : null}
        {route === "new-bridge" ? <NewBridgeScreen onCreate={createBridge} onJoin={joinBridge} onListNearby={props.onListNearbyDevices} onJoinNearby={joinNearbyDevice} onCompleteNearby={completeNearbyJoin} nearbyDiscoveryAvailable={props.nearbyDiscoveryAvailable} menuButton={menuButton} /> : null}
        {route === "inbox" ? <InboxScreen items={props.activityItems} rooms={props.rooms} inboxDir={props.config.inbox_dir} onRevealInFolder={props.onRevealInFolder} menuButton={menuButton} /> : null}
        {route.startsWith("settings") ? <SettingsScreen route={route as SettingsRoute} config={props.config} onNavigate={navigate} onConfigChange={props.onConfigChange} menuButton={menuButton} /> : null}
      </main>
    </div>
  );
}

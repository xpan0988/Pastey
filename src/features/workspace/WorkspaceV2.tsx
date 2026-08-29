import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useAgentTaskLifecycle } from "./AgentTaskLifecycle";
import { BridgeWorkspace } from "./BridgeWorkspace";
import { acceptDeveloperTerminal, denyDeveloperTerminal, enterDeveloperMode, getDeveloperTerminalWorkspace } from "../../lib/tauri";
import type { DeveloperModeUiSession, DeveloperTerminalWorkspace } from "../../lib/types";
import {
  AboutSettings,
  DiagnosticsSettings,
  ProviderSettings,
  SettingsOverview,
  TransferSettings,
  TroubleshootingSettings,
} from "./SettingsScreens";
import { BridgeContextPanel, BridgeNavigation } from "./WorkspaceChrome";
import { ActivityScreen, DevicesScreen, InboxScreen, NewBridgeScreen } from "./WorkspaceScreens";
import { type WorkspaceRoute, type WorkspaceV2Props } from "./workspaceTypes";
import { bridgeCode, roomMembers } from "./workspaceViewModel";

const ROUTES: WorkspaceRoute[] = [
  "bridge", "activity", "devices", "new-bridge", "inbox", "settings",
  "settings-diagnostics", "settings-provider", "settings-transfer", "settings-troubleshooting", "settings-about",
];

function initialRoute(): WorkspaceRoute {
  const requested = new URLSearchParams(window.location.search).get("view") as WorkspaceRoute | null;
  return requested && ROUTES.includes(requested) ? requested : "bridge";
}

export function WorkspaceV2(props: WorkspaceV2Props) {
  const [route, setRoute] = useState<WorkspaceRoute>(initialRoute);
  const [bridgeMode, setBridgeMode] = useState<"normal" | "developer">("normal");
  const [message, setMessage] = useState<string | null>(null);
  const [developerWorkspaces, setDeveloperWorkspaces] = useState<Record<string, DeveloperTerminalWorkspace>>({});
  const [developerSessions, setDeveloperSessions] = useState<Record<string, DeveloperModeUiSession>>({});
  const [admissionBusy, setAdmissionBusy] = useState<string | null>(null);
  const developerSessionsRef = useRef(developerSessions);
  const task = useAgentTaskLifecycle();
  const activeRoom = props.room;
  const roomIdsKey = props.rooms.filter((room) => room.status !== "burned").map((room) => room.id).sort().join(":");

  const activeCount = props.transfers.filter((transfer) => !["completed", "failed", "cancelled", "burned", "interrupted"].includes(transfer.status)).length
    + props.queueItems.filter((item) => ["queued", "preparing", "sending"].includes(item.status)).length;
  const pendingCount = task.status?.state === "draft" || task.status?.state === "approved" ? 1 : 0;
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
    const settled = await Promise.allSettled(roomIds.map(async (roomId) => ({ roomId, workspace: await getDeveloperTerminalWorkspace(roomId) })));
    const next: Record<string, DeveloperTerminalWorkspace> = {};
    for (const result of settled) if (result.status === "fulfilled") next[result.value.roomId] = result.value.workspace;
    setDeveloperWorkspaces(next);
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

  const taskSummary = useMemo(() => task.status && task.presentation ? {
    eyebrow: task.status.state === "draft" ? "Action required" : "Task",
    title: task.presentation.label,
    detail: `${task.progress} · ${task.status.readyHosts}/${task.status.totalHosts} Hosts ready`,
    tone: task.presentation.tone,
  } : null, [task.presentation, task.progress, task.status]);

  function navigate(next: WorkspaceRoute) {
    setRoute(next);
    setMessage(null);
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

  async function joinBridge(code: string) {
    setMessage(null);
    try {
      await props.onJoinBridge(code.replace(/\D/g, ""));
      setRoute("bridge");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not join this Bridge.");
      throw error;
    }
  }

  async function joinNearbyDevice(deviceId: string) {
    setMessage(null);
    try {
      await props.onJoinNearbyDevice(deviceId);
      setRoute("bridge");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not join this nearby device.");
      throw error;
    }
  }

  function openBridge(room: NonNullable<typeof activeRoom>) {
    setRoute("bridge");
    setBridgeMode("normal");
    void props.onOpenBridge(room);
  }

  async function ensureDeveloperSession(roomId: string): Promise<DeveloperModeUiSession> {
    const existing = developerSessionsRef.current[roomId];
    if (existing && existing.expiresAt > Math.floor(Date.now() / 1_000) + 2) return existing;
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

  return (
    <div className="v2-app-shell">
      <BridgeNavigation route={route} rooms={props.rooms} activeRoom={activeRoom} inboxCount={inboxCount} onNavigate={navigate} onOpenBridge={openBridge} />
      <main className="v2-main">
        {message ? <p className="v2-global-message">{message}</p> : null}
        {pendingAdmission && admissionRoom ? <section className="v2-admission-banner" role="alert"><div><strong>Developer Mode request</strong><p>{requestingPeer?.displayName ?? "A connected Host"} requests terminal access to this Host in Bridge {bridgeCode(admissionRoom)}.</p></div><div><button type="button" className="v2-button" disabled={admissionBusy !== null} onClick={() => void decideTerminalAdmission(false)}>Deny</button><button type="button" className="v2-button primary" disabled={admissionBusy !== null} onClick={() => void decideTerminalAdmission(true)}>{admissionBusy ? "Responding…" : "Accept"}</button></div></section> : null}
        {route === "bridge" ? <BridgeWorkspace room={activeRoom} items={props.roomItems} queueItems={props.queueItems} task={task} developerMode={bridgeMode === "developer"} developerSession={activeRoom ? developerSessions[activeRoom.id] ?? null : null} developerWorkspace={activeRoom ? developerWorkspaces[activeRoom.id] ?? { pendingRequests: [], sessions: [] } : { pendingRequests: [], sessions: [] }} onDeveloperSession={(session) => setDeveloperSessions((current) => ({ ...current, [session.roomId]: session }))} onRefreshDeveloper={refreshDeveloperWorkspaces} onNewBridge={() => navigate("new-bridge")} onRefresh={props.onRefreshBridge} onDeveloper={() => setBridgeMode((current) => current === "normal" ? "developer" : "normal")} onBurn={props.onBurnBridge} onEnqueue={props.onEnqueueTransferInputs} /> : null}
        {route === "activity" ? <ActivityScreen items={props.activityItems} transfers={props.transfers} queueItems={props.queueItems} onRevealInFolder={props.onRevealInFolder} /> : null}
        {route === "devices" ? <DevicesScreen room={activeRoom} /> : null}
        {route === "new-bridge" ? <NewBridgeScreen onCreate={createBridge} onJoin={joinBridge} onListNearby={props.onListNearbyDevices} onJoinNearby={joinNearbyDevice} nearbyDiscoveryAvailable={props.nearbyDiscoveryAvailable} /> : null}
        {route === "inbox" ? <InboxScreen items={props.activityItems} inboxDir={props.config.inbox_dir} onRevealInFolder={props.onRevealInFolder} /> : null}
        {route === "settings" ? <SettingsOverview config={props.config} onNavigate={navigate} onConfigChange={props.onConfigChange} /> : null}
        {route === "settings-diagnostics" ? <DiagnosticsSettings onNavigate={navigate} /> : null}
        {route === "settings-provider" ? <ProviderSettings onNavigate={navigate} /> : null}
        {route === "settings-transfer" ? <TransferSettings config={props.config} onNavigate={navigate} /> : null}
        {route === "settings-troubleshooting" ? <TroubleshootingSettings onNavigate={navigate} /> : null}
        {route === "settings-about" ? <AboutSettings config={props.config} onNavigate={navigate} /> : null}
      </main>
      <BridgeContextPanel room={activeRoom} route={route} activeCount={activeCount} pendingCount={pendingCount} taskSummary={taskSummary} onNavigate={navigate} />
    </div>
  );
}

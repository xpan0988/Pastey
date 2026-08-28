import { useMemo, useState } from "react";
import { useAgentTaskLifecycle } from "./AgentTaskLifecycle";
import { BridgeWorkspace } from "./BridgeWorkspace";
import { DeveloperContextPanel, DeveloperModeScreen } from "./DeveloperModeScreen";
import {
  AboutSettings,
  DiagnosticsSettings,
  ProviderSettings,
  SettingsOverview,
  TransferSettings,
  TroubleshootingSettings,
} from "./SettingsScreens";
import { BridgeContextPanel, BridgeNavigation, WindowTitlebar } from "./WorkspaceChrome";
import { ActivityScreen, DeviceDetailPanel, DevicesScreen, InboxScreen, NewBridgeScreen } from "./WorkspaceScreens";
import { routeTitle, SETTINGS_ROUTES, type WorkspaceRoute, type WorkspaceV2Props } from "./workspaceTypes";

const ROUTES: WorkspaceRoute[] = [
  "bridge", "activity", "devices", "new-bridge", "developer", "inbox", "settings",
  "settings-diagnostics", "settings-provider", "settings-transfer", "settings-troubleshooting", "settings-about",
];

function initialRoute(): WorkspaceRoute {
  const requested = new URLSearchParams(window.location.search).get("view") as WorkspaceRoute | null;
  return requested && ROUTES.includes(requested) ? requested : "bridge";
}

export function WorkspaceV2(props: WorkspaceV2Props) {
  const [route, setRoute] = useState<WorkspaceRoute>(initialRoute);
  const [message, setMessage] = useState<string | null>(null);
  const task = useAgentTaskLifecycle();
  const activeRoom = props.room
    ?? props.rooms.find((room) => room.peer_connected && room.status === "active")
    ?? props.rooms.find((room) => room.status === "active")
    ?? null;

  const activeCount = props.transfers.filter((transfer) => !["completed", "failed", "cancelled", "burned", "interrupted"].includes(transfer.status)).length
    + props.queueItems.filter((item) => ["queued", "preparing", "sending"].includes(item.status)).length;
  const pendingCount = task.status?.state === "draft" || task.status?.state === "approved" ? 1 : 0;
  const inboxCount = props.activityItems.filter((item) => item.direction === "incoming").length;
  const fullWidth = SETTINGS_ROUTES.has(route) || route === "new-bridge" || route === "inbox";

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

  function openBridge(room: NonNullable<typeof activeRoom>) {
    setRoute("bridge");
    void props.onOpenBridge(room);
  }

  return (
    <div className={`v2-app-shell ${fullWidth ? "v2-full-width" : ""}`}>
      <WindowTitlebar title={routeTitle(route)} />
      <BridgeNavigation route={route} rooms={props.rooms} activeRoom={activeRoom} inboxCount={inboxCount} onNavigate={navigate} onOpenBridge={openBridge} />
      <main className="v2-main">
        {message ? <p className="v2-global-message">{message}</p> : null}
        {route === "bridge" ? <BridgeWorkspace room={activeRoom} items={props.roomItems} queueItems={props.queueItems} task={task} onCreate={() => void createBridge()} onJoin={() => navigate("new-bridge")} onRefresh={props.onRefreshBridge} onBurn={props.onBurnBridge} onEnqueue={props.onEnqueueTransferInputs} /> : null}
        {route === "activity" ? <ActivityScreen items={props.activityItems} transfers={props.transfers} queueItems={props.queueItems} /> : null}
        {route === "devices" ? <DevicesScreen room={activeRoom} onAddDevice={() => navigate("new-bridge")} /> : null}
        {route === "new-bridge" ? <NewBridgeScreen onCreate={createBridge} onJoin={joinBridge} /> : null}
        {route === "developer" ? <DeveloperModeScreen room={activeRoom} /> : null}
        {route === "inbox" ? <InboxScreen items={props.activityItems} inboxDir={props.config.inbox_dir} /> : null}
        {route === "settings" ? <SettingsOverview config={props.config} onNavigate={navigate} /> : null}
        {route === "settings-diagnostics" ? <DiagnosticsSettings onNavigate={navigate} /> : null}
        {route === "settings-provider" ? <ProviderSettings onNavigate={navigate} /> : null}
        {route === "settings-transfer" ? <TransferSettings config={props.config} onNavigate={navigate} /> : null}
        {route === "settings-troubleshooting" ? <TroubleshootingSettings onNavigate={navigate} /> : null}
        {route === "settings-about" ? <AboutSettings config={props.config} onNavigate={navigate} /> : null}
      </main>
      {!fullWidth && route === "devices" ? <DeviceDetailPanel room={activeRoom} onDeveloper={() => navigate("developer")} /> : null}
      {!fullWidth && route === "developer" ? <DeveloperContextPanel room={activeRoom} /> : null}
      {!fullWidth && route !== "devices" && route !== "developer" ? <BridgeContextPanel room={activeRoom} route={route} activeCount={activeCount} pendingCount={pendingCount} taskSummary={taskSummary} onNavigate={navigate} /> : null}
    </div>
  );
}

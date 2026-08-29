import type { RoomInfo } from "../../lib/types";
import { bridgeCode, bridgeLabel, roomMembers } from "./workspaceViewModel";
import type { NavigateWorkspace, WorkspaceRoute } from "./workspaceTypes";

interface BridgeNavigationProps {
  route: WorkspaceRoute;
  rooms: RoomInfo[];
  activeRoom: RoomInfo | null;
  inboxCount: number;
  onNavigate: NavigateWorkspace;
  onOpenBridge: (room: RoomInfo) => void;
}

export function BridgeNavigation({ route, rooms, activeRoom, inboxCount, onNavigate, onOpenBridge }: BridgeNavigationProps) {
  const activeRooms = rooms.filter((room) => room.status !== "burned");
  return (
    <aside className="v2-sidebar">
      <div className="v2-brand">Pastey <span aria-hidden="true">⌄</span></div>
      <button type="button" className="v2-new-bridge" onClick={() => onNavigate("new-bridge")}>＋ New Bridge</button>
      <button type="button" className={`v2-nav-item ${route === "inbox" ? "active" : ""}`} onClick={() => onNavigate("inbox")}>
        <span>↓&nbsp;&nbsp;Inbox</span>{inboxCount > 0 ? <b>{inboxCount}</b> : null}
      </button>
      <button type="button" className={`v2-nav-item ${route === "devices" ? "active" : ""}`} onClick={() => onNavigate("devices")}>
        <span>▣&nbsp;&nbsp;Devices</span>
      </button>
      <p className="v2-nav-label">Bridges</p>
      <div className="v2-bridge-list">
        {activeRooms.map((room) => (
          <button key={room.id} type="button" className={`v2-bridge-nav ${activeRoom?.id === room.id && route === "bridge" ? "active" : ""}`} onClick={() => onOpenBridge(room)}>
            <i className={room.peer_connected ? "connected" : ""} />
            <span>{bridgeLabel(room)}</span>
          </button>
        ))}
        {activeRooms.length === 0 ? <p className="v2-empty-nav">No active Bridges</p> : null}
      </div>
      <button type="button" className="v2-show-bridges" disabled={activeRooms.length < 4}>Show all bridges</button>
      <div className="v2-sidebar-footer">
        <button type="button" className={`v2-nav-item ${route.startsWith("settings") ? "active" : ""}`} onClick={() => onNavigate("settings")}>
          <span>⚙&nbsp;&nbsp;Settings<small>Pastey 2.0</small></span>
        </button>
      </div>
    </aside>
  );
}

interface BridgeContextPanelProps {
  room: RoomInfo | null;
  route: WorkspaceRoute;
  activeCount: number;
  pendingCount: number;
  taskSummary?: { eyebrow: string; title: string; detail: string; tone: string } | null;
  onNavigate: NavigateWorkspace;
}

export function BridgeContextPanel({ room, route, activeCount, pendingCount, taskSummary, onNavigate }: BridgeContextPanelProps) {
  const peers = roomMembers(room);
  const deviceCount = room ? peers.length + 1 : 0;
  const peerState = room?.peer_connected ? "Connected" : peers.some((peer) => peer.liveness === "reconnecting") ? "Reconnecting" : peers.some((peer) => peer.liveness === "disconnected") ? "Disconnected" : room ? "Waiting" : "Unavailable";
  return (
    <aside className="v2-context-panel">
      <p className="v2-eyebrow">Current Bridge</p>
      <div className="v2-context-status"><span><i className={`v2-dot ${room?.peer_connected ? "connected" : peerState === "Reconnecting" ? "pending" : ""}`} /> {peerState}</span><small>{room ? bridgeCode(room) : "No Bridge selected"}</small></div>
      <div className="v2-context-section">
        <small>Network</small>
        <strong><i className={`v2-dot ${room?.peer_connected ? "connected" : ""}`} /> {room ? (room.peer_connected ? "Local network · Ready" : peers.length > 0 ? "No routeable peer session" : "Awaiting a connected device") : "Host data unavailable"}</strong>
        <small>Session</small>
        <strong><i className="v2-dot pending" /> Current session only</strong>
      </div>
      {taskSummary ? (
        <div className={`v2-task-summary ${taskSummary.tone}`}>
          <small>{taskSummary.eyebrow}</small><strong>{taskSummary.title}</strong><p>{taskSummary.detail}</p>
        </div>
      ) : null}
      <button type="button" className={`v2-context-link ${route === "activity" ? "active" : ""}`} onClick={() => onNavigate("activity")}>
        <span><strong>Activity</strong><small>{activeCount} active · {pendingCount} pending</small></span><b>→</b>
      </button>
      <button type="button" className={`v2-context-link ${route === "devices" ? "active" : ""}`} onClick={() => onNavigate("devices")}>
        <span><strong>Devices</strong><small>{deviceCount} current members</small></span><b>→</b>
      </button>
      {room ? (
        <div className="v2-context-devices">
          <span><i className="v2-dot connected" /><strong>This device</strong><small>Local Host</small></span>
          {peers.map((peer) => <span key={peer.peerSessionId}><i className={`v2-dot ${peer.liveness === "connected" ? "connected" : ""}`} /><strong>{peer.displayName}</strong><small>{peer.liveness}</small></span>)}
        </div>
      ) : <p className="v2-context-empty">Bridge and device state will appear when the Pastey Host is available.</p>}
    </aside>
  );
}

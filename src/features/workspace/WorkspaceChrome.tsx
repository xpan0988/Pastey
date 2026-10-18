import type { RoomInfo } from "../../lib/types";
import { bridgeStatus } from "./BridgeNetworkPanel";
import { Dot, Icon } from "./ui";
import { bridgeCode, roomMembers } from "./workspaceViewModel";
import type { NavigateWorkspace, WorkspaceRoute } from "./workspaceTypes";

interface BridgeNavigationProps {
  route: WorkspaceRoute;
  rooms: RoomInfo[];
  activeRoom: RoomInfo | null;
  inboxCount: number;
  appVersion: string;
  onNavigate: NavigateWorkspace;
  onOpenBridge: (room: RoomInfo) => void;
}

/** Global navigation: what is device-wide (Inbox, Settings) and the list
 * of Bridges. Everything about one Bridge lives inside that Bridge. */
export function BridgeNavigation({ route, rooms, activeRoom, inboxCount, appVersion, onNavigate, onOpenBridge }: BridgeNavigationProps) {
  const activeRooms = rooms.filter((room) => room.status !== "burned");
  return (
    <aside className="sidebar" aria-label="Navigation">
      <div className="sidebar-brand"><strong>Pastey</strong></div>
      <button type="button" className={`nav-item primary${route === "new-bridge" ? " active" : ""}`} onClick={() => onNavigate("new-bridge")}><Icon name="plus" />New Bridge</button>
      <button type="button" className={`nav-item${route === "inbox" ? " active" : ""}`} onClick={() => onNavigate("inbox")}>
        <Icon name="inbox" />Inbox{inboxCount > 0 ? <span className="count">{inboxCount}</span> : null}
      </button>
      <p className="nav-label">Bridges</p>
      <div className="nav-list">
        {activeRooms.map((room) => {
          const members = roomMembers(room);
          const status = bridgeStatus(room);
          const name = members.length === 0 ? "Waiting for a device" : members.length === 1 ? members[0].displayName : `${members.length + 1} devices`;
          return <button key={room.id} type="button" className={`nav-bridge${activeRoom?.id === room.id && route === "bridge" ? " active" : ""}`} onClick={() => onOpenBridge(room)} aria-current={activeRoom?.id === room.id && route === "bridge" ? "page" : undefined}>
            <Dot tone={status.tone} />
            <span><strong>{name}</strong><small>{status.label} · <span className="mono">{bridgeCode(room)}</span></small></span>
          </button>;
        })}
        {activeRooms.length === 0 ? <p className="nav-empty">No Bridges yet</p> : null}
      </div>
      <div className="sidebar-footer">
        <button type="button" className={`nav-item${route.startsWith("settings") ? " active" : ""}`} onClick={() => onNavigate("settings")}><Icon name="settings" />Settings<small>{appVersion}</small></button>
      </div>
    </aside>
  );
}

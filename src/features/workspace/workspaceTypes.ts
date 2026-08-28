import type { AppConfig, FileTransferProgressEvent, NearbyDevice, RoomInfo, RoomItem } from "../../lib/types";
import type { TransferQueueInput, TransferQueueItem } from "../../lib/transferScheduler";

export type WorkspaceRoute =
  | "bridge"
  | "activity"
  | "devices"
  | "new-bridge"
  | "developer"
  | "inbox"
  | "settings"
  | "settings-diagnostics"
  | "settings-provider"
  | "settings-transfer"
  | "settings-troubleshooting"
  | "settings-about";

export interface WorkspaceV2Props {
  config: AppConfig;
  rooms: RoomInfo[];
  room: RoomInfo | null;
  roomItems: RoomItem[];
  activityItems: RoomItem[];
  transfers: FileTransferProgressEvent[];
  queueItems: TransferQueueItem[];
  focusRequest?: { target: "home" | "settings"; token: number };
  onCreateBridge: () => Promise<void>;
  onJoinBridge: (code: string) => Promise<void>;
  onListNearbyDevices: () => Promise<NearbyDevice[]>;
  onJoinNearbyDevice: (deviceId: string) => Promise<void>;
  nearbyDiscoveryAvailable: boolean;
  onOpenBridge: (room: RoomInfo) => Promise<void>;
  onRefreshBridge: () => Promise<void>;
  onRevealInFolder: (path: string) => Promise<void>;
  onLeaveBridge: (room: RoomInfo) => Promise<void>;
  onBurnBridge: (room: RoomInfo) => Promise<void>;
  onEnqueueTransferInputs: (roomId: string, inputs: TransferQueueInput[]) => void;
}

export type NavigateWorkspace = (route: WorkspaceRoute) => void;

export const SETTINGS_ROUTES = new Set<WorkspaceRoute>([
  "settings",
  "settings-diagnostics",
  "settings-provider",
  "settings-transfer",
  "settings-troubleshooting",
  "settings-about",
]);

export function routeTitle(route: WorkspaceRoute): string {
  return ({
    bridge: "Bridge Overview",
    activity: "Activity",
    devices: "Devices Expanded",
    "new-bridge": "New Bridge / Join",
    developer: "Developer Mode",
    inbox: "Inbox",
    settings: "Settings",
    "settings-diagnostics": "Diagnostics",
    "settings-provider": "Task Provider",
    "settings-transfer": "Transfer Diagnostics",
    "settings-troubleshooting": "Troubleshooting",
    "settings-about": "About",
  })[route];
}

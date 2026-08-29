import type { AppConfig, FileTransferProgressEvent, NearbyDevice, RoomInfo, RoomItem } from "../../lib/types";
import type { TransferQueueInput, TransferQueueItem } from "../../lib/transferScheduler";

export type WorkspaceRoute =
  | "bridge"
  | "activity"
  | "devices"
  | "new-bridge"
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
  onBurnBridge: (room: RoomInfo) => Promise<void>;
  onConfigChange: (config: AppConfig) => void;
  onEnqueueTransferInputs: (roomId: string, inputs: TransferQueueInput[]) => void;
}

export type NavigateWorkspace = (route: WorkspaceRoute) => void;

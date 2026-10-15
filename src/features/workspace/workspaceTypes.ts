import type { AppConfig, FileTransferProgressEvent, NearbyDevice, NearbyJoinPending, RoomInfo, RoomItem } from "../../lib/types";
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
  /** `address` reaches a device on another network (overlay IP or name). */
  onJoinBridge: (code: string, address?: string) => Promise<void>;
  onListNearbyDevices: () => Promise<NearbyDevice[]>;
  /** Asks a nearby device to start a Bridge; resolves once its user accepts. */
  /** Resolves to the code step, or null when a device paired before connected. */
  onJoinNearbyDevice: (deviceId: string) => Promise<NearbyJoinPending | null>;
  /** Finishes a nearby join with the code the accepting device shows. */
  onCompleteNearbyJoin: (requestId: string, code: string) => Promise<void>;
  nearbyDiscoveryAvailable: boolean;
  onOpenBridge: (room: RoomInfo) => Promise<void>;
  onRefreshBridge: () => Promise<void>;
  onRevealInFolder: (path: string) => Promise<void>;
  onBurnBridge: (room: RoomInfo) => Promise<void>;
  onConfigChange: (config: AppConfig) => void;
  onEnqueueTransferInputs: (roomId: string, inputs: TransferQueueInput[]) => void;
}

export type NavigateWorkspace = (route: WorkspaceRoute) => void;

export type PayloadType = "text" | "file";

export type RoomStatus = "active" | "peer_left" | "burned" | "expired";

export type LocalRole = "creator" | "joined";

export type RoomItemDirection = "outgoing" | "incoming";

export type RoomItemStatus = "created" | "sent" | "received" | "failed" | "cancelled" | "interrupted";

export type TransferStatus = "pending" | "transferring" | "completed" | "failed" | "cancelled" | "burned" | "interrupted";

export interface RoomInfo {
  id: string;
  room_code?: string | null;
  room_code_display?: string | null;
  created_at: number;
  expires_at: number;
  status: RoomStatus;
  local_role: LocalRole;
  peer_device_name?: string | null;
  auto_burn_after_expiry: boolean;
  peer_connected: boolean;
  local_burned_at?: number | null;
  peer_burned_at?: number | null;
}

export interface NearbyDevice {
  device_id: string;
  display_name: string;
  platform: string;
  app_version: string;
  availability: "Available" | "Waiting" | "Busy" | "Expired" | string;
  capabilities: string[];
  last_seen_seconds_ago: number;
  compatible: boolean;
}

export interface JoinRequestPrompt {
  request_id: string;
  device_name: string;
  platform: string;
  app_version: string;
  received_at: number;
  expires_at: number;
}

export interface RoomItem {
  id: string;
  room_id: string;
  direction: RoomItemDirection;
  item_kind: "text" | "outgoing_file" | "incoming_file";
  payload_type: PayloadType;
  display_name?: string | null;
  mime_type?: string | null;
  size_bytes: number;
  created_at: number;
  status: RoomItemStatus;
  text?: string | null;
  saved_path?: string | null;
  error_message?: string | null;
}

export interface AppConfig {
  default_expiry_minutes: number;
  inbox_dir?: string | null;
  auto_burn_after_download: boolean;
  transfer_window_override?: number | null;
  dev_tools_enabled: boolean;
  shortcut: string;
  app_data_path: string;
  app_version: string;
}

export interface FileTransferProgressEvent {
  transfer_id: string;
  room_id: string;
  item_id: string;
  direction: RoomItemDirection;
  file_name: string;
  file_size: number;
  chunk_size: number;
  total_chunks: number;
  transferred_bytes: number;
  status: TransferStatus;
  current_speed_bps: number;
  average_speed_bps: number;
  eta_seconds?: number | null;
  error_message?: string | null;
}

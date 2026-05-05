export type PayloadType = "text" | "file";

export type RoomStatus = "active" | "peer_left" | "burned" | "expired";

export type LocalRole = "creator" | "joined";

export type RoomItemDirection = "outgoing" | "incoming";

export type RoomItemStatus = "created" | "sent" | "received" | "failed";

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

export interface RoomItem {
  id: string;
  room_id: string;
  direction: RoomItemDirection;
  payload_type: PayloadType;
  display_name?: string | null;
  mime_type?: string | null;
  size_bytes: number;
  created_at: number;
  status: RoomItemStatus;
  text?: string | null;
  saved_path?: string | null;
}

export interface AppConfig {
  default_expiry_minutes: number;
  inbox_dir?: string | null;
  auto_burn_after_download: boolean;
  shortcut: string;
  app_data_path: string;
}

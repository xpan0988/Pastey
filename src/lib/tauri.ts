import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, RoomInfo, RoomItem } from "./types";

export async function createRoom(expiryMinutes: number): Promise<RoomInfo> {
  return invoke("create_room", { expiryMinutes });
}

export async function joinRoom(code: string): Promise<RoomInfo> {
  return invoke("join_room", { code });
}

export async function listRooms(): Promise<RoomInfo[]> {
  return invoke("list_rooms");
}

export async function getRoom(roomId: string): Promise<RoomInfo> {
  return invoke("get_room", { roomId });
}

export async function listRoomItems(roomId: string): Promise<RoomItem[]> {
  return invoke("list_room_items", { roomId });
}

export async function sendTextToRoom(roomId: string, text: string): Promise<RoomItem> {
  return invoke("send_text_to_room", { roomId, text });
}

export async function sendFileToRoom(roomId: string, path: string): Promise<RoomItem> {
  return invoke("send_file_to_room", { roomId, path });
}

export async function burnRoom(roomId: string): Promise<boolean> {
  return invoke("burn_room", { roomId });
}

export async function leaveRoom(roomId: string): Promise<boolean> {
  return invoke("leave_room", { roomId });
}

export async function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

export async function updateConfig(config: AppConfig): Promise<AppConfig> {
  return invoke("update_config", { config });
}

export async function revealInFolder(path: string): Promise<void> {
  return invoke("reveal_in_folder", { path });
}

export async function copyTextToClipboard(text: string): Promise<void> {
  return invoke("copy_text_to_clipboard", { text });
}

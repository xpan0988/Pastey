import { getRouteableBridgePeers, type BridgePeerSession } from "../../lib/bridgePeers";
import { legacyRoomToBridgePeerCollection } from "../../lib/bridgeRoomAdapter";
import type { NearbyDevice, RoomInfo } from "../../lib/types";

export function uniqueNearbyDevices(devices: readonly NearbyDevice[]): NearbyDevice[] {
  return [...new Map(devices.map((device) => [device.device_id, device])).values()];
}

export function roomPeers(room: RoomInfo | null): BridgePeerSession[] {
  if (!room) return [];
  try {
    return [...getRouteableBridgePeers(legacyRoomToBridgePeerCollection(room))];
  } catch {
    return [];
  }
}

export function roomMembers(room: RoomInfo | null): BridgePeerSession[] {
  if (!room) return [];
  try {
    return [...legacyRoomToBridgePeerCollection(room).peers];
  } catch {
    return [];
  }
}

export function bridgeCode(room: RoomInfo): string {
  const raw = room.room_code_display ?? room.room_code ?? room.id.slice(0, 8);
  return raw.replace(/(\d{4})(\d{4})/, "$1 $2");
}

export function bridgeDeviceCount(room: RoomInfo): number {
  return roomMembers(room).length + 1;
}

export function bridgeLabel(room: RoomInfo): string {
  const peers = roomMembers(room);
  const summary = peers.length === 0
    ? "Waiting for device"
    : peers.length === 1
      ? peers[0].displayName
      : `${peers.length + 1} devices`;
  return `${bridgeCode(room)} · ${summary}`;
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

export function formatClock(timestampSeconds: number): string {
  return new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" })
    .format(new Date(timestampSeconds * 1_000));
}

export function formatBytes(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${Math.max(1, Math.round(bytes / 1_024))} KB`;
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}

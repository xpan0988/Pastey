import type { RoomInfo } from "./types";

export function visibleBridgeRooms(rooms: readonly RoomInfo[], closedRoomIds: ReadonlySet<string> = new Set()): RoomInfo[] {
  return rooms.filter((room) => room.status !== "burned" && !closedRoomIds.has(room.id));
}

export function chooseInitialBridgeId(rooms: readonly RoomInfo[], persistedRoomId: string | null): string {
  const visible = visibleBridgeRooms(rooms);
  if (persistedRoomId && visible.some((room) => room.id === persistedRoomId)) return persistedRoomId;
  const connected = visible.filter((room) => room.peer_connected);
  if (connected.length === 1) return connected[0].id;
  if (visible.length === 1) return visible[0].id;
  return "";
}

export function reconcileSelectedBridgeId(currentRoomId: string, rooms: readonly RoomInfo[], closedRoomIds: ReadonlySet<string>): string {
  return visibleBridgeRooms(rooms, closedRoomIds).some((room) => room.id === currentRoomId) ? currentRoomId : "";
}

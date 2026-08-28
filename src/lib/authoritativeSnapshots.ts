import type { RoomInfo, RoomItem } from "./types";

function equalJsonValue(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

export function reconcileValue<T>(current: T, next: T): T {
  return equalJsonValue(current, next) ? current : next;
}

/** Last authoritative value wins for a repeated id, while preserving order. */
export function uniqueById<T extends { id: string }>(items: readonly T[]): T[] {
  const byId = new Map<string, T>();
  for (const item of items) byId.set(item.id, item);
  return [...byId.values()];
}

/**
 * Reconciles an authoritative snapshot without creating a React state change
 * when the backend repeated the same facts.
 */
export function reconcileSnapshot<T extends { id: string }>(current: readonly T[], next: readonly T[]): T[] {
  const uniqueNext = uniqueById(next);
  if (
    current.length === uniqueNext.length
    && current.every((item, index) => item.id === uniqueNext[index].id && equalJsonValue(item, uniqueNext[index]))
  ) {
    return current as T[];
  }
  return uniqueNext;
}

export function reconcileRooms(current: readonly RoomInfo[], next: readonly RoomInfo[]): RoomInfo[] {
  return reconcileSnapshot(current, next);
}

export function reconcileRoomItems(current: readonly RoomItem[], next: readonly RoomItem[]): RoomItem[] {
  return reconcileSnapshot(current, next);
}

export function mergeRoomItems(current: readonly RoomItem[], next: readonly RoomItem[]): RoomItem[] {
  return reconcileRoomItems(
    current,
    [...current, ...next].sort((left, right) => right.created_at - left.created_at),
  );
}

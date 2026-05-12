import type { FileTransferProgressEvent, TransferStatus } from "./types";

const terminalTransferStatuses = new Set<TransferStatus>(["completed", "failed", "cancelled", "burned", "interrupted"]);

export function isTerminalTransferStatus(status: TransferStatus): boolean {
  return terminalTransferStatuses.has(status);
}

export function mergeTransferEvent(
  current: Record<string, FileTransferProgressEvent>,
  next: FileTransferProgressEvent,
  closedRoomIds: ReadonlySet<string>
): Record<string, FileTransferProgressEvent> {
  if (closedRoomIds.has(next.room_id)) {
    return current;
  }

  const existing = current[next.transfer_id];
  if (existing && isTerminalTransferStatus(existing.status)) {
    return current;
  }

  return {
    ...current,
    [next.transfer_id]: next
  };
}

export type TransferQueueItemStatus = "queued" | "preparing" | "sending" | "completed" | "failed" | "cancelled";

export type TransferQueueBatchStatus = "running" | "completed" | "completed_with_errors" | "cancelled";

export interface TransferQueueInput {
  path: string;
  displayName?: string;
  mimeType?: string | null;
  sizeBytes?: number;
  modifiedMs?: number;
  dedupeKey?: string;
  deleteWhenDone?: boolean;
}

export interface TransferQueueItem {
  id: string;
  batchId: string;
  roomId: string;
  path: string;
  displayName?: string;
  mimeType?: string | null;
  sizeBytes?: number;
  modifiedMs?: number;
  dedupeKey?: string;
  activeTransferId?: string;
  status: TransferQueueItemStatus;
  errorMessage?: string;
  deleteWhenDone: boolean;
  cancelRequested: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface TransferQueueBatch {
  id: string;
  roomId: string;
  itemIds: string[];
  status: TransferQueueBatchStatus;
  cancelRequested: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface TransferSchedulerState {
  batches: Record<string, TransferQueueBatch>;
  items: Record<string, TransferQueueItem>;
  batchOrder: string[];
}

export interface TransferQueueSummary {
  total: number;
  completed: number;
  failed: number;
  queued: number;
  cancelled: number;
  activeItem?: TransferQueueItem;
}

export interface RoomTransferQueueView {
  batches: TransferQueueBatch[];
  items: TransferQueueItem[];
  summary: TransferQueueSummary;
}

interface ProgressCorrelationInput {
  roomId: string;
  direction: string;
  fileName: string;
  fileSize: number;
  transferId: string;
  status: string;
}

let nextId = 1;

const terminalItemStatuses = new Set<TransferQueueItemStatus>(["completed", "failed", "cancelled"]);

export function createTransferSchedulerState(): TransferSchedulerState {
  return {
    batches: {},
    items: {},
    batchOrder: []
  };
}

export function isTerminalQueueItem(item: TransferQueueItem): boolean {
  return terminalItemStatuses.has(item.status);
}

export function fileIdentityKey(name: string, size: number, modifiedMs: number): string {
  return `${name}:${size}:${modifiedMs}`;
}

export function enqueueTransferBatch(
  state: TransferSchedulerState,
  roomId: string,
  inputs: TransferQueueInput[]
): TransferSchedulerState {
  const now = Date.now();
  const uniqueInputs = inputs.filter((input) => input.path.trim().length > 0);
  if (uniqueInputs.length === 0) {
    return state;
  }

  const existingKeys = nonterminalDedupeKeys(state);
  const batchId = createId("batch");
  const nextItems: Record<string, TransferQueueItem> = {};
  const itemIds: string[] = [];

  for (const input of uniqueInputs) {
    const keys = inputDedupeKeys(input);
    if (keys.some((key) => existingKeys.has(key))) {
      continue;
    }

    for (const key of keys) {
      existingKeys.add(key);
    }

    const itemId = createId("item");
    itemIds.push(itemId);
    nextItems[itemId] = {
      id: itemId,
      batchId,
      roomId,
      path: input.path,
      displayName: input.displayName,
      mimeType: input.mimeType,
      sizeBytes: input.sizeBytes,
      modifiedMs: input.modifiedMs,
      dedupeKey: input.dedupeKey,
      status: "queued",
      deleteWhenDone: input.deleteWhenDone ?? false,
      cancelRequested: false,
      createdAt: now,
      updatedAt: now
    };
  }

  if (itemIds.length === 0) {
    return state;
  }

  return {
    batches: {
      ...state.batches,
      [batchId]: {
        id: batchId,
        roomId,
        itemIds,
        status: "running",
        cancelRequested: false,
        createdAt: now,
        updatedAt: now
      }
    },
    items: {
      ...state.items,
      ...nextItems
    },
    batchOrder: [...state.batchOrder, batchId]
  };
}

export function nextQueuedTransferItem(state: TransferSchedulerState): TransferQueueItem | null {
  if (Object.values(state.items).some((item) => item.status === "preparing" || item.status === "sending")) {
    return null;
  }

  for (const batchId of state.batchOrder) {
    const batch = state.batches[batchId];
    if (!batch || batch.status !== "running" || batch.cancelRequested) {
      continue;
    }

    for (const itemId of batch.itemIds) {
      const item = state.items[itemId];
      if (item?.status === "queued" && !item.cancelRequested) {
        return item;
      }
    }
  }

  return null;
}

export function markQueueItemPreparing(state: TransferSchedulerState, itemId: string): TransferSchedulerState {
  return updateItemStatus(state, itemId, "preparing");
}

export function markQueueItemSending(
  state: TransferSchedulerState,
  itemId: string,
  metadata: {
    displayName: string;
    mimeType?: string | null;
    sizeBytes: number;
    modifiedMs: number;
  }
): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item) return state;

  return replaceItem(
    state,
    {
      ...item,
      displayName: metadata.displayName,
      mimeType: metadata.mimeType,
      sizeBytes: metadata.sizeBytes,
      modifiedMs: metadata.modifiedMs,
      dedupeKey: fileIdentityKey(metadata.displayName, metadata.sizeBytes, metadata.modifiedMs),
      status: "sending",
      errorMessage: undefined,
      updatedAt: Date.now()
    },
    true
  );
}

export function markQueueItemCompleted(state: TransferSchedulerState, itemId: string): TransferSchedulerState {
  return updateItemStatus(state, itemId, "completed");
}

export function markQueueItemFailed(
  state: TransferSchedulerState,
  itemId: string,
  errorMessage: string
): TransferSchedulerState {
  return updateItemStatus(state, itemId, "failed", errorMessage);
}

export function markQueueItemCancelled(state: TransferSchedulerState, itemId: string): TransferSchedulerState {
  return updateItemStatus(state, itemId, "cancelled");
}

export function cancelQueueItem(state: TransferSchedulerState, itemId: string): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item || isTerminalQueueItem(item)) {
    return state;
  }

  if (item.status === "queued" || item.status === "preparing") {
    return updateItemStatus(state, itemId, "cancelled");
  }

  const nextState = replaceItem(
    state,
    {
      ...item,
      cancelRequested: true,
      updatedAt: Date.now()
    },
    true
  );

  if (item.activeTransferId) {
    return nextState;
  }

  return cancelBatchLocally(nextState, item.batchId);
}

export function cancelBatchLocally(state: TransferSchedulerState, batchId: string): TransferSchedulerState {
  const batch = state.batches[batchId];
  if (!batch) {
    return state;
  }

  const now = Date.now();
  let nextState: TransferSchedulerState = {
    ...state,
    batches: {
      ...state.batches,
      [batchId]: {
        ...batch,
        status: "cancelled",
        cancelRequested: true,
        updatedAt: now
      }
    },
    items: {
      ...state.items
    }
  };

  for (const itemId of batch.itemIds) {
    const item = nextState.items[itemId];
    if (!item || isTerminalQueueItem(item)) {
      continue;
    }

    nextState.items[itemId] = item.status === "sending"
      ? { ...item, cancelRequested: true, updatedAt: now }
      : { ...item, status: "cancelled", cancelRequested: true, updatedAt: now };
  }

  return nextState;
}

export function clearQueuedItemsForRoom(state: TransferSchedulerState, roomId: string): TransferSchedulerState {
  let nextState = state;
  for (const batch of Object.values(state.batches)) {
    if (batch.roomId === roomId) {
      nextState = cancelBatchLocally(nextState, batch.id);
    }
  }
  return nextState;
}

export function correlateTransferProgress(
  state: TransferSchedulerState,
  progress: ProgressCorrelationInput
): TransferSchedulerState {
  if (progress.direction !== "outgoing" || progress.status !== "pending" && progress.status !== "transferring") {
    return state;
  }

  const item = Object.values(state.items).find((candidate) => (
    candidate.roomId === progress.roomId &&
    candidate.status === "sending" &&
    !candidate.activeTransferId &&
    candidate.displayName === progress.fileName &&
    candidate.sizeBytes === progress.fileSize
  ));

  if (!item) {
    return state;
  }

  return replaceItem(
    state,
    {
      ...item,
      activeTransferId: progress.transferId,
      updatedAt: Date.now()
    },
    true
  );
}

export function hasNonterminalDedupeKey(
  state: TransferSchedulerState,
  dedupeKey: string,
  excludingItemId: string
): boolean {
  return Object.values(state.items).some((item) => (
    item.id !== excludingItemId &&
    !isTerminalQueueItem(item) &&
    item.dedupeKey === dedupeKey
  ));
}

export function selectRoomTransferQueue(state: TransferSchedulerState, roomId: string): RoomTransferQueueView {
  const batches = state.batchOrder
    .map((batchId) => state.batches[batchId])
    .filter((batch): batch is TransferQueueBatch => Boolean(batch) && batch.roomId === roomId);
  const batchIds = new Set(batches.map((batch) => batch.id));
  const items = Object.values(state.items)
    .filter((item) => batchIds.has(item.batchId))
    .sort((left, right) => left.createdAt - right.createdAt);
  const activeItem = items.find((item) => item.status === "preparing" || item.status === "sending");

  return {
    batches,
    items,
    summary: {
      total: items.length,
      completed: items.filter((item) => item.status === "completed").length,
      failed: items.filter((item) => item.status === "failed").length,
      queued: items.filter((item) => item.status === "queued").length,
      cancelled: items.filter((item) => item.status === "cancelled").length,
      activeItem
    }
  };
}

export function activeCancellableTransferIds(state: TransferSchedulerState): string[] {
  return Object.values(state.items)
    .filter((item) => item.status === "sending" && item.cancelRequested && item.activeTransferId)
    .map((item) => item.activeTransferId)
    .filter((transferId): transferId is string => Boolean(transferId));
}

function updateItemStatus(
  state: TransferSchedulerState,
  itemId: string,
  status: TransferQueueItemStatus,
  errorMessage?: string
): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item) return state;

  return replaceItem(
    state,
    {
      ...item,
      status,
      errorMessage,
      cancelRequested: status === "cancelled" ? true : item.cancelRequested,
      updatedAt: Date.now()
    },
    true
  );
}

function replaceItem(
  state: TransferSchedulerState,
  item: TransferQueueItem,
  shouldUpdateBatch: boolean
): TransferSchedulerState {
  const nextState = {
    ...state,
    items: {
      ...state.items,
      [item.id]: item
    }
  };

  return shouldUpdateBatch ? updateBatchStatus(nextState, item.batchId) : nextState;
}

function updateBatchStatus(state: TransferSchedulerState, batchId: string): TransferSchedulerState {
  const batch = state.batches[batchId];
  if (!batch || batch.cancelRequested || batch.status === "cancelled") {
    return state;
  }

  const items = batch.itemIds.map((itemId) => state.items[itemId]).filter(Boolean);
  const hasActive = items.some((item) => item.status === "queued" || item.status === "preparing" || item.status === "sending");
  if (hasActive) {
    return replaceBatch(state, { ...batch, status: "running", updatedAt: Date.now() });
  }

  const hasFailure = items.some((item) => item.status === "failed");
  return replaceBatch(state, {
    ...batch,
    status: hasFailure ? "completed_with_errors" : "completed",
    updatedAt: Date.now()
  });
}

function replaceBatch(state: TransferSchedulerState, batch: TransferQueueBatch): TransferSchedulerState {
  return {
    ...state,
    batches: {
      ...state.batches,
      [batch.id]: batch
    }
  };
}

function nonterminalDedupeKeys(state: TransferSchedulerState): Set<string> {
  const keys = new Set<string>();
  for (const item of Object.values(state.items)) {
    if (isTerminalQueueItem(item)) {
      continue;
    }

    for (const key of inputDedupeKeys(item)) {
      keys.add(key);
    }
  }
  return keys;
}

function inputDedupeKeys(input: Pick<TransferQueueInput, "path" | "dedupeKey">): string[] {
  return [pathDedupeKey(input.path), input.dedupeKey].filter((key): key is string => Boolean(key));
}

function pathDedupeKey(path: string): string {
  return `path:${path}`;
}

function createId(prefix: string): string {
  nextId += 1;
  return `${prefix}-${Date.now()}-${nextId}`;
}

import {
  classifyTransferPlannerSize,
  DEFAULT_TRANSFER_PLANNER_POLICY,
  planWeightedTransfers,
  type MicroFlowGroupPlan,
  type TransferPlannerPolicy,
  type TransferPlannerResult,
  type TransferPlannerRunnablePlan,
  type TransferPlannerTask,
  type TransferPlannerTaskKind
} from "./transferPlanner";
import type { RoomInfo } from "./types";

export type TransferQueueItemStatus = "queued" | "preparing" | "sending" | "completed" | "failed" | "cancelled";

export type TransferQueueItemMetadataStatus = "unknown" | "loading" | "ready" | "failed";

export type TransferQueueBatchStatus = "running" | "completed" | "completed_with_errors" | "cancelled";

export type MicroFlowGroupStatus =
  | "queued"
  | "running"
  | "completed"
  | "completed_with_errors"
  | "cancelled"
  | "interrupted";

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
  requestedWindow?: number;
  status: TransferQueueItemStatus;
  metadataStatus: TransferQueueItemMetadataStatus;
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

export interface MicroFlowGroupRuntimeState {
  id: string;
  roomId: string;
  childItemIds: string[];
  requestedWindow: number;
  status: MicroFlowGroupStatus;
  completedChildIds: string[];
  failedChildIds: string[];
  cancelledChildIds: string[];
  createdAt: number;
  updatedAt: number;
  startedAt?: number;
  completedAt?: number;
  terminalReason?: string;
}

export interface TransferSchedulerState {
  batches: Record<string, TransferQueueBatch>;
  items: Record<string, TransferQueueItem>;
  microGroups: Record<string, MicroFlowGroupRuntimeState>;
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

export interface RunnableTransferLaunchPlan extends TransferPlannerRunnablePlan {
  itemId: string;
}

export interface RunnableMicroFlowGroupLaunchPlan extends MicroFlowGroupPlan {
  childItemIds: string[];
}

export interface TransferLaunchPlannerResult {
  plannerResult: TransferPlannerResult;
  runnablePlans: RunnableTransferLaunchPlan[];
  microGroupPlans: RunnableMicroFlowGroupLaunchPlan[];
}

export interface TransferWindowRebalancePlan {
  itemId: string;
  transferId: string;
  requestedWindow: number;
  previousWindow: number;
}

export interface MicroFlowGroupPlanningDiagnostics {
  tinyCandidates: number;
  eligibleTinyCandidates: number;
  largestEligibleBucket: number;
  overChildSizeLimit: number;
  metadataMissing: number;
  roomUnavailable: number;
  cancelledOrTerminal: number;
  contention: boolean;
  contentionSeverity: string;
  oneWindowQuantumBytes: number;
  dynamicChildCapBytes: number;
  dynamicGroupCapBytes: number;
  microGroupSkipReason: string;
}

export interface CancellableTransferRequest {
  transferId: string;
  itemId: string;
  batchId: string;
  roomId: string;
}

interface ProgressCorrelationInput {
  roomId: string;
  queueItemId?: string | null;
  direction: string;
  fileName: string;
  fileSize: number;
  transferId: string;
  status: string;
}

let nextId = 1;

const terminalItemStatuses = new Set<TransferQueueItemStatus>(["completed", "failed", "cancelled"]);
const terminalMicroGroupStatuses = new Set<MicroFlowGroupStatus>([
  "completed",
  "completed_with_errors",
  "cancelled",
  "interrupted"
]);
const DYNAMIC_MICRO_GROUP_MIN_WINDOW_QUANTUM_BYTES = 4 * 1024 * 1024;
const DYNAMIC_MICRO_GROUP_MAX_WINDOW_QUANTUM_BYTES = 16 * 1024 * 1024;
const DYNAMIC_MICRO_GROUP_MIN_CHILD_CAP_BYTES = 1024 * 1024;
const DYNAMIC_MICRO_GROUP_MAX_CHILD_CAP_BYTES = 4 * 1024 * 1024;
const DYNAMIC_MICRO_GROUP_MIN_GROUP_CAP_BYTES = 4 * 1024 * 1024;
const DYNAMIC_MICRO_GROUP_MAX_GROUP_CAP_BYTES = 16 * 1024 * 1024;

export function createTransferSchedulerState(): TransferSchedulerState {
  return {
    batches: {},
    items: {},
    microGroups: {},
    batchOrder: []
  };
}

export function isTerminalQueueItem(item: TransferQueueItem): boolean {
  return terminalItemStatuses.has(item.status);
}

export function isTerminalMicroFlowGroup(group: MicroFlowGroupRuntimeState): boolean {
  return terminalMicroGroupStatuses.has(group.status);
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
    const hasCompleteMetadata = inputHasCompleteMetadata(input);
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
      dedupeKey: input.dedupeKey ?? (hasCompleteMetadata
        ? fileIdentityKey(input.displayName ?? "", input.sizeBytes ?? 0, input.modifiedMs ?? 0)
        : undefined),
      status: "queued",
      metadataStatus: hasCompleteMetadata ? "ready" : "unknown",
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
    microGroups: state.microGroups,
    batchOrder: [...state.batchOrder, batchId]
  };
}

export function markQueueItemPreparing(
  state: TransferSchedulerState,
  itemId: string,
  requestedWindow?: number
): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item || isTerminalQueueItem(item)) return state;

  return replaceItem(
    state,
    {
      ...item,
      status: "preparing",
      requestedWindow: requestedWindow ?? item.requestedWindow,
      errorMessage: undefined,
      updatedAt: Date.now()
    },
    true
  );
}

export function markQueueItemMetadataLoading(state: TransferSchedulerState, itemId: string): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item || isTerminalQueueItem(item)) return state;

  return replaceItem(
    state,
    {
      ...item,
      metadataStatus: "loading",
      errorMessage: undefined,
      updatedAt: Date.now()
    },
    true
  );
}

export function markQueueItemMetadataReady(
  state: TransferSchedulerState,
  itemId: string,
  metadata: {
    displayName: string;
    mimeType?: string | null;
    sizeBytes: number;
    modifiedMs: number;
    dedupeKey: string;
  }
): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item || isTerminalQueueItem(item)) return state;

  return replaceItem(
    state,
    {
      ...item,
      displayName: metadata.displayName,
      mimeType: metadata.mimeType,
      sizeBytes: metadata.sizeBytes,
      modifiedMs: metadata.modifiedMs,
      dedupeKey: metadata.dedupeKey,
      metadataStatus: "ready",
      errorMessage: undefined,
      updatedAt: Date.now()
    },
    true
  );
}

export function markQueueItemMetadataFailed(
  state: TransferSchedulerState,
  itemId: string,
  errorMessage: string
): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item || isTerminalQueueItem(item)) return state;

  return replaceItem(
    state,
    {
      ...item,
      status: "failed",
      metadataStatus: "failed",
      errorMessage,
      updatedAt: Date.now()
    },
    true
  );
}

export function markQueueItemSending(
  state: TransferSchedulerState,
  itemId: string,
  metadata: {
    displayName: string;
    mimeType?: string | null;
    sizeBytes: number;
    modifiedMs: number;
    dedupeKey?: string;
  }
): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item || isTerminalQueueItem(item)) return state;

  return replaceItem(
    state,
    {
      ...item,
      displayName: metadata.displayName,
      mimeType: metadata.mimeType,
      sizeBytes: metadata.sizeBytes,
      modifiedMs: metadata.modifiedMs,
      dedupeKey: metadata.dedupeKey ?? fileIdentityKey(metadata.displayName, metadata.sizeBytes, metadata.modifiedMs),
      status: "sending",
      metadataStatus: "ready",
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

export function markQueueItemRuntimeWindow(
  state: TransferSchedulerState,
  itemId: string,
  requestedWindow: number
): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item || isTerminalQueueItem(item)) return state;

  return replaceItem(
    state,
    {
      ...item,
      requestedWindow,
      updatedAt: Date.now()
    },
    true
  );
}

export function markMicroFlowGroupQueued(
  state: TransferSchedulerState,
  plan: Pick<MicroFlowGroupPlan, "groupId" | "roomId" | "childTaskIds" | "requestedWindow">
): TransferSchedulerState {
  const existing = state.microGroups?.[plan.groupId];
  if (existing && isTerminalMicroFlowGroup(existing)) {
    return state;
  }

  const now = Date.now();
  const nextGroup: MicroFlowGroupRuntimeState = existing
    ? {
      ...existing,
      childItemIds: plan.childTaskIds,
      requestedWindow: plan.requestedWindow,
      updatedAt: now
    }
    : {
      id: plan.groupId,
      roomId: plan.roomId,
      childItemIds: plan.childTaskIds,
      requestedWindow: plan.requestedWindow,
      status: "queued",
      completedChildIds: [],
      failedChildIds: [],
      cancelledChildIds: [],
      createdAt: now,
      updatedAt: now
    };

  return replaceMicroFlowGroup(state, nextGroup);
}

export function markMicroFlowGroupRunning(
  state: TransferSchedulerState,
  groupId: string
): TransferSchedulerState {
  const group = state.microGroups[groupId];
  if (!group || isTerminalMicroFlowGroup(group)) {
    return state;
  }

  const now = Date.now();
  return replaceMicroFlowGroup(state, {
    ...group,
    status: "running",
    startedAt: group.startedAt ?? now,
    updatedAt: now
  });
}

export function recordMicroFlowGroupChildTerminal(
  state: TransferSchedulerState,
  groupId: string,
  childItemId: string,
  status: Extract<TransferQueueItemStatus, "completed" | "failed" | "cancelled">
): TransferSchedulerState {
  const group = state.microGroups[groupId];
  if (!group || isTerminalMicroFlowGroup(group)) {
    return state;
  }

  const completedChildIds = withoutId(group.completedChildIds, childItemId);
  const failedChildIds = withoutId(group.failedChildIds, childItemId);
  const cancelledChildIds = withoutId(group.cancelledChildIds, childItemId);

  if (status === "completed") {
    completedChildIds.push(childItemId);
  } else if (status === "failed") {
    failedChildIds.push(childItemId);
  } else {
    cancelledChildIds.push(childItemId);
  }

  return replaceMicroFlowGroup(state, {
    ...group,
    completedChildIds,
    failedChildIds,
    cancelledChildIds,
    updatedAt: Date.now()
  });
}

export function completeMicroFlowGroupFromChildren(
  state: TransferSchedulerState,
  groupId: string
): TransferSchedulerState {
  const group = state.microGroups[groupId];
  if (!group || isTerminalMicroFlowGroup(group)) {
    return state;
  }

  const childCount = group.childItemIds.length;
  const completedCount = group.completedChildIds.length;
  const failedCount = group.failedChildIds.length;
  const cancelledCount = group.cancelledChildIds.length;
  const accountedCount = completedCount + failedCount + cancelledCount;
  let status: MicroFlowGroupStatus;
  let terminalReason: string;

  if (childCount > 0 && completedCount === childCount) {
    status = "completed";
    terminalReason = "all_children_completed";
  } else if (childCount > 0 && completedCount === 0 && failedCount === 0 && cancelledCount === childCount) {
    status = "cancelled";
    terminalReason = "all_children_cancelled";
  } else {
    status = "completed_with_errors";
    terminalReason = accountedCount < childCount
      ? "some_children_unaccounted"
      : "one_or_more_children_failed_or_cancelled";
  }

  return finishMicroFlowGroup(state, groupId, status, terminalReason);
}

export function finishMicroFlowGroup(
  state: TransferSchedulerState,
  groupId: string,
  status: Exclude<MicroFlowGroupStatus, "queued" | "running">,
  terminalReason: string
): TransferSchedulerState {
  const group = state.microGroups[groupId];
  if (!group || isTerminalMicroFlowGroup(group)) {
    return state;
  }

  const now = Date.now();
  return replaceMicroFlowGroup(state, {
    ...group,
    status,
    terminalReason,
    completedAt: now,
    updatedAt: now
  });
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

  return nextState;
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

  nextState = finishMicroFlowGroupsForBatch(nextState, batchId, "cancelled", "batch_cancelled");

  return nextState;
}

export function clearQueuedItemsForRoom(state: TransferSchedulerState, roomId: string): TransferSchedulerState {
  const now = Date.now();
  let nextState: TransferSchedulerState = {
    ...state,
    batches: { ...state.batches },
    items: { ...state.items }
  };

  for (const batch of Object.values(state.batches)) {
    if (batch.roomId !== roomId) {
      continue;
    }

    nextState.batches[batch.id] = {
      ...batch,
      status: "cancelled",
      cancelRequested: true,
      updatedAt: now
    };

    for (const itemId of batch.itemIds) {
      const item = nextState.items[itemId];
      if (!item || isTerminalQueueItem(item)) {
        continue;
      }

      nextState.items[itemId] = {
        ...item,
        status: "cancelled",
        cancelRequested: true,
        updatedAt: now
      };
    }
  }

  nextState = finishMicroFlowGroupsForRoom(nextState, roomId, "interrupted", "room_cleared_or_burned");

  return nextState;
}

export function correlateTransferProgress(
  state: TransferSchedulerState,
  progress: ProgressCorrelationInput
): TransferSchedulerState {
  if (progress.direction !== "outgoing" || progress.status !== "pending" && progress.status !== "transferring") {
    return state;
  }

  if (progress.queueItemId) {
    const item = state.items[progress.queueItemId];
    if (
      !item ||
      item.roomId !== progress.roomId ||
      item.status !== "sending" ||
      item.activeTransferId
    ) {
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
  return activeCancellableTransferRequests(state).map((request) => request.transferId);
}

export function activeCancellableTransferRequests(state: TransferSchedulerState): CancellableTransferRequest[] {
  return Object.values(state.items)
    .filter((item) => item.status === "sending" && item.cancelRequested && item.activeTransferId)
    .map((item) => ({
      transferId: item.activeTransferId as string,
      itemId: item.id,
      batchId: item.batchId,
      roomId: item.roomId
    }));
}

export function planRunnableTransferLaunches(
  state: TransferSchedulerState,
  rooms: readonly Pick<RoomInfo, "id" | "status">[],
  closedRoomIds: ReadonlySet<string> = new Set(),
  launchingItemWindows: ReadonlyMap<string, number> = new Map(),
  rebalanceActiveWindows = false
): TransferLaunchPlannerResult {
  const roomStatusById = new Map(rooms.map((room) => [room.id, room.status]));
  const tasks: TransferPlannerTask[] = [];

  for (const batchId of state.batchOrder) {
    const batch = state.batches[batchId];
    if (!batch) {
      continue;
    }

    for (const itemId of batch.itemIds) {
      const item = state.items[itemId];
      if (!item) {
        continue;
      }

      const roomStatus = roomStatusById.get(item.roomId);
      const isLaunching = item.status === "queued" && launchingItemWindows.has(item.id);
      const isBatchCancelled = batch.cancelRequested || batch.status === "cancelled";
      const isActive = isLaunching || item.status === "preparing" || item.status === "sending";
      tasks.push({
        id: item.id,
        roomId: item.roomId,
        kind: transferPlannerTaskKind(item),
        state: plannerTaskState(item, isLaunching),
        metadataStatus: item.metadataStatus,
        sizeBytes: item.sizeBytes,
        roomStatus: closedRoomIds.has(item.roomId) ? "unavailable" : roomStatus ?? "unavailable",
        roomAvailable: !closedRoomIds.has(item.roomId) && roomStatus === "active",
        cancelRequested: isActive ? false : item.cancelRequested || isBatchCancelled,
        requestedWindow: isLaunching ? launchingItemWindows.get(item.id) : item.requestedWindow,
        activeRequestedWindow: isLaunching ? launchingItemWindows.get(item.id) : item.requestedWindow,
        mimeType: item.mimeType,
        createdAt: item.createdAt
      });
    }
  }

  const plannerResult = planWeightedTransfers(tasks, { rebalanceActiveWindows });
  const runnablePlans = plannerResult.runnablePlans
    .filter((plan) => {
      if (plan.kind === "micro_group") {
        return false;
      }
      const item = state.items[plan.taskId];
      const batch = item ? state.batches[item.batchId] : undefined;
      return Boolean(
        item &&
        batch &&
        item.status === "queued" &&
        item.metadataStatus === "ready" &&
        !item.cancelRequested &&
        batch.status === "running" &&
        !batch.cancelRequested &&
        !closedRoomIds.has(item.roomId) &&
        roomStatusById.get(item.roomId) === "active" &&
        !launchingItemWindows.has(item.id)
      );
    })
    .map((plan) => ({ ...plan, itemId: plan.taskId }));
  const microGroupPlans = plannerResult.microGroupPlans
    .filter((plan) => (
      plan.dispatchMode === "serial" &&
      plan.childTaskIds.every((itemId) => {
        const item = state.items[itemId];
        const batch = item ? state.batches[item.batchId] : undefined;
        return Boolean(
          item &&
          batch &&
          item.status === "queued" &&
          item.metadataStatus === "ready" &&
          !item.cancelRequested &&
          batch.status === "running" &&
          !batch.cancelRequested &&
          !closedRoomIds.has(item.roomId) &&
          roomStatusById.get(item.roomId) === "active" &&
          !launchingItemWindows.has(item.id)
        );
      })
    ))
    .map((plan) => ({ ...plan, childItemIds: plan.childTaskIds }));

  return {
    plannerResult,
    runnablePlans,
    microGroupPlans
  };
}

export function planActiveTransferWindowRebalances(
  state: TransferSchedulerState,
  rooms: readonly Pick<RoomInfo, "id" | "status">[],
  closedRoomIds: ReadonlySet<string> = new Set(),
  launchingItemWindows: ReadonlyMap<string, number> = new Map()
): TransferWindowRebalancePlan[] {
  const { plannerResult } = planRunnableTransferLaunches(
    state,
    rooms,
    closedRoomIds,
    launchingItemWindows,
    true
  );
  const roomStatusById = new Map(rooms.map((room) => [room.id, room.status]));

  return plannerResult.activePlans
    .map((plan): TransferWindowRebalancePlan | null => {
      const item = state.items[plan.taskId];
      const batch = item ? state.batches[item.batchId] : undefined;
      if (
        !item ||
        !batch ||
        item.status !== "sending" ||
        item.cancelRequested ||
        batch.cancelRequested ||
        batch.status !== "running" ||
        closedRoomIds.has(item.roomId) ||
        roomStatusById.get(item.roomId) !== "active" ||
        !item.activeTransferId ||
        item.requestedWindow === plan.requestedWindow
      ) {
        return null;
      }

      return {
        itemId: item.id,
        transferId: item.activeTransferId,
        requestedWindow: plan.requestedWindow,
        previousWindow: item.requestedWindow ?? plan.requestedWindow
      };
    })
    .filter((plan): plan is TransferWindowRebalancePlan => Boolean(plan));
}

export function summarizeMicroFlowGroupPlanning(
  state: TransferSchedulerState,
  rooms: readonly Pick<RoomInfo, "id" | "status">[],
  closedRoomIds: ReadonlySet<string> = new Set(),
  policy: TransferPlannerPolicy = DEFAULT_TRANSFER_PLANNER_POLICY
): MicroFlowGroupPlanningDiagnostics {
  const roomStatusById = new Map(rooms.map((room) => [room.id, room.status]));
  const eligibleBuckets = new Map<string, number>();
  let tinyCandidates = 0;
  let eligibleTinyCandidates = 0;
  let overChildSizeLimit = 0;
  let metadataMissing = 0;
  let roomUnavailable = 0;
  let cancelledOrTerminal = 0;
  let readyQueuedCount = 0;
  let largestReadyQueuedBytes = 0;
  let hasBulkCandidate = false;

  for (const batchId of state.batchOrder) {
    const batch = state.batches[batchId];
    if (!batch) {
      continue;
    }

    for (const itemId of batch.itemIds) {
      const item = state.items[itemId];
      if (!item) {
        continue;
      }

      if (isTerminalQueueItem(item) || item.cancelRequested || batch.cancelRequested || batch.status === "cancelled") {
        cancelledOrTerminal += 1;
        continue;
      }
      if (item.status !== "queued") {
        continue;
      }

      const roomStatus = closedRoomIds.has(item.roomId) ? "unavailable" : roomStatusById.get(item.roomId);
      if (roomStatus !== "active") {
        roomUnavailable += 1;
        continue;
      }
      if (item.metadataStatus !== "ready" || typeof item.sizeBytes !== "number") {
        metadataMissing += 1;
        continue;
      }
      readyQueuedCount += 1;
      largestReadyQueuedBytes = Math.max(largestReadyQueuedBytes, item.sizeBytes);
      if (item.sizeBytes > policy.microGroupMaxChildSizeBytes) {
        overChildSizeLimit += 1;
      }

      const sizeClass = classifyTransferPlannerSize(item.sizeBytes);
      const lane = sizeClass === "large" || sizeClass === "huge" ? "bulk_file" : "small_file";
      if (lane === "bulk_file") {
        hasBulkCandidate = true;
      }
      if (item.sizeBytes > policy.microGroupMaxChildSizeBytes) {
        continue;
      }
      if ((sizeClass !== "tiny" && sizeClass !== "small") || lane !== "small_file") {
        continue;
      }

      tinyCandidates += 1;
      eligibleTinyCandidates += 1;
      const key = [
        item.roomId,
        lane,
        sizeClass,
        "file_like",
        broadMimeFamily(item.mimeType)
      ].join(":");
      eligibleBuckets.set(key, (eligibleBuckets.get(key) ?? 0) + 1);
    }
  }

  const largestEligibleBucket = Math.max(0, ...eligibleBuckets.values());
  const contention = readyQueuedCount > policy.safetyActiveTransferCap ||
    (hasBulkCandidate && readyQueuedCount > 1 && (eligibleTinyCandidates > 0 || overChildSizeLimit > 0));
  const unclampedOneWindowQuantumBytes = Math.floor(largestReadyQueuedBytes / policy.globalWindowBudget);
  const oneWindowQuantumBytes = contention
    ? clampBytes(
      unclampedOneWindowQuantumBytes,
      DYNAMIC_MICRO_GROUP_MIN_WINDOW_QUANTUM_BYTES,
      DYNAMIC_MICRO_GROUP_MAX_WINDOW_QUANTUM_BYTES
    )
    : Math.max(policy.microGroupMaxChildSizeBytes, unclampedOneWindowQuantumBytes);
  const dynamicChildCapBytes = contention
    ? clampBytes(
      Math.max(policy.microGroupMaxChildSizeBytes, oneWindowQuantumBytes),
      DYNAMIC_MICRO_GROUP_MIN_CHILD_CAP_BYTES,
      DYNAMIC_MICRO_GROUP_MAX_CHILD_CAP_BYTES
    )
    : policy.microGroupMaxChildSizeBytes;
  const dynamicGroupCapBytes = contention
    ? clampBytes(
      Math.max(policy.microGroupMaxGroupBytes, dynamicChildCapBytes * 4),
      DYNAMIC_MICRO_GROUP_MIN_GROUP_CAP_BYTES,
      DYNAMIC_MICRO_GROUP_MAX_GROUP_CAP_BYTES
    )
    : policy.microGroupMaxGroupBytes;

  return {
    tinyCandidates,
    eligibleTinyCandidates,
    largestEligibleBucket,
    overChildSizeLimit,
    metadataMissing,
    roomUnavailable,
    cancelledOrTerminal,
    contention,
    contentionSeverity: contentionSeverity(readyQueuedCount, policy),
    oneWindowQuantumBytes,
    dynamicChildCapBytes,
    dynamicGroupCapBytes,
    microGroupSkipReason: microGroupSkipReason({
      policy,
      eligibleTinyCandidates,
      largestEligibleBucket,
      overChildSizeLimit,
      metadataMissing,
      roomUnavailable,
      contention
    })
  };
}

export function queuedItemsNeedingMetadata(
  state: TransferSchedulerState,
  rooms: readonly Pick<RoomInfo, "id" | "status">[],
  closedRoomIds: ReadonlySet<string> = new Set(),
  loadingItemIds: ReadonlySet<string> = new Set()
): TransferQueueItem[] {
  const roomStatusById = new Map(rooms.map((room) => [room.id, room.status]));
  const items: TransferQueueItem[] = [];

  for (const batchId of state.batchOrder) {
    const batch = state.batches[batchId];
    if (!batch || batch.status !== "running" || batch.cancelRequested) {
      continue;
    }

    for (const itemId of batch.itemIds) {
      const item = state.items[itemId];
      if (
        item?.status === "queued" &&
        item.metadataStatus === "unknown" &&
        !item.cancelRequested &&
        !closedRoomIds.has(item.roomId) &&
        roomStatusById.get(item.roomId) === "active" &&
        !loadingItemIds.has(item.id)
      ) {
        items.push(item);
      }
    }
  }

  return items;
}

function updateItemStatus(
  state: TransferSchedulerState,
  itemId: string,
  status: TransferQueueItemStatus,
  errorMessage?: string
): TransferSchedulerState {
  const item = state.items[itemId];
  if (!item) return state;
  if (isTerminalQueueItem(item) && item.status !== status) return state;

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

function replaceMicroFlowGroup(
  state: TransferSchedulerState,
  group: MicroFlowGroupRuntimeState
): TransferSchedulerState {
  return {
    ...state,
    microGroups: {
      ...(state.microGroups ?? {}),
      [group.id]: group
    }
  };
}

function finishMicroFlowGroupsForBatch(
  state: TransferSchedulerState,
  batchId: string,
  status: Exclude<MicroFlowGroupStatus, "queued" | "running">,
  terminalReason: string
): TransferSchedulerState {
  const batch = state.batches[batchId];
  if (!batch) {
    return state;
  }

  const batchItemIds = new Set(batch.itemIds);
  let nextState = state;
  for (const group of Object.values(state.microGroups ?? {})) {
    if (
      !isTerminalMicroFlowGroup(group) &&
      group.childItemIds.some((itemId) => batchItemIds.has(itemId))
    ) {
      nextState = finishMicroFlowGroup(nextState, group.id, status, terminalReason);
    }
  }
  return nextState;
}

function finishMicroFlowGroupsForRoom(
  state: TransferSchedulerState,
  roomId: string,
  status: Exclude<MicroFlowGroupStatus, "queued" | "running">,
  terminalReason: string
): TransferSchedulerState {
  let nextState = state;
  for (const group of Object.values(state.microGroups ?? {})) {
    if (!isTerminalMicroFlowGroup(group) && group.roomId === roomId) {
      nextState = finishMicroFlowGroup(nextState, group.id, status, terminalReason);
    }
  }
  return nextState;
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

function plannerTaskState(item: TransferQueueItem, isLaunching: boolean): TransferPlannerTask["state"] {
  if (item.status === "queued") {
    return isLaunching ? "active" : "queued";
  }
  if (item.status === "preparing" || item.status === "sending") {
    return "active";
  }
  return item.status;
}

function transferPlannerTaskKind(item: TransferQueueItem): TransferPlannerTaskKind {
  if (item.deleteWhenDone && item.mimeType?.startsWith("image/")) {
    return "pasted_image";
  }
  if (item.mimeType?.startsWith("image/")) {
    return "image";
  }
  return "file";
}

function broadMimeFamily(mimeType?: string | null): string {
  const trimmed = mimeType?.trim().toLowerCase();
  if (!trimmed || !trimmed.includes("/")) {
    return "unknown";
  }
  return trimmed.split("/", 1)[0] || "unknown";
}

function microGroupSkipReason(input: {
  policy: TransferPlannerPolicy;
  eligibleTinyCandidates: number;
  largestEligibleBucket: number;
  overChildSizeLimit: number;
  metadataMissing: number;
  roomUnavailable: number;
  contention: boolean;
}): string {
  if (input.policy.microGroupDispatchMode !== "serial" && input.policy.microGroupDispatchMode !== "shadow") {
    return "disabled";
  }
  if (!input.contention) {
    return "no_contention";
  }
  if (input.eligibleTinyCandidates === 0 && input.metadataMissing > 0) {
    return "metadata_missing";
  }
  if (input.eligibleTinyCandidates === 0 && input.roomUnavailable > 0) {
    return "room_unavailable";
  }
  if (input.eligibleTinyCandidates === 0 && input.overChildSizeLimit > 0) {
    return "over_child_size_limit";
  }
  if (input.largestEligibleBucket < 2) {
    return "not_enough_eligible_children";
  }
  return "not_selected_by_allocator";
}

function clampBytes(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.floor(value)));
}

function contentionSeverity(readyQueuedCount: number, policy: TransferPlannerPolicy): string {
  if (readyQueuedCount <= policy.safetyActiveTransferCap) {
    return "none";
  }
  if (readyQueuedCount <= policy.safetyActiveTransferCap * 2) {
    return "moderate";
  }
  return "high";
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

function inputHasCompleteMetadata(input: TransferQueueInput): boolean {
  return Boolean(input.displayName) &&
    typeof input.sizeBytes === "number" &&
    typeof input.modifiedMs === "number";
}

function pathDedupeKey(path: string): string {
  return `path:${path}`;
}

function withoutId(values: readonly string[], id: string): string[] {
  return values.filter((value) => value !== id);
}

function createId(prefix: string): string {
  nextId += 1;
  return `${prefix}-${Date.now()}-${nextId}`;
}

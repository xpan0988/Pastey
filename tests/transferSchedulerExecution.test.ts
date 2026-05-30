import assert from "node:assert/strict";
import test from "node:test";

import {
  activeCancellableTransferIds,
  cancelBatchLocally,
  cancelQueueItem,
  clearQueuedItemsForRoom,
  correlateTransferProgress,
  createTransferSchedulerState,
  enqueueTransferBatch,
  markQueueItemCompleted,
  markQueueItemFailed,
  markQueueItemMetadataFailed,
  markQueueItemMetadataReady,
  markQueueItemPreparing,
  markQueueItemRuntimeWindow,
  markQueueItemSending,
  planActiveTransferWindowRebalances,
  planRunnableTransferLaunches,
  type TransferQueueItem,
  type TransferSchedulerState
} from "../src/lib/transferScheduler";

const MiB = 1024 * 1024;
const GiB = 1024 * MiB;
const activeRooms = [{ id: "room-1", status: "active" as const }];

function readyInput(name: string, sizeBytes: number, path = `/tmp/${name}`, modifiedMs = sizeBytes) {
  return {
    path,
    displayName: name,
    mimeType: "application/octet-stream",
    sizeBytes,
    modifiedMs
  };
}

function queuedItems(state: TransferSchedulerState): TransferQueueItem[] {
  return Object.values(state.items).sort((left, right) => left.createdAt - right.createdAt);
}

test("huge-only queue starts one transfer", () => {
  const state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("huge.bin", 2 * GiB)
  ]);

  const { runnablePlans } = planRunnableTransferLaunches(state, activeRooms);

  assert.equal(runnablePlans.length, 1);
  assert.equal(runnablePlans[0].requestedWindow, 8);
});

test("huge-plus-small queue starts both with expected requested windows", () => {
  const state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("huge.bin", 2 * GiB, "/tmp/huge.bin", 1),
    readyInput("small.bin", 1 * MiB, "/tmp/small.bin", 2)
  ]);

  const { runnablePlans } = planRunnableTransferLaunches(state, activeRooms);
  const byName = new Map(runnablePlans.map((plan) => [state.items[plan.itemId].displayName, plan]));

  assert.equal(runnablePlans.length, 2);
  assert.equal(byName.get("huge.bin")?.requestedWindow, 7);
  assert.equal(byName.get("small.bin")?.requestedWindow, 1);
});

test("many-small queue starts bounded multiple transfers", () => {
  const state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    Array.from({ length: 16 }, (_, index) => readyInput(`small-${index}.bin`, 128 * 1024, `/tmp/small-${index}.bin`, index))
  );

  const { runnablePlans } = planRunnableTransferLaunches(state, activeRooms);

  assert.equal(runnablePlans.length, 4);
  assert.deepEqual(runnablePlans.map((plan) => plan.requestedWindow), [2, 2, 2, 2]);
});

test("planner rerun does not duplicate an already launching item", () => {
  const state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("huge.bin", 2 * GiB)
  ]);
  const item = queuedItems(state)[0];
  const launching = new Map([[item.id, 8]]);

  const { runnablePlans, plannerResult } = planRunnableTransferLaunches(state, activeRooms, new Set(), launching);

  assert.equal(runnablePlans.length, 0);
  assert.equal(plannerResult.activePlans.length, 1);
  assert.equal(plannerResult.activePlans[0].requestedWindow, 8);
});

test("cancelled item does not launch", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("cancelled.bin", 1 * MiB)
  ]);
  const item = queuedItems(state)[0];
  state = cancelBatchLocally(state, item.batchId);

  const { runnablePlans } = planRunnableTransferLaunches(state, activeRooms);

  assert.equal(runnablePlans.length, 0);
});

test("burned and closed room items do not launch", () => {
  const burnedState = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("burned.bin", 1 * MiB)
  ]);
  const closedState = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("closed.bin", 1 * MiB)
  ]);

  assert.equal(planRunnableTransferLaunches(burnedState, [{ id: "room-1", status: "burned" }]).runnablePlans.length, 0);
  assert.equal(planRunnableTransferLaunches(closedState, activeRooms, new Set(["room-1"])).runnablePlans.length, 0);
});

test("batch cancellation handles queued, preparing, and active sending items", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("queued.bin", 1 * MiB, "/tmp/queued.bin", 1),
    readyInput("preparing.bin", 1 * MiB, "/tmp/preparing.bin", 2),
    readyInput("sending-a.bin", 1 * MiB, "/tmp/sending-a.bin", 3),
    readyInput("sending-b.bin", 1 * MiB, "/tmp/sending-b.bin", 4)
  ]);
  const [queued, preparing, sendingA, sendingB] = queuedItems(state);
  state = markQueueItemPreparing(state, preparing.id, 1);
  state = markQueueItemSending(state, sendingA.id, {
    displayName: "sending-a.bin",
    sizeBytes: 1 * MiB,
    modifiedMs: 3,
    dedupeKey: "sending-a"
  });
  state = markQueueItemSending(state, sendingB.id, {
    displayName: "sending-b.bin",
    sizeBytes: 1 * MiB,
    modifiedMs: 4,
    dedupeKey: "sending-b"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: sendingA.id,
    direction: "outgoing",
    fileName: "sending-a.bin",
    fileSize: 1 * MiB,
    transferId: "transfer-sending-a",
    status: "transferring"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: sendingB.id,
    direction: "outgoing",
    fileName: "sending-b.bin",
    fileSize: 1 * MiB,
    transferId: "transfer-sending-b",
    status: "transferring"
  });

  state = cancelBatchLocally(state, queued.batchId);

  assert.equal(state.items[queued.id].status, "cancelled");
  assert.equal(state.items[preparing.id].status, "cancelled");
  assert.equal(state.items[sendingA.id].status, "sending");
  assert.equal(state.items[sendingA.id].cancelRequested, true);
  assert.equal(state.items[sendingB.id].status, "sending");
  assert.equal(state.items[sendingB.id].cancelRequested, true);
  assert.deepEqual(activeCancellableTransferIds(state), ["transfer-sending-a", "transfer-sending-b"]);

  const { runnablePlans, plannerResult } = planRunnableTransferLaunches(state, activeRooms);
  assert.equal(runnablePlans.length, 0);
  assert.equal(plannerResult.activePlans.length, 2);
  assert.equal(plannerResult.requestedWindowTotal, 2);
});

test("single cancel before transfer id correlation does not cancel unrelated queued work", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("sending.bin", 2 * GiB, "/tmp/sending.bin", 1),
    readyInput("queued.bin", 1 * MiB, "/tmp/queued.bin", 2)
  ]);
  const [sending, queued] = queuedItems(state);
  state = markQueueItemPreparing(state, sending.id, 8);
  state = markQueueItemSending(state, sending.id, {
    displayName: "sending.bin",
    sizeBytes: 2 * GiB,
    modifiedMs: 1,
    dedupeKey: "sending"
  });

  state = cancelQueueItem(state, sending.id);

  assert.equal(state.items[sending.id].status, "sending");
  assert.equal(state.items[sending.id].cancelRequested, true);
  assert.equal(state.items[queued.id].status, "queued");
  assert.equal(state.batches[sending.batchId].status, "running");
  assert.deepEqual(activeCancellableTransferIds(state), []);

  const { runnablePlans, plannerResult } = planRunnableTransferLaunches(state, activeRooms);
  assert.equal(runnablePlans.length, 0);
  assert.equal(plannerResult.activePlans.length, 1);
  assert.equal(plannerResult.activePlans[0].requestedWindow, 8);

  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: sending.id,
    direction: "outgoing",
    fileName: "sending.bin",
    fileSize: 2 * GiB,
    transferId: "transfer-sending",
    status: "transferring"
  });

  assert.deepEqual(activeCancellableTransferIds(state), ["transfer-sending"]);
});

test("single cancel after transfer id correlation targets only that active item", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("active.bin", 1 * MiB, "/tmp/active.bin", 1),
    readyInput("next.bin", 1 * MiB, "/tmp/next.bin", 2)
  ]);
  const [active, next] = queuedItems(state);
  state = markQueueItemSending(state, active.id, {
    displayName: "active.bin",
    sizeBytes: 1 * MiB,
    modifiedMs: 1,
    dedupeKey: "active"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: active.id,
    direction: "outgoing",
    fileName: "active.bin",
    fileSize: 1 * MiB,
    transferId: "transfer-active",
    status: "transferring"
  });

  state = cancelQueueItem(state, active.id);

  assert.equal(state.items[active.id].status, "sending");
  assert.equal(state.items[active.id].cancelRequested, true);
  assert.equal(state.items[next.id].status, "queued");
  assert.deepEqual(activeCancellableTransferIds(state), ["transfer-active"]);

  const { runnablePlans, plannerResult } = planRunnableTransferLaunches(state, activeRooms);
  assert.equal(runnablePlans.length, 1);
  assert.equal(runnablePlans[0].itemId, next.id);
  assert.equal(plannerResult.requestedWindowTotal, 8);
});

test("burned room cancellation stops queued, preparing, and several active sending items", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("queued.bin", 1 * MiB, "/tmp/queued.bin", 1),
    readyInput("preparing.bin", 1 * MiB, "/tmp/preparing.bin", 2),
    readyInput("sending-a.bin", 1 * MiB, "/tmp/sending-a.bin", 3),
    readyInput("sending-b.bin", 1 * MiB, "/tmp/sending-b.bin", 4)
  ]);
  const [queued, preparing, sendingA, sendingB] = queuedItems(state);
  state = markQueueItemPreparing(state, preparing.id, 1);
  state = markQueueItemSending(state, sendingA.id, {
    displayName: "sending-a.bin",
    sizeBytes: 1 * MiB,
    modifiedMs: 3,
    dedupeKey: "sending-a"
  });
  state = markQueueItemSending(state, sendingB.id, {
    displayName: "sending-b.bin",
    sizeBytes: 1 * MiB,
    modifiedMs: 4,
    dedupeKey: "sending-b"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: sendingA.id,
    direction: "outgoing",
    fileName: "sending-a.bin",
    fileSize: 1 * MiB,
    transferId: "transfer-sending-a",
    status: "transferring"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: sendingB.id,
    direction: "outgoing",
    fileName: "sending-b.bin",
    fileSize: 1 * MiB,
    transferId: "transfer-sending-b",
    status: "transferring"
  });

  state = clearQueuedItemsForRoom(state, "room-1");

  assert.equal(state.items[queued.id].status, "cancelled");
  assert.equal(state.items[preparing.id].status, "cancelled");
  assert.equal(state.items[sendingA.id].status, "sending");
  assert.equal(state.items[sendingA.id].cancelRequested, true);
  assert.equal(state.items[sendingB.id].status, "sending");
  assert.equal(state.items[sendingB.id].cancelRequested, true);
  assert.deepEqual(activeCancellableTransferIds(state), ["transfer-sending-a", "transfer-sending-b"]);
  assert.equal(planRunnableTransferLaunches(state, [{ id: "room-1", status: "burned" }]).runnablePlans.length, 0);
});

test("failed item does not block unrelated queued work", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("failed.bin", 1 * MiB, "/tmp/failed.bin", 1),
    readyInput("next.bin", 1 * MiB, "/tmp/next.bin", 2)
  ]);
  const [failed, next] = queuedItems(state);
  state = markQueueItemFailed(state, failed.id, "boom");

  const { runnablePlans } = planRunnableTransferLaunches(state, activeRooms);

  assert.deepEqual(runnablePlans.map((plan) => plan.itemId), [next.id]);
});

test("late queue mutations do not resurrect terminal queue items", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("cancelled.bin", 1 * MiB)
  ]);
  const item = queuedItems(state)[0];
  state = cancelQueueItem(state, item.id);

  const afterReady = markQueueItemMetadataReady(state, item.id, {
    displayName: "cancelled.bin",
    mimeType: "application/octet-stream",
    sizeBytes: 1 * MiB,
    modifiedMs: 1,
    dedupeKey: "cancelled"
  });
  const afterFailed = markQueueItemMetadataFailed(afterReady, item.id, "late metadata failure");
  const afterSending = markQueueItemSending(afterFailed, item.id, {
    displayName: "cancelled.bin",
    sizeBytes: 1 * MiB,
    modifiedMs: 1,
    dedupeKey: "cancelled"
  });
  const afterCompleted = markQueueItemCompleted(afterSending, item.id);
  const afterFailedStatus = markQueueItemFailed(afterCompleted, item.id, "late transfer failure");

  assert.equal(afterFailedStatus.items[item.id].status, "cancelled");
  assert.equal(afterFailedStatus.items[item.id].errorMessage, undefined);
  assert.equal(planRunnableTransferLaunches(afterFailedStatus, activeRooms).runnablePlans.length, 0);
});

test("same-name same-size concurrent queued files keep queue-item correlation", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("same.bin", 1 * MiB, "/tmp/a/same.bin", 1),
    readyInput("same.bin", 1 * MiB, "/tmp/b/same.bin", 2)
  ]);
  const [first, second] = queuedItems(state);
  state = markQueueItemSending(state, first.id, {
    displayName: "same.bin",
    sizeBytes: 1 * MiB,
    modifiedMs: 1,
    dedupeKey: "first"
  });
  state = markQueueItemSending(state, second.id, {
    displayName: "same.bin",
    sizeBytes: 1 * MiB,
    modifiedMs: 2,
    dedupeKey: "second"
  });

  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: second.id,
    direction: "outgoing",
    fileName: "same.bin",
    fileSize: 1 * MiB,
    transferId: "transfer-second",
    status: "transferring"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: first.id,
    direction: "outgoing",
    fileName: "same.bin",
    fileSize: 1 * MiB,
    transferId: "transfer-first",
    status: "transferring"
  });

  assert.equal(state.items[first.id].activeTransferId, "transfer-first");
  assert.equal(state.items[second.id].activeTransferId, "transfer-second");
});

test("completion-only rebalance requests remaining huge transfer window update", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("huge.bin", 2 * GiB, "/tmp/huge.bin", 1),
    readyInput("small.bin", 1 * MiB, "/tmp/small.bin", 2)
  ]);
  const [huge, small] = queuedItems(state);
  state = markQueueItemPreparing(state, huge.id, 7);
  state = markQueueItemSending(state, huge.id, {
    displayName: "huge.bin",
    sizeBytes: 2 * GiB,
    modifiedMs: 1,
    dedupeKey: "huge"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: huge.id,
    direction: "outgoing",
    fileName: "huge.bin",
    fileSize: 2 * GiB,
    transferId: "transfer-huge",
    status: "transferring"
  });
  state = markQueueItemCompleted(state, small.id);

  const plans = planActiveTransferWindowRebalances(state, activeRooms);

  assert.deepEqual(plans, [{
    itemId: huge.id,
    transferId: "transfer-huge",
    requestedWindow: 8,
    previousWindow: 7
  }]);
  assert.deepEqual(activeCancellableTransferIds(state), []);
});

test("completion-only rebalance ignores stale launching marker for already sending item", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("huge.bin", 2 * GiB, "/tmp/huge.bin", 1),
    readyInput("small.bin", 1 * MiB, "/tmp/small.bin", 2)
  ]);
  const [huge, small] = queuedItems(state);
  state = markQueueItemPreparing(state, huge.id, 7);
  state = markQueueItemSending(state, huge.id, {
    displayName: "huge.bin",
    sizeBytes: 2 * GiB,
    modifiedMs: 1,
    dedupeKey: "huge"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: huge.id,
    direction: "outgoing",
    fileName: "huge.bin",
    fileSize: 2 * GiB,
    transferId: "transfer-huge",
    status: "transferring"
  });
  state = markQueueItemCompleted(state, small.id);

  const plans = planActiveTransferWindowRebalances(
    state,
    activeRooms,
    new Set(),
    new Map([[huge.id, 7]])
  );

  assert.deepEqual(plans, [{
    itemId: huge.id,
    transferId: "transfer-huge",
    requestedWindow: 8,
    previousWindow: 7
  }]);
});

test("runtime rebalance skips unchanged, uncorrelated, cancelled, and closed-room active items", () => {
  let unchangedState = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("huge.bin", 2 * GiB)
  ]);
  const unchanged = queuedItems(unchangedState)[0];
  unchangedState = markQueueItemPreparing(unchangedState, unchanged.id, 8);
  unchangedState = markQueueItemSending(unchangedState, unchanged.id, {
    displayName: "huge.bin",
    sizeBytes: 2 * GiB,
    modifiedMs: 2 * GiB,
    dedupeKey: "huge"
  });
  unchangedState = correlateTransferProgress(unchangedState, {
    roomId: "room-1",
    queueItemId: unchanged.id,
    direction: "outgoing",
    fileName: "huge.bin",
    fileSize: 2 * GiB,
    transferId: "transfer-huge",
    status: "transferring"
  });
  assert.equal(planActiveTransferWindowRebalances(unchangedState, activeRooms).length, 0);

  let uncorrelatedState = markQueueItemRuntimeWindow(unchangedState, unchanged.id, 7);
  uncorrelatedState = {
    ...uncorrelatedState,
    items: {
      ...uncorrelatedState.items,
      [unchanged.id]: {
        ...uncorrelatedState.items[unchanged.id],
        activeTransferId: undefined
      }
    }
  };
  assert.equal(planActiveTransferWindowRebalances(uncorrelatedState, activeRooms).length, 0);

  let cancelledState = markQueueItemRuntimeWindow(unchangedState, unchanged.id, 7);
  cancelledState = cancelQueueItem(cancelledState, unchanged.id);
  assert.equal(planActiveTransferWindowRebalances(cancelledState, activeRooms).length, 0);
  assert.equal(planActiveTransferWindowRebalances(unchangedState, activeRooms, new Set(["room-1"])).length, 0);
});

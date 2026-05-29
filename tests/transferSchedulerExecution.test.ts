import assert from "node:assert/strict";
import test from "node:test";

import {
  activeCancellableTransferIds,
  cancelBatchLocally,
  correlateTransferProgress,
  createTransferSchedulerState,
  enqueueTransferBatch,
  markQueueItemFailed,
  markQueueItemPreparing,
  markQueueItemSending,
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
    readyInput("sending.bin", 1 * MiB, "/tmp/sending.bin", 3)
  ]);
  const [queued, preparing, sending] = queuedItems(state);
  state = markQueueItemPreparing(state, preparing.id, 1);
  state = markQueueItemSending(state, sending.id, {
    displayName: "sending.bin",
    sizeBytes: 1 * MiB,
    modifiedMs: 3,
    dedupeKey: "sending"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: sending.id,
    direction: "outgoing",
    fileName: "sending.bin",
    fileSize: 1 * MiB,
    transferId: "transfer-sending",
    status: "transferring"
  });

  state = cancelBatchLocally(state, queued.batchId);

  assert.equal(state.items[queued.id].status, "cancelled");
  assert.equal(state.items[preparing.id].status, "cancelled");
  assert.equal(state.items[sending.id].status, "sending");
  assert.equal(state.items[sending.id].cancelRequested, true);
  assert.deepEqual(activeCancellableTransferIds(state), ["transfer-sending"]);
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

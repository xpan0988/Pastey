import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_TRANSFER_PLANNER_POLICY
} from "../src/lib/transferPlanner";
import {
  activeCancellableTransferIds,
  cancelBatchLocally,
  cancelQueueItem,
  clearQueuedItemsForRoom,
  completeMicroFlowGroupFromChildren,
  correlateTransferProgress,
  createTransferSchedulerState,
  enqueueTransferBatch,
  markMicroFlowGroupQueued,
  markMicroFlowGroupRunning,
  markQueueItemCompleted,
  markQueueItemFailed,
  markQueueItemMetadataFailed,
  markQueueItemMetadataReady,
  markQueueItemPreparing,
  markQueueItemRuntimeWindow,
  markQueueItemSending,
  planActiveTransferWindowRebalances,
  planRunnableTransferLaunches,
  recordMicroFlowGroupChildTerminal,
  summarizeMicroFlowGroupPlanning,
  type TransferQueueItem,
  type TransferSchedulerState
} from "../src/lib/transferScheduler";

const MiB = 1024 * 1024;
const GiB = 1024 * MiB;
const TWO_POINT_SEVEN_GB_BYTES = 2707513952;
const ONE_HUNDRED_FORTY_SEVEN_MB_BYTES = 147642115;
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

test("2.7GB plus 147MB metadata-ready together are both runnable in one planner pass", () => {
  const state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("2.7gb.bin", TWO_POINT_SEVEN_GB_BYTES, "/tmp/2.7gb.bin", 1),
    readyInput("147mb.bin", ONE_HUNDRED_FORTY_SEVEN_MB_BYTES, "/tmp/147mb.bin", 2)
  ]);

  const { runnablePlans, plannerResult } = planRunnableTransferLaunches(state, activeRooms);
  const byName = new Map(runnablePlans.map((plan) => [state.items[plan.itemId].displayName, plan]));

  assert.equal(runnablePlans.length, 2);
  assert.equal(plannerResult.activePlans.length, 0);
  assert.equal(byName.get("2.7gb.bin")?.lane, "bulk_file");
  assert.equal(byName.get("147mb.bin")?.lane, "bulk_file");
  assert.equal(byName.get("2.7gb.bin")?.requestedWindow, 7);
  assert.equal(byName.get("147mb.bin")?.requestedWindow, 1);
});

test("all runnable plans from one planner pass can be marked launching together", () => {
  const state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("2.7gb.bin", TWO_POINT_SEVEN_GB_BYTES, "/tmp/2.7gb.bin", 1),
    readyInput("147mb.bin", ONE_HUNDRED_FORTY_SEVEN_MB_BYTES, "/tmp/147mb.bin", 2)
  ]);
  const firstPass = planRunnableTransferLaunches(state, activeRooms);
  const launching = new Map(firstPass.runnablePlans.map((plan) => [plan.itemId, plan.requestedWindow]));

  const secondPass = planRunnableTransferLaunches(state, activeRooms, new Set(), launching);

  assert.equal(firstPass.runnablePlans.length, 2);
  assert.equal(launching.size, 2);
  assert.equal(secondPass.runnablePlans.length, 0);
  assert.equal(secondPass.plannerResult.activePlans.length, 2);
  assert.deepEqual(
    secondPass.plannerResult.activePlans.map((plan) => plan.requestedWindow).sort((left, right) => left - right),
    [1, 7]
  );
});

test("staggered metadata readiness documents small-first launch behavior", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    { path: "/tmp/2.7gb.bin" },
    readyInput("147mb.bin", ONE_HUNDRED_FORTY_SEVEN_MB_BYTES, "/tmp/147mb.bin", 2)
  ]);
  const [large, small] = queuedItems(state);

  const smallOnlyPass = planRunnableTransferLaunches(state, activeRooms);

  assert.deepEqual(smallOnlyPass.runnablePlans.map((plan) => plan.itemId), [small.id]);
  assert.equal(smallOnlyPass.runnablePlans[0].requestedWindow, 8);
  assert.equal(smallOnlyPass.plannerResult.heldPlans.find((plan) => plan.taskId === large.id)?.reason, "missing_metadata");

  state = markQueueItemPreparing(state, small.id, 8);
  state = markQueueItemSending(state, small.id, {
    displayName: "147mb.bin",
    sizeBytes: ONE_HUNDRED_FORTY_SEVEN_MB_BYTES,
    modifiedMs: 2,
    dedupeKey: "147mb"
  });
  state = markQueueItemMetadataReady(state, large.id, {
    displayName: "2.7gb.bin",
    mimeType: "application/octet-stream",
    sizeBytes: TWO_POINT_SEVEN_GB_BYTES,
    modifiedMs: 1,
    dedupeKey: "2.7gb"
  });

  const largeReadyWhileSmallActive = planRunnableTransferLaunches(state, activeRooms);

  assert.equal(largeReadyWhileSmallActive.runnablePlans.length, 0);
  assert.equal(largeReadyWhileSmallActive.plannerResult.activePlans[0].taskId, small.id);
  assert.equal(largeReadyWhileSmallActive.plannerResult.activePlans[0].requestedWindow, 8);
  assert.equal(
    largeReadyWhileSmallActive.plannerResult.heldPlans.find((plan) => plan.taskId === large.id)?.reason,
    "global_budget_exhausted"
  );

  state = markQueueItemCompleted(state, small.id);
  const afterSmallCompletes = planRunnableTransferLaunches(state, activeRooms);

  assert.deepEqual(afterSmallCompletes.runnablePlans.map((plan) => plan.itemId), [large.id]);
  assert.equal(afterSmallCompletes.runnablePlans[0].requestedWindow, 8);
});

test("many-tiny queue produces one serial micro group launch plan", () => {
  const state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    Array.from({ length: 16 }, (_, index) => readyInput(`small-${index}.bin`, 128 * 1024, `/tmp/small-${index}.bin`, index))
  );

  const { runnablePlans, microGroupPlans, plannerResult } = planRunnableTransferLaunches(state, activeRooms);

  assert.equal(runnablePlans.length, 0);
  assert.equal(microGroupPlans.length, 1);
  assert.equal(microGroupPlans[0].dispatchMode, "serial");
  assert.equal(microGroupPlans[0].requestedWindow, 1);
  assert.equal(microGroupPlans[0].childItemIds.length, 16);
  assert.equal(plannerResult.requestedWindowTotal, 1);
});

test("single eligible tiny queue item explains no micro group", () => {
  const state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    [readyInput("tiny-one.bin", 350 * 1024, "/tmp/tiny-one.bin", 1)]
  );

  const { runnablePlans, microGroupPlans } = planRunnableTransferLaunches(state, activeRooms);
  const diagnostics = summarizeMicroFlowGroupPlanning(state, activeRooms);

  assert.equal(microGroupPlans.length, 0);
  assert.equal(runnablePlans.length, 1);
  assert.equal(diagnostics.tinyCandidates, 1);
  assert.equal(diagnostics.eligibleTinyCandidates, 1);
  assert.equal(diagnostics.largestEligibleBucket, 1);
  assert.equal(diagnostics.microGroupSkipReason, "no_contention");
});

test("dynamic micro group diagnostics clamp huge-file quantum conservatively", () => {
  const state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    [
      readyInput("huge.bin", 2 * GiB, "/tmp/huge.bin", 1),
      readyInput("small-over-a.bin", 1.1 * MiB, "/tmp/small-over-a.bin", 2),
      readyInput("small-over-b.bin", 1.2 * MiB, "/tmp/small-over-b.bin", 3),
      readyInput("small-over-c.bin", 1.3 * MiB, "/tmp/small-over-c.bin", 4),
      readyInput("single-sub-mib.bin", 350 * 1024, "/tmp/single-sub-mib.bin", 5)
    ]
  );

  const diagnostics = summarizeMicroFlowGroupPlanning(state, activeRooms);

  assert.equal(diagnostics.contention, true);
  assert.equal(diagnostics.oneWindowQuantumBytes, 16 * MiB);
  assert.equal(diagnostics.dynamicChildCapBytes, 4 * MiB);
  assert.equal(diagnostics.dynamicGroupCapBytes, 16 * MiB);
});

test("no-contention skip reason takes precedence over child-size limit", () => {
  const state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    [
      readyInput("small-over-a.bin", 1.1 * MiB, "/tmp/small-over-a.bin", 1),
      readyInput("small-over-b.bin", 1.2 * MiB, "/tmp/small-over-b.bin", 2)
    ]
  );

  const diagnostics = summarizeMicroFlowGroupPlanning(state, activeRooms);
  const dynamicDiagnostics = summarizeMicroFlowGroupPlanning(state, activeRooms, new Set(), {
    ...DEFAULT_TRANSFER_PLANNER_POLICY,
    microGroupDispatchMode: "shadow",
    microGroupMaxChildSizeBytes: diagnostics.dynamicChildCapBytes,
    microGroupMaxGroupBytes: diagnostics.dynamicGroupCapBytes
  });

  assert.equal(diagnostics.contention, false);
  assert.equal(dynamicDiagnostics.overChildSizeLimit, 2);
  assert.equal(dynamicDiagnostics.microGroupSkipReason, "no_contention");
});

test("twenty sub-one-megabyte queue items group and suppress child runnable plans", () => {
  const state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    Array.from({ length: 20 }, (_, index) => (
      readyInput(`sub-mib-${index}.bin`, (100 + index * 30) * 1024, `/tmp/sub-mib-${index}.bin`, index)
    ))
  );

  const { runnablePlans, microGroupPlans, plannerResult } = planRunnableTransferLaunches(state, activeRooms);
  const groupedChildIds = new Set(microGroupPlans.flatMap((plan) => plan.childItemIds));
  const diagnostics = summarizeMicroFlowGroupPlanning(state, activeRooms);

  assert.ok(microGroupPlans.length >= 1);
  assert.ok(microGroupPlans.every((plan) => plan.requestedWindow === 1));
  assert.equal(runnablePlans.filter((plan) => groupedChildIds.has(plan.itemId)).length, 0);
  assert.equal(plannerResult.requestedWindowTotal, microGroupPlans.length);
  assert.equal(diagnostics.eligibleTinyCandidates, 20);
  assert.ok(diagnostics.largestEligibleBucket >= 2);
});

test("huge plus many tiny queue gives huge runnable window seven and micro group window one", () => {
  const state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    [
      readyInput("huge.bin", 2 * GiB, "/tmp/huge.bin", 1),
      ...Array.from({ length: 16 }, (_, index) => (
        readyInput(`tiny-${index}.bin`, 128 * 1024, `/tmp/tiny-${index}.bin`, index + 2)
      ))
    ]
  );

  const { runnablePlans, microGroupPlans, plannerResult } = planRunnableTransferLaunches(state, activeRooms);
  const hugePlan = runnablePlans.find((plan) => state.items[plan.itemId].displayName === "huge.bin");

  assert.equal(runnablePlans.length, 1);
  assert.equal(hugePlan?.requestedWindow, 7);
  assert.equal(microGroupPlans.length, 1);
  assert.equal(microGroupPlans[0].requestedWindow, 1);
  assert.equal(microGroupPlans[0].childItemIds.length, 16);
  assert.equal(plannerResult.requestedWindowTotal, 8);
});

test("micro group terminal state completes when all children complete", () => {
  let state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    Array.from({ length: 3 }, (_, index) => readyInput(`tiny-${index}.bin`, 128 * 1024, `/tmp/tiny-${index}.bin`, index))
  );
  const groupPlan = planRunnableTransferLaunches(state, activeRooms).microGroupPlans[0];

  state = markMicroFlowGroupQueued(state, groupPlan);
  state = markMicroFlowGroupRunning(state, groupPlan.groupId);
  for (const childItemId of groupPlan.childItemIds) {
    state = recordMicroFlowGroupChildTerminal(state, groupPlan.groupId, childItemId, "completed");
  }
  state = completeMicroFlowGroupFromChildren(state, groupPlan.groupId);

  const group = state.microGroups[groupPlan.groupId];
  assert.equal(group.status, "completed");
  assert.equal(group.terminalReason, "all_children_completed");
  assert.deepEqual(group.completedChildIds, groupPlan.childItemIds);
});

test("micro group terminal state records completed_with_errors when one child fails", () => {
  let state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    Array.from({ length: 3 }, (_, index) => readyInput(`tiny-${index}.bin`, 128 * 1024, `/tmp/tiny-${index}.bin`, index))
  );
  const groupPlan = planRunnableTransferLaunches(state, activeRooms).microGroupPlans[0];
  const [first, second, third] = groupPlan.childItemIds;

  state = markMicroFlowGroupQueued(state, groupPlan);
  state = markMicroFlowGroupRunning(state, groupPlan.groupId);
  state = recordMicroFlowGroupChildTerminal(state, groupPlan.groupId, first, "completed");
  state = recordMicroFlowGroupChildTerminal(state, groupPlan.groupId, second, "failed");
  state = recordMicroFlowGroupChildTerminal(state, groupPlan.groupId, third, "completed");
  state = completeMicroFlowGroupFromChildren(state, groupPlan.groupId);

  const group = state.microGroups[groupPlan.groupId];
  assert.equal(group.status, "completed_with_errors");
  assert.equal(group.terminalReason, "one_or_more_children_failed_or_cancelled");
  assert.deepEqual(group.failedChildIds, [second]);
});

test("micro group terminal state is cancelled by batch cancellation", () => {
  let state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    Array.from({ length: 3 }, (_, index) => readyInput(`tiny-${index}.bin`, 128 * 1024, `/tmp/tiny-${index}.bin`, index))
  );
  const groupPlan = planRunnableTransferLaunches(state, activeRooms).microGroupPlans[0];
  const firstChild = state.items[groupPlan.childItemIds[0]];

  state = markMicroFlowGroupQueued(state, groupPlan);
  state = markMicroFlowGroupRunning(state, groupPlan.groupId);
  state = cancelBatchLocally(state, firstChild.batchId);

  const group = state.microGroups[groupPlan.groupId];
  assert.equal(group.status, "cancelled");
  assert.equal(group.terminalReason, "batch_cancelled");
});

test("micro group terminal state is interrupted when room work is cleared", () => {
  let state = enqueueTransferBatch(
    createTransferSchedulerState(),
    "room-1",
    Array.from({ length: 3 }, (_, index) => readyInput(`tiny-${index}.bin`, 128 * 1024, `/tmp/tiny-${index}.bin`, index))
  );
  const groupPlan = planRunnableTransferLaunches(state, activeRooms).microGroupPlans[0];

  state = markMicroFlowGroupQueued(state, groupPlan);
  state = markMicroFlowGroupRunning(state, groupPlan.groupId);
  state = clearQueuedItemsForRoom(state, "room-1");

  const group = state.microGroups[groupPlan.groupId];
  assert.equal(group.status, "interrupted");
  assert.equal(group.terminalReason, "room_cleared_or_burned");
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
  assert.equal(state.items[sendingA.id].status, "cancelled");
  assert.equal(state.items[sendingA.id].cancelRequested, true);
  assert.equal(state.items[sendingB.id].status, "cancelled");
  assert.equal(state.items[sendingB.id].cancelRequested, true);
  assert.deepEqual(activeCancellableTransferIds(state), []);
  assert.equal(planRunnableTransferLaunches(state, [{ id: "room-1", status: "burned" }]).runnablePlans.length, 0);
});

test("burned room queue cleanup does not block same file in a new room", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "old-room", [
    readyInput("model.bin", 2 * GiB, "/tmp/model.bin", 1)
  ]);
  const oldItem = queuedItems(state)[0];
  state = markQueueItemSending(state, oldItem.id, {
    displayName: "model.bin",
    sizeBytes: 2 * GiB,
    modifiedMs: 1,
    dedupeKey: "model"
  });
  state = correlateTransferProgress(state, {
    roomId: "old-room",
    queueItemId: oldItem.id,
    direction: "outgoing",
    fileName: "model.bin",
    fileSize: 2 * GiB,
    transferId: "old-transfer",
    status: "transferring"
  });

  state = clearQueuedItemsForRoom(state, "old-room");
  state = enqueueTransferBatch(state, "new-room", [
    readyInput("model.bin", 2 * GiB, "/tmp/model.bin", 1)
  ]);

  const newItem = Object.values(state.items).find((item) => item.roomId === "new-room");
  assert.ok(newItem);
  assert.equal(newItem.status, "queued");
  assert.equal(newItem.metadataStatus, "ready");

  const { runnablePlans, plannerResult } = planRunnableTransferLaunches(
    state,
    [
      { id: "old-room", status: "burned" },
      { id: "new-room", status: "active" }
    ],
    new Set(["old-room"])
  );
  assert.deepEqual(runnablePlans.map((plan) => plan.itemId), [newItem.id]);
  assert.equal(plannerResult.activePlans.length, 0);
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

test("completion-only rebalance uses post-terminal state immediately after small transfer completes", () => {
  let state = enqueueTransferBatch(createTransferSchedulerState(), "room-1", [
    readyInput("2.7gb.bin", TWO_POINT_SEVEN_GB_BYTES, "/tmp/2.7gb.bin", 1),
    readyInput("147mb.bin", ONE_HUNDRED_FORTY_SEVEN_MB_BYTES, "/tmp/147mb.bin", 2)
  ]);
  const [huge, small] = queuedItems(state);

  state = markQueueItemPreparing(state, huge.id, 7);
  state = markQueueItemSending(state, huge.id, {
    displayName: "2.7gb.bin",
    sizeBytes: TWO_POINT_SEVEN_GB_BYTES,
    modifiedMs: 1,
    dedupeKey: "huge"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: huge.id,
    direction: "outgoing",
    fileName: "2.7gb.bin",
    fileSize: TWO_POINT_SEVEN_GB_BYTES,
    transferId: "transfer-huge",
    status: "transferring"
  });
  state = markQueueItemPreparing(state, small.id, 1);
  state = markQueueItemSending(state, small.id, {
    displayName: "147mb.bin",
    sizeBytes: ONE_HUNDRED_FORTY_SEVEN_MB_BYTES,
    modifiedMs: 2,
    dedupeKey: "small"
  });
  state = correlateTransferProgress(state, {
    roomId: "room-1",
    queueItemId: small.id,
    direction: "outgoing",
    fileName: "147mb.bin",
    fileSize: ONE_HUNDRED_FORTY_SEVEN_MB_BYTES,
    transferId: "transfer-small",
    status: "transferring"
  });

  assert.deepEqual(planActiveTransferWindowRebalances(state, activeRooms), []);

  const postTerminalState = markQueueItemCompleted(state, small.id);
  const plans = planActiveTransferWindowRebalances(postTerminalState, activeRooms);

  assert.deepEqual(plans, [{
    itemId: huge.id,
    transferId: "transfer-huge",
    requestedWindow: 8,
    previousWindow: 7
  }]);
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

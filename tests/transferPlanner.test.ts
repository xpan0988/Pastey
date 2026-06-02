import assert from "node:assert/strict";
import test from "node:test";

import "./transferSchedulerExecution.test";

import {
  planWeightedTransfers,
  type TransferPlannerTask
} from "../src/lib/transferPlanner";

const MiB = 1024 * 1024;
const GiB = 1024 * MiB;
const TWO_POINT_SEVEN_GB_BYTES = 2707513952;
const ONE_HUNDRED_FORTY_SEVEN_MB_BYTES = 147642115;

function fileTask(overrides: Partial<TransferPlannerTask> & Pick<TransferPlannerTask, "id">): TransferPlannerTask {
  return {
    id: overrides.id,
    roomId: overrides.roomId ?? "room-1",
    kind: overrides.kind ?? "file",
    state: overrides.state ?? "queued",
    metadataStatus: overrides.metadataStatus ?? "ready",
    sizeBytes: overrides.sizeBytes ?? 10 * MiB,
    priority: overrides.priority,
    latencySensitive: overrides.latencySensitive,
    throughputSensitive: overrides.throughputSensitive,
    roomStatus: overrides.roomStatus ?? "active",
    roomAvailable: overrides.roomAvailable,
    cancelRequested: overrides.cancelRequested,
    requestedWindow: overrides.requestedWindow,
    activeRequestedWindow: overrides.activeRequestedWindow,
    mimeType: overrides.mimeType,
    createdAt: overrides.createdAt
  };
}

test("huge only creates one bulk runnable plan with window 8", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "huge", sizeBytes: 2 * GiB })
  ]);

  assert.equal(result.runnablePlans.length, 1);
  assert.equal(result.runnablePlans[0].lane, "bulk_file");
  assert.equal(result.runnablePlans[0].requestedWindow, 8);
  assert.equal(result.requestedWindowTotal, 8);
});

test("huge plus small gives bulk about 7 and small 1", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "huge", sizeBytes: 2 * GiB, createdAt: 1 }),
    fileTask({ id: "small", sizeBytes: 1 * MiB, createdAt: 2 })
  ]);

  const byId = new Map(result.runnablePlans.map((plan) => [plan.taskId, plan]));
  assert.equal(byId.get("huge")?.requestedWindow, 7);
  assert.equal(byId.get("small")?.requestedWindow, 1);
  assert.equal(result.requestedWindowTotal, 8);
});

test("huge plus tiny keeps the tiny transfer bounded at window 1", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "huge", sizeBytes: 4 * GiB, createdAt: 1 }),
    fileTask({ id: "tiny", sizeBytes: 4 * 1024, createdAt: 2 })
  ]);

  const byId = new Map(result.runnablePlans.map((plan) => [plan.taskId, plan]));
  assert.equal(byId.get("huge")?.requestedWindow, 7);
  assert.equal(byId.get("tiny")?.requestedWindow, 1);
  assert.equal(result.requestedWindowTotal, 8);
});

test("2.7GB plus 147MB uses batch-relative windows around 7 and 1", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "2.7gb", sizeBytes: TWO_POINT_SEVEN_GB_BYTES, createdAt: 1 }),
    fileTask({ id: "147mb", sizeBytes: ONE_HUNDRED_FORTY_SEVEN_MB_BYTES, createdAt: 2 })
  ]);

  const byId = new Map(result.runnablePlans.map((plan) => [plan.taskId, plan]));
  assert.equal(byId.get("2.7gb")?.requestedWindow, 7);
  assert.equal(byId.get("147mb")?.requestedWindow, 1);
  assert.equal(result.requestedWindowTotal, 8);
});

test("similarly large files split the global window fairly", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "2.7gb", sizeBytes: TWO_POINT_SEVEN_GB_BYTES, createdAt: 1 }),
    fileTask({ id: "2.5gb", sizeBytes: 2.5 * GiB, createdAt: 2 })
  ]);

  assert.deepEqual(
    result.runnablePlans.map((plan) => plan.requestedWindow).sort((left, right) => left - right),
    [4, 4]
  );
  assert.equal(result.requestedWindowTotal, 8);
});

test("three similarly large files split fairly within the global budget", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "large-a", sizeBytes: 2.7 * GiB, createdAt: 1 }),
    fileTask({ id: "large-b", sizeBytes: 2.6 * GiB, createdAt: 2 }),
    fileTask({ id: "large-c", sizeBytes: 2.5 * GiB, createdAt: 3 })
  ]);

  assert.equal(result.runnablePlans.length, 3);
  assert.deepEqual(
    result.runnablePlans.map((plan) => plan.requestedWindow).sort((left, right) => left - right),
    [2, 3, 3]
  );
  assert.equal(result.requestedWindowTotal, 8);
});

test("many tiny files create one serial micro group with one planner window", () => {
  const result = planWeightedTransfers(Array.from({ length: 12 }, (_, index) => (
    fileTask({ id: `small-${index}`, sizeBytes: 128 * 1024, createdAt: index })
  )));

  assert.equal(result.runnablePlans.length, 1);
  assert.equal(result.runnablePlans[0].kind, "micro_group");
  assert.equal(result.runnablePlans[0].requestedWindow, 1);
  assert.equal(result.microGroupPlans.length, 1);
  assert.equal(result.microGroupPlans[0].dispatchMode, "serial");
  assert.equal(result.microGroupPlans[0].requestedWindow, 1);
  assert.deepEqual(result.microGroupPlans[0].childTaskIds, Array.from({ length: 12 }, (_, index) => `small-${index}`));
  assert.equal(result.requestedWindowTotal, 1);
});

test("shadow micro group reports possible grouping without changing runnable child plans", () => {
  const result = planWeightedTransfers(
    Array.from({ length: 6 }, (_, index) => (
      fileTask({ id: `tiny-${index}`, sizeBytes: 128 * 1024, createdAt: index })
    )),
    { microGroupDispatchMode: "shadow" }
  );

  assert.equal(result.microGroupPlans.length, 1);
  assert.equal(result.microGroupPlans[0].dispatchMode, "shadow");
  assert.equal(result.microGroupPlans[0].requestedWindow, 1);
  assert.deepEqual(result.runnablePlans.map((plan) => plan.taskId), ["tiny-0", "tiny-1", "tiny-2", "tiny-3"]);
  assert.deepEqual(result.runnablePlans.map((plan) => plan.requestedWindow), [2, 2, 2, 2]);
  assert.equal(result.requestedWindowTotal, 8);
});

test("huge plus many tiny gives the huge seven windows and the micro group one", () => {
  const tinyTasks = Array.from({ length: 16 }, (_, index) => (
    fileTask({ id: `tiny-${index}`, sizeBytes: 128 * 1024, createdAt: index + 2 })
  ));
  const result = planWeightedTransfers([
    fileTask({ id: "huge", sizeBytes: 2 * GiB, createdAt: 1 }),
    ...tinyTasks
  ]);

  assert.equal(result.microGroupPlans.length, 1);
  assert.equal(result.microGroupPlans[0].requestedWindow, 1);
  assert.equal(result.microGroupPlans[0].childTaskIds.length, 16);

  const byId = new Map(result.runnablePlans.map((plan) => [plan.taskId, plan]));
  assert.equal(byId.get("huge")?.requestedWindow, 7);
  assert.equal(byId.get(result.microGroupPlans[0].groupId)?.kind, "micro_group");
  assert.equal(byId.get(result.microGroupPlans[0].groupId)?.requestedWindow, 1);
  assert.equal(result.requestedWindowTotal, 8);
});

test("safety cap prevents many tiny files from creating unbounded runnable transfers", () => {
  const result = planWeightedTransfers(Array.from({ length: 40 }, (_, index) => (
    fileTask({ id: `tiny-${index}`, sizeBytes: 4 * 1024, createdAt: index })
  )));

  assert.ok(result.runnablePlans.length <= 4);
  assert.equal(result.requestedWindowTotal, 2);
  assert.equal(result.microGroupPlans.length, 2);
  assert.ok(result.microGroupPlans.every((plan) => plan.requestedWindow === 1));
});

test("burned room does not produce runnable plans", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "burned", roomStatus: "burned" })
  ]);

  assert.equal(result.runnablePlans.length, 0);
  assert.equal(result.heldPlans[0].reason, "room_burned");
});

test("cancelled item does not produce runnable plans", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "cancelled", state: "cancelled" }),
    fileTask({ id: "cancel-requested", cancelRequested: true })
  ]);

  assert.equal(result.runnablePlans.length, 0);
  assert.deepEqual(result.heldPlans.map((plan) => plan.reason), ["cancelled", "cancelled"]);
});

test("missing metadata is held with reason", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "missing", metadataStatus: "unknown", sizeBytes: null })
  ]);

  assert.equal(result.runnablePlans.length, 0);
  assert.equal(result.heldPlans[0].reason, "missing_metadata");
});

test("active transfers reserve existing budget so runnable plans cannot overrun global budget", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "active-huge", state: "active", sizeBytes: 2 * GiB, activeRequestedWindow: 6 }),
    fileTask({ id: "queued-huge", sizeBytes: 2 * GiB })
  ]);

  assert.equal(result.activePlans.length, 1);
  assert.equal(result.activePlans[0].requestedWindow, 6);
  assert.equal(result.runnablePlans.length, 1);
  assert.equal(result.runnablePlans[0].requestedWindow, 2);
  assert.equal(result.requestedWindowTotal, 8);
});

test("active transfer keeps its requested window when another lane appears", () => {
  const result = planWeightedTransfers([
    fileTask({ id: "active-huge", state: "active", sizeBytes: 2 * GiB, activeRequestedWindow: 8 }),
    fileTask({ id: "queued-small", sizeBytes: 1 * MiB })
  ]);

  assert.equal(result.activePlans.length, 1);
  assert.equal(result.activePlans[0].requestedWindow, 8);
  assert.equal(result.runnablePlans.length, 0);
  assert.equal(result.requestedWindowTotal, 8);
});

test("total requested windows never exceed global budget", () => {
  const scenarios: TransferPlannerTask[][] = [
    [fileTask({ id: "huge", sizeBytes: 2 * GiB })],
    [
      fileTask({ id: "huge", sizeBytes: 2 * GiB }),
      fileTask({ id: "small", sizeBytes: 1 * MiB })
    ],
    Array.from({ length: 20 }, (_, index) => fileTask({ id: `small-${index}`, sizeBytes: 32 * 1024 })),
    [
      fileTask({ id: "active", state: "active", sizeBytes: 2 * GiB, activeRequestedWindow: 7 }),
      fileTask({ id: "queued", sizeBytes: 1 * MiB })
    ]
  ];

  for (const scenario of scenarios) {
    const result = planWeightedTransfers(scenario);
    assert.ok(result.requestedWindowTotal <= result.globalWindowBudget);
    for (const plan of [...result.activePlans, ...result.runnablePlans]) {
      assert.ok(plan.requestedWindow >= 1);
    }
  }
});

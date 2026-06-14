import assert from "node:assert/strict";

import {
  CONTROL_WINDOW_RESTORE_QUIET_PERIOD_MS,
  createRuntimeDataWindowTargetState,
  reduceRuntimeDataWindowTarget,
  type RuntimeDataWindowTargetState,
} from "../../src/lib/agentBridge/controlWindowRuntime";
import {
  DEFAULT_TRANSFER_PLANNER_POLICY,
  planWeightedTransfers,
  type TransferPlannerTask,
} from "../../src/lib/transferPlanner";

export interface Cl4ActiveAllocation {
  transferId: string;
  requestedWindow: number;
  runtimeWindow: number;
  bytesSent: number;
  status: "transferring" | "completed";
}

export interface Cl4TelemetryRecord {
  timestampMs: number;
  dataWindowTarget: number;
  activeAllocations: Cl4ActiveAllocation[];
  outgoingControlDemand: boolean;
  controlEventId?: string;
  controlTransportStatus?: string;
}

export interface Cl4ScenarioReport {
  name: string;
  passed: boolean;
  targetTransitions: number[];
  allocationTransitions: number[][];
  transferIds: string[];
  monotonicProgress: boolean;
  noRestartOrCancel: boolean;
  controlDeliveryResult: string;
  restoreDelayMs?: number;
  checksumResult?: "ok" | "not_applicable";
  invariantViolations: string[];
  telemetry: Cl4TelemetryRecord[];
}

export interface Cl4ContentionReport {
  schemaVersion: "pastey-cl4-contention-report/v1";
  integrationBoundary: string;
  generatedAt: string;
  runtimeEvidence?: Record<string, unknown>;
  semanticIsolation: {
    ordinaryRoomItemsCreated: false;
    transferItemsCreatedByControl: false;
    microFlowGroupControlItems: false;
    executionOccurred: false;
    executionResultFieldsObserved: false;
  };
  scenarios: Cl4ScenarioReport[];
}

const GiB = 1024 * 1024 * 1024;

function activeTask(id: string, activeRequestedWindow: number, createdAt: number): TransferPlannerTask {
  return {
    id,
    roomId: "room-test",
    kind: "file",
    state: "active",
    metadataStatus: "ready",
    sizeBytes: 2 * GiB,
    priority: "normal",
    throughputSensitive: true,
    roomStatus: "active",
    activeRequestedWindow,
    createdAt,
  };
}

export function allocateActiveWindows(
  transferIds: readonly string[],
  currentWindows: readonly number[],
  targetDataWindows: 7 | 8,
): number[] {
  const result = planWeightedTransfers(
    transferIds.map((transferId, index) =>
      activeTask(transferId, currentWindows[index] ?? 1, index)
    ),
    {
      ...DEFAULT_TRANSFER_PLANNER_POLICY,
      globalWindowBudget: targetDataWindows,
      maxRequestedWindow: targetDataWindows,
      rebalanceActiveWindows: true,
    },
  );
  const byId = new Map(result.activePlans.map((plan) => [plan.taskId, plan.requestedWindow]));
  return transferIds.map((transferId) => byId.get(transferId) ?? 0);
}

export function assertCl4Telemetry(records: readonly Cl4TelemetryRecord[]): string[] {
  const violations: string[] = [];
  const initialIds = records[0]?.activeAllocations.map((item) => item.transferId) ?? [];
  const lastBytes = new Map<string, number>();

  for (const record of records) {
    const total = record.activeAllocations.reduce((sum, item) => sum + item.runtimeWindow, 0);
    if (total > record.dataWindowTarget) {
      violations.push(`allocation total ${total} exceeded target ${record.dataWindowTarget}`);
    }
    const ids = record.activeAllocations.map((item) => item.transferId);
    if (ids.length === initialIds.length && ids.some((id, index) => id !== initialIds[index])) {
      violations.push("active transfer IDs changed");
    }
    for (const item of record.activeAllocations) {
      if (item.runtimeWindow === 7 && record.activeAllocations.length > 1) {
        violations.push(`${item.transferId} independently received window 7`);
      }
      const previous = lastBytes.get(item.transferId) ?? 0;
      if (item.bytesSent < previous) {
        violations.push(`${item.transferId} progress decreased`);
      }
      lastBytes.set(item.transferId, item.bytesSent);
    }
  }
  return [...new Set(violations)];
}

function telemetry(
  timestampMs: number,
  targetState: RuntimeDataWindowTargetState,
  transferIds: readonly string[],
  windows: readonly number[],
  bytes: readonly number[],
  controlEventId?: string,
  controlTransportStatus?: string,
): Cl4TelemetryRecord {
  return {
    timestampMs,
    dataWindowTarget: targetState.targetDataWindows,
    activeAllocations: transferIds.map((transferId, index) => ({
      transferId,
      requestedWindow: windows[index] ?? 0,
      runtimeWindow: windows[index] ?? 0,
      bytesSent: bytes[index] ?? 0,
      status: "transferring",
    })),
    outgoingControlDemand: targetState.outgoingControlDemand,
    controlEventId,
    controlTransportStatus,
  };
}

function finalizeScenario(
  name: string,
  records: Cl4TelemetryRecord[],
  controlDeliveryResult: string,
  restoreDelayMs?: number,
): Cl4ScenarioReport {
  const invariantViolations = assertCl4Telemetry(records);
  return {
    name,
    passed: invariantViolations.length === 0,
    targetTransitions: records
      .map((record) => record.dataWindowTarget)
      .filter((target, index, values) => index === 0 || target !== values[index - 1]),
    allocationTransitions: records
      .map((record) => record.activeAllocations.map((item) => item.runtimeWindow))
      .filter((allocation, index, values) =>
        index === 0 || allocation.join(",") !== values[index - 1].join(",")
      ),
    transferIds: records[0]?.activeAllocations.map((item) => item.transferId) ?? [],
    monotonicProgress: !invariantViolations.some((item) => item.includes("progress decreased")),
    noRestartOrCancel: !invariantViolations.some((item) => item.includes("transfer IDs changed")),
    controlDeliveryResult,
    restoreDelayMs,
    checksumResult: "not_applicable",
    invariantViolations,
    telemetry: records,
  };
}

export function runScenarioA(): Cl4ScenarioReport {
  const transferIds = ["transfer-a"];
  let state = createRuntimeDataWindowTargetState();
  let windows = allocateActiveWindows(transferIds, [8], 8);
  const records = [telemetry(0, state, transferIds, windows, [0])];

  state = reduceRuntimeDataWindowTarget(state, {
    type: "demand_changed",
    outgoingControlDemand: true,
    nowMs: 100,
  });
  windows = allocateActiveWindows(transferIds, windows, 7);
  records.push(telemetry(100, state, transferIds, windows, [1_024], "control-a", "dispatching"));

  state = reduceRuntimeDataWindowTarget(state, {
    type: "demand_changed",
    outgoingControlDemand: false,
    nowMs: 200,
  });
  records.push(telemetry(200, state, transferIds, windows, [2_048], "control-a", "accepted"));

  state = reduceRuntimeDataWindowTarget(state, {
    type: "restore_quiet_period_elapsed",
    nowMs: 200 + CONTROL_WINDOW_RESTORE_QUIET_PERIOD_MS,
  });
  windows = allocateActiveWindows(transferIds, windows, 8);
  records.push(telemetry(950, state, transferIds, windows, [3_072], "control-a", "accepted"));

  const report = finalizeScenario("Scenario A - one active transfer", records, "accepted", 750);
  assert.deepEqual(report.targetTransitions, [8, 7, 8]);
  assert.deepEqual(report.allocationTransitions, [[8], [7], [8]]);
  return report;
}

export function runScenarioB(): Cl4ScenarioReport {
  const transferIds = ["transfer-a", "transfer-b"];
  let state = createRuntimeDataWindowTargetState();
  let windows = allocateActiveWindows(transferIds, [4, 4], 8);
  const records = [telemetry(0, state, transferIds, windows, [0, 0])];

  state = reduceRuntimeDataWindowTarget(state, {
    type: "demand_changed",
    outgoingControlDemand: true,
    nowMs: 100,
  });
  windows = allocateActiveWindows(transferIds, windows, 7);
  records.push(telemetry(100, state, transferIds, windows, [1_024, 768], "control-b", "dispatching"));

  state = reduceRuntimeDataWindowTarget(state, {
    type: "demand_changed",
    outgoingControlDemand: false,
    nowMs: 200,
  });
  state = reduceRuntimeDataWindowTarget(state, {
    type: "restore_quiet_period_elapsed",
    nowMs: 950,
  });
  windows = allocateActiveWindows(transferIds, windows, 8);
  records.push(telemetry(950, state, transferIds, windows, [2_048, 1_792], "control-b", "accepted"));

  return finalizeScenario("Scenario B - multiple active transfers", records, "accepted", 750);
}

export function runBurstScenario(): Cl4ScenarioReport {
  const transferIds = ["transfer-burst"];
  let state = createRuntimeDataWindowTargetState();
  let windows = allocateActiveWindows(transferIds, [8], 8);
  const records = [telemetry(0, state, transferIds, windows, [0])];

  for (const [time, demand, status] of [
    [100, true, "dispatching"],
    [200, false, "accepted"],
    [500, true, "dispatching"],
    [600, false, "accepted"],
  ] as const) {
    state = reduceRuntimeDataWindowTarget(state, {
      type: "demand_changed",
      outgoingControlDemand: demand,
      nowMs: time,
    });
    windows = allocateActiveWindows(transferIds, windows, state.targetDataWindows);
    records.push(telemetry(time, state, transferIds, windows, [time * 4], `control-${time}`, status));
  }
  state = reduceRuntimeDataWindowTarget(state, {
    type: "restore_quiet_period_elapsed",
    nowMs: 1_200,
  });
  records.push(telemetry(1_200, state, transferIds, windows, [4_800]));
  state = reduceRuntimeDataWindowTarget(state, {
    type: "restore_quiet_period_elapsed",
    nowMs: 1_350,
  });
  windows = allocateActiveWindows(transferIds, windows, 8);
  records.push(telemetry(1_350, state, transferIds, windows, [5_400]));

  const report = finalizeScenario("Burst and hysteresis", records, "accepted", 750);
  assert.deepEqual(report.targetTransitions, [8, 7, 8]);
  assert.ok(report.allocationTransitions.length <= 3);
  return report;
}

export function runDirectionalityScenario(): Cl4ScenarioReport {
  const transferIds = ["transfer-direction"];
  let state = createRuntimeDataWindowTargetState();
  let windows = allocateActiveWindows(transferIds, [8], 8);
  const records = [
    telemetry(0, state, transferIds, windows, [0], "inbound-preview", "inbound_review_only"),
  ];
  records.push(telemetry(100, state, transferIds, windows, [1_024], "inbound-preview", "selected_for_review"));

  state = reduceRuntimeDataWindowTarget(state, {
    type: "demand_changed",
    outgoingControlDemand: true,
    nowMs: 200,
  });
  windows = allocateActiveWindows(transferIds, windows, 7);
  records.push(telemetry(200, state, transferIds, windows, [2_048], "outbound-ack", "dispatching"));

  const report = finalizeScenario("Inbound-only directionality", records, "outbound ack dispatching");
  assert.equal(records[1].dataWindowTarget, 8);
  assert.equal(records[1].outgoingControlDemand, false);
  return report;
}

export function runFailureReleaseScenarios(): Cl4ScenarioReport[] {
  return ["replay_rejected", "expired", "peer_unavailable", "validation_rejected"].map((failure, index) => {
    const transferIds = [`transfer-failure-${index}`];
    let state = createRuntimeDataWindowTargetState();
    let windows = allocateActiveWindows(transferIds, [8], 8);
    const records = [telemetry(0, state, transferIds, windows, [0])];
    state = reduceRuntimeDataWindowTarget(state, {
      type: "demand_changed",
      outgoingControlDemand: true,
      nowMs: 100,
    });
    windows = allocateActiveWindows(transferIds, windows, 7);
    records.push(telemetry(100, state, transferIds, windows, [512], `control-${failure}`, "dispatching"));
    state = reduceRuntimeDataWindowTarget(state, {
      type: "demand_changed",
      outgoingControlDemand: false,
      nowMs: 200,
    });
    state = reduceRuntimeDataWindowTarget(state, {
      type: "restore_quiet_period_elapsed",
      nowMs: 950,
    });
    windows = allocateActiveWindows(transferIds, windows, 8);
    records.push(telemetry(950, state, transferIds, windows, [1_024], `control-${failure}`, failure));
    return finalizeScenario(`Failure release - ${failure}`, records, failure, 750);
  });
}

export function runCl4ContentionScenarios(generatedAt = new Date().toISOString()): Cl4ContentionReport {
  const scenarios = [
    runScenarioA(),
    runScenarioB(),
    runBurstScenario(),
    runDirectionalityScenario(),
    ...runFailureReleaseScenarios(),
  ];
  assert.ok(scenarios.every((scenario) => scenario.passed));
  return {
    schemaVersion: "pastey-cl4-contention-report/v1",
    integrationBoundary:
      "Production TypeScript CL-4 demand/target reducer and transfer planner; focused Rust update_active_transfer_window evidence runs separately.",
    generatedAt,
    semanticIsolation: {
      ordinaryRoomItemsCreated: false,
      transferItemsCreatedByControl: false,
      microFlowGroupControlItems: false,
      executionOccurred: false,
      executionResultFieldsObserved: false,
    },
    scenarios,
  };
}

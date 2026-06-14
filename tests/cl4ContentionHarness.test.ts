import assert from "node:assert/strict";
import test from "node:test";

import {
  allocateActiveWindows,
  assertCl4Telemetry,
  runBurstScenario,
  runCl4ContentionScenarios,
  runDirectionalityScenario,
  runScenarioA,
  runScenarioB,
} from "./helpers/cl4ContentionHarness";

test("single-transfer contention records stable 8 to 7 to 8 evidence", () => {
  const report = runScenarioA();
  assert.equal(report.passed, true);
  assert.deepEqual(report.targetTransitions, [8, 7, 8]);
  assert.deepEqual(report.allocationTransitions, [[8], [7], [8]]);
  assert.equal(report.restoreDelayMs, 750);
});

test("multiple transfers share the combined target", () => {
  const report = runScenarioB();
  assert.equal(report.passed, true);
  assert.ok(report.telemetry.every((record) =>
    record.activeAllocations.reduce((sum, allocation) => sum + allocation.runtimeWindow, 0) <=
      record.dataWindowTarget
  ));
  assert.ok(report.telemetry.every((record) =>
    record.activeAllocations.length === 1 ||
      record.activeAllocations.every((allocation) => allocation.runtimeWindow !== 7)
  ));
});

test("production planner preserves minimum and fairness under seven and eight", () => {
  assert.deepEqual(allocateActiveWindows(["a", "b"], [4, 4], 8).sort(), [4, 4]);
  const underDemand = allocateActiveWindows(["a", "b"], [4, 4], 7);
  assert.equal(underDemand.reduce((sum, value) => sum + value, 0), 7);
  assert.ok(underDemand.every((value) => value >= 1 && value < 7));
});

test("burst demand does not flap during the quiet period", () => {
  const report = runBurstScenario();
  assert.deepEqual(report.targetTransitions, [8, 7, 8]);
  assert.ok(report.allocationTransitions.length <= 3);
});

test("inbound-only review does not reserve the sender-side window", () => {
  const report = runDirectionalityScenario();
  assert.equal(report.telemetry[1].dataWindowTarget, 8);
  assert.equal(report.telemetry[1].outgoingControlDemand, false);
  assert.equal(report.telemetry[2].dataWindowTarget, 7);
});

test("telemetry assertions reject allocation overflow and decreasing progress", () => {
  const violations = assertCl4Telemetry([
    {
      timestampMs: 0,
      dataWindowTarget: 7,
      outgoingControlDemand: true,
      activeAllocations: [
        { transferId: "a", requestedWindow: 4, runtimeWindow: 4, bytesSent: 10, status: "transferring" },
        { transferId: "b", requestedWindow: 4, runtimeWindow: 4, bytesSent: 10, status: "transferring" },
      ],
    },
    {
      timestampMs: 1,
      dataWindowTarget: 7,
      outgoingControlDemand: true,
      activeAllocations: [
        { transferId: "a", requestedWindow: 4, runtimeWindow: 4, bytesSent: 9, status: "transferring" },
        { transferId: "b", requestedWindow: 3, runtimeWindow: 3, bytesSent: 11, status: "transferring" },
      ],
    },
  ]);
  assert.ok(violations.some((item) => item.includes("exceeded target")));
  assert.ok(violations.some((item) => item.includes("progress decreased")));
});

test("all deterministic contention scenarios pass", () => {
  const report = runCl4ContentionScenarios("2026-06-13T00:00:00.000Z");
  assert.equal(report.scenarios.length, 8);
  assert.ok(report.scenarios.every((scenario) => scenario.passed));
  assert.ok(report.scenarios.filter((scenario) => scenario.name.startsWith("Failure release")).every(
    (scenario) => scenario.targetTransitions.at(-1) === 8,
  ));
});

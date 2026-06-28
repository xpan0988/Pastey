import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

test("Layer 4 validation matrix documents required route and authority boundaries", () => {
  const validation = readFileSync("docs/transfer/validation.md", "utf8");
  const routing = readFileSync("docs/architecture/bridge-routing.md", "utf8");

  assert.match(validation, /## Layer 4 Validation Matrix/);
  assert.match(validation, /selected-peer \| ordinary text/);
  assert.match(validation, /selected-peers \| ordinary text/);
  assert.match(validation, /broadcast \| ordinary text/);
  assert.match(validation, /file\/image\/pasted-image/);
  assert.match(validation, /room-control event/);
  assert.match(validation, /Agent Bridge capability preview/);
  assert.match(validation, /Agent Bridge execution request\/result/);
  assert.match(validation, /durable paired identity only/);
  assert.match(validation, /delivery receipt does not create consent/);
  assert.match(validation, /Manual Smoke Checklist \(Pending\)/);

  assert.match(routing, /## Layer 4 Runtime Status/);
  assert.match(routing, /Current-session peer table/);
  assert.match(routing, /Ordinary data selected-peer/);
  assert.match(routing, /Ordinary data selected-peers/);
  assert.match(routing, /Ordinary data broadcast/);
  assert.match(routing, /Room-control selected-peer backend route/);
  assert.match(routing, /Control\/capability fan-out/);
  assert.match(routing, /Full cryptographic paired-key rotation/);
  assert.match(routing, /Two-machine\/release validation/);
});

test("Layer 4 validation runner keeps matrix evidence grouped by invariant", () => {
  const runner = readFileSync("scripts/run-layer4-validation-matrix.mjs", "utf8");

  for (const area of [
    "ordinary-data-routing",
    "queue-children-and-terminal-state",
    "control-capability-selected-peer",
    "consent-and-hello-peer",
    "backend-route-and-durable-boundaries",
  ]) {
    assert.match(runner, new RegExp(area));
  }

  assert.match(runner, /bridgeRoutingRuntime\.test\.ts/);
  assert.match(runner, /transferSchedulerExecution\.test\.ts/);
  assert.match(runner, /room_control::tests::/);
  assert.match(runner, /storage::tests::/);
  assert.match(runner, /bridge_route_payload/);
});

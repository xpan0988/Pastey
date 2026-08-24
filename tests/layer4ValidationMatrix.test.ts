import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

test("Layer 4 validation matrix documents required route and authority boundaries", () => {
  const development = readFileSync("docs/development.md", "utf8");
  const bridge = readFileSync("docs/layers/layer-4-bridge.md", "utf8");

  assert.match(development, /## Transfer and Layer 4 validation/);
  assert.match(development, /run-layer4-validation-matrix/);
  assert.match(development, /single-machine dual-instance smoke/);
  assert.match(development, /two-device smoke/);

  assert.match(bridge, /selected peer/);
  assert.match(bridge, /selected peers/);
  assert.match(bridge, /broadcast to Bridge/);
  assert.match(bridge, /File, image, and pasted-image/);
  assert.match(bridge, /Bridge Plan control messages remain exact selected-peer only/);
  assert.match(bridge, /delivery receipt says only/);
  assert.match(bridge, /display\/recognition metadata only/);
  assert.match(bridge, /Full cryptographic paired-key rotation is not implemented/);
  assert.match(bridge, /Two-device\/package validation remains a required manual\/release check/);
});

test("Layer 4 validation runner keeps matrix evidence grouped by invariant", () => {
  const runner = readFileSync("scripts/run-layer4-validation-matrix.mjs", "utf8");

  for (const area of [
    "ordinary-data-routing",
    "queue-children-and-terminal-state",
    "developer-terminal-emulation",
    "backend-route-and-durable-boundaries",
  ]) {
    assert.match(runner, new RegExp(area));
  }

  assert.match(runner, /bridgeRoutingRuntime\.test\.ts/);
  assert.match(runner, /bridgeDetailPolling\.test\.ts/);
  assert.match(runner, /transferSchedulerExecution\.test\.ts/);
  assert.match(runner, /developerTerminalFrontend\.test\.ts/);
  assert.match(runner, /room_control::tests::/);
  assert.match(runner, /storage::tests::/);
  assert.match(runner, /bridge_route_payload/);
});

test("explicit Bridge departure is authenticated membership convergence, not remote Burn", () => {
  const roomControl = readFileSync("src-tauri/src/room_control.rs", "utf8");
  const transfer = readFileSync("src-tauri/src/transfer.rs", "utf8");
  const commands = readFileSync("src-tauri/src/commands.rs", "utf8");

  assert.match(roomControl, /"bridge_membership\.departure"/);
  assert.match(roomControl, /remove_departed_bridge_peer/);
  assert.match(roomControl, /purge_bridge_runtime_authority/);
  assert.match(commands, /prepare_bridge_departure_delivery/);
  assert.match(commands, /deliver_prepared_room_control_event/);
  assert.doesNotMatch(transfer, /rooms\/:room_id\/burn/);
  assert.doesNotMatch(transfer, /rooms\/:room_id\/leave/);
  assert.doesNotMatch(transfer, /remote_burn_handler/);
  assert.doesNotMatch(transfer, /remote_leave_handler/);
});

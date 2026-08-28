import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  ACTIVE_BRIDGE_POLL_INTERVAL_MS,
  bridgePollingIntervalMs,
  reconcileSelectedPeerIds,
} from "../src/lib/agentBridge/bridgeDetailPolling";

test("Bridge detail uses active polling only while an operation is active", () => {
  assert.equal(bridgePollingIntervalMs(true), ACTIVE_BRIDGE_POLL_INTERVAL_MS);
  assert.equal(bridgePollingIntervalMs(true), 1_600);
  assert.equal(bridgePollingIntervalMs(false), null);
});

test("unchanged selected peers preserve state identity across room rerenders", () => {
  const current = ["peer-a"];
  const next = reconcileSelectedPeerIds(current, ["peer-a"]);
  assert.equal(next, current);
});

test("selected peers change only when the current route is no longer routeable", () => {
  const current = ["peer-a"];
  assert.deepEqual(reconcileSelectedPeerIds(current, ["peer-b"]), ["peer-b"]);
  assert.deepEqual(reconcileSelectedPeerIds(current, []), []);
});

test("native-v2 lifecycle polling is scoped to one opened revision and cleaned up", () => {
  const component = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  assert.equal(component.match(/window\.setInterval/g)?.length, 1);
  assert.match(component, /revisionId \? window\.setInterval\(\(\) => void refresh\(revisionId\), 2_000\) : null/);
  assert.match(component, /if \(interval !== null\) window\.clearInterval\(interval\)/);
  assert.match(component, /pastey:\/\/native-v2-plan-status/);
  assert.match(component, /event\.payload\.revisionId === revisionId/);
});

test("Send and Agent Task keep their routing semantics separate", () => {
  const component = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.match(component, /Send mode targets one selected device/);
  assert.match(component, /Agent Tasks use the whole Plan scope/);
  assert.match(component, /target: \{ kind: "selected_peer"/);
  assert.match(component, /bridgeTargetKind: "selected_peer"/);
});

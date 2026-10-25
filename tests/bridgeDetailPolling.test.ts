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
  // Plan status is polled by exactly one interval, only while a revision is open.
  const statusPolls = component.match(/window\.setInterval\([^;]*refresh\(/g) ?? [];
  assert.equal(statusPolls.length, 1);
  assert.match(component, /revisionId \? window\.setInterval\(\(\) => void refresh\(revisionId\), 2_000\) : null/);
  assert.match(component, /if \(interval !== null\) window\.clearInterval\(interval\)/);
  // Other intervals (such as result-Return reconciliation) are fine, but every
  // interval the component starts is cleared by its effect cleanup.
  assert.equal(
    component.match(/window\.setInterval\(/g)?.length,
    component.match(/window\.clearInterval\(/g)?.length,
  );
  // Status events are accepted only for the opened revision, or adopt one when none is open.
  assert.match(component, /pastey:\/\/native-v2-plan-status/);
  assert.match(
    component,
    /revisionIdRef\.current === null \|\| event\.payload\.revisionId === revisionIdRef\.current/,
  );
});

test("Send and Agent Task keep their routing semantics separate", () => {
  // Share sends to the one selected device's exact current session...
  const share = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.match(share, /target: \{ kind: "selected_peer"/);
  assert.match(share, /bridgeTargetKind: "selected_peer"/);
  // ...and never starts Agent work, which is addressed separately: to the
  // exact Host (HostRef) the Agents tab chose, over its current session.
  assert.doesNotMatch(share, /startRemoteNative\w*Task\(/);
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  assert.match(lifecycle, /startRemoteNativeCodexTask\(roomId, peerSessionId, hostRef,/);
});

test("an active workspace movement exposes Stop Codex through its task identity", () => {
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  const tracker = readFileSync("src/features/workspace/AgentTaskTracker.tsx", "utf8");
  assert.match(lifecycle, /movement\?\.state === "agent_running"\s*\? movement\.taskId/);
  assert.match(lifecycle, /canCancel: !!cancellableTaskId/);
  // The turn's Stop button names its Agent; for Codex tasks that is "Stop Codex".
  assert.match(tracker, /agent\.canCancel \? <button[^>]*>[\s\S]*?>Stop \{agentName\}<\/button>/);
  assert.match(lifecycle, /"agent\.coding\.codex": "Codex"/);
});

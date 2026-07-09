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

test("Bridge detail owns one interval with cleanup on session change and unmount", () => {
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const detail = pages.slice(
    pages.indexOf("export function BridgeDetailPage"),
    pages.indexOf("function HelloPeerDemoPanel"),
  );
  assert.equal(detail.match(/window\.setInterval/g)?.length, 1);
  assert.match(detail, /bridgePollingIntervalMs\(roomControlPollingActive\)/);
  assert.match(detail, /if \(interval !== null\) window\.clearInterval\(interval\)/);
  assert.match(detail, /window\.removeEventListener\("focus", refresh\)/);
  assert.match(detail, /refreshBridgeControlInboxRef\.current = refreshBridgeControlInbox/);
  assert.match(detail, /\[helloSession, roomControlPollingActive\]/);
});

test("drag-drop listener registration is stable and cleans up async registration races", () => {
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const detail = pages.slice(
    pages.indexOf("export function BridgeDetailPage"),
    pages.indexOf("function HelloPeerDemoPanel"),
  );
  assert.match(detail, /onDragDropEvent/);
  assert.match(detail, /if \(cancelled\) \{\s*fn\(\)/);
  assert.match(detail, /\}, \[room\.id\]\)/);
  assert.match(detail, /enqueueDroppedFilesRef\.current/);
});

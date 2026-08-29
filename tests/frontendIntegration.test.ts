import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { mergeRoomItems, reconcileRoomItems, reconcileRooms } from "../src/lib/authoritativeSnapshots";
import { chooseInitialBridgeId, reconcileSelectedBridgeId, visibleBridgeRooms } from "../src/lib/bridgeSelection";
import { ownAsyncDisposer } from "../src/lib/subscriptionLifecycle";
import { mergeTransferEvent } from "../src/lib/transferState";
import { uniqueNearbyDevices } from "../src/features/workspace/workspaceViewModel";
import type { FileTransferProgressEvent, NearbyDevice, RoomInfo, RoomItem } from "../src/lib/types";

function room(peerConnected: boolean): RoomInfo {
  return {
    id: "room-a",
    created_at: 1,
    expires_at: 2,
    status: "active",
    local_role: "creator",
    auto_burn_after_expiry: false,
    peer_connected: peerConnected,
  };
}

function item(status: RoomItem["status"] = "sent"): RoomItem {
  return {
    id: "item-a",
    room_id: "room-a",
    direction: "outgoing",
    item_kind: "text",
    payload_type: "text",
    size_bytes: 5,
    created_at: 1,
    status,
    text: "hello",
  };
}

function transfer(status: FileTransferProgressEvent["status"] = "transferring"): FileTransferProgressEvent {
  return {
    transfer_id: "transfer-a",
    room_id: "room-a",
    item_id: "item-a",
    direction: "incoming",
    file_name: "hello.txt",
    file_size: 5,
    chunk_size: 5,
    total_chunks: 1,
    transferred_bytes: status === "completed" ? 5 : 0,
    status,
    current_speed_bps: 0,
    average_speed_bps: 0,
  };
}

test("late native-listener registration is unlistened after StrictMode cleanup", async () => {
  let resolveRegistration!: (dispose: () => void) => void;
  const registration = new Promise<() => void>((resolve) => { resolveRegistration = resolve; });
  let unlistenCount = 0;

  const dispose = ownAsyncDisposer(registration);
  dispose();
  dispose();
  resolveRegistration(() => { unlistenCount += 1; });
  await registration;
  await Promise.resolve();

  assert.equal(unlistenCount, 1);
});

test("resolved native-listener registration is cleaned up exactly once", async () => {
  let unlistenCount = 0;
  const dispose = ownAsyncDisposer(Promise.resolve(() => { unlistenCount += 1; }));
  await Promise.resolve();
  dispose();
  dispose();
  assert.equal(unlistenCount, 1);
});

test("authoritative room snapshots preserve identity until device liveness changes", () => {
  const current = [room(false)];
  assert.equal(reconcileRooms(current, [room(false)]), current);
  const connected = reconcileRooms(current, [room(true)]);
  assert.notEqual(connected, current);
  assert.equal(connected[0].peer_connected, true);
});

test("poll and event reconciliation keep each room item once", () => {
  const current = [item()];
  assert.equal(reconcileRoomItems(current, [item(), item()]), current);
  const merged = mergeRoomItems(current, [item(), { ...item(), id: "item-b", created_at: 2 }]);
  assert.deepEqual(merged.map((entry) => entry.id), ["item-b", "item-a"]);
});

test("replayed transfer progress does not create a frontend state transition", () => {
  const event = transfer();
  const first = mergeTransferEvent({}, event, new Set());
  assert.equal(mergeTransferEvent(first, { ...event }, new Set()), first);
  const completed = mergeTransferEvent(first, transfer("completed"), new Set());
  assert.equal(completed["transfer-a"].status, "completed");
  assert.equal(mergeTransferEvent(completed, transfer(), new Set()), completed);
});

test("new Bridge view consumes the real renderer-safe nearby discovery binding", () => {
  const screen = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");
  assert.match(screen, /onListNearby\(\)/);
  assert.match(screen, /onJoinNearby\(device\.device_id\)/);
  assert.match(screen, /device\.display_name/);
  assert.match(screen, /\? "Refreshing…" : "Refresh"/);
  assert.match(screen, /run\("create", onCreate\)/);
  assert.match(screen, /A new Bridge and 8-digit code are created only after this explicit action/);
  assert.doesNotMatch(screen, /useEffect[\s\S]{0,800}onCreate\(/);
  assert.doesNotMatch(screen, /Scan nearby.*disabled/);
  assert.doesNotMatch(screen, /Nearby discovery results are not exposed/);
  assert.match(app, /onListNearbyDevices=\{listNearbyDevices\}/);
  assert.match(app, /nearbyDiscoveryAvailable=\{hasTauriRuntime\(\)\}/);
  assert.match(app, /requestNearbyJoin\(deviceId\)/);
});

test("nearby refresh deduplicates one physical device without mutating Bridge state", () => {
  const device: NearbyDevice = {
    device_id: "device-a",
    display_name: "Laptop",
    platform: "macOS",
    app_version: "1.9.2",
    availability: "Available",
    capabilities: ["nearby_join"],
    last_seen_seconds_ago: 0,
    compatible: true,
  };
  const duplicate = { ...device, display_name: "Laptop refreshed", last_seen_seconds_ago: 1 };
  assert.deepEqual(uniqueNearbyDevices([device, duplicate]), [duplicate]);
});

test("Devices is inspection-only and navigation cannot create or join a Bridge", () => {
  const screens = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const workspace = readFileSync("src/features/workspace/WorkspaceV2.tsx", "utf8");
  const devicesBody = screens.slice(screens.indexOf("export function DevicesScreen"), screens.indexOf("export function NewBridgeScreen"));
  assert.doesNotMatch(devicesBody, /onCreate|onJoin|onListNearby|onJoinNearby|Add device|Find nearby/);
  assert.match(devicesBody, /Device admission is not supported from this view/);
  assert.match(workspace, /route === "devices" \? <DevicesScreen room=\{activeRoom\}/);
});

test("opening New Bridge is a choice view; creation is wired only to explicit Create", () => {
  const screens = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const workspace = readFileSync("src/features/workspace/WorkspaceV2.tsx", "utf8");
  const bridge = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.match(workspace, /route === "new-bridge" \? <NewBridgeScreen onCreate=\{createBridge\}/);
  assert.match(screens, /onClick=\{\(\) => void run\("create", onCreate\)\}/);
  assert.match(bridge, /onNewBridge/);
  assert.doesNotMatch(bridge, /onCreateBridge|createRoom/);
});

test("selection reconciliation removes a burned Bridge and ignores late snapshots", () => {
  const active = room(true);
  const other = { ...room(false), id: "room-b" };
  assert.equal(chooseInitialBridgeId([active, other], "room-a"), "room-a");
  assert.equal(reconcileSelectedBridgeId("room-a", [active, other], new Set(["room-a"])), "");
  assert.deepEqual(visibleBridgeRooms([active, other], new Set(["room-a"])).map((entry) => entry.id), ["room-b"]);
  assert.equal(reconcileSelectedBridgeId("room-a", [active], new Set(["room-a"])), "");
});

test("Developer Mode stays inside the selected Bridge and receiver observation is token-free", () => {
  const routeTypes = readFileSync("src/features/workspace/workspaceTypes.ts", "utf8");
  const workspace = readFileSync("src/features/workspace/WorkspaceV2.tsx", "utf8");
  const bridge = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.doesNotMatch(routeTypes, /"developer"/);
  assert.doesNotMatch(workspace, /route === "developer"/);
  assert.match(workspace, /bridgeMode === "developer"/);
  assert.match(bridge, /developerMode \? <DeveloperModeScreen/);
  assert.match(workspace, /getDeveloperTerminalWorkspace\(roomId\)/);
  assert.match(workspace, /Developer Mode request/);
  assert.match(workspace, />Deny</);
  assert.match(workspace, /Accept/);
});

test("Bridge send selection never falls back from a stale session to another peer", () => {
  const bridge = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.match(bridge, /peers\.find\(\(peer\) => peer\.peerSessionId === selectedPeerId\) \?\? null/);
  assert.doesNotMatch(bridge, /peerSessionId === selectedPeerId\) \?\? peers\[0\]/);
});

test("nearby polling and native subscriptions have explicit remount cleanup", () => {
  const screen = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");
  assert.equal(screen.match(/window\.setInterval/g)?.length, 1);
  assert.equal(screen.match(/window\.clearInterval/g)?.length, 1);
  assert.match(screen, /cancelled = true/);
  assert.match(app, /disposeAll\(disposers\)/);
  assert.match(app, /setWorkspaceFocusRequest/);
});

test("all workspace Tauri listeners use late-resolution-safe ownership", () => {
  for (const path of [
    "src/App.tsx",
    "src/features/workspace/AgentTaskLifecycle.tsx",
    "src/components/DeveloperTerminalViewport.tsx",
  ]) {
    const source = readFileSync(path, "utf8");
    const listenCalls = source.match(/\blisten</g)?.length ?? 0;
    const ownedCalls = source.match(/ownAsyncDisposer\(listen</g)?.length ?? 0;
    assert.equal(ownedCalls, listenCalls, path);
  }
});

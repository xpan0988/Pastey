import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  assertCapabilityEventHasSelectedPeerRoute,
  assertControlEventHasSelectedPeerRoute,
  bridgeRoutePayload,
  deriveAuthoritativeCapabilityRoute,
  deriveAuthoritativeControlRoute,
  deriveAuthoritativeFileSendRoute,
  deriveAuthoritativeImageSendRoute,
  deriveAuthoritativeTextSendRoute,
  deriveBridgeRoutingStateForRoom,
  enqueueFilePathsWithBridgeRoute,
  enqueueTransferInputsWithBridgeRoute,
  requireReadySelectedPeerRouteForContentKind,
  routeStateLabel,
  sendFileToRoomWithBridgeRoute,
  sendTextToRoomWithBridgeRoute,
} from "../src/lib/bridgeRoutingRuntime";
import { bridgePeerSessionId, validateBridgeRoute } from "../src/lib/bridgeRouting";
import type { RoomControlSessionContext, RoomInfo, RoomItem } from "../src/lib/types";

const ROOM: RoomInfo = {
  id: "room-1",
  room_code: "12345678",
  room_code_display: "1234 5678",
  created_at: 1,
  expires_at: 2,
  status: "active",
  local_role: "creator",
  peer_device_name: "Peer Device",
  auto_burn_after_expiry: false,
  peer_connected: true,
  local_burned_at: null,
  peer_burned_at: null,
};

const CONTROL_SESSION: RoomControlSessionContext = {
  roomId: "room-1",
  localSessionRef: "local-session-1",
  peerSessionRef: "peer-session-1",
  peerRouteRef: "legacy-room-peer:room-1",
  peerConnected: true,
};

function item(roomId = ROOM.id): RoomItem {
  return {
    id: "item-1",
    room_id: roomId,
    direction: "outgoing",
    item_kind: "text",
    payload_type: "text",
    size_bytes: 0,
    created_at: 1,
    status: "sent",
    text: "hello",
  };
}

function controlEvent(overrides: Partial<{
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  kind: string;
}> = {}) {
  return {
    roomRef: CONTROL_SESSION.roomId,
    sourceDeviceRef: CONTROL_SESSION.localSessionRef,
    targetPeerRef: CONTROL_SESSION.peerSessionRef,
    kind: "capability_preview",
    ...overrides,
  };
}

test("production routing helper derives selected_peer for one routeable remote peer", () => {
  const state = deriveBridgeRoutingStateForRoom(ROOM);

  assert.equal(state.status, "ready_selected_peer");
  if (state.status !== "ready_selected_peer") return;
  assert.equal(state.route.target.kind, "selected_peer");
  assert.equal(routeStateLabel(state), "Selected peer: Peer Device");
});

test("production routing helper returns no_route for zero routeable remote peers", () => {
  const state = deriveBridgeRoutingStateForRoom({ ...ROOM, peer_connected: false });

  assert.equal(state.status, "no_route");
});

test("production routing helper returns requires_explicit_selection for multiple routeable remote peers", () => {
  const state = deriveBridgeRoutingStateForRoom({
    ...ROOM,
    peers: [
      { peerSessionId: "peer:a", displayName: "A", connected: true },
      { peerSessionId: "peer:b", displayName: "B", connected: true },
    ],
  });

  assert.equal(state.status, "requires_explicit_selection");
  assert.equal("route" in state, false);
});

test("text send wrapper derives selected-peer route payload for Tauri", async () => {
  const calls: unknown[] = [];
  const result = await sendTextToRoomWithBridgeRoute(ROOM, "hello", async (roomId, text, bridgeRoute) => {
    calls.push({ roomId, text, bridgeRoute });
    return item(roomId);
  });

  assert.equal(result.room_id, ROOM.id);
  assert.deepEqual(calls, [{
    roomId: ROOM.id,
    text: "hello",
    bridgeRoute: {
      schemaVersion: "pastey-bridge-text-route/v1",
      bridgeSessionId: "legacy-room:room-1",
      target: {
        kind: "selected_peer",
        peerSessionId: "legacy-room-peer:room-1",
      },
    },
  }]);
});

test("text send wrapper sends explicit selected-peers and broadcast payloads", async () => {
  const multiPeerRoom: RoomInfo = {
    ...ROOM,
    peers: [
      { peerSessionId: "peer:a", displayName: "A", connected: true, liveness: "connected", joinMethod: "nearby_accept", currentSessionOnly: true },
      { peerSessionId: "peer:b", displayName: "B", connected: true, liveness: "connected", joinMethod: "nearby_accept", currentSessionOnly: true },
    ],
  };
  const calls: unknown[] = [];
  const sender = async (roomId: string, text: string, bridgeRoute?: unknown) => {
    calls.push({ roomId, text, bridgeRoute });
    return item(roomId);
  };

  await sendTextToRoomWithBridgeRoute(multiPeerRoom, "hello", sender, {
    bridgeSessionId: "legacy-room:room-1",
    target: { kind: "selected_peers", peerSessionIds: [bridgePeerSessionId("peer:a"), bridgePeerSessionId("peer:b")] },
  });
  await sendTextToRoomWithBridgeRoute(multiPeerRoom, "all", sender, {
    bridgeSessionId: "legacy-room:room-1",
    target: { kind: "broadcast_bridge", explicit: true },
  });

  assert.deepEqual(calls, [
    {
      roomId: ROOM.id,
      text: "hello",
      bridgeRoute: {
        schemaVersion: "pastey-bridge-text-route/v1",
        bridgeSessionId: "legacy-room:room-1",
        target: { kind: "selected_peers", peerSessionIds: ["peer:a", "peer:b"] },
      },
    },
    {
      roomId: ROOM.id,
      text: "all",
      bridgeRoute: {
        schemaVersion: "pastey-bridge-text-route/v1",
        bridgeSessionId: "legacy-room:room-1",
        target: { kind: "broadcast_bridge", explicit: true },
      },
    },
  ]);
});

test("explicit stale selected-peer route is route-expired and does not fall back to reconnected peer", async () => {
  const reconnectedRoom: RoomInfo = {
    ...ROOM,
    peers: [
      {
        peerSessionId: "legacy-room-peer:room-1",
        displayName: "Old route",
        connected: false,
        liveness: "stale",
        joinMethod: "nearby_accept",
        currentSessionOnly: true,
      },
      {
        peerSessionId: "legacy-room-peer:room-1:reconnect:1",
        displayName: "Reconnected route",
        connected: true,
        liveness: "connected",
        joinMethod: "nearby_accept",
        currentSessionOnly: true,
      },
    ],
  };
  const calls: unknown[] = [];
  const staleRoute = {
    bridgeSessionId: "legacy-room:room-1",
    target: {
      kind: "selected_peer" as const,
      peerSessionId: bridgePeerSessionId("legacy-room-peer:room-1"),
    },
  };

  await assert.rejects(
    () => sendFileToRoomWithBridgeRoute(reconnectedRoom, "/tmp/a.png", {}, async (roomId, path, options) => {
      calls.push({ roomId, path, options });
      return { ...item(roomId), payload_type: "file", item_kind: "outgoing_file" };
    }, staleRoute),
    /route expired/i,
  );
  assert.deepEqual(calls, []);
});

test("zero routeable peers blocks text send and does not call sender", async () => {
  const calls: unknown[] = [];

  await assert.rejects(
    () => sendTextToRoomWithBridgeRoute({ ...ROOM, peer_connected: false }, "hello", async (roomId, text, bridgeRoute) => {
      calls.push({ roomId, text, bridgeRoute });
      return item(roomId);
    }),
    /No routeable remote Bridge peer/,
  );
  assert.deepEqual(calls, []);
});

test("multiple routeable peers blocks text send without broadcast", async () => {
  const calls: unknown[] = [];

  await assert.rejects(
    () => sendTextToRoomWithBridgeRoute({
      ...ROOM,
      peers: [
        { peerSessionId: "peer:a", displayName: "A", connected: true },
        { peerSessionId: "peer:b", displayName: "B", connected: true },
      ],
    }, "hello", async (roomId, text, bridgeRoute) => {
      calls.push({ roomId, text, bridgeRoute });
      return item(roomId);
    }),
    /explicit target selection/,
  );
  assert.deepEqual(calls, []);
});

test("production text route guard allows selected-peers and broadcast data routes", () => {
  const selectedPeers = validateBridgeRoute({
    bridgeSessionId: "bridge:one",
    target: { kind: "selected_peers", peerSessionIds: ["peer:a", "peer:b"] },
  });
  const broadcast = validateBridgeRoute({
    bridgeSessionId: "bridge:one",
    target: { kind: "broadcast_bridge", explicit: true },
  });
  assert.equal(selectedPeers.valid, true, selectedPeers.valid ? "" : selectedPeers.errors.join(" "));
  assert.equal(broadcast.valid, true, broadcast.valid ? "" : broadcast.errors.join(" "));
  if (!selectedPeers.valid || !broadcast.valid) return;

  assert.deepEqual(deriveAuthoritativeTextSendRoute(selectedPeers.route), selectedPeers.route);
  assert.deepEqual(deriveAuthoritativeTextSendRoute(broadcast.route), broadcast.route);
});

test("route payload model can represent selected-peer selected-peers and broadcast without granting authority", () => {
  const selectedPeer = validateBridgeRoute({
    bridgeSessionId: "bridge:one",
    target: { kind: "selected_peer", peerSessionId: "peer:a" },
  });
  const selectedPeers = validateBridgeRoute({
    bridgeSessionId: "bridge:one",
    target: { kind: "selected_peers", peerSessionIds: ["peer:a", "peer:b"] },
  });
  const broadcast = validateBridgeRoute({
    bridgeSessionId: "bridge:one",
    target: { kind: "broadcast_bridge", explicit: true },
  });
  assert.equal(selectedPeer.valid, true, selectedPeer.valid ? "" : selectedPeer.errors.join(" "));
  assert.equal(selectedPeers.valid, true, selectedPeers.valid ? "" : selectedPeers.errors.join(" "));
  assert.equal(broadcast.valid, true, broadcast.valid ? "" : broadcast.errors.join(" "));
  if (!selectedPeer.valid || !selectedPeers.valid || !broadcast.valid) return;

  assert.deepEqual(bridgeRoutePayload(selectedPeer.route, "pastey-bridge-text-route/v1"), {
    schemaVersion: "pastey-bridge-text-route/v1",
    bridgeSessionId: "bridge:one",
    target: { kind: "selected_peer", peerSessionId: "peer:a" },
  });
  assert.deepEqual(bridgeRoutePayload(selectedPeers.route, "pastey-bridge-text-route/v1"), {
    schemaVersion: "pastey-bridge-text-route/v1",
    bridgeSessionId: "bridge:one",
    target: { kind: "selected_peers", peerSessionIds: ["peer:a", "peer:b"] },
  });
  assert.deepEqual(bridgeRoutePayload(broadcast.route, "pastey-bridge-text-route/v1"), {
    schemaVersion: "pastey-bridge-text-route/v1",
    bridgeSessionId: "bridge:one",
    target: { kind: "broadcast_bridge", explicit: true },
  });
  assert.equal(JSON.stringify([selectedPeer, selectedPeers, broadcast]).includes("authority"), false);
  assert.equal(JSON.stringify([selectedPeer, selectedPeers, broadcast]).includes("consent"), false);
});

test("authoritative text route guard does not mutate Room state or create durable fields", () => {
  const room = structuredClone(ROOM);
  const before = JSON.stringify(room);
  const route = deriveAuthoritativeTextSendRoute(room);
  const stateRoute = deriveAuthoritativeTextSendRoute(deriveBridgeRoutingStateForRoom(room));
  const serialized = JSON.stringify(route);

  assert.equal(JSON.stringify(room), before);
  assert.equal(route.target.kind, "selected_peer");
  assert.deepEqual(stateRoute, route);
  assert.equal(serialized.includes("durable"), false);
  assert.equal(serialized.includes("history"), false);
  assert.equal(serialized.includes("trust"), false);
  assert.equal(serialized.includes("consent"), false);
});

test("file send wrapper derives selected-peer route payload for Tauri", async () => {
  const options = {
    displayName: "a.png",
    mimeType: "image/png",
    queueItemId: "queue-1",
    requestedWindow: 7,
  };
  const calls: unknown[] = [];
  await sendFileToRoomWithBridgeRoute(ROOM, "/tmp/a.png", options, async (roomId, path, sendOptions) => {
    calls.push({ roomId, path, options: sendOptions });
    return { ...item(roomId), payload_type: "file", item_kind: "outgoing_file" };
  });

  assert.deepEqual(calls, [{
    roomId: ROOM.id,
    path: "/tmp/a.png",
    options: {
      ...options,
      bridgeRoute: {
        schemaVersion: "pastey-bridge-file-route/v1",
        bridgeSessionId: "legacy-room:room-1",
        target: {
          kind: "selected_peer",
          peerSessionId: "legacy-room-peer:room-1",
        },
      },
    },
  }]);
});

test("zero or multiple routeable peers block file dispatch before Tauri", async () => {
  const calls: unknown[] = [];
  const sender = async (roomId: string, path: string, sendOptions?: unknown) => {
    calls.push({ roomId, path, sendOptions });
    return { ...item(roomId), payload_type: "file" as const, item_kind: "outgoing_file" as const };
  };

  await assert.rejects(
    () => sendFileToRoomWithBridgeRoute({ ...ROOM, peer_connected: false }, "/tmp/a.png", {}, sender),
    /No routeable remote Bridge peer/,
  );
  await assert.rejects(
    () => sendFileToRoomWithBridgeRoute({
      ...ROOM,
      peers: [
        { peerSessionId: "peer:a", displayName: "A", connected: true },
        { peerSessionId: "peer:b", displayName: "B", connected: true },
      ],
    }, "/tmp/a.png", {}, sender),
    /explicit target selection/,
  );
  assert.deepEqual(calls, []);
});

test("file and pasted-image enqueue wrappers keep room id and payload shape", () => {
  const fileCalls: unknown[] = [];
  const inputCalls: unknown[] = [];
  const inputs = [{
    path: "/tmp/paste.png",
    displayName: "paste.png",
    mimeType: "image/png",
    sizeBytes: 10,
    dedupeKey: "paste-key",
    deleteWhenDone: true,
  }];

  enqueueFilePathsWithBridgeRoute(ROOM, ["/tmp/a.txt"], (roomId, paths) => {
    fileCalls.push({ roomId, paths });
  });
  enqueueTransferInputsWithBridgeRoute(ROOM, inputs, "pasted_image", (roomId, queuedInputs) => {
    inputCalls.push({ roomId, inputs: queuedInputs });
  });

  assert.deepEqual(fileCalls, [{ roomId: ROOM.id, paths: ["/tmp/a.txt"] }]);
  assert.deepEqual(inputCalls, [{ roomId: ROOM.id, inputs }]);
});

test("zero routeable peers blocks file and pasted-image enqueue", () => {
  const fileCalls: unknown[] = [];
  const inputCalls: unknown[] = [];
  const disconnectedRoom = { ...ROOM, peer_connected: false };

  assert.throws(
    () => enqueueFilePathsWithBridgeRoute(disconnectedRoom, ["/tmp/a.txt"], (roomId, paths) => {
      fileCalls.push({ roomId, paths });
    }),
    /No routeable remote Bridge peer/,
  );
  assert.throws(
    () => enqueueTransferInputsWithBridgeRoute(disconnectedRoom, [{ path: "/tmp/paste.png" }], "pasted_image", (roomId, inputs) => {
      inputCalls.push({ roomId, inputs });
    }),
    /No routeable remote Bridge peer/,
  );
  assert.deepEqual(fileCalls, []);
  assert.deepEqual(inputCalls, []);
});

test("multiple routeable peers blocks file and image enqueue without broadcast", () => {
  const fileCalls: unknown[] = [];
  const imageCalls: unknown[] = [];
  const multiPeerRoom = {
    ...ROOM,
    peers: [
      { peerSessionId: "peer:a", displayName: "A", connected: true },
      { peerSessionId: "peer:b", displayName: "B", connected: true },
    ],
  };

  assert.throws(
    () => enqueueFilePathsWithBridgeRoute(multiPeerRoom, ["/tmp/a.txt"], (roomId, paths) => {
      fileCalls.push({ roomId, paths });
    }),
    /explicit target selection/,
  );
  assert.throws(
    () => enqueueTransferInputsWithBridgeRoute(multiPeerRoom, [{ path: "/tmp/a.png" }], "image", (roomId, inputs) => {
      imageCalls.push({ roomId, inputs });
    }),
    /explicit target selection/,
  );
  assert.deepEqual(fileCalls, []);
  assert.deepEqual(imageCalls, []);
});

test("production file and image route guards allow selected-peers and broadcast data routes", () => {
  const selectedPeers = validateBridgeRoute({
    bridgeSessionId: "bridge:one",
    target: { kind: "selected_peers", peerSessionIds: ["peer:a", "peer:b"] },
  });
  const broadcast = validateBridgeRoute({
    bridgeSessionId: "bridge:one",
    target: { kind: "broadcast_bridge", explicit: true },
  });
  assert.equal(selectedPeers.valid, true, selectedPeers.valid ? "" : selectedPeers.errors.join(" "));
  assert.equal(broadcast.valid, true, broadcast.valid ? "" : broadcast.errors.join(" "));
  if (!selectedPeers.valid || !broadcast.valid) return;

  assert.deepEqual(deriveAuthoritativeFileSendRoute(selectedPeers.route), selectedPeers.route);
  assert.deepEqual(deriveAuthoritativeFileSendRoute(broadcast.route), broadcast.route);
  assert.deepEqual(deriveAuthoritativeImageSendRoute(selectedPeers.route), selectedPeers.route);
  assert.deepEqual(deriveAuthoritativeImageSendRoute(broadcast.route), broadcast.route);
});

test("authoritative file and image route guards do not mutate Room state or create durable fields", () => {
  const room = structuredClone(ROOM);
  const before = JSON.stringify(room);
  const fileRoute = deriveAuthoritativeFileSendRoute(room);
  const imageRoute = deriveAuthoritativeImageSendRoute(deriveBridgeRoutingStateForRoom(room));
  const serialized = JSON.stringify({ fileRoute, imageRoute });

  assert.equal(JSON.stringify(room), before);
  assert.equal(fileRoute.target.kind, "selected_peer");
  assert.equal(imageRoute.target.kind, "selected_peer");
  assert.equal(serialized.includes("durable"), false);
  assert.equal(serialized.includes("history"), false);
  assert.equal(serialized.includes("trust"), false);
  assert.equal(serialized.includes("consent"), false);
});

test("room-control and capability routing rejects broadcast policy", () => {
  const ready = requireReadySelectedPeerRouteForContentKind(ROOM, "agent_bridge_capability_event");
  assert.equal(ready.route.target.kind, "selected_peer");
  assert.throws(
    () => requireReadySelectedPeerRouteForContentKind({
      ...ROOM,
      peers: [
        { peerSessionId: "peer:a", displayName: "A", connected: true },
        { peerSessionId: "peer:b", displayName: "B", connected: true },
      ],
    }, "agent_bridge_capability_event"),
    /explicit target selection/,
  );
});

test("room-control capability helper derives exact selected-peer route for active session event", () => {
  const route = assertCapabilityEventHasSelectedPeerRoute(CONTROL_SESSION, controlEvent());
  const controlRoute = assertControlEventHasSelectedPeerRoute(CONTROL_SESSION, controlEvent({ kind: "capability_execute_request" }));

  assert.equal(route.bridgeSessionId, `legacy-room:${CONTROL_SESSION.roomId}`);
  assert.equal(route.target.kind, "selected_peer");
  assert.equal(controlRoute.target.kind, "selected_peer");
  if (route.target.kind !== "selected_peer") return;
  assert.equal(route.target.peerSessionId, CONTROL_SESSION.peerRouteRef);
});

test("room-control capability helper blocks no route and mismatched event targets", () => {
  assert.throws(
    () => assertCapabilityEventHasSelectedPeerRoute(
      { ...CONTROL_SESSION, peerConnected: false },
      controlEvent(),
    ),
    /connected selected Bridge peer/,
  );
  assert.throws(
    () => assertCapabilityEventHasSelectedPeerRoute(CONTROL_SESSION, controlEvent({ targetPeerRef: "other-peer" })),
    /exactly one selected Bridge peer target/,
  );
  assert.throws(
    () => assertCapabilityEventHasSelectedPeerRoute(CONTROL_SESSION, controlEvent({ sourceDeviceRef: "other-local" })),
    /active local session source/,
  );
  assert.throws(
    () => assertCapabilityEventHasSelectedPeerRoute(CONTROL_SESSION, controlEvent({ roomRef: "other-room" })),
    /active Bridge session/,
  );
});

test("production room-control and capability send reject selected-peers and broadcast routes in this pass", () => {
  const selectedPeers = validateBridgeRoute({
    bridgeSessionId: "bridge:one",
    target: { kind: "selected_peers", peerSessionIds: ["peer:a", "peer:b"] },
  });
  const broadcast = validateBridgeRoute({
    bridgeSessionId: "bridge:one",
    target: { kind: "broadcast_bridge", explicit: true },
  });
  assert.equal(selectedPeers.valid, true, selectedPeers.valid ? "" : selectedPeers.errors.join(" "));
  assert.equal(broadcast.valid, true, broadcast.valid ? "" : broadcast.errors.join(" "));
  if (!selectedPeers.valid || !broadcast.valid) return;

  assert.throws(() => deriveAuthoritativeControlRoute(selectedPeers.route), /Production control send requires/);
  assert.throws(() => deriveAuthoritativeControlRoute(broadcast.route), /Production control send requires/);
  assert.throws(() => deriveAuthoritativeCapabilityRoute(selectedPeers.route), /Production capability send requires/);
  assert.throws(() => deriveAuthoritativeCapabilityRoute(broadcast.route), /Production capability send requires/);
});

test("room-control route assertion does not create consent trust authority or history", () => {
  const session = structuredClone(CONTROL_SESSION);
  const event = controlEvent();
  const before = JSON.stringify({ session, event });
  const route = assertCapabilityEventHasSelectedPeerRoute(session, event);
  const serialized = JSON.stringify(route);

  assert.equal(JSON.stringify({ session, event }), before);
  assert.equal(serialized.includes("durable"), false);
  assert.equal(serialized.includes("history"), false);
  assert.equal(serialized.includes("trust"), false);
  assert.equal(serialized.includes("consent"), false);
  assert.equal(serialized.includes("authority"), false);
});

test("route derivation does not mutate Room state or create durable authority fields", () => {
  const room = structuredClone(ROOM);
  const before = JSON.stringify(room);
  const state = deriveBridgeRoutingStateForRoom(room);
  const serialized = JSON.stringify(state);

  assert.equal(JSON.stringify(room), before);
  assert.equal(serialized.includes("durable"), false);
  assert.equal(serialized.includes("history"), false);
  assert.equal(serialized.includes("trust"), false);
  assert.equal(serialized.includes("consent"), false);
});

test("production integration sends data and selected-peer-only control route payloads", () => {
  const roomPage = readFileSync("src/pages/RoomPage.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");
  const controlPanel = readFileSync("src/components/agentBridge/RoomControlPanel.tsx", "utf8");
  const tauri = readFileSync("src/lib/tauri.ts", "utf8");
  const scheduler = readFileSync("src/lib/transferScheduler.ts", "utf8");
  const outcomeTypes = readFileSync("src/lib/bridgeDeliveryOutcome.ts", "utf8");
  const controlWrapper = tauri.slice(
    tauri.indexOf("export async function sendRoomControlEvent"),
    tauri.indexOf("export async function getRoomControlSessionContext"),
  );

  assert.match(roomPage, /deriveBridgeRoutingStateForRoom\(room\)/);
  assert.match(roomPage, /sendTextToRoomWithBridgeRoute\(room, text, sendTextToRoom, selectedBridgeRoute\)/);
  assert.match(roomPage, /formatBridgeRouteErrorForUser\(err\)/);
  assert.match(app, /sendFileToRoomWithBridgeRoute\(\s*roomForRoute,\s*item\.path,\s*sendOptions,\s*sendFileToRoom,\s*item\.bridgeRoute,/s);
  assert.match(app, /formatBridgeRouteErrorForUser\(err\)/);
  assert.match(app, /No current Room state is available for Bridge file route derivation/);
  assert.doesNotMatch(app, /else \{\s*await sendFileToRoom\(item\.roomId/s);
  assert.match(controlPanel, /assertCapabilityEventHasSelectedPeerRoute\(session, event\)/);
  assert.match(controlPanel, /bridgeRoutePayload\(route, "pastey-bridge-control-route\/v1"\)/);
  assert.match(tauri, /invoke\("send_text_to_room", \{/);
  assert.match(tauri, /bridgeRoute: bridgeRoute \?\? null/);
  assert.match(tauri, /invoke\("send_file_to_room", \{/);
  assert.match(controlWrapper, /bridgeRoute: bridgeRoute \?\? null/);
  assert.doesNotMatch(controlWrapper, /selectedPeers|broadcast/);
  assert.match(scheduler, /bridgeOperationId/);
  assert.match(scheduler, /targetPeerSessionId/);
  assert.doesNotMatch(scheduler, /durable|trust|consent|history/);
  assert.match(outcomeTypes, /export interface BridgeDeliveryOutcome/);
  assert.match(outcomeTypes, /peerSessionRef/);
  assert.match(outcomeTypes, /export interface BridgeSendOperation/);
  assert.match(outcomeTypes, /outcomes: readonly BridgeDeliveryOutcome\[\]/);
});

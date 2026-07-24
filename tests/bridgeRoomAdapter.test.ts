import assert from "node:assert/strict";
import test from "node:test";

import { validateBridgeRoute } from "../src/lib/bridgeRouting";
import {
  assertLegacyRoomRouteAllowedForContentKind,
  describeLegacyRoomRoutingState,
  deriveLegacyRoomDefaultBridgeRoute,
  legacyRoomToBridgePeerCollection,
  type LegacyRoomBridgeInput,
} from "../src/lib/bridgeRoomAdapter";

const ROOM: LegacyRoomBridgeInput = {
  id: "room-1",
  status: "active",
  local_role: "creator",
  peer_device_name: "Peer Device",
  peer_connected: true,
  peer_burned_at: null,
};

function broadcastRoute() {
  const result = validateBridgeRoute({
    bridgeSessionId: "legacy-room:room-1",
    target: { kind: "broadcast_bridge", explicit: true },
  });
  assert.equal(result.valid, true, result.valid ? "" : result.errors.join(" "));
  if (!result.valid) throw new Error("Expected valid broadcast route.");
  return result.route;
}

test("one routeable remote legacy peer derives selected_peer", () => {
  const result = deriveLegacyRoomDefaultBridgeRoute(ROOM);

  assert.equal(result.status, "selected_peer");
  if (result.status !== "selected_peer") return;
  assert.equal(result.route.bridgeSessionId, "legacy-room:room-1");
  assert.equal(result.route.target.kind, "selected_peer");
  assert.equal(result.peer.peerSessionId, "legacy-room-peer:room-1");
  assert.equal(result.peer.currentSessionOnly, true);
});

test("zero routeable remote peers returns no-route", () => {
  const result = deriveLegacyRoomDefaultBridgeRoute({
    ...ROOM,
    peer_connected: false,
  });

  assert.equal(result.status, "no_route");
  assert.deepEqual(result.routeablePeerIds, []);
});

test("multiple routeable remote peers requires explicit selection and does not broadcast", () => {
  const result = deriveLegacyRoomDefaultBridgeRoute({
    ...ROOM,
    peers: [
      { peerSessionId: "peer:a", displayName: "A", connected: true },
      { peerSessionId: "peer:b", displayName: "B", connected: true },
    ],
  });

  assert.equal(result.status, "requires_explicit_selection");
  assert.deepEqual(result.routeablePeerIds, ["peer:a", "peer:b"]);
  assert.equal("route" in result, false);
});

for (const [label, room] of [
  ["disconnected", { ...ROOM, peer_connected: false }],
  ["left", { ...ROOM, status: "peer_left", peer_connected: true }],
  ["stale", { ...ROOM, status: "expired", peer_connected: true }],
] as const) {
  test(`${label} legacy peer does not produce selected route`, () => {
    assert.equal(deriveLegacyRoomDefaultBridgeRoute(room).status, "no_route");
  });
}

test("local self is not treated as a remote send target", () => {
  const result = deriveLegacyRoomDefaultBridgeRoute({
    ...ROOM,
    peers: [{ peerSessionId: "peer:local", displayName: "This device", connected: true, isLocalSelf: true }],
  });

  assert.equal(result.status, "no_route");
});

test("manual-code and nearby-accept join methods remain current-session only", () => {
  const nearby = legacyRoomToBridgePeerCollection(ROOM);
  const manual = legacyRoomToBridgePeerCollection({
    ...ROOM,
    local_role: "joined",
  });

  assert.equal(nearby.peers[0]?.joinMethod, "nearby_accept");
  assert.equal(manual.peers[0]?.joinMethod, "manual_code");
  assert.equal(nearby.peers[0]?.currentSessionOnly, true);
  assert.equal(manual.peers[0]?.currentSessionOnly, true);
});

test("adapter output does not include durable identity, history, trust, or consent fields", () => {
  const collection = legacyRoomToBridgePeerCollection(ROOM);
  const route = deriveLegacyRoomDefaultBridgeRoute(ROOM);
  const description = describeLegacyRoomRoutingState(ROOM);
  const serialized = JSON.stringify({ collection, route, description });

  assert.equal(serialized.includes("durable"), false);
  assert.equal(serialized.includes("identity"), false);
  assert.equal(serialized.includes("history"), false);
  assert.equal(serialized.includes("trust"), false);
  assert.equal(serialized.includes("consent"), false);
});

test("adapter does not mutate input Room data", () => {
  const room: LegacyRoomBridgeInput = {
    ...ROOM,
    peers: [{ peerSessionId: "peer:a", displayName: "A", connected: true }],
  };
  const before = JSON.stringify(room);

  legacyRoomToBridgePeerCollection(room);
  deriveLegacyRoomDefaultBridgeRoute(room);
  describeLegacyRoomRoutingState(room);

  assert.equal(JSON.stringify(room), before);
});

test("adapter behavior is deterministic", () => {
  const firstCollection = legacyRoomToBridgePeerCollection(ROOM);
  const secondCollection = legacyRoomToBridgePeerCollection(ROOM);
  const firstRoute = deriveLegacyRoomDefaultBridgeRoute(ROOM);
  const secondRoute = deriveLegacyRoomDefaultBridgeRoute(ROOM);
  const firstDescription = describeLegacyRoomRoutingState(ROOM);
  const secondDescription = describeLegacyRoomRoutingState(ROOM);

  assert.deepEqual(firstCollection, secondCollection);
  assert.deepEqual(firstRoute, secondRoute);
  assert.deepEqual(firstDescription, secondDescription);
});

import assert from "node:assert/strict";
import test from "node:test";

import { validateBridgeRoute, type BridgeRoute } from "../src/lib/bridgeRouting";
import {
  assertPeerCanBeRouteTarget,
  assertRouteCompatibleWithPeerCollection,
  bridgePeerDisplayName,
  bridgePeerSessionId,
  deriveDefaultBridgeRouteForCurrentSession,
  getRouteableBridgePeers,
  normalizeBridgePeerSession,
  resolveBridgeRoutePeerIds,
  validateBridgePeerCollection,
  type BridgePeerCollection,
  type BridgePeerJoinMethod,
  type BridgePeerSession,
} from "../src/lib/bridgePeers";

const BRIDGE_SESSION = "bridge-session:current";
const PEER_A = bridgePeerSessionId("peer-session:a");
const PEER_B = bridgePeerSessionId("peer-session:b");
const PEER_C = bridgePeerSessionId("peer-session:c");
const LOCAL_PEER = bridgePeerSessionId("peer-session:local");

function peer(
  peerSessionId = PEER_A,
  overrides: Partial<BridgePeerSession> = {},
): BridgePeerSession {
  return {
    bridgeSessionId: BRIDGE_SESSION,
    peerSessionId,
    displayName: bridgePeerDisplayName(`Peer ${String(peerSessionId).split(":").at(-1) ?? "remote"}`),
    joinMethod: "nearby_accept",
    liveness: "connected",
    accepted: true,
    sessionVerified: true,
    currentSessionOnly: true,
    ...overrides,
  };
}

function collection(peers: readonly BridgePeerSession[]): BridgePeerCollection {
  const result = validateBridgePeerCollection({ bridgeSessionId: BRIDGE_SESSION, peers });
  assert.equal(result.valid, true, result.valid ? "" : result.errors.join(" "));
  if (!result.valid) throw new Error("Expected valid collection.");
  return result.collection;
}

function route(target: unknown): BridgeRoute {
  const result = validateBridgeRoute({ bridgeSessionId: BRIDGE_SESSION, target });
  assert.equal(result.valid, true, result.valid ? "" : result.errors.join(" "));
  if (!result.valid) throw new Error("Expected valid route.");
  return result.route;
}

function assertRejects(fn: () => void, expected: string): void {
  assert.throws(fn, (error) => {
    assert.equal(error instanceof Error, true);
    assert.match(String((error as Error).message), new RegExp(expected));
    return true;
  });
}

test("accepted nearby peer is current-session only", () => {
  const result = normalizeBridgePeerSession({
    bridgeSessionId: BRIDGE_SESSION,
    peerSessionId: PEER_A,
    displayName: "Nearby peer",
    joinMethod: "nearby_accept",
    liveness: "connected",
    accepted: true,
    sessionVerified: true,
    currentSessionOnly: true,
  });

  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.peer.joinMethod, "nearby_accept");
  assert.equal(result.peer.accepted, true);
  assert.equal(result.peer.sessionVerified, true);
  assert.equal(result.peer.currentSessionOnly, true);
  assert.equal("durableIdentity" in result.peer, false);
});

test("manual-code peer is current-session only", () => {
  const result = normalizeBridgePeerSession({
    bridgeSessionId: BRIDGE_SESSION,
    peerSessionId: PEER_A,
    displayName: "Code peer",
    joinMethod: "manual_code",
    liveness: "connected",
    accepted: true,
    sessionVerified: true,
    currentSessionOnly: true,
  });

  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.peer.joinMethod, "manual_code");
  assert.equal(result.peer.currentSessionOnly, true);
});

test("accepted peer is not durable trusted device", () => {
  const normalized = normalizeBridgePeerSession({
    bridgeSessionId: BRIDGE_SESSION,
    peerSessionId: PEER_A,
    displayName: "Session peer",
    joinMethod: "nearby_accept",
    liveness: "connected",
    accepted: true,
    sessionVerified: true,
    currentSessionOnly: true,
  });
  assert.equal(normalized.ok, true);
  if (!normalized.ok) return;

  const serialized = JSON.stringify(normalized.peer);
  assert.equal(serialized.includes("trusted"), false);
  assert.equal(serialized.includes("durable"), false);
  assert.equal(serialized.includes("history"), false);

  const trusted = normalizeBridgePeerSession({
    ...normalized.peer,
    trustedDeviceId: "not-current-session",
  });
  assert.equal(trusted.ok, false);
});

test("duplicate current-session peer ids are rejected", () => {
  const result = validateBridgePeerCollection({
    bridgeSessionId: BRIDGE_SESSION,
    peers: [peer(PEER_A), peer(PEER_A, { displayName: bridgePeerDisplayName("Duplicate") })],
  });

  assert.equal(result.valid, false);
  if (!result.valid) {
    assert.ok(result.errors.some((error) => error.includes("duplicate")));
  }
});

test("routeable filtering excludes stale, left, disconnected, and local self peers", () => {
  const peers = collection([
    peer(PEER_A),
    peer(PEER_B, { liveness: "disconnected" }),
    peer(PEER_C, { liveness: "stale" }),
    peer(bridgePeerSessionId("peer-session:left"), { liveness: "left" }),
    peer(LOCAL_PEER, { isLocalSelf: true }),
  ]);

  assert.deepEqual(getRouteableBridgePeers(peers).map((candidate) => candidate.peerSessionId), [PEER_A]);
  assert.deepEqual(
    getRouteableBridgePeers(peers, { allowLocalSelf: true }).map((candidate) => candidate.peerSessionId),
    [PEER_A, LOCAL_PEER],
  );
  assertRejects(() => assertPeerCanBeRouteTarget(peer(LOCAL_PEER, { isLocalSelf: true })), "remote");
});

test("selected peer route rejects unknown peer id", () => {
  const peers = collection([peer(PEER_A)]);
  const unknown = route({ kind: "selected_peer", peerSessionId: bridgePeerSessionId("peer-session:unknown") });

  assertRejects(() => assertRouteCompatibleWithPeerCollection(unknown, peers), "known current-session peer");
});

test("selected peer route rejects non-routeable peer", () => {
  const peers = collection([peer(PEER_A, { liveness: "disconnected" })]);
  const selected = route({ kind: "selected_peer", peerSessionId: PEER_A });

  assertRejects(() => assertRouteCompatibleWithPeerCollection(selected, peers), "connected");
});

test("selected peers route rejects any unknown or non-routeable target", () => {
  const peers = collection([peer(PEER_A), peer(PEER_B, { liveness: "left" })]);
  const withUnknown = route({
    kind: "selected_peers",
    peerSessionIds: [PEER_A, bridgePeerSessionId("peer-session:unknown")],
  });
  const withLeftPeer = route({ kind: "selected_peers", peerSessionIds: [PEER_A, PEER_B] });
  const duplicateRoute = validateBridgeRoute({
    bridgeSessionId: BRIDGE_SESSION,
    target: { kind: "selected_peers", peerSessionIds: [PEER_A, PEER_A] },
  });

  assertRejects(() => assertRouteCompatibleWithPeerCollection(withUnknown, peers), "known current-session peer");
  assertRejects(() => assertRouteCompatibleWithPeerCollection(withLeftPeer, peers), "connected");
  assert.equal(duplicateRoute.valid, false);
});

test("broadcast resolves only routeable remote peers", () => {
  const peers = collection([
    peer(PEER_A),
    peer(PEER_B, { liveness: "disconnected" }),
    peer(PEER_C),
    peer(LOCAL_PEER, { isLocalSelf: true }),
  ]);
  const broadcast = route({ kind: "broadcast_bridge", explicit: true });

  assert.deepEqual(resolveBridgeRoutePeerIds(broadcast, peers), [PEER_A, PEER_C]);
});

test("Agent Bridge capability route requires exact selected peer", () => {
  const peers = collection([peer(PEER_A), peer(PEER_B)]);
  const selected = route({ kind: "selected_peer", peerSessionId: PEER_A });
  const selectedPeers = route({ kind: "selected_peers", peerSessionIds: [PEER_A, PEER_B] });
  const broadcast = route({ kind: "broadcast_bridge", explicit: true });

  assert.doesNotThrow(() =>
    assertRouteCompatibleWithPeerCollection(selected, peers, { contentKind: "agent_bridge_capability_event" })
  );
  assertRejects(
    () => assertRouteCompatibleWithPeerCollection(selectedPeers, peers, {
      contentKind: "agent_bridge_capability_event",
    }),
    "requires exactly one selected peer",
  );
  assertRejects(
    () => assertRouteCompatibleWithPeerCollection(broadcast, peers, {
      contentKind: "agent_bridge_capability_event",
    }),
    "requires exactly one selected peer",
  );
});

test("default route derives selected peer only for one routeable remote peer", () => {
  const onePeer = deriveDefaultBridgeRouteForCurrentSession(collection([peer(PEER_A)]));
  assert.equal(onePeer.status, "selected_peer");
  if (onePeer.status === "selected_peer") {
    assert.equal(onePeer.route.target.kind, "selected_peer");
    assert.deepEqual(onePeer.routeablePeerIds, [PEER_A]);
  }

  const noPeer = deriveDefaultBridgeRouteForCurrentSession(collection([
    peer(PEER_A, { liveness: "disconnected" }),
  ]));
  assert.equal(noPeer.status, "no_route");

  const multiplePeers = deriveDefaultBridgeRouteForCurrentSession(collection([peer(PEER_A), peer(PEER_B)]));
  assert.equal(multiplePeers.status, "requires_explicit_selection");
  assert.deepEqual(multiplePeers.routeablePeerIds, [PEER_A, PEER_B]);
});

test("route validation does not create consent, trust, authority, or history", () => {
  const peers = collection([peer(PEER_A, { joinMethod: "manual_code" as BridgePeerJoinMethod })]);
  const selected = route({ kind: "selected_peer", peerSessionId: PEER_A });
  assertRouteCompatibleWithPeerCollection(selected, peers);

  const serialized = JSON.stringify({ peers, selected });
  assert.equal(serialized.includes("consent"), false);
  assert.equal(serialized.includes("trust"), false);
  assert.equal(serialized.includes("authority"), false);
  assert.equal(serialized.includes("history"), false);

  const withConsent = validateBridgePeerCollection({
    bridgeSessionId: BRIDGE_SESSION,
    peers: [peer(PEER_A)],
    consentId: "not-membership",
  });
  assert.equal(withConsent.valid, false);
});

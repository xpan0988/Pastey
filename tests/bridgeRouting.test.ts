import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_BRIDGE_ROUTING_POLICIES,
  assertRouteAllowedForContentKind,
  bridgePeerSessionId,
  getExplicitTargetPeerIds,
  isBroadcastRoute,
  normalizeBridgeTarget,
  validateBridgeRoute,
  type BridgeRoute,
  type BridgeRoutingPolicy,
} from "../src/lib/bridgeRouting";

const BRIDGE_SESSION = "bridge-session:current";
const PEER_A = bridgePeerSessionId("peer-session:a");
const PEER_B = bridgePeerSessionId("peer-session:b");
const PEER_C = bridgePeerSessionId("peer-session:c");
const ACCEPTED_PEERS = [PEER_A, PEER_B, PEER_C] as const;

function route(target: unknown): BridgeRoute {
  const result = validateBridgeRoute(
    { bridgeSessionId: BRIDGE_SESSION, target },
    { acceptedPeerSessionIds: ACCEPTED_PEERS },
  );
  assert.equal(result.valid, true, result.valid ? "" : result.errors.join(" "));
  if (!result.valid) throw new Error("Expected valid route.");
  return result.route;
}

function assertPolicyRejects(fn: () => void, expected: string): void {
  assert.throws(fn, (error) => {
    assert.equal(error instanceof Error, true);
    assert.match(String((error as Error).message), new RegExp(expected));
    return true;
  });
}

test("selected peer route is valid", () => {
  const selectedPeer = route({ kind: "selected_peer", peerSessionId: PEER_A });

  assert.equal(selectedPeer.bridgeSessionId, BRIDGE_SESSION);
  assert.deepEqual(getExplicitTargetPeerIds(selectedPeer), [PEER_A]);
  assert.equal(isBroadcastRoute(selectedPeer), false);
});

test("selected peers rejects empty list", () => {
  const result = normalizeBridgeTarget({ kind: "selected_peers", peerSessionIds: [] });

  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.ok(result.errors.some((error) => error.includes("two or more")));
});

test("selected peers rejects duplicates", () => {
  const result = normalizeBridgeTarget({ kind: "selected_peers", peerSessionIds: [PEER_A, PEER_A] });

  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.ok(result.errors.some((error) => error.includes("duplicate")));
});

test("broadcast route is explicit and not missing target", () => {
  const missingTarget = validateBridgeRoute({ bridgeSessionId: BRIDGE_SESSION }, {
    acceptedPeerSessionIds: ACCEPTED_PEERS,
  });
  assert.equal(missingTarget.valid, false);

  const implicitBroadcast = normalizeBridgeTarget({ kind: "broadcast_bridge" });
  assert.equal(implicitBroadcast.ok, false);
  if (!implicitBroadcast.ok) {
    assert.ok(implicitBroadcast.errors.some((error) => error.includes("explicit")));
  }

  const broadcast = route({ kind: "broadcast_bridge", explicit: true });
  assert.equal(isBroadcastRoute(broadcast), true);
  assert.deepEqual(getExplicitTargetPeerIds(broadcast), []);
});

test("text allows broadcast", () => {
  const broadcast = route({ kind: "broadcast_bridge", explicit: true });

  assert.doesNotThrow(() => assertRouteAllowedForContentKind(broadcast, "text"));
});

test("file, image, and pasted image broadcast requires explicit policy", () => {
  const broadcast = route({ kind: "broadcast_bridge", explicit: true });
  const explicitFileBroadcastPolicy: BridgeRoutingPolicy = {
    ...DEFAULT_BRIDGE_ROUTING_POLICIES.file,
    allowBroadcast: true,
  };
  const explicitImageBroadcastPolicy: BridgeRoutingPolicy = {
    ...DEFAULT_BRIDGE_ROUTING_POLICIES.image,
    allowBroadcast: true,
  };
  const explicitPastedImageBroadcastPolicy: BridgeRoutingPolicy = {
    ...DEFAULT_BRIDGE_ROUTING_POLICIES.pasted_image,
    allowBroadcast: true,
  };

  assertPolicyRejects(() => assertRouteAllowedForContentKind(broadcast, "file"), "does not allow broadcast");
  assertPolicyRejects(() => assertRouteAllowedForContentKind(broadcast, "image"), "does not allow broadcast");
  assertPolicyRejects(() => assertRouteAllowedForContentKind(broadcast, "pasted_image"), "does not allow broadcast");
  assert.doesNotThrow(() => assertRouteAllowedForContentKind(broadcast, "file", explicitFileBroadcastPolicy));
  assert.doesNotThrow(() => assertRouteAllowedForContentKind(broadcast, "image", explicitImageBroadcastPolicy));
  assert.doesNotThrow(() =>
    assertRouteAllowedForContentKind(broadcast, "pasted_image", explicitPastedImageBroadcastPolicy)
  );
});

test("bridge control event rejects broadcast", () => {
  const broadcast = route({ kind: "broadcast_bridge", explicit: true });

  assertPolicyRejects(
    () => assertRouteAllowedForContentKind(broadcast, "bridge_control_event"),
    "does not allow broadcast",
  );
});

test("Agent Bridge capability event rejects broadcast", () => {
  const broadcast = route({ kind: "broadcast_bridge", explicit: true });

  assertPolicyRejects(
    () => assertRouteAllowedForContentKind(broadcast, "agent_bridge_capability_event"),
    "requires exactly one selected peer",
  );
});

test("capability route requires exact selected peer", () => {
  const selectedPeer = route({ kind: "selected_peer", peerSessionId: PEER_A });
  const selectedPeers = route({ kind: "selected_peers", peerSessionIds: [PEER_A, PEER_B] });

  assert.doesNotThrow(() => assertRouteAllowedForContentKind(selectedPeer, "agent_bridge_capability_event"));
  assertPolicyRejects(
    () => assertRouteAllowedForContentKind(selectedPeers, "agent_bridge_capability_event"),
    "requires exactly one selected peer",
  );
});

test("route validation does not create consent, trust, or authority", () => {
  const selectedPeer = route({ kind: "selected_peer", peerSessionId: PEER_A });
  const serialized = JSON.stringify(selectedPeer);

  assert.equal(serialized.includes("consent"), false);
  assert.equal(serialized.includes("trust"), false);
  assert.equal(serialized.includes("authority"), false);
  assert.equal(serialized.includes("history"), false);
  assert.equal(serialized.includes("identity"), false);

  const withAuthority = validateBridgeRoute({
    bridgeSessionId: BRIDGE_SESSION,
    target: { kind: "selected_peer", peerSessionId: PEER_A },
    consentId: "not-a-route-field",
  });
  assert.equal(withAuthority.valid, false);
  if (!withAuthority.valid) {
    assert.ok(withAuthority.errors.some((error) => error.includes("routing is not consent")));
  }
});

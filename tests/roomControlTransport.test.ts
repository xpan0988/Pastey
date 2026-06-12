import assert from "node:assert/strict";
import test from "node:test";

import {
  buildCapabilityRequestPreviewEnvelope,
  buildHelloPeerRequestFromPendingAction,
  buildMockAiContextSnapshot,
  buildMockHelloPeerPlan,
  confirmPendingAiAction,
  createPendingAiAction,
  evaluateAiPolicy,
  validateCapabilityRequestPreviewEnvelope,
  validateHelloPeerRequest,
} from "../src/lib/ai";
import {
  buildSessionBoundCapabilityPreviewControlEvent,
  validateRoomControlEvent,
} from "../src/lib/agentBridge";

const NOW = new Date("2026-06-12T00:00:00.000Z");

function envelope() {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = createPendingAiAction(plan, policy, {
    now: NOW,
    ttlMs: 120_000,
    pendingId: "transport-pending",
  });
  const confirmed = confirmPendingAiAction(pending, NOW);
  const request = buildHelloPeerRequestFromPendingAction(confirmed, {
    now: NOW,
    requestId: "transport-request",
    nonce: "transport-nonce",
  });
  assert.equal(request.ok, true);
  if (!request.ok) throw new Error("Expected request.");
  const result = buildCapabilityRequestPreviewEnvelope(request.request, {
    roomRef: "mock-room",
    now: NOW,
    envelopeId: "transport-envelope",
  });
  assert.equal(result.ok, true);
  if (!result.ok) throw new Error("Expected envelope.");
  return result.envelope;
}

test("rebinds a preview event to current room-session refs", () => {
  const result = buildSessionBoundCapabilityPreviewControlEvent(envelope(), {
    roomId: "active-room",
    localSessionRef: "room-session:local",
    peerSessionRef: "room-session:peer",
    peerConnected: true,
  }, { now: NOW });
  assert.equal(result.ok, true);
  if (!result.ok || result.event.kind !== "capability_preview") return;
  assert.equal(result.event.roomRef, "active-room");
  assert.equal(result.event.sourceDeviceRef, "room-session:local");
  assert.equal(result.event.targetPeerRef, "room-session:peer");
  assert.equal(result.event.payload.request.sourceDeviceRef, "room-session:local");
  assert.equal(result.event.payload.request.targetPeerRef, "room-session:peer");
  assert.equal(validateHelloPeerRequest(result.event.payload.request, { now: NOW }).valid, true);
  assert.equal(validateCapabilityRequestPreviewEnvelope(result.event.payload, {
    now: NOW,
    expectedRoomRef: "active-room",
    expectedTargetPeerRef: "room-session:peer",
  }).valid, true);
  assert.equal(validateRoomControlEvent(result.event, {
    now: NOW,
    expectedRoomRef: "active-room",
    expectedSourceDeviceRef: "room-session:local",
    expectedTargetPeerRef: "room-session:peer",
  }).valid, true);
});

test("session-bound transport event remains preview-only without execution result fields", () => {
  const result = buildSessionBoundCapabilityPreviewControlEvent(envelope(), {
    roomId: "active-room",
    localSessionRef: "room-session:local",
    peerSessionRef: "room-session:peer",
    peerConnected: true,
  }, { now: NOW });
  assert.equal(result.ok, true);
  const serialized = JSON.stringify(result);
  assert.doesNotMatch(serialized, /stdout|stderr|exitCode/);
  assert.match(serialized, /previewOnly/);
});

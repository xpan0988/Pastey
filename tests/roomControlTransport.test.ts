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
  computeControlLaneBudget,
  createControlQueueState,
  createIdleRoomControlSendState,
  mapRoomControlSendError,
  preserveRoomControlSendStateForSession,
  sendCurrentRoomControlEvent,
  validateRoomControlEvent,
  type RoomControlEvent,
  type RoomControlSendState,
} from "../src/lib/agentBridge";
import type { RoomControlDeliveryReceipt, RoomControlSessionContext } from "../src/lib/types";

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

const SESSION: RoomControlSessionContext = {
  roomId: "active-room",
  localSessionRef: "room-session:local",
  peerSessionRef: "room-session:peer",
  peerConnected: true,
};

function transportEvent(): RoomControlEvent {
  const result = buildSessionBoundCapabilityPreviewControlEvent(envelope(), SESSION, { now: NOW });
  assert.equal(result.ok, true);
  if (!result.ok) throw new Error("Expected transport event.");
  return result.event;
}

function acceptedReceipt(eventId: string): RoomControlDeliveryReceipt {
  return {
    schemaVersion: "pastey-room-control-delivery-v1",
    eventId,
    acceptedForLocalInbox: true,
    receivedAt: "2026-06-12T00:00:01.000Z",
  };
}

test("rebinds a preview event to current room-session refs", () => {
  const result = buildSessionBoundCapabilityPreviewControlEvent(envelope(), SESSION, { now: NOW });
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
  const result = buildSessionBoundCapabilityPreviewControlEvent(envelope(), SESSION, { now: NOW });
  assert.equal(result.ok, true);
  const serialized = JSON.stringify(result);
  assert.doesNotMatch(serialized, /stdout|stderr|exitCode/);
  assert.match(serialized, /previewOnly/);
});

test("send emits sending immediately and then accepted", async () => {
  const event = transportEvent();
  const states: RoomControlSendState[] = [];
  const result = await sendCurrentRoomControlEvent(
    event,
    async () => acceptedReceipt(event.eventId),
    (state) => states.push(state),
    () => NOW,
  );
  assert.deepEqual(states.map((state) => state.status), ["sending", "accepted"]);
  assert.equal(states[0]?.status === "sending" ? states[0].eventId : "", event.eventId);
  assert.equal(result.status, "accepted");
});

test("duplicate/replay, expiry, peer unavailable, and malformed receipt are visible rejected states", () => {
  assert.equal(mapRoomControlSendError({ code: "replay" }, "event", NOW).errorCode, "replay");
  assert.equal(mapRoomControlSendError({ code: "expired" }, "event", NOW).errorCode, "expired");
  assert.equal(mapRoomControlSendError({ code: "peer_unavailable" }, "event", NOW).errorCode, "peer_unavailable");
  assert.equal(mapRoomControlSendError({ code: "malformed_receipt" }, "event", NOW).errorCode, "malformed_receipt");
});

test("malformed receipt produces one visible terminal failure", async () => {
  const event = transportEvent();
  const states: RoomControlSendState[] = [];
  const result = await sendCurrentRoomControlEvent(
    event,
    async () => ({ ...acceptedReceipt(event.eventId), eventId: "wrong-event" }),
    (state) => states.push(state),
    () => NOW,
  );
  assert.deepEqual(states.map((state) => state.status), ["sending", "rejected"]);
  assert.equal(result.status === "rejected" ? result.errorCode : "", "malformed_receipt");
});

test("latest result survives inbox and active-room refresh when session is unchanged", () => {
  const accepted: RoomControlSendState = {
    status: "accepted",
    eventId: "event",
    receivedAt: NOW.toISOString(),
    acceptedForLocalInbox: true,
  };
  const refreshedSession = { ...SESSION, peerConnected: false };
  assert.deepEqual(preserveRoomControlSendStateForSession(accepted, SESSION, SESSION), accepted);
  assert.deepEqual(preserveRoomControlSendStateForSession(accepted, SESSION, refreshedSession), accepted);
});

test("latest result clears when room or session identity changes", () => {
  const rejected = mapRoomControlSendError({ code: "replay" }, "event", NOW);
  assert.deepEqual(
    preserveRoomControlSendStateForSession(rejected, SESSION, {
      ...SESSION,
      peerSessionRef: "room-session:new-peer",
    }),
    createIdleRoomControlSendState(),
  );
});

test("repeated clicks send the same event ID with no automatic retry", async () => {
  const event = transportEvent();
  const sentIds: string[] = [];
  const sender = async (sent: RoomControlEvent) => {
    sentIds.push(sent.eventId);
    if (sentIds.length === 2) throw { code: "replay" };
    return acceptedReceipt(sent.eventId);
  };
  await sendCurrentRoomControlEvent(event, sender, () => {}, () => NOW);
  const second = await sendCurrentRoomControlEvent(event, sender, () => {}, () => NOW);
  assert.deepEqual(sentIds, [event.eventId, event.eventId]);
  assert.equal(second.status, "rejected");
  assert.equal(sentIds.length, 2);
});

test("send observability does not mutate local queues or scheduler budgets", async () => {
  const event = transportEvent();
  const queue = createControlQueueState();
  const queueBefore = JSON.stringify(queue);
  const budgetBefore = computeControlLaneBudget({ controlBacklog: false });
  await sendCurrentRoomControlEvent(
    event,
    async () => acceptedReceipt(event.eventId),
    () => {},
    () => NOW,
  );
  assert.equal(JSON.stringify(queue), queueBefore);
  assert.deepEqual(computeControlLaneBudget({ controlBacklog: false }), budgetBefore);
});

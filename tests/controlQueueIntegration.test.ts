import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  buildCapabilityRequestPreviewEnvelope,
  buildHelloPeerRequestFromPendingAction,
  buildMockAiContextSnapshot,
  buildMockHelloPeerPlan,
  confirmPendingAiAction,
  createPendingAiAction,
  evaluateAiPolicy,
} from "../src/lib/ai";
import {
  buildCapabilityPreviewStatusControlEvent,
  buildSessionBoundCapabilityPreviewControlEvent,
  createControlQueueState,
  enqueueInboundRoomControlEvents,
  enqueueRoomControlEvent,
  getControlQueueBudget,
  preserveControlQueueForSession,
  processNextControlQueueItem,
  selectNextControlQueueItem,
  type CapabilityPreviewControlStatus,
  type CapabilityPreviewRoomControlEvent,
  type ControlQueueState,
  type RoomControlEvent,
} from "../src/lib/agentBridge";
import type { RoomControlDeliveryReceipt, RoomControlSessionContext } from "../src/lib/types";

const NOW = new Date("2026-06-12T00:00:00.000Z");
const LATER = new Date("2026-06-12T00:00:01.000Z");
const SESSION: RoomControlSessionContext = {
  roomId: "room-1",
  localSessionRef: "room-session:local",
  peerSessionRef: "room-session:peer",
  peerConnected: true,
};

function previewEvent(eventId = "preview-event"): CapabilityPreviewRoomControlEvent {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = confirmPendingAiAction(
    createPendingAiAction(plan, policy, {
      now: new Date("2026-06-11T23:59:00.000Z"),
      ttlMs: 900_000,
      pendingId: `pending-${eventId}`,
    }),
    new Date("2026-06-11T23:59:30.000Z"),
  );
  const request = buildHelloPeerRequestFromPendingAction(pending, {
    now: NOW,
    ttlMs: 600_000,
    requestId: `request-${eventId}`,
    nonce: `nonce-${eventId}`,
    sourceDeviceRef: SESSION.localSessionRef,
    targetPeerRef: SESSION.peerSessionRef,
  });
  assert.equal(request.ok, true);
  if (!request.ok) throw new Error("Expected request.");
  const envelope = buildCapabilityRequestPreviewEnvelope(request.request, {
    roomRef: SESSION.roomId,
    now: NOW,
    ttlMs: 600_000,
    envelopeId: `envelope-${eventId}`,
  });
  assert.equal(envelope.ok, true);
  if (!envelope.ok) throw new Error("Expected envelope.");
  const result = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, SESSION, {
    now: NOW,
  });
  assert.equal(result.ok, true, result.ok ? "" : result.errors.join(" "));
  if (!result.ok || result.event.kind !== "capability_preview") throw new Error("Expected preview.");
  return { ...result.event, eventId };
}

function inboundPreview(eventId = "inbound-preview"): CapabilityPreviewRoomControlEvent {
  const event = previewEvent(eventId);
  const result = buildSessionBoundCapabilityPreviewControlEvent(event.payload, {
    roomId: SESSION.roomId,
    localSessionRef: SESSION.peerSessionRef,
    peerSessionRef: SESSION.localSessionRef,
    peerConnected: true,
  }, {
    now: NOW,
  });
  assert.equal(result.ok, true, result.ok ? "" : result.errors.join(" "));
  if (!result.ok || result.event.kind !== "capability_preview") {
    throw new Error("Expected inbound preview.");
  }
  return {
    ...result.event,
    eventId,
  };
}

function statusEvent(
  source: CapabilityPreviewRoomControlEvent,
  status: CapabilityPreviewControlStatus,
  eventId: string,
): RoomControlEvent {
  const result = buildCapabilityPreviewStatusControlEvent(source, status, {
    eventId,
    now: NOW,
    ttlMs: 300_000,
    sourceDeviceRef: SESSION.peerSessionRef,
    targetPeerRef: SESSION.localSessionRef,
  });
  assert.equal(result.ok, true);
  if (!result.ok) throw new Error("Expected status.");
  return result.event;
}

function enqueueOutbound(): ControlQueueState {
  const result = enqueueRoomControlEvent(createControlQueueState(), previewEvent(), "outbound", {
    now: NOW,
    queueId: "outbound-queue",
  });
  assert.equal(result.ok, true);
  return result.state;
}

function receipt(eventId: string): RoomControlDeliveryReceipt {
  return {
    schemaVersion: "pastey-room-control-delivery-v1",
    eventId,
    acceptedForLocalInbox: true,
    receivedAt: LATER.toISOString(),
  };
}

test("outbound queue item enters transport sending then delivered without acknowledgement", async () => {
  const states: ControlQueueState[] = [];
  const result = await processNextControlQueueItem(
    enqueueOutbound(),
    async (event) => receipt(event.eventId),
    { now: () => LATER, onState: (state) => states.push(state) },
  );
  assert.equal(result.ok, true);
  assert.deepEqual(
    states.map((state) => state.outbound[0]?.status),
    ["selected", "transport_sending", "transport_delivered"],
  );
  assert.equal(result.item.status, "transport_delivered");
  assert.notEqual(result.item.status, "acknowledged_preview_only");
  assert.equal(result.item.transportResultCode, "accepted_for_local_inbox");
});

test("replay, expiry, and network failures update the same outbound item without retry", async () => {
  for (const code of ["replay", "expired", "transport_error"] as const) {
    let calls = 0;
    const result = await processNextControlQueueItem(
      enqueueOutbound(),
      async () => {
        calls += 1;
        throw { code };
      },
      { now: () => LATER },
    );
    assert.equal(result.ok, false);
    assert.equal(result.item?.queueId, "outbound-queue");
    assert.equal(result.item?.status, "transport_rejected");
    assert.equal(result.item?.transportResultCode, code);
    assert.equal(calls, 1);
  }
});

test("duplicate outbound event is not silently queued twice", () => {
  const state = enqueueOutbound();
  const duplicate = enqueueRoomControlEvent(state, previewEvent(), "outbound", { now: NOW });
  assert.equal(duplicate.ok, false);
  assert.equal(duplicate.state.outbound.length, 1);
});

test("non-destructive Rust inbox refresh queues valid inbound event once", () => {
  const event = inboundPreview();
  const first = enqueueInboundRoomControlEvents(createControlQueueState(), [event], {
    now: NOW,
    expectedRoomRef: SESSION.roomId,
    expectedSourceDeviceRef: SESSION.peerSessionRef,
    expectedTargetPeerRef: SESSION.localSessionRef,
  });
  assert.equal(first.added.length, 1);
  const second = enqueueInboundRoomControlEvents(first.state, [event], {
    now: NOW,
    expectedRoomRef: SESSION.roomId,
    expectedSourceDeviceRef: SESSION.peerSessionRef,
    expectedTargetPeerRef: SESSION.localSessionRef,
  });
  assert.equal(second.added.length, 0);
  assert.equal(second.state.inbound.length, 1);
  assert.match(second.diagnostics.join(" "), /duplicate/i);
});

test("invalid and expired inbound events fail closed", () => {
  const invalid = { ...inboundPreview("invalid"), targetPeerRef: "wrong-target" };
  const expired = { ...inboundPreview("expired"), expiresAt: "2026-06-11T23:59:59.000Z" };
  const result = enqueueInboundRoomControlEvents(createControlQueueState(), [invalid, expired], {
    now: NOW,
    expectedRoomRef: SESSION.roomId,
    expectedSourceDeviceRef: SESSION.peerSessionRef,
    expectedTargetPeerRef: SESSION.localSessionRef,
  });
  assert.equal(result.added.length, 0);
  assert.equal(result.state.inbound.length, 0);
  assert.match(result.diagnostics.join(" "), /(target|expired)/i);
});

test("real inbound priority remains deny then ack then outbound preview with FIFO ties", () => {
  const source = inboundPreview("source");
  let state = enqueueOutbound();
  for (const [event, queueId] of [
    [statusEvent(source, "acknowledged_preview_only", "ack-1"), "ack-1"],
    [statusEvent(source, "acknowledged_preview_only", "ack-2"), "ack-2"],
    [statusEvent(source, "denied", "deny"), "deny"],
  ] as const) {
    const enqueue = enqueueRoomControlEvent(state, event, "inbound", { now: NOW, queueId });
    assert.equal(enqueue.ok, true);
    state = enqueue.state;
  }
  const selected = selectNextControlQueueItem(state, { now: LATER });
  assert.equal(selected.ok, true);
  assert.equal(selected.item.queueId, "deny");

  const withoutDeny: ControlQueueState = {
    ...state,
    inbound: state.inbound.filter((item) => item.queueId !== "deny"),
  };
  const selectedAck = selectNextControlQueueItem(withoutDeny, { now: LATER });
  assert.equal(selectedAck.ok, true);
  assert.equal(selectedAck.item.queueId, "ack-1");
});

test("queue survives unchanged session refresh and clears on session change", () => {
  const state = enqueueOutbound();
  assert.equal(preserveControlQueueForSession(state, SESSION, { ...SESSION }), state);
  const cleared = preserveControlQueueForSession(state, SESSION, {
    ...SESSION,
    peerSessionRef: "room-session:new-peer",
  });
  assert.deepEqual(cleared, createControlQueueState());
});

test("legacy local queue budget helper remains independent from runtime demand classification", () => {
  assert.deepEqual(getControlQueueBudget(createControlQueueState(), { now: NOW }), {
    totalWindows: 8,
    dataWindows: 8,
    controlWindows: 0,
    controlBacklog: false,
  });
  assert.deepEqual(getControlQueueBudget(enqueueOutbound(), { now: NOW }), {
    totalWindows: 8,
    dataWindows: 7,
    controlWindows: 1,
    controlBacklog: true,
  });
});

test("receiver room-control queue UI is not gated by an outbound advisory plan", () => {
  const source = readFileSync("src/components/AiSlotPreview.tsx", "utf8");
  const panel = source.indexOf("<RoomControlPanel");
  const advisoryResultGate = source.indexOf("{result ? (");
  assert.ok(panel >= 0);
  assert.match(source, /onEnqueueCandidatePayloadHandoff=\{onEnqueueCandidatePayloadHandoff\}/);
  assert.ok(advisoryResultGate >= 0);
  assert.ok(panel < advisoryResultGate);
});

test("Agent Bridge UI uses compact defaults and collapsed advanced diagnostics", () => {
  const source = readFileSync("src/components/AiSlotPreview.tsx", "utf8");
  const roomControl = readFileSync(
    "src/components/agentBridge/RoomControlPanel.tsx",
    "utf8",
  );
  const advanced = readFileSync(
    "src/components/agentBridge/AgentBridgeAdvancedDiagnostics.tsx",
    "utf8",
  );
  assert.match(source, /<strong>Agent Bridge<\/strong>/);
  assert.match(source, /Preview-only room control\. Delivery is not consent\./);
  assert.match(source, /Allow once and execution request remain explicit/);
  assert.match(roomControl, /data-testid="agent-bridge-queue-summary"/);
  assert.match(roomControl, /data-testid="agent-bridge-latest-send"/);
  assert.match(roomControl, /data-testid="agent-bridge-refresh-inbox"/);
  assert.match(roomControl, /data-testid="agent-bridge-process-next"/);
  assert.match(roomControl, /data-testid="agent-bridge-runtime-window-status"/);
  assert.match(roomControl, /Runtime scheduler reservation/);
  assert.match(advanced, /data-testid="agent-bridge-advanced-diagnostics"/);
  assert.doesNotMatch(advanced, /<details[^>]*\sopen/);
});

test("detailed safety and local simulation tools remain under disclosures", () => {
  const roomControl = readFileSync(
    "src/components/agentBridge/RoomControlPanel.tsx",
    "utf8",
  );
  const advanced = readFileSync(
    "src/components/agentBridge/AgentBridgeAdvancedDiagnostics.tsx",
    "utf8",
  );
  const roomDetails = roomControl.indexOf('<details className="agent-bridge-room-details">');
  const simulation = roomControl.indexOf("<strong>Local simulation only</strong>");
  assert.ok(roomDetails >= 0);
  assert.ok(simulation > roomDetails);
  assert.match(advanced, /Provider output is untrusted/);
  assert.match(advanced, /Trusted room membership and preview acknowledgement are not execution authorization/);
});

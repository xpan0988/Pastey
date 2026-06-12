import assert from "node:assert/strict";
import test from "node:test";

import {
  buildCapabilityRequestPreviewEnvelope,
  buildHelloPeerRequestFromPendingAction,
  buildMockAiContextSnapshot,
  buildMockHelloPeerPlan,
  confirmPendingAiAction,
  createPendingAiAction,
  evaluateAiPolicy
} from "../src/lib/ai";
import {
  buildCapabilityPreviewControlEvent,
  buildCapabilityPreviewStatusControlEvent,
  checkAndRecordRoomControlEvent,
  computeControlLaneBudget,
  createRoomControlEventSessionState,
  validateRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type RoomControlEvent
} from "../src/lib/agentBridge";

const NOW = new Date("2026-06-11T00:01:00.000Z");

function deterministicCapabilityPreviewEnvelope() {
  const pendingAt = new Date("2026-06-11T00:00:00.000Z");
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = createPendingAiAction(plan, policy, {
    now: pendingAt,
    ttlMs: 180_000,
    pendingId: "room-control-pending"
  });
  const confirmed = confirmPendingAiAction(pending, new Date("2026-06-11T00:00:30.000Z"));
  const requestResult = buildHelloPeerRequestFromPendingAction(confirmed, {
    now: NOW,
    ttlMs: 120_000,
    requestId: "room-control-request",
    nonce: "room-control-nonce",
    sourceDeviceRef: "room-control-source"
  });
  assert.equal(requestResult.ok, true);
  if (!requestResult.ok) throw new Error("Expected deterministic Hello Peer request.");

  const envelopeResult = buildCapabilityRequestPreviewEnvelope(requestResult.request, {
    roomRef: "room-control-room",
    now: NOW,
    ttlMs: 120_000,
    envelopeId: "room-control-envelope"
  });
  assert.equal(envelopeResult.ok, true);
  if (!envelopeResult.ok) throw new Error("Expected deterministic capability preview envelope.");
  return envelopeResult.envelope;
}

function deterministicPreviewControlEvent(
  eventId = "room-control-event-preview",
  envelope = deterministicCapabilityPreviewEnvelope()
): CapabilityPreviewRoomControlEvent {
  const result = buildCapabilityPreviewControlEvent(envelope, {
    roomRef: envelope.roomRef,
    now: NOW,
    ttlMs: 60_000,
    eventId
  });
  assert.equal(result.ok, true);
  if (!result.ok || result.event.kind !== "capability_preview") {
    throw new Error("Expected deterministic capability preview control event.");
  }
  return result.event;
}

test("safe capability preview envelope builds and validates a RoomControlEvent", () => {
  const event = deterministicPreviewControlEvent();

  assert.equal(event.schemaVersion, "pastey-room-control-event/v1");
  assert.equal(event.kind, "capability_preview");
  assert.equal(event.previewOnly, true);
  assert.equal(event.payload.envelopeId, "room-control-envelope");
  assert.equal(validateRoomControlEvent(event, {
    now: NOW,
    expectedRoomRef: "room-control-room",
    expectedSourceDeviceRef: "room-control-source",
    expectedTargetPeerRef: "mock-peer-1"
  }).valid, true);
});

for (const [status, kind] of [
  ["acknowledged_preview_only", "capability_preview_ack"],
  ["denied", "capability_preview_deny"],
  ["invalid", "capability_preview_invalid"],
  ["expired", "capability_preview_expired"]
] as const) {
  test(`${kind} builds and validates as bounded preview-only status`, () => {
    const result = buildCapabilityPreviewStatusControlEvent(
      deterministicPreviewControlEvent(),
      status,
      {
        now: new Date("2026-06-11T00:01:10.000Z"),
        ttlMs: 30_000,
        eventId: `room-control-event-${status}`,
        reason: status === "acknowledged_preview_only" ? undefined : "bounded status reason"
      }
    );

    assert.equal(result.ok, true);
    if (!result.ok) return;
    assert.equal(result.event.kind, kind);
    assert.equal(result.event.payload.status, status);
    assert.equal("stdout" in result.event.payload, false);
    assert.equal("stderr" in result.event.payload, false);
    assert.equal("exitCode" in result.event.payload, false);
    assert.equal(validateRoomControlEvent(result.event, {
      now: new Date("2026-06-11T00:01:10.000Z")
    }).valid, true);
  });
}

test("unknown kind and previewOnly false are rejected", () => {
  const event = deterministicPreviewControlEvent();

  assert.equal(validateRoomControlEvent({ ...event, kind: "capability_request" }, { now: NOW }).valid, false);
  assert.equal(validateRoomControlEvent({ ...event, previewOnly: false }, { now: NOW }).valid, false);
});

test("wrong schema and missing event fields are rejected", () => {
  const event = deterministicPreviewControlEvent();
  assert.equal(validateRoomControlEvent({
    ...event,
    schemaVersion: "pastey-room-control-event/v2"
  }, { now: NOW }).valid, false);

  const missingEventId = structuredClone(event) as unknown as Record<string, unknown>;
  delete missingEventId.eventId;
  assert.equal(validateRoomControlEvent(missingEventId, { now: NOW }).valid, false);
});

for (const unsafeField of ["command", "code", "path", "shell", "stdout", "stderr", "exitCode"]) {
  test(`RoomControlEvent rejects unsafe field ${unsafeField}`, () => {
    const event = structuredClone(deterministicPreviewControlEvent()) as unknown as Record<string, unknown>;
    event[unsafeField] = unsafeField === "exitCode" ? 0 : "unsafe";

    const validation = validateRoomControlEvent(event, { now: NOW });
    assert.equal(validation.valid, false);
    assert.ok(validation.errors.some((error) => error.includes("Unsafe or execution-like field")));
  });
}

test("invalid embedded capability preview envelope is rejected", () => {
  const event = structuredClone(deterministicPreviewControlEvent()) as unknown as Record<string, unknown>;
  const payload = event.payload as Record<string, unknown>;
  payload.previewOnly = false;

  const validation = validateRoomControlEvent(event, { now: NOW });
  assert.equal(validation.valid, false);
  assert.ok(validation.errors.some((error) => error.includes("Embedded preview envelope")));
});

test("room, target, and source mismatch are rejected", () => {
  const event = deterministicPreviewControlEvent();

  assert.equal(validateRoomControlEvent(event, { now: NOW, expectedRoomRef: "wrong-room" }).valid, false);
  assert.equal(validateRoomControlEvent(event, { now: NOW, expectedTargetPeerRef: "wrong-peer" }).valid, false);
  assert.equal(validateRoomControlEvent(event, { now: NOW, expectedSourceDeviceRef: "wrong-source" }).valid, false);
});

test("current-session helper rejects duplicate event ID", () => {
  const event = deterministicPreviewControlEvent();
  const first = checkAndRecordRoomControlEvent(event, createRoomControlEventSessionState(), { now: NOW });
  assert.equal(first.ok, true);
  if (!first.ok) return;

  const duplicate = checkAndRecordRoomControlEvent(event, first.state, { now: NOW });
  assert.equal(duplicate.ok, false);
  if (duplicate.ok) return;
  assert.equal(duplicate.reason, "duplicate_event");
});

test("current-session helper rejects duplicate preview envelope ID", () => {
  const firstEvent = deterministicPreviewControlEvent();
  const secondEvent = deterministicPreviewControlEvent("room-control-event-second");
  const first = checkAndRecordRoomControlEvent(firstEvent, createRoomControlEventSessionState(), { now: NOW });
  assert.equal(first.ok, true);
  if (!first.ok) return;

  const duplicate = checkAndRecordRoomControlEvent(secondEvent, first.state, { now: NOW });
  assert.equal(duplicate.ok, false);
  if (duplicate.ok) return;
  assert.equal(duplicate.reason, "duplicate_envelope");
});

test("current-session helper rejects duplicate embedded request ID", () => {
  const firstEvent = deterministicPreviewControlEvent();
  const secondEnvelope = {
    ...deterministicCapabilityPreviewEnvelope(),
    envelopeId: "room-control-envelope-second"
  };
  const secondEvent = deterministicPreviewControlEvent("room-control-event-third", secondEnvelope);
  const first = checkAndRecordRoomControlEvent(firstEvent, createRoomControlEventSessionState(), { now: NOW });
  assert.equal(first.ok, true);
  if (!first.ok) return;

  const duplicate = checkAndRecordRoomControlEvent(secondEvent, first.state, { now: NOW });
  assert.equal(duplicate.ok, false);
  if (duplicate.ok) return;
  assert.equal(duplicate.reason, "duplicate_request");
});

test("expired RoomControlEvent is rejected by validator and current-session helper", () => {
  const event = deterministicPreviewControlEvent();
  const afterExpiry = new Date("2026-06-11T00:03:00.000Z");

  assert.equal(validateRoomControlEvent(event, { now: afterExpiry }).valid, false);
  const replay = checkAndRecordRoomControlEvent(event, createRoomControlEventSessionState(), { now: afterExpiry });
  assert.equal(replay.ok, false);
  if (replay.ok) return;
  assert.equal(replay.reason, "expired");
});

test("computeControlLaneBudget mirrors the 8-window feasibility model", () => {
  assert.deepEqual(computeControlLaneBudget({ controlBacklog: false }), {
    totalWindows: 8,
    controlWindows: 0,
    dataWindows: 8,
    controlBacklog: false
  });
  assert.deepEqual(computeControlLaneBudget({ controlBacklog: true }), {
    totalWindows: 8,
    controlWindows: 1,
    dataWindows: 7,
    controlBacklog: true
  });
});

test("computeControlLaneBudget clamps invalid totals to a positive integer", () => {
  assert.deepEqual(computeControlLaneBudget({ totalWindows: 0, controlBacklog: true }), {
    totalWindows: 1,
    controlWindows: 1,
    dataWindows: 0,
    controlBacklog: true
  });
  assert.equal(computeControlLaneBudget({ totalWindows: 4.9, controlBacklog: false }).totalWindows, 4);
});

test("status payload rejects execution-like and unsupported status fields", () => {
  const result = buildCapabilityPreviewStatusControlEvent(
    deterministicPreviewControlEvent(),
    "denied",
    {
      now: new Date("2026-06-11T00:01:10.000Z"),
      eventId: "room-control-event-denied"
    }
  );
  assert.equal(result.ok, true);
  if (!result.ok) return;

  const unsafe = structuredClone(result.event) as unknown as Record<string, unknown>;
  const unsafePayload = unsafe.payload as Record<string, unknown>;
  unsafePayload.stdout = "not allowed";
  assert.equal(validateRoomControlEvent(unsafe, {
    now: new Date("2026-06-11T00:01:10.000Z")
  }).valid, false);

  const wrongStatus = structuredClone(result.event) as unknown as Record<string, unknown>;
  const wrongStatusPayload = wrongStatus.payload as Record<string, unknown>;
  wrongStatusPayload.status = "acknowledged_preview_only";
  assert.equal(validateRoomControlEvent(wrongStatus, {
    now: new Date("2026-06-11T00:01:10.000Z")
  }).valid, false);
});

test("RoomControlEvent rejects oversized bounded status reason", () => {
  const result = buildCapabilityPreviewStatusControlEvent(
    deterministicPreviewControlEvent(),
    "denied",
    {
      now: new Date("2026-06-11T00:01:10.000Z"),
      eventId: "room-control-event-oversized-reason",
      reason: "x".repeat(513)
    }
  );

  assert.equal(result.ok, false);
});

test("RoomControlEvent remains a pure type-only value", () => {
  const event: RoomControlEvent = deterministicPreviewControlEvent();

  assert.equal("send" in event, false);
  assert.equal("execute" in event, false);
});

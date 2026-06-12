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
} from "../src/lib/ai/index.ts";
import {
  buildCapabilityPreviewControlEvent,
  buildCapabilityPreviewStatusControlEvent,
  createControlQueueState,
  enqueueRoomControlEvent,
  getControlQueueBudget,
  markControlQueueItemAcknowledged,
  markControlQueueItemDenied,
  selectNextControlQueueItem,
  type CapabilityPreviewControlStatus,
  type CapabilityPreviewRoomControlEvent,
  type RoomControlEvent,
} from "../src/lib/agentBridge/index.ts";

const NOW = new Date("2026-06-11T00:00:00.000Z");
const LATER = new Date("2026-06-11T00:01:00.000Z");

function previewEvent(
  overrides: Partial<CapabilityPreviewRoomControlEvent> = {},
): CapabilityPreviewRoomControlEvent {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = createPendingAiAction(plan, policy, {
    now: new Date("2026-06-10T23:59:00.000Z"),
    ttlMs: 900_000,
    pendingId: "queue-pending",
  });
  const confirmed = confirmPendingAiAction(
    pending,
    new Date("2026-06-10T23:59:30.000Z"),
  );
  const requestResult = buildHelloPeerRequestFromPendingAction(confirmed, {
    now: NOW,
    ttlMs: 600_000,
    requestId: "request-1",
    nonce: "queue-nonce",
    sourceDeviceRef: "queue-source",
  });
  assert.equal(requestResult.ok, true);
  if (!requestResult.ok) throw new Error("Expected request.");
  const envelopeResult = buildCapabilityRequestPreviewEnvelope(requestResult.request, {
    roomRef: "room-1",
    now: NOW,
    ttlMs: 600_000,
    envelopeId: "envelope-1",
  });
  assert.equal(envelopeResult.ok, true);
  if (!envelopeResult.ok) throw new Error("Expected envelope.");

  const result = buildCapabilityPreviewControlEvent(envelopeResult.envelope, {
    roomRef: "room-1",
    eventId: "event-preview-1",
    now: NOW,
    ttlMs: 300_000,
  });
  assert.equal(result.ok, true);
  if (!result.ok || result.event.kind !== "capability_preview") {
    throw new Error("Expected preview event.");
  }
  return { ...result.event, ...overrides };
}

function statusEvent(
  kind:
    | "capability_preview_ack"
    | "capability_preview_deny"
    | "capability_preview_invalid"
    | "capability_preview_expired",
  eventId: string,
  source = previewEvent(),
): RoomControlEvent {
  const status: CapabilityPreviewControlStatus = {
    capability_preview_ack: "acknowledged_preview_only",
    capability_preview_deny: "denied",
    capability_preview_invalid: "invalid",
    capability_preview_expired: "expired",
  }[kind];
  const result = buildCapabilityPreviewStatusControlEvent(source, status, {
    eventId,
    now: NOW,
    ttlMs: 300_000,
  });
  assert.equal(result.ok, true);
  return result.event;
}

test("enqueues safe outbound preview and computes future backlog budget", () => {
  const result = enqueueRoomControlEvent(
    createControlQueueState(),
    previewEvent(),
    "outbound",
    { now: NOW, queueId: "queue-1" },
  );
  assert.equal(result.ok, true);
  assert.equal(result.item.status, "queued");
  assert.equal(result.item.priority, 3);
  assert.deepEqual(getControlQueueBudget(result.state, { now: NOW }), {
    totalWindows: 8,
    dataWindows: 7,
    controlWindows: 1,
    controlBacklog: true,
  });
});

test("enqueues safe inbound ack and deny events", () => {
  let state = createControlQueueState();
  const ack = enqueueRoomControlEvent(
    state,
    statusEvent("capability_preview_ack", "event-ack"),
    "inbound",
    { now: NOW, queueId: "ack" },
  );
  assert.equal(ack.ok, true);
  state = ack.state;
  const deny = enqueueRoomControlEvent(
    state,
    statusEvent("capability_preview_deny", "event-deny"),
    "inbound",
    { now: NOW, queueId: "deny" },
  );
  assert.equal(deny.ok, true);
  assert.equal(ack.item.priority, 2);
  assert.equal(deny.item.priority, 1);
});

test("rejects unsafe fields through RoomControlEvent validation", () => {
  const unsafe = { ...previewEvent(), stdout: "secret" } as RoomControlEvent;
  const result = enqueueRoomControlEvent(createControlQueueState(), unsafe, "outbound", {
    now: NOW,
  });
  assert.equal(result.ok, false);
  assert.match(result.errors.join(" "), /stdout/i);
});

test("rejects duplicate eventId", () => {
  const first = enqueueRoomControlEvent(
    createControlQueueState(),
    previewEvent(),
    "outbound",
    { now: NOW },
  );
  assert.equal(first.ok, true);
  const duplicate = enqueueRoomControlEvent(first.state, previewEvent(), "outbound", {
    now: NOW,
  });
  assert.equal(duplicate.ok, false);
  assert.match(duplicate.errors.join(" "), /event ID/i);
});

test("rejects duplicate envelopeId and requestId for previews", () => {
  const first = enqueueRoomControlEvent(
    createControlQueueState(),
    previewEvent(),
    "outbound",
    { now: NOW },
  );
  assert.equal(first.ok, true);
  const duplicate = enqueueRoomControlEvent(
    first.state,
    previewEvent({ eventId: "event-preview-2" }),
    "outbound",
    { now: NOW },
  );
  assert.equal(duplicate.ok, false);
  assert.match(duplicate.errors.join(" "), /(envelope ID|request ID)/i);

  const requestDuplicateEvent = previewEvent({ eventId: "event-preview-3" });
  requestDuplicateEvent.payload = {
    ...requestDuplicateEvent.payload,
    envelopeId: "envelope-2",
  };
  const requestDuplicate = enqueueRoomControlEvent(
    first.state,
    requestDuplicateEvent,
    "outbound",
    { now: NOW },
  );
  assert.equal(requestDuplicate.ok, false);
  assert.match(requestDuplicate.errors.join(" "), /request ID/i);
});

test("rejects already expired events", () => {
  const expired = previewEvent({
    eventId: "expired-event",
    expiresAt: "2026-06-10T23:59:00.000Z",
  });
  const result = enqueueRoomControlEvent(
    createControlQueueState(),
    expired,
    "outbound",
    { now: NOW },
  );
  assert.equal(result.ok, false);
  assert.match(result.errors.join(" "), /expired/i);
});

test("selects inbound deny before inbound ack and outbound preview", () => {
  let state = createControlQueueState();
  for (const [event, direction, queueId] of [
    [previewEvent(), "outbound", "preview"],
    [statusEvent("capability_preview_ack", "ack-event"), "inbound", "ack"],
    [statusEvent("capability_preview_deny", "deny-event"), "inbound", "deny"],
  ] as const) {
    const result = enqueueRoomControlEvent(state, event, direction, {
      now: NOW,
      queueId,
    });
    assert.equal(result.ok, true);
    state = result.state;
  }
  const selected = selectNextControlQueueItem(state, { now: LATER });
  assert.equal(selected.ok, true);
  assert.equal(selected.item.queueId, "deny");
  assert.equal(selected.item.status, "selected");
});

test("selects inbound ack before outbound preview", () => {
  let state = createControlQueueState();
  const preview = enqueueRoomControlEvent(state, previewEvent(), "outbound", {
    now: NOW,
    queueId: "preview",
  });
  assert.equal(preview.ok, true);
  state = preview.state;
  const ack = enqueueRoomControlEvent(
    state,
    statusEvent("capability_preview_ack", "ack-event"),
    "inbound",
    { now: NOW, queueId: "ack" },
  );
  assert.equal(ack.ok, true);
  const selected = selectNextControlQueueItem(ack.state, { now: LATER });
  assert.equal(selected.ok, true);
  assert.equal(selected.item.queueId, "ack");
});

test("preserves FIFO order within a priority", () => {
  let state = createControlQueueState();
  const first = enqueueRoomControlEvent(
    state,
    statusEvent("capability_preview_ack", "ack-1"),
    "inbound",
    { now: NOW, queueId: "first" },
  );
  assert.equal(first.ok, true);
  state = first.state;
  const second = enqueueRoomControlEvent(
    state,
    statusEvent("capability_preview_ack", "ack-2"),
    "inbound",
    { now: LATER, queueId: "second" },
  );
  assert.equal(second.ok, true);
  const selected = selectNextControlQueueItem(second.state, {
    now: new Date("2026-06-11T00:02:00.000Z"),
  });
  assert.equal(selected.ok, true);
  assert.equal(selected.item.queueId, "first");
});

test("returns idle budget when no item is selectable", () => {
  const result = selectNextControlQueueItem(createControlQueueState(), { now: NOW });
  assert.equal(result.ok, false);
  assert.equal(result.reason, "no_selectable_control_item");
  assert.deepEqual(result.budget, {
    totalWindows: 8,
    dataWindows: 8,
    controlWindows: 0,
    controlBacklog: false,
  });
});

test("marks a queued item expired before local selection", () => {
  const queued = enqueueRoomControlEvent(
    createControlQueueState(),
    previewEvent({
      expiresAt: "2026-06-11T00:00:30.000Z",
    }),
    "outbound",
    { now: NOW, queueId: "expires-soon" },
  );
  assert.equal(queued.ok, true);
  const result = selectNextControlQueueItem(queued.state, { now: LATER });
  assert.equal(result.ok, false);
  assert.equal(result.state.outbound[0]?.status, "expired");
  assert.deepEqual(result.budget, {
    totalWindows: 8,
    dataWindows: 8,
    controlWindows: 0,
    controlBacklog: false,
  });
});

test("ack is preview-only and deny creates no retry or escalation", () => {
  const queued = enqueueRoomControlEvent(
    createControlQueueState(),
    previewEvent(),
    "outbound",
    { now: NOW, queueId: "preview" },
  );
  assert.equal(queued.ok, true);
  const acknowledged = markControlQueueItemAcknowledged(queued.state, "preview", {
    now: LATER,
  });
  assert.equal(acknowledged.ok, true);
  assert.equal(acknowledged.item.status, "acknowledged_preview_only");
  assert.equal("execution" in acknowledged.item, false);

  const second = enqueueRoomControlEvent(
    createControlQueueState(),
    previewEvent(),
    "outbound",
    { now: NOW, queueId: "preview-deny" },
  );
  assert.equal(second.ok, true);
  const denied = markControlQueueItemDenied(second.state, "preview-deny", {
    now: LATER,
  });
  assert.equal(denied.ok, true);
  assert.equal("retry" in denied.item, false);
  assert.equal("escalation" in denied.item, false);
});

test("invalid transitions fail closed and statuses contain no runtime result fields", () => {
  const queued = enqueueRoomControlEvent(
    createControlQueueState(),
    previewEvent(),
    "outbound",
    { now: NOW, queueId: "preview" },
  );
  assert.equal(queued.ok, true);
  const denied = markControlQueueItemDenied(queued.state, "preview", { now: LATER });
  assert.equal(denied.ok, true);
  const invalidTransition = markControlQueueItemAcknowledged(
    denied.state,
    "preview",
    { now: LATER },
  );
  assert.equal(invalidTransition.ok, false);
  assert.match(invalidTransition.errors.join(" "), /finalized/i);

  const serialized = JSON.stringify(denied.item);
  assert.doesNotMatch(serialized, /stdout|stderr|exitCode/);
});

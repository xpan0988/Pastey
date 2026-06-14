import assert from "node:assert/strict";
import test from "node:test";

import {
  CONTROL_WINDOW_RESTORE_QUIET_PERIOD_MS,
  createControlQueueState,
  createIdleRoomControlSendState,
  createRuntimeDataWindowTargetState,
  getOutgoingControlWindowDemand,
  hasOutgoingControlWindowDemand,
  publishRuntimeControlWindowStatus,
  reduceRuntimeDataWindowTarget,
  resetOutgoingControlWindowDemandForSession,
  setOutgoingControlWindowDemand,
  subscribeOutgoingControlWindowDemand,
  waitForRuntimeDataWindowTarget,
  type ControlQueueItem,
  type ControlQueueItemStatus,
  type ControlQueueState,
} from "../src/lib/agentBridge";

function queueWith(
  direction: "outbound" | "inbound",
  status: ControlQueueItemStatus,
): ControlQueueState {
  const item = {
    queueId: `${direction}-${status}`,
    direction,
    status,
    event: { expiresAt: "2099-01-01T00:00:00.000Z" },
  } as unknown as ControlQueueItem;
  return {
    ...createControlQueueState(),
    [direction]: [item],
  };
}

test("outgoing transport work creates runtime control demand", () => {
  for (const status of ["queued", "selected", "transport_sending"] as const) {
    assert.equal(
      hasOutgoingControlWindowDemand(queueWith("outbound", status), createIdleRoomControlSendState()),
      true,
    );
  }
  assert.equal(
    hasOutgoingControlWindowDemand(
      createControlQueueState(),
      { status: "sending", startedAt: "2026-06-13T00:00:00.000Z", eventId: "event-1" },
    ),
    true,
  );
});

test("terminal outbound and inbound-only review state do not create demand", () => {
  for (const status of [
    "transport_delivered",
    "transport_rejected",
    "allowed_once",
    "acknowledged_preview_only",
    "denied",
    "invalid",
    "expired",
    "duplicate",
  ] as const) {
    assert.equal(
      hasOutgoingControlWindowDemand(queueWith("outbound", status), createIdleRoomControlSendState()),
      false,
    );
  }
  assert.equal(
    hasOutgoingControlWindowDemand(queueWith("inbound", "queued"), createIdleRoomControlSendState()),
    false,
  );
  assert.equal(
    hasOutgoingControlWindowDemand(queueWith("inbound", "selected"), createIdleRoomControlSendState()),
    false,
  );
  assert.equal(
    hasOutgoingControlWindowDemand(queueWith("inbound", "awaiting_peer_decision"), createIdleRoomControlSendState()),
    false,
  );
});

test("outbound acknowledgement or denial queue entries create demand", () => {
  assert.equal(
    hasOutgoingControlWindowDemand(queueWith("outbound", "queued"), createIdleRoomControlSendState()),
    true,
  );
});

test("expired queued outbound work does not retain runtime demand", () => {
  const state = queueWith("outbound", "queued");
  state.outbound[0] = {
    ...state.outbound[0],
    event: {
      ...state.outbound[0].event,
      expiresAt: "2026-06-13T00:00:00.000Z",
    },
  };
  assert.equal(
    hasOutgoingControlWindowDemand(state, createIdleRoomControlSendState(), {
      now: new Date("2026-06-13T00:00:01.000Z"),
    }),
    false,
  );
});

test("target drops immediately and restores only after deterministic quiet period", () => {
  const initial = createRuntimeDataWindowTargetState();
  const demanded = reduceRuntimeDataWindowTarget(initial, {
    type: "demand_changed",
    outgoingControlDemand: true,
    nowMs: 1_000,
  });
  assert.equal(demanded.targetDataWindows, 7);
  assert.equal(demanded.restoreAfterMs, null);

  const quiet = reduceRuntimeDataWindowTarget(demanded, {
    type: "demand_changed",
    outgoingControlDemand: false,
    nowMs: 2_000,
  });
  assert.equal(quiet.targetDataWindows, 7);
  assert.equal(quiet.restoreAfterMs, 2_000 + CONTROL_WINDOW_RESTORE_QUIET_PERIOD_MS);

  const tooSoon = reduceRuntimeDataWindowTarget(quiet, {
    type: "restore_quiet_period_elapsed",
    nowMs: 2_000 + CONTROL_WINDOW_RESTORE_QUIET_PERIOD_MS - 1,
  });
  assert.equal(tooSoon.targetDataWindows, 7);

  const restored = reduceRuntimeDataWindowTarget(quiet, {
    type: "restore_quiet_period_elapsed",
    nowMs: 2_000 + CONTROL_WINDOW_RESTORE_QUIET_PERIOD_MS,
  });
  assert.equal(restored.targetDataWindows, 8);
});

test("new demand cancels pending restoration and notifications coalesce", () => {
  const quiet = reduceRuntimeDataWindowTarget({
    targetDataWindows: 7,
    outgoingControlDemand: false,
    restoreAfterMs: 2_000,
  }, {
    type: "demand_changed",
    outgoingControlDemand: true,
    nowMs: 1_500,
  });
  assert.deepEqual(quiet, {
    targetDataWindows: 7,
    outgoingControlDemand: true,
    restoreAfterMs: null,
  });

  let notifications = 0;
  const unsubscribe = subscribeOutgoingControlWindowDemand(() => {
    notifications += 1;
  });
  setOutgoingControlWindowDemand("test-source", true);
  setOutgoingControlWindowDemand("test-source", true);
  assert.equal(getOutgoingControlWindowDemand(), true);
  assert.equal(notifications, 1);
  setOutgoingControlWindowDemand("test-source", false);
  setOutgoingControlWindowDemand("test-source", false);
  assert.equal(getOutgoingControlWindowDemand(), false);
  assert.equal(notifications, 2);
  unsubscribe();
});

test("session reset clears old demand without hidden cross-session state", () => {
  setOutgoingControlWindowDemand("old-session", true);
  assert.equal(getOutgoingControlWindowDemand(), true);
  resetOutgoingControlWindowDemandForSession();
  assert.equal(getOutgoingControlWindowDemand(), false);
});

test("control delivery barrier fails closed until target seven is applied", async () => {
  publishRuntimeControlWindowStatus({
    targetDataWindows: 7,
    reason: "outgoing_control_demand",
    reservationReady: false,
    activeAllocationUpdates: "transfer:update_failed",
    lastError: "One or more active data-window updates failed.",
  });
  assert.equal(await waitForRuntimeDataWindowTarget(7, 5), false);

  publishRuntimeControlWindowStatus({
    targetDataWindows: 7,
    reason: "outgoing_control_demand",
    reservationReady: true,
    activeAllocationUpdates: "transfer:8->7 (updated)",
  });
  assert.equal(await waitForRuntimeDataWindowTarget(7, 5), true);

  publishRuntimeControlWindowStatus({
    targetDataWindows: 8,
    reason: "idle",
    reservationReady: true,
    activeAllocationUpdates: "No active allocation updates.",
  });
});

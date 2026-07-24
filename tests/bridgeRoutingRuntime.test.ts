import assert from "node:assert/strict";
import test from "node:test";

import { deriveBridgeRoutingStateForRoom, sendTextToRoomWithBridgeRoute } from "../src/lib/bridgeRoutingRuntime";
import type { RoomInfo, RoomItem } from "../src/lib/types";

const room: RoomInfo = {
  id: "room-1", room_code: "12345678", room_code_display: "1234 5678", created_at: 1,
  expires_at: 2, status: "active", local_role: "creator", peer_device_name: "Peer",
  auto_burn_after_expiry: false, peer_connected: true, local_burned_at: null, peer_burned_at: null,
};

test("ordinary data derives one current selected-peer route", () => {
  const state = deriveBridgeRoutingStateForRoom(room);
  assert.equal(state.status, "ready_selected_peer");
  if (state.status !== "ready_selected_peer") return;
  assert.equal(state.route.target.kind, "selected_peer");
});

test("ordinary data does not fall back when no peer is routeable", async () => {
  await assert.rejects(
    () => sendTextToRoomWithBridgeRoute({ ...room, peer_connected: false }, "hello", async () => {
      throw new Error("sender must not run");
    }),
    /No routeable remote Bridge peer/,
  );
});

test("ordinary text sends the validated selected-peer route to Tauri", async () => {
  const item: RoomItem = { id: "item", room_id: room.id, direction: "outgoing", item_kind: "text", payload_type: "text", size_bytes: 5, created_at: 1, status: "sent", text: "hello" };
  const result = await sendTextToRoomWithBridgeRoute(room, "hello", async (roomId, text, route) => {
    assert.equal(roomId, room.id);
    assert.equal(text, "hello");
    assert.equal(route?.target.kind, "selected_peer");
    return item;
  });
  assert.equal(result.id, item.id);
});

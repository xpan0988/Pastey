import {
  buildCapabilityPreviewControlEvent,
  type RoomControlEvent,
  type RoomControlEventBuildResult,
} from "./roomControlEvent";
import {
  createControlQueueState,
  markControlQueueItemStatus,
  selectNextControlQueueItem,
  type ControlQueueItem,
  type ControlQueueState,
} from "./controlQueue";
import {
  hashHelloPeerRequestPayload,
  hashHelloStdoutRequestPayload,
  HELLO_STDOUT_CAPABILITY,
  validateCapabilityRequestPreviewEnvelope,
  type CapabilityRequest,
  type CapabilityRequestPreviewEnvelope,
  type HelloPeerRequest,
  type HelloStdoutRequest,
} from "../ai";
import type {
  RoomControlDeliveryReceipt,
  RoomControlSessionContext,
} from "../types";

export type RoomControlSendErrorCode =
  | "expired"
  | "duplicate"
  | "replay"
  | "invalid_event"
  | "room_mismatch"
  | "source_mismatch"
  | "target_mismatch"
  | "session_mismatch"
  | "session_unavailable"
  | "peer_unavailable"
  | "inbox_full"
  | "rate_limited"
  | "oversized"
  | "malformed_receipt"
  | "transport_error"
  | "unknown";

export type RoomControlSendState =
  | { status: "idle" }
  | { status: "sending"; startedAt: string; eventId: string }
  | {
      status: "accepted";
      eventId: string;
      receivedAt: string;
      acceptedForLocalInbox: true;
    }
  | {
      status: "rejected";
      eventId: string;
      errorCode: RoomControlSendErrorCode;
      message: string;
      occurredAt: string;
    };

export type RoomControlRejectedSendState = Extract<RoomControlSendState, { status: "rejected" }>;

export type ProcessNextControlQueueItemResult =
  | {
      ok: true;
      action: "selected_inbound" | "transport_delivered";
      state: ControlQueueState;
      item: ControlQueueItem;
      sendState?: RoomControlSendState;
    }
  | {
      ok: false;
      action: "no_selectable_item" | "transport_rejected" | "invalid_transition";
      state: ControlQueueState;
      message: string;
      item?: ControlQueueItem;
      sendState?: RoomControlSendState;
    };

interface StructuredRoomControlSendError {
  code?: unknown;
}

const SEND_ERROR_MESSAGES: Record<RoomControlSendErrorCode, string> = {
  expired: "Rejected: event expired before delivery.",
  duplicate: "Rejected: duplicate event.",
  replay: "Rejected: duplicate/replay event.",
  invalid_event: "Rejected: invalid room-control event.",
  room_mismatch: "Rejected: room mismatch.",
  source_mismatch: "Rejected: source peer/session mismatch.",
  target_mismatch: "Rejected: target peer/session mismatch.",
  session_mismatch: "Rejected: target peer/session mismatch.",
  session_unavailable: "Transport failed: room session unavailable.",
  peer_unavailable: "Transport failed: peer unavailable.",
  inbox_full: "Rejected: peer local control inbox is full.",
  rate_limited: "Rejected: peer control-event rate limit reached.",
  oversized: "Rejected: room-control event is oversized.",
  malformed_receipt: "Transport failed: malformed delivery receipt.",
  transport_error: "Transport failed.",
  unknown: "Room-control send failed.",
};

export function createIdleRoomControlSendState(): RoomControlSendState {
  return { status: "idle" };
}

export function roomControlSessionIdentity(
  session: RoomControlSessionContext | null,
): string | null {
  return session
    ? `${session.roomId}:${session.localSessionRef}:${session.peerSessionRef}:${session.peerRouteRef ?? ""}`
    : null;
}

export function preserveRoomControlSendStateForSession(
  state: RoomControlSendState,
  previousSession: RoomControlSessionContext | null,
  nextSession: RoomControlSessionContext | null,
): RoomControlSendState {
  return roomControlSessionIdentity(previousSession) === roomControlSessionIdentity(nextSession)
    ? state
    : createIdleRoomControlSendState();
}

export function preserveControlQueueForSession(
  state: ControlQueueState,
  previousSession: RoomControlSessionContext | null,
  nextSession: RoomControlSessionContext | null,
): ControlQueueState {
  return roomControlSessionIdentity(previousSession) === roomControlSessionIdentity(nextSession)
    ? state
    : createControlQueueState();
}

export function mapRoomControlSendError(
  error: unknown,
  eventId: string,
  now = new Date(),
): RoomControlRejectedSendState {
  const structuredCode =
    typeof error === "object" && error !== null
      ? (error as StructuredRoomControlSendError).code
      : undefined;
  const rawCode = typeof structuredCode === "string" ? structuredCode : "";
  const rawMessage =
    error instanceof Error ? error.message : typeof error === "string" ? error : "";
  const normalized = `${rawCode} ${rawMessage}`.toLowerCase();
  let errorCode: RoomControlSendErrorCode = isRoomControlSendErrorCode(rawCode)
    ? rawCode
    : "unknown";
  if (errorCode === "unknown") {
    if (normalized.includes("expired")) errorCode = "expired";
    else if (normalized.includes("duplicate")) errorCode = "duplicate";
    else if (normalized.includes("replay") || normalized.includes("already received")) errorCode = "replay";
    else if (normalized.includes("target") && normalized.includes("mismatch")) errorCode = "target_mismatch";
    else if (normalized.includes("source") && normalized.includes("mismatch")) errorCode = "source_mismatch";
    else if (normalized.includes("session") && normalized.includes("mismatch")) errorCode = "session_mismatch";
    else if (normalized.includes("room") && normalized.includes("mismatch")) errorCode = "room_mismatch";
    else if (normalized.includes("session") && normalized.includes("unavailable")) errorCode = "session_unavailable";
    else if (normalized.includes("peer") && normalized.includes("unavailable")) errorCode = "peer_unavailable";
    else if (normalized.includes("inbox") && normalized.includes("full")) errorCode = "inbox_full";
    else if (normalized.includes("rate")) errorCode = "rate_limited";
    else if (normalized.includes("too large") || normalized.includes("oversized")) errorCode = "oversized";
    else if (normalized.includes("receipt") && normalized.includes("invalid")) errorCode = "malformed_receipt";
    else if (normalized.includes("invalid") || normalized.includes("validation")) errorCode = "invalid_event";
    else if (normalized.includes("transport") || normalized.includes("network") || normalized.includes("timed out")) {
      errorCode = "transport_error";
    }
  }
  return {
    status: "rejected",
    eventId,
    errorCode,
    message: SEND_ERROR_MESSAGES[errorCode],
    occurredAt: now.toISOString(),
  };
}

export async function sendCurrentRoomControlEvent(
  event: RoomControlEvent,
  sender: (event: RoomControlEvent) => Promise<RoomControlDeliveryReceipt>,
  onState: (state: RoomControlSendState) => void,
  now: () => Date = () => new Date(),
): Promise<RoomControlSendState> {
  onState({ status: "sending", startedAt: now().toISOString(), eventId: event.eventId });
  try {
    const receipt = await sender(event);
    if (
      receipt.schemaVersion !== "pastey-room-control-delivery/v1"
      || receipt.eventId !== event.eventId
      || receipt.acceptedForLocalInbox !== true
    ) {
      throw { code: "malformed_receipt" };
    }
    const accepted: RoomControlSendState = {
      status: "accepted",
      eventId: receipt.eventId,
      receivedAt: receipt.receivedAt,
      acceptedForLocalInbox: true,
    };
    onState(accepted);
    return accepted;
  } catch (error) {
    const rejected = mapRoomControlSendError(error, event.eventId, now());
    onState(rejected);
    return rejected;
  }
}

export async function processNextControlQueueItem(
  state: ControlQueueState,
  sender: (event: RoomControlEvent) => Promise<RoomControlDeliveryReceipt>,
  options: {
    now?: () => Date;
    onState?: (state: ControlQueueState) => void;
    onSendState?: (state: RoomControlSendState) => void;
  } = {},
): Promise<ProcessNextControlQueueItemResult> {
  const now = options.now ?? (() => new Date());
  const selection = selectNextControlQueueItem(state, { now: now() });
  options.onState?.(selection.state);
  if (!selection.ok) {
    return {
      ok: false,
      action: "no_selectable_item",
      state: selection.state,
      message: selection.reason,
    };
  }
  if (selection.item.direction === "inbound") {
    return {
      ok: true,
      action: "selected_inbound",
      state: selection.state,
      item: selection.item,
    };
  }

  const sending = markControlQueueItemStatus(
    selection.state,
    selection.item.queueId,
    "transport_sending",
    { now: now(), reason: "Sending through preview-only room-control transport." },
  );
  if (!sending.ok) {
    return {
      ok: false,
      action: "invalid_transition",
      state: sending.state,
      item: selection.item,
      message: sending.errors.join(" ").slice(0, 512),
    };
  }
  options.onState?.(sending.state);

  const sendState = await sendCurrentRoomControlEvent(
    sending.item.event,
    sender,
    (next) => options.onSendState?.(next),
    now,
  );
  if (sendState.status === "accepted") {
    const delivered = markControlQueueItemStatus(
      sending.state,
      sending.item.queueId,
      "transport_delivered",
      {
        now: now(),
        reason: "Accepted for peer local inbox. Transport delivery is not peer consent.",
        transportResultCode: "accepted_for_local_inbox",
        transportReceivedAt: sendState.receivedAt,
      },
    );
    if (!delivered.ok) {
      return {
        ok: false,
        action: "invalid_transition",
        state: delivered.state,
        item: sending.item,
        sendState,
        message: delivered.errors.join(" ").slice(0, 512),
      };
    }
    options.onState?.(delivered.state);
    return {
      ok: true,
      action: "transport_delivered",
      state: delivered.state,
      item: delivered.item,
      sendState,
    };
  }
  if (sendState.status !== "rejected") {
    return {
      ok: false,
      action: "invalid_transition",
      state: sending.state,
      item: sending.item,
      sendState,
      message: "Room-control send did not reach a terminal state.",
    };
  }

  const rejected = markControlQueueItemStatus(
    sending.state,
    sending.item.queueId,
    "transport_rejected",
    {
      now: now(),
      reason: sendState.message,
      transportResultCode: sendState.errorCode,
    },
  );
  if (!rejected.ok) {
    return {
      ok: false,
      action: "invalid_transition",
      state: rejected.state,
      item: sending.item,
      sendState,
      message: rejected.errors.join(" ").slice(0, 512),
    };
  }
  options.onState?.(rejected.state);
  return {
    ok: false,
    action: "transport_rejected",
    state: rejected.state,
    item: rejected.item,
    sendState,
    message: sendState.message,
  };
}

function isRoomControlSendErrorCode(value: string): value is RoomControlSendErrorCode {
  return Object.prototype.hasOwnProperty.call(SEND_ERROR_MESSAGES, value);
}

export function buildSessionBoundCapabilityPreviewControlEvent(
  envelope: CapabilityRequestPreviewEnvelope,
  session: RoomControlSessionContext,
  options: { now?: Date } = {},
): RoomControlEventBuildResult {
  const { requestPayloadHash: _requestPayloadHash, ...requestWithoutHash } =
    envelope.request;
  const request = rebuildCapabilityRequestHash({
    ...requestWithoutHash,
    sourceDeviceRef: session.localSessionRef,
    targetPeerRef: session.peerSessionRef,
  } as Omit<CapabilityRequest, "requestPayloadHash">);
  const reboundEnvelope: CapabilityRequestPreviewEnvelope = {
    ...envelope,
    roomRef: session.roomId,
    sourceDeviceRef: session.localSessionRef,
    targetPeerRef: session.peerSessionRef,
    request,
  };
  const validation = validateCapabilityRequestPreviewEnvelope(reboundEnvelope, {
    now: options.now,
    expectedRoomRef: session.roomId,
    expectedTargetPeerRef: session.peerSessionRef,
  });
  if (!validation.valid) {
    return { ok: false, errors: validation.errors };
  }
  return buildCapabilityPreviewControlEvent(validation.value, {
    roomRef: session.roomId,
    sourceDeviceRef: session.localSessionRef,
    targetPeerRef: session.peerSessionRef,
    now: options.now,
  });
}

function rebuildCapabilityRequestHash(
  requestWithoutHash: Omit<CapabilityRequest, "requestPayloadHash">,
): CapabilityRequest {
  if (requestWithoutHash.capability === HELLO_STDOUT_CAPABILITY) {
    const rebound = requestWithoutHash as Omit<HelloStdoutRequest, "requestPayloadHash">;
    return {
      ...rebound,
      requestPayloadHash: hashHelloStdoutRequestPayload(rebound),
    };
  }
  const rebound = requestWithoutHash as Omit<HelloPeerRequest, "requestPayloadHash">;
  return {
    ...rebound,
    requestPayloadHash: hashHelloPeerRequestPayload(rebound),
  };
}

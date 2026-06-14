import {
  checkAndRecordRoomControlEvent,
  computeControlLaneBudget,
  createRoomControlEventSessionState,
  validateRoomControlEvent,
  type ControlLaneBudget,
  type RoomControlEvent,
  type RoomControlEventSessionState,
} from "./roomControlEvent";

export type ControlQueueDirection = "outbound" | "inbound";

export type ControlQueueItemStatus =
  | "queued"
  | "selected"
  | "transport_sending"
  | "transport_delivered"
  | "transport_rejected"
  | "awaiting_peer_decision"
  | "allowed_once"
  | "acknowledged_preview_only"
  | "denied"
  | "invalid"
  | "expired"
  | "duplicate"
  | "execution_consumed"
  | "execution_succeeded"
  | "execution_rejected"
  | "execution_failed"
  | "already_consumed";

export interface ControlQueueItem {
  queueId: string;
  direction: ControlQueueDirection;
  event: RoomControlEvent;
  status: ControlQueueItemStatus;
  enqueuedAt: string;
  lastUpdatedAt: string;
  priority: number;
  reason?: string;
  transportResultCode?: string;
  transportReceivedAt?: string;
}

export interface ControlQueueState {
  outbound: ControlQueueItem[];
  inbound: ControlQueueItem[];
  session: RoomControlEventSessionState;
}

export type ControlQueueEnqueueResult =
  | { ok: true; state: ControlQueueState; item: ControlQueueItem }
  | { ok: false; state: ControlQueueState; errors: string[] };

export type ControlQueueSelectionResult =
  | {
      ok: true;
      state: ControlQueueState;
      item: ControlQueueItem;
      budget: ControlLaneBudget;
    }
  | {
      ok: false;
      state: ControlQueueState;
      reason: string;
      budget: ControlLaneBudget;
    };

export type ControlQueueTransitionStatus = Exclude<
  ControlQueueItemStatus,
  "queued" | "selected" | "duplicate"
>;

export interface InboundControlQueueIntegrationResult {
  state: ControlQueueState;
  added: ControlQueueItem[];
  diagnostics: string[];
}

export type ControlQueueTransitionResult =
  | { ok: true; state: ControlQueueState; item: ControlQueueItem }
  | { ok: false; state: ControlQueueState; errors: string[] };

const TERMINAL_STATUSES = new Set<ControlQueueItemStatus>([
  "transport_rejected",
  "allowed_once",
  "acknowledged_preview_only",
  "denied",
  "invalid",
  "expired",
  "duplicate",
  "execution_consumed",
  "execution_succeeded",
  "execution_rejected",
  "execution_failed",
  "already_consumed",
]);

let localQueueSequence = 0;

export function createControlQueueState(): ControlQueueState {
  return {
    outbound: [],
    inbound: [],
    session: createRoomControlEventSessionState(),
  };
}

/**
 * Lower numbers have higher priority.
 */
export function getRoomControlEventPriority(
  event: RoomControlEvent,
  direction: ControlQueueDirection,
): number {
  if (
    direction === "inbound" &&
    (event.kind === "capability_preview_deny" ||
      event.kind === "capability_preview_invalid" ||
      event.kind === "capability_preview_expired")
  ) {
    return 1;
  }

  if (direction === "inbound") {
    return 2;
  }

  if (event.kind === "capability_preview" || event.kind === "capability_execute_request") {
    return 3;
  }

  return 4;
}

export function enqueueRoomControlEvent(
  state: ControlQueueState,
  event: RoomControlEvent,
  direction: ControlQueueDirection,
  options: { now?: Date; queueId?: string } = {},
): ControlQueueEnqueueResult {
  const now = options.now ?? new Date();
  const validation = validateRoomControlEvent(event, { now });
  if (!validation.valid) {
    return { ok: false, state, errors: validation.errors };
  }

  const replayCheck = checkAndRecordRoomControlEvent(validation.value, state.session, { now });
  if (!replayCheck.ok) {
    return { ok: false, state, errors: replayCheck.errors };
  }

  const timestamp = now.toISOString();
  const queueId = options.queueId ?? createLocalQueueId();
  if (allQueueItems(state).some((item) => item.queueId === queueId)) {
    return { ok: false, state, errors: [`Control queue ID is a duplicate: ${queueId}`] };
  }
  const item: ControlQueueItem = {
    queueId,
    direction,
    event: validation.value,
    status: "queued",
    enqueuedAt: timestamp,
    lastUpdatedAt: timestamp,
    priority: getRoomControlEventPriority(event, direction),
  };

  return {
    ok: true,
    item,
    state: {
      ...state,
      [direction]: [...state[direction], item],
      session: replayCheck.state,
    },
  };
}

export function hasControlBacklog(
  state: ControlQueueState,
  options: { now?: Date } = {},
): boolean {
  const now = options.now ?? new Date();
  return allQueueItems(state).some(
    (item) =>
      (item.status === "queued" ||
        item.status === "selected" ||
        item.status === "transport_sending") &&
      !isExpired(item.event, now),
  );
}

export function getControlQueueBudget(
  state: ControlQueueState,
  options: { now?: Date } = {},
): ControlLaneBudget {
  return computeControlLaneBudget({
    controlBacklog: hasControlBacklog(state, options),
  });
}

export function selectNextControlQueueItem(
  state: ControlQueueState,
  options: { now?: Date } = {},
): ControlQueueSelectionResult {
  const now = options.now ?? new Date();
  const stateWithExpiry = expireQueuedItems(state, now);
  const budget = getControlQueueBudget(stateWithExpiry, { now });
  const alreadySelected = allQueueItems(stateWithExpiry).find(
    (item) => item.status === "selected",
  );

  if (alreadySelected) {
    return {
      ok: true,
      state: stateWithExpiry,
      item: alreadySelected,
      budget,
    };
  }

  const next = allQueueItems(stateWithExpiry)
    .filter((item) => item.status === "queued")
    .sort(compareQueueItems)[0];

  if (!next) {
    return {
      ok: false,
      state: stateWithExpiry,
      reason: "no_selectable_control_item",
      budget,
    };
  }

  const selected: ControlQueueItem = {
    ...next,
    status: "selected",
    lastUpdatedAt: now.toISOString(),
  };
  const nextState = replaceQueueItem(stateWithExpiry, selected);

  return {
    ok: true,
    state: nextState,
    item: selected,
    budget: getControlQueueBudget(nextState, { now }),
  };
}

export function markControlQueueItemStatus(
  state: ControlQueueState,
  queueId: string,
  status: ControlQueueTransitionStatus,
  options: {
    now?: Date;
    reason?: string;
    transportResultCode?: string;
    transportReceivedAt?: string;
  } = {},
): ControlQueueTransitionResult {
  const item = allQueueItems(state).find((candidate) => candidate.queueId === queueId);
  if (!item) {
    return {
      ok: false,
      state,
      errors: [`Control queue item not found: ${queueId}`],
    };
  }

  if (TERMINAL_STATUSES.has(item.status)) {
    return {
      ok: false,
      state,
      errors: [`Cannot transition finalized control queue item from ${item.status}.`],
    };
  }

  if (
    (status === "transport_sending" &&
      (item.direction !== "outbound" || item.status !== "selected")) ||
    ((status === "transport_delivered" || status === "transport_rejected") &&
      (item.direction !== "outbound" || item.status !== "transport_sending"))
  ) {
    return {
      ok: false,
      state,
      errors: [`Invalid control queue transport transition from ${item.status} to ${status}.`],
    };
  }

  if (
    (status === "awaiting_peer_decision" &&
      (item.direction !== "inbound" ||
        item.event.kind !== "capability_preview" ||
        item.status !== "selected")) ||
    (status === "allowed_once" &&
      (item.direction !== "inbound" ||
        item.event.kind !== "capability_preview" ||
        item.status !== "awaiting_peer_decision"))
  ) {
    return {
      ok: false,
      state,
      errors: [`Invalid peer review transition from ${item.status} to ${status}.`],
    };
  }

  if (options.reason !== undefined && options.reason.length > 512) {
    return {
      ok: false,
      state,
      errors: ["Control queue transition reason exceeds 512 characters."],
    };
  }

  const updated: ControlQueueItem = {
    ...item,
    status,
    lastUpdatedAt: (options.now ?? new Date()).toISOString(),
    ...(options.reason ? { reason: options.reason } : {}),
    ...(options.transportResultCode
      ? { transportResultCode: options.transportResultCode.slice(0, 64) }
      : {}),
    ...(options.transportReceivedAt
      ? { transportReceivedAt: options.transportReceivedAt.slice(0, 64) }
      : {}),
  };

  return { ok: true, state: replaceQueueItem(state, updated), item: updated };
}

export function enqueueInboundRoomControlEvents(
  state: ControlQueueState,
  events: unknown[],
  options: {
    now?: Date;
    expectedRoomRef?: string;
    expectedSourceDeviceRef?: string;
    expectedTargetPeerRef?: string;
  } = {},
): InboundControlQueueIntegrationResult {
  const now = options.now ?? new Date();
  let nextState = state;
  const added: ControlQueueItem[] = [];
  const diagnostics: string[] = [];

  for (const event of events) {
    const validation = validateRoomControlEvent(event, {
      now,
      expectedRoomRef: options.expectedRoomRef,
      expectedSourceDeviceRef: options.expectedSourceDeviceRef,
      expectedTargetPeerRef: options.expectedTargetPeerRef,
    });
    if (!validation.valid) {
      diagnostics.push(`Inbound control event rejected: ${validation.errors.join(" ").slice(0, 512)}`);
      continue;
    }
    const enqueue = enqueueRoomControlEvent(nextState, validation.value, "inbound", { now });
    if (!enqueue.ok) {
      diagnostics.push(`Inbound control event ${validation.value.eventId} not queued: ${enqueue.errors.join(" ").slice(0, 384)}`);
      continue;
    }
    nextState = enqueue.state;
    added.push(enqueue.item);
    diagnostics.push(`Inbound control event queued: ${enqueue.item.event.eventId}.`);
  }

  return { state: nextState, added, diagnostics };
}

export function markControlQueueItemAcknowledged(
  state: ControlQueueState,
  queueId: string,
  options: { now?: Date; reason?: string } = {},
): ControlQueueTransitionResult {
  return markControlQueueItemStatus(
    state,
    queueId,
    "acknowledged_preview_only",
    options,
  );
}

export function markControlQueueItemDenied(
  state: ControlQueueState,
  queueId: string,
  options: { now?: Date; reason?: string } = {},
): ControlQueueTransitionResult {
  return markControlQueueItemStatus(state, queueId, "denied", options);
}

export function markControlQueueItemInvalid(
  state: ControlQueueState,
  queueId: string,
  options: { now?: Date; reason?: string } = {},
): ControlQueueTransitionResult {
  return markControlQueueItemStatus(state, queueId, "invalid", options);
}

export function markControlQueueItemExpired(
  state: ControlQueueState,
  queueId: string,
  options: { now?: Date; reason?: string } = {},
): ControlQueueTransitionResult {
  return markControlQueueItemStatus(state, queueId, "expired", options);
}

function createLocalQueueId(): string {
  localQueueSequence += 1;
  return `local-control-${localQueueSequence}`;
}

function allQueueItems(state: ControlQueueState): ControlQueueItem[] {
  return [...state.outbound, ...state.inbound];
}

function compareQueueItems(a: ControlQueueItem, b: ControlQueueItem): number {
  return (
    a.priority - b.priority ||
    Date.parse(a.enqueuedAt) - Date.parse(b.enqueuedAt) ||
    a.queueId.localeCompare(b.queueId)
  );
}

function isExpired(event: RoomControlEvent, now: Date): boolean {
  return Date.parse(event.expiresAt) <= now.getTime();
}

function expireQueuedItems(state: ControlQueueState, now: Date): ControlQueueState {
  return allQueueItems(state)
    .filter((item) => item.status === "queued" && isExpired(item.event, now))
    .reduce(
      (nextState, item) =>
        replaceQueueItem(nextState, {
          ...item,
          status: "expired",
          lastUpdatedAt: now.toISOString(),
          reason: "Event expired before local selection.",
        }),
      state,
    );
}

function replaceQueueItem(
  state: ControlQueueState,
  updated: ControlQueueItem,
): ControlQueueState {
  return {
    ...state,
    [updated.direction]: state[updated.direction].map((item) =>
      item.queueId === updated.queueId ? updated : item,
    ),
  };
}

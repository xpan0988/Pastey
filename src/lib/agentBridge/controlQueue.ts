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
  | "acknowledged_preview_only"
  | "denied"
  | "invalid"
  | "expired"
  | "duplicate";

export interface ControlQueueItem {
  queueId: string;
  direction: ControlQueueDirection;
  event: RoomControlEvent;
  status: ControlQueueItemStatus;
  enqueuedAt: string;
  lastUpdatedAt: string;
  priority: number;
  reason?: string;
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

export type ControlQueueTransitionResult =
  | { ok: true; state: ControlQueueState; item: ControlQueueItem }
  | { ok: false; state: ControlQueueState; errors: string[] };

const TERMINAL_STATUSES = new Set<ControlQueueItemStatus>([
  "acknowledged_preview_only",
  "denied",
  "invalid",
  "expired",
  "duplicate",
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

  if (event.kind === "capability_preview") {
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
      (item.status === "queued" || item.status === "selected") &&
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
  options: { now?: Date; reason?: string } = {},
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
  };

  return { ok: true, state: replaceQueueItem(state, updated), item: updated };
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

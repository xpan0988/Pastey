import type { ControlQueueState } from "./controlQueue";
import type { RoomControlSendState } from "./roomControlTransport";

export const IDLE_DATA_WINDOW_TARGET = 8 as const;
export const CONTROL_DEMAND_DATA_WINDOW_TARGET = 7 as const;
export const CONTROL_WINDOW_RESTORE_QUIET_PERIOD_MS = 750;

export type RuntimeDataWindowTarget =
  | typeof IDLE_DATA_WINDOW_TARGET
  | typeof CONTROL_DEMAND_DATA_WINDOW_TARGET;

export interface RuntimeDataWindowTargetState {
  targetDataWindows: RuntimeDataWindowTarget;
  outgoingControlDemand: boolean;
  restoreAfterMs: number | null;
}

export type RuntimeDataWindowTargetEvent =
  | { type: "demand_changed"; outgoingControlDemand: boolean; nowMs: number }
  | { type: "restore_quiet_period_elapsed"; nowMs: number };

export interface RuntimeControlWindowStatus {
  targetDataWindows: RuntimeDataWindowTarget;
  reason: "outgoing_control_demand" | "restore_quiet_period" | "idle";
  reservationReady: boolean;
  activeAllocationUpdates: string;
  lastError?: string;
}

const demandBySource = new Map<string, boolean>();
const demandListeners = new Set<() => void>();
const sessionResetListeners = new Set<() => void>();
const runtimeStatusListeners = new Set<() => void>();
let aggregateOutgoingControlDemand = false;
let controlWindowSessionRevision = 0;
let runtimeStatus: RuntimeControlWindowStatus = {
  targetDataWindows: IDLE_DATA_WINDOW_TARGET,
  reason: "idle",
  reservationReady: true,
  activeAllocationUpdates: "No active allocation updates.",
};

export function hasOutgoingControlWindowDemand(
  state: ControlQueueState,
  sendState: RoomControlSendState,
  options: { now?: Date } = {},
): boolean {
  if (sendState.status === "sending") {
    return true;
  }
  const nowMs = (options.now ?? new Date()).getTime();
  return state.outbound.some((item) =>
    (item.status === "queued" ||
      item.status === "selected" ||
      item.status === "transport_sending") &&
    Date.parse(item.event.expiresAt) > nowMs
  );
}

export function createRuntimeDataWindowTargetState(): RuntimeDataWindowTargetState {
  return {
    targetDataWindows: IDLE_DATA_WINDOW_TARGET,
    outgoingControlDemand: false,
    restoreAfterMs: null,
  };
}

export function reduceRuntimeDataWindowTarget(
  state: RuntimeDataWindowTargetState,
  event: RuntimeDataWindowTargetEvent,
): RuntimeDataWindowTargetState {
  if (event.type === "demand_changed") {
    if (event.outgoingControlDemand) {
      return {
        targetDataWindows: CONTROL_DEMAND_DATA_WINDOW_TARGET,
        outgoingControlDemand: true,
        restoreAfterMs: null,
      };
    }
    if (state.targetDataWindows === IDLE_DATA_WINDOW_TARGET) {
      return {
        targetDataWindows: IDLE_DATA_WINDOW_TARGET,
        outgoingControlDemand: false,
        restoreAfterMs: null,
      };
    }
    return {
      targetDataWindows: CONTROL_DEMAND_DATA_WINDOW_TARGET,
      outgoingControlDemand: false,
      restoreAfterMs: event.nowMs + CONTROL_WINDOW_RESTORE_QUIET_PERIOD_MS,
    };
  }

  if (
    state.outgoingControlDemand ||
    state.restoreAfterMs === null ||
    event.nowMs < state.restoreAfterMs
  ) {
    return state;
  }
  return {
    targetDataWindows: IDLE_DATA_WINDOW_TARGET,
    outgoingControlDemand: false,
    restoreAfterMs: null,
  };
}

export function setOutgoingControlWindowDemand(sourceId: string, demand: boolean): void {
  if (demand) {
    demandBySource.set(sourceId, true);
  } else {
    demandBySource.delete(sourceId);
  }
  const nextAggregate = demandBySource.size > 0;
  if (nextAggregate === aggregateOutgoingControlDemand) {
    return;
  }
  aggregateOutgoingControlDemand = nextAggregate;
  for (const listener of demandListeners) {
    listener();
  }
}

export function getOutgoingControlWindowDemand(): boolean {
  return aggregateOutgoingControlDemand;
}

export function subscribeOutgoingControlWindowDemand(listener: () => void): () => void {
  demandListeners.add(listener);
  return () => demandListeners.delete(listener);
}

export function resetOutgoingControlWindowDemandForSession(): void {
  demandBySource.clear();
  aggregateOutgoingControlDemand = false;
  controlWindowSessionRevision += 1;
  for (const listener of demandListeners) {
    listener();
  }
  for (const listener of sessionResetListeners) {
    listener();
  }
}

export function getControlWindowSessionRevision(): number {
  return controlWindowSessionRevision;
}

export function subscribeControlWindowSessionRevision(listener: () => void): () => void {
  sessionResetListeners.add(listener);
  return () => sessionResetListeners.delete(listener);
}

export function publishRuntimeControlWindowStatus(next: RuntimeControlWindowStatus): void {
  if (
    next.targetDataWindows === runtimeStatus.targetDataWindows &&
    next.reason === runtimeStatus.reason &&
    next.reservationReady === runtimeStatus.reservationReady &&
    next.activeAllocationUpdates === runtimeStatus.activeAllocationUpdates &&
    next.lastError === runtimeStatus.lastError
  ) {
    return;
  }
  runtimeStatus = next;
  for (const listener of runtimeStatusListeners) {
    listener();
  }
}

export function getRuntimeControlWindowStatus(): RuntimeControlWindowStatus {
  return runtimeStatus;
}

export function subscribeRuntimeControlWindowStatus(listener: () => void): () => void {
  runtimeStatusListeners.add(listener);
  return () => runtimeStatusListeners.delete(listener);
}

export function waitForRuntimeDataWindowTarget(
  targetDataWindows: RuntimeDataWindowTarget,
  timeoutMs = 2_000,
): Promise<boolean> {
  if (runtimeStatus.targetDataWindows === targetDataWindows && runtimeStatus.reservationReady) {
    return Promise.resolve(true);
  }
  return new Promise((resolve) => {
    const timeout = globalThis.setTimeout(() => {
      unsubscribe();
      resolve(false);
    }, timeoutMs);
    const unsubscribe = subscribeRuntimeControlWindowStatus(() => {
      if (
        runtimeStatus.targetDataWindows !== targetDataWindows ||
        !runtimeStatus.reservationReady
      ) {
        return;
      }
      globalThis.clearTimeout(timeout);
      unsubscribe();
      resolve(true);
    });
  });
}

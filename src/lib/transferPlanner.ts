export type TransferPlannerTaskKind =
  | "file"
  | "image"
  | "pasted_image"
  | "text"
  | "control"
  | "agent"
  | "command";

export type TransferPlannerSizeClass = "tiny" | "small" | "medium" | "large" | "huge";

export type TransferPlannerLane = "control_text" | "small_file" | "bulk_file";

export type TransferPlannerPriority = "low" | "normal" | "high" | "urgent";

export type TransferPlannerTaskState = "queued" | "active" | "completed" | "failed" | "cancelled";

export type TransferPlannerMetadataStatus = "unknown" | "loading" | "ready" | "failed";

export type TransferPlannerRoomStatus = "active" | "peer_left" | "burned" | "expired" | "unavailable";

export type TransferPlannerHeldReason =
  | "cancelled"
  | "terminal_status"
  | "room_burned"
  | "room_unavailable"
  | "missing_metadata"
  | "global_budget_exhausted"
  | "lane_budget_exhausted"
  | "safety_cap_reached";

export interface TransferPlannerTask {
  id: string;
  roomId: string;
  kind: TransferPlannerTaskKind;
  state: TransferPlannerTaskState;
  metadataStatus: TransferPlannerMetadataStatus;
  sizeBytes?: number | null;
  priority?: TransferPlannerPriority;
  latencySensitive?: boolean;
  throughputSensitive?: boolean;
  roomStatus?: TransferPlannerRoomStatus;
  roomAvailable?: boolean;
  cancelRequested?: boolean;
  requestedWindow?: number | null;
  activeRequestedWindow?: number | null;
  createdAt?: number;
}

export interface TransferPlannerPolicy {
  globalWindowBudget: number;
  minRequestedWindow: number;
  maxRequestedWindow: number;
  safetyActiveTransferCap: number;
  laneWeights: Record<TransferPlannerLane, number>;
}

export interface TransferPlannerPlan {
  taskId: string;
  roomId: string;
  kind: TransferPlannerTaskKind;
  lane: TransferPlannerLane;
  sizeClass: TransferPlannerSizeClass;
  priority: TransferPlannerPriority;
  latencySensitive: boolean;
  throughputSensitive: boolean;
  requestedWindow: number;
}

export type TransferPlannerRunnablePlan = TransferPlannerPlan;
export type TransferPlannerActivePlan = TransferPlannerPlan;

export interface TransferPlannerHeldPlan {
  taskId: string;
  roomId: string;
  kind: TransferPlannerTaskKind;
  lane?: TransferPlannerLane;
  sizeClass?: TransferPlannerSizeClass;
  priority: TransferPlannerPriority;
  reason: TransferPlannerHeldReason;
  debugReason: string;
}

export interface TransferPlannerLaneBudgetReport {
  lane: TransferPlannerLane;
  requestedBudget: number;
  allocatedBudget: number;
  activeReservedWindow: number;
  runnableAllocatedWindow: number;
  runnableCount: number;
  heldCount: number;
}

export interface TransferPlannerResult {
  runnablePlans: TransferPlannerRunnablePlan[];
  activePlans: TransferPlannerActivePlan[];
  heldPlans: TransferPlannerHeldPlan[];
  laneBudgets: TransferPlannerLaneBudgetReport[];
  requestedWindowTotal: number;
  globalWindowBudget: number;
  debugReasons: string[];
}

interface ClassifiedTask {
  task: TransferPlannerTask;
  lane: TransferPlannerLane;
  sizeClass: TransferPlannerSizeClass;
  priority: TransferPlannerPriority;
  latencySensitive: boolean;
  throughputSensitive: boolean;
}

const KiB = 1024;
const MiB = 1024 * KiB;

export const DEFAULT_TRANSFER_PLANNER_POLICY: TransferPlannerPolicy = {
  globalWindowBudget: 8,
  minRequestedWindow: 1,
  maxRequestedWindow: 8,
  safetyActiveTransferCap: 4,
  laneWeights: {
    control_text: 1,
    small_file: 1,
    bulk_file: 7
  }
};

const lanes: TransferPlannerLane[] = ["control_text", "small_file", "bulk_file"];

export function planWeightedTransfers(
  tasks: readonly TransferPlannerTask[],
  policyOverrides: Partial<TransferPlannerPolicy> = {}
): TransferPlannerResult {
  const policy = normalizePolicy(policyOverrides);
  const heldPlans: TransferPlannerHeldPlan[] = [];
  const activeCandidates: ClassifiedTask[] = [];
  const queuedCandidates: ClassifiedTask[] = [];
  const debugReasons: string[] = [];

  for (const task of tasks) {
    const priority = task.priority ?? "normal";
    const terminalReason = terminalHeldReason(task);
    if (terminalReason) {
      heldPlans.push(createHeldPlan(task, priority, terminalReason, terminalReason));
      continue;
    }

    const roomReason = roomHeldReason(task);
    if (roomReason) {
      heldPlans.push(createHeldPlan(task, priority, roomReason, roomReason));
      continue;
    }

    if (task.metadataStatus !== "ready" || typeof task.sizeBytes !== "number") {
      heldPlans.push(createHeldPlan(task, priority, "missing_metadata", "metadata is not ready"));
      continue;
    }

    const classified = classifyTask(task, priority);
    if (task.state === "active") {
      activeCandidates.push(classified);
    } else {
      queuedCandidates.push(classified);
    }
  }

  const laneBudgets = computeLaneBudgets([...activeCandidates, ...queuedCandidates], policy);
  const activePlans: TransferPlannerActivePlan[] = [];
  const runnablePlans: TransferPlannerRunnablePlan[] = [];
  const laneRemaining = new Map<TransferPlannerLane, number>();
  for (const lane of lanes) {
    laneRemaining.set(lane, laneBudgets[lane] ?? 0);
  }

  let globalRemaining = policy.globalWindowBudget;
  const sortedActive = sortCandidates(activeCandidates);
  for (const candidate of sortedActive) {
    const requestedWindow = clampWindow(
      candidate.task.activeRequestedWindow ?? candidate.task.requestedWindow ?? policy.minRequestedWindow,
      policy
    );
    const laneBudget = laneRemaining.get(candidate.lane) ?? 0;
    const reservedWindow = Math.min(requestedWindow, globalRemaining);
    if (reservedWindow < policy.minRequestedWindow) {
      heldPlans.push(createHeldPlan(
        candidate.task,
        candidate.priority,
        "global_budget_exhausted",
        "active transfer could not reserve planner budget",
        candidate
      ));
      continue;
    }

    activePlans.push(createPlan(candidate, reservedWindow));
    laneRemaining.set(candidate.lane, Math.max(0, laneBudget - reservedWindow));
    globalRemaining -= reservedWindow;
  }

  const activeTransferCount = activePlans.length;
  let runnableSlots = Math.max(0, policy.safetyActiveTransferCap - activeTransferCount);

  for (const lane of lanes) {
    const laneBudget = Math.min(laneRemaining.get(lane) ?? 0, globalRemaining);
    const laneQueued = sortCandidates(queuedCandidates.filter((candidate) => candidate.lane === lane));
    const runnableCount = Math.min(laneQueued.length, laneBudget, runnableSlots);
    const selected = laneQueued.slice(0, runnableCount);
    const windows = distributeWindows(laneBudget, selected.length);

    selected.forEach((candidate, index) => {
      const requestedWindow = windows[index] ?? 0;
      if (requestedWindow < policy.minRequestedWindow) {
        return;
      }
      runnablePlans.push(createPlan(candidate, requestedWindow));
      globalRemaining -= requestedWindow;
      runnableSlots -= 1;
    });

    const skipped = laneQueued.slice(runnableCount);
    for (const candidate of skipped) {
      let reason: TransferPlannerHeldReason = "lane_budget_exhausted";
      if (runnableSlots <= 0) {
        reason = "safety_cap_reached";
      } else if (globalRemaining <= 0) {
        reason = "global_budget_exhausted";
      }
      heldPlans.push(createHeldPlan(candidate.task, candidate.priority, reason, reason, candidate));
    }
  }

  const requestedWindowTotal = sumWindows(activePlans) + sumWindows(runnablePlans);
  if (requestedWindowTotal > policy.globalWindowBudget) {
    debugReasons.push("requested windows exceeded global budget before final accounting");
  }

  const reports = lanes.map((lane) => {
    const allocatedBudget = laneBudgets[lane] ?? 0;
    const activeReservedWindow = sumWindows(activePlans.filter((plan) => plan.lane === lane));
    const runnableAllocatedWindow = sumWindows(runnablePlans.filter((plan) => plan.lane === lane));
    return {
      lane,
      requestedBudget: allocatedBudget,
      allocatedBudget,
      activeReservedWindow,
      runnableAllocatedWindow,
      runnableCount: runnablePlans.filter((plan) => plan.lane === lane).length,
      heldCount: heldPlans.filter((plan) => plan.lane === lane).length
    };
  });

  return {
    runnablePlans,
    activePlans,
    heldPlans,
    laneBudgets: reports,
    requestedWindowTotal,
    globalWindowBudget: policy.globalWindowBudget,
    debugReasons
  };
}

export function classifyTransferPlannerSize(sizeBytes: number): TransferPlannerSizeClass {
  if (sizeBytes <= 256 * KiB) return "tiny";
  if (sizeBytes <= 8 * MiB) return "small";
  if (sizeBytes <= 64 * MiB) return "medium";
  if (sizeBytes <= 512 * MiB) return "large";
  return "huge";
}

function normalizePolicy(overrides: Partial<TransferPlannerPolicy>): TransferPlannerPolicy {
  const globalWindowBudget = Math.max(1, Math.floor(overrides.globalWindowBudget ?? DEFAULT_TRANSFER_PLANNER_POLICY.globalWindowBudget));
  const minRequestedWindow = Math.max(1, Math.floor(overrides.minRequestedWindow ?? DEFAULT_TRANSFER_PLANNER_POLICY.minRequestedWindow));
  const maxRequestedWindow = Math.max(
    minRequestedWindow,
    Math.floor(overrides.maxRequestedWindow ?? DEFAULT_TRANSFER_PLANNER_POLICY.maxRequestedWindow)
  );
  const safetyActiveTransferCap = Math.max(1, Math.floor(overrides.safetyActiveTransferCap ?? DEFAULT_TRANSFER_PLANNER_POLICY.safetyActiveTransferCap));

  return {
    globalWindowBudget,
    minRequestedWindow,
    maxRequestedWindow,
    safetyActiveTransferCap,
    laneWeights: {
      ...DEFAULT_TRANSFER_PLANNER_POLICY.laneWeights,
      ...(overrides.laneWeights ?? {})
    }
  };
}

function terminalHeldReason(task: TransferPlannerTask): TransferPlannerHeldReason | null {
  if (task.cancelRequested || task.state === "cancelled") {
    return "cancelled";
  }
  if (task.state === "completed" || task.state === "failed") {
    return "terminal_status";
  }
  return null;
}

function roomHeldReason(task: TransferPlannerTask): TransferPlannerHeldReason | null {
  if (task.roomStatus === "burned") {
    return "room_burned";
  }
  if (task.roomAvailable === false || task.roomStatus === "peer_left" || task.roomStatus === "expired" || task.roomStatus === "unavailable") {
    return "room_unavailable";
  }
  return null;
}

function classifyTask(task: TransferPlannerTask, priority: TransferPlannerPriority): ClassifiedTask {
  const sizeClass = classifyTransferPlannerSize(task.sizeBytes ?? 0);
  const lane = classifyLane(task.kind, sizeClass);
  return {
    task,
    lane,
    sizeClass,
    priority,
    latencySensitive: task.latencySensitive ?? (lane === "control_text" || sizeClass === "tiny" || sizeClass === "small"),
    throughputSensitive: task.throughputSensitive ?? (lane === "bulk_file")
  };
}

function classifyLane(kind: TransferPlannerTaskKind, sizeClass: TransferPlannerSizeClass): TransferPlannerLane {
  if (kind === "text" || kind === "control" || kind === "agent" || kind === "command") {
    return "control_text";
  }
  return sizeClass === "large" || sizeClass === "huge" ? "bulk_file" : "small_file";
}

function computeLaneBudgets(
  candidates: readonly ClassifiedTask[],
  policy: TransferPlannerPolicy
): Record<TransferPlannerLane, number> {
  const demandLanes = lanes.filter((lane) => candidates.some((candidate) => candidate.lane === lane));
  const budgets: Record<TransferPlannerLane, number> = {
    control_text: 0,
    small_file: 0,
    bulk_file: 0
  };
  if (demandLanes.length === 0) {
    return budgets;
  }
  if (demandLanes.length === 1) {
    budgets[demandLanes[0]] = policy.globalWindowBudget;
    return budgets;
  }

  const totalWeight = demandLanes.reduce((total, lane) => total + Math.max(0, policy.laneWeights[lane]), 0) || demandLanes.length;
  const remainders: Array<{ lane: TransferPlannerLane; remainder: number }> = [];
  let allocated = 0;

  for (const lane of demandLanes) {
    const exact = policy.globalWindowBudget * Math.max(0, policy.laneWeights[lane]) / totalWeight;
    const laneBudget = Math.max(policy.minRequestedWindow, Math.floor(exact));
    budgets[lane] = laneBudget;
    allocated += laneBudget;
    remainders.push({ lane, remainder: exact - Math.floor(exact) });
  }

  while (allocated > policy.globalWindowBudget) {
    const candidate = [...remainders]
      .filter((entry) => budgets[entry.lane] > policy.minRequestedWindow)
      .sort((left, right) => left.remainder - right.remainder || lanes.indexOf(right.lane) - lanes.indexOf(left.lane))[0];
    if (!candidate) break;
    budgets[candidate.lane] -= 1;
    allocated -= 1;
  }

  while (allocated < policy.globalWindowBudget) {
    const candidate = [...remainders]
      .sort((left, right) => right.remainder - left.remainder || lanes.indexOf(left.lane) - lanes.indexOf(right.lane))[0];
    if (!candidate) break;
    budgets[candidate.lane] += 1;
    allocated += 1;
  }

  return budgets;
}

function sortCandidates(candidates: readonly ClassifiedTask[]): ClassifiedTask[] {
  return [...candidates].sort((left, right) => (
    priorityValue(right.priority) - priorityValue(left.priority) ||
    Number(right.latencySensitive) - Number(left.latencySensitive) ||
    (left.task.createdAt ?? 0) - (right.task.createdAt ?? 0) ||
    left.task.id.localeCompare(right.task.id)
  ));
}

function priorityValue(priority: TransferPlannerPriority): number {
  switch (priority) {
    case "urgent":
      return 3;
    case "high":
      return 2;
    case "normal":
      return 1;
    case "low":
      return 0;
  }
}

function clampWindow(value: number, policy: TransferPlannerPolicy): number {
  const normalized = Number.isFinite(value) ? Math.floor(value) : policy.minRequestedWindow;
  return Math.min(policy.maxRequestedWindow, Math.max(policy.minRequestedWindow, normalized));
}

function distributeWindows(budget: number, count: number): number[] {
  if (count <= 0 || budget <= 0) {
    return [];
  }
  const base = Math.floor(budget / count);
  let remainder = budget % count;
  return Array.from({ length: count }, () => {
    const window = base + (remainder > 0 ? 1 : 0);
    remainder -= 1;
    return window;
  });
}

function createPlan(candidate: ClassifiedTask, requestedWindow: number): TransferPlannerPlan {
  return {
    taskId: candidate.task.id,
    roomId: candidate.task.roomId,
    kind: candidate.task.kind,
    lane: candidate.lane,
    sizeClass: candidate.sizeClass,
    priority: candidate.priority,
    latencySensitive: candidate.latencySensitive,
    throughputSensitive: candidate.throughputSensitive,
    requestedWindow
  };
}

function createHeldPlan(
  task: TransferPlannerTask,
  priority: TransferPlannerPriority,
  reason: TransferPlannerHeldReason,
  debugReason: string,
  classified?: ClassifiedTask
): TransferPlannerHeldPlan {
  const sizeClass = classified?.sizeClass ?? (typeof task.sizeBytes === "number" ? classifyTransferPlannerSize(task.sizeBytes) : undefined);
  const lane = classified?.lane ?? (sizeClass ? classifyLane(task.kind, sizeClass) : undefined);
  return {
    taskId: task.id,
    roomId: task.roomId,
    kind: task.kind,
    lane,
    sizeClass,
    priority,
    reason,
    debugReason
  };
}

function sumWindows(plans: readonly TransferPlannerPlan[]): number {
  return plans.reduce((total, plan) => total + plan.requestedWindow, 0);
}

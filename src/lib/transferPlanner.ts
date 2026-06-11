export type TransferPlannerTaskKind =
  | "file"
  | "image"
  | "pasted_image"
  | "micro_group"
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

export type MicroFlowGroupMode = "fixed" | "dynamic";
export type MicroFlowGroupDispatchMode = "serial";

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
  mimeType?: string | null;
  createdAt?: number;
}

export interface TransferPlannerPolicy {
  globalWindowBudget: number;
  minRequestedWindow: number;
  maxRequestedWindow: number;
  safetyActiveTransferCap: number;
  laneWeights: Record<TransferPlannerLane, number>;
  rebalanceActiveWindows: boolean;
  microGroupMode: MicroFlowGroupMode;
  microGroupMaxChildSizeBytes: number;
  microGroupMaxGroupBytes: number;
  microGroupMaxGroupItems: number;
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

export interface MicroFlowGroupPlan {
  groupId: string;
  roomId: string;
  lane: TransferPlannerLane;
  requestedWindow: number;
  childTaskIds: string[];
  totalBytes: number;
  reason: string;
  dispatchMode: MicroFlowGroupDispatchMode;
  microGroupMode: MicroFlowGroupMode;
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
  microGroupPlans: MicroFlowGroupPlan[];
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

interface PlannedMicroFlowGroup {
  groupId: string;
  roomId: string;
  lane: TransferPlannerLane;
  childTaskIds: string[];
  totalBytes: number;
  reason: string;
  candidate: ClassifiedTask;
}

const KiB = 1024;
const MiB = 1024 * KiB;
export const MICRO_GROUP_PER_FILE_OVERHEAD_BYTES = 256 * KiB;
export const DYNAMIC_MICRO_GROUP_MIN_WINDOW_QUANTUM_BYTES = 4 * MiB;
export const DYNAMIC_MICRO_GROUP_MAX_WINDOW_QUANTUM_BYTES = 16 * MiB;
export const DYNAMIC_MICRO_GROUP_MIN_CHILD_CAP_BYTES = MiB;
export const DYNAMIC_MICRO_GROUP_MAX_CHILD_CAP_BYTES = 4 * MiB;
export const DYNAMIC_MICRO_GROUP_MIN_GROUP_CAP_BYTES = 4 * MiB;
export const DYNAMIC_MICRO_GROUP_MAX_GROUP_CAP_BYTES = 16 * MiB;
export const DYNAMIC_MICRO_GROUP_MAX_ACTIVE_WINDOWS = 1;

export const DEFAULT_TRANSFER_PLANNER_POLICY: TransferPlannerPolicy = {
  globalWindowBudget: 8,
  minRequestedWindow: 1,
  maxRequestedWindow: 8,
  safetyActiveTransferCap: 4,
  rebalanceActiveWindows: false,
  microGroupMode: "fixed",
  microGroupMaxChildSizeBytes: MiB,
  microGroupMaxGroupBytes: 4 * MiB,
  microGroupMaxGroupItems: 32,
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
  const liveMicroGroupPolicy = resolveLiveMicroGroupPolicy([...activeCandidates, ...queuedCandidates], policy);
  const microGroupCandidates = planMicroFlowGroups(queuedCandidates, liveMicroGroupPolicy);
  const groupedTaskIds = new Set(microGroupCandidates.flatMap((group) => group.childTaskIds));
  const allocationQueuedCandidates = [
    ...queuedCandidates.filter((candidate) => !groupedTaskIds.has(candidate.task.id)),
    ...microGroupCandidates.map((group) => group.candidate)
  ];

  let globalRemaining = policy.globalWindowBudget;
  if (policy.rebalanceActiveWindows) {
    const maxPlannedTransfers = Math.floor(policy.globalWindowBudget / policy.minRequestedWindow);
    const sortedActive = sortAllocationCandidates(activeCandidates);
    const plannedActive = sortedActive.slice(0, maxPlannedTransfers);
    for (const candidate of sortedActive.slice(maxPlannedTransfers)) {
      heldPlans.push(createHeldPlan(
        candidate.task,
        candidate.priority,
        "global_budget_exhausted",
        "active transfer could not reserve planner budget",
        candidate
      ));
    }

    const runnableSlots = Math.max(0, policy.safetyActiveTransferCap - plannedActive.length);
    const selectedQueued = selectRunnableCandidates(
      allocationQueuedCandidates,
      runnableSlots,
      policy.globalWindowBudget - plannedActive.length * policy.minRequestedWindow,
      policy
    );
    const selectedQueuedIds = new Set(selectedQueued.map((candidate) => candidate.task.id));
    const selected = [...plannedActive, ...selectedQueued];
    const windows = distributeWeightedWindows(selected, policy.globalWindowBudget, policy);

    selected.forEach((candidate, index) => {
      const requestedWindow = windows[index] ?? 0;
      if (requestedWindow < policy.minRequestedWindow) {
        return;
      }
      if (candidate.task.state === "active") {
        activePlans.push(createPlan(candidate, requestedWindow));
      } else {
        runnablePlans.push(createPlan(candidate, requestedWindow));
      }
      globalRemaining -= requestedWindow;
    });

    const remainingRunnableSlots = Math.max(0, runnableSlots - selectedQueued.length);
    const remainingRunnableBudget = (
      policy.globalWindowBudget -
      plannedActive.length * policy.minRequestedWindow -
      selectedQueued.length * policy.minRequestedWindow
    );
    for (const candidate of allocationQueuedCandidates) {
      if (selectedQueuedIds.has(candidate.task.id)) {
        continue;
      }
      const reason = heldReasonForSkippedRunnable(
        remainingRunnableSlots,
        remainingRunnableBudget,
        policy
      );
      heldPlans.push(createHeldPlan(candidate.task, candidate.priority, reason, reason, candidate));
    }
  } else {
    const sortedActive = sortCandidates(activeCandidates);
    for (const candidate of sortedActive) {
      const requestedWindow = clampWindow(
        candidate.task.activeRequestedWindow ?? candidate.task.requestedWindow ?? policy.minRequestedWindow,
        policy
      );
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
      globalRemaining -= reservedWindow;
    }

    const activeTransferCount = activePlans.length;
    let runnableSlots = Math.max(0, policy.safetyActiveTransferCap - activeTransferCount);
    const selectedQueued = selectRunnableCandidates(allocationQueuedCandidates, runnableSlots, globalRemaining, policy);
    const selectedQueuedIds = new Set(selectedQueued.map((candidate) => candidate.task.id));
    const windows = distributeWeightedWindows(selectedQueued, globalRemaining, policy);

    selectedQueued.forEach((candidate, index) => {
      const requestedWindow = windows[index] ?? 0;
      if (requestedWindow < policy.minRequestedWindow) {
        return;
      }
      runnablePlans.push(createPlan(candidate, requestedWindow));
      globalRemaining -= requestedWindow;
      runnableSlots -= 1;
    });

    for (const candidate of allocationQueuedCandidates) {
      if (selectedQueuedIds.has(candidate.task.id)) {
        continue;
      }
      const reason = heldReasonForSkippedRunnable(runnableSlots, globalRemaining, policy);
      heldPlans.push(createHeldPlan(candidate.task, candidate.priority, reason, reason, candidate));
    }
  }

  const requestedWindowTotal = sumWindows(activePlans) + sumWindows(runnablePlans);
  const serialMicroGroupWindows = new Map(
    runnablePlans
      .filter((plan) => plan.kind === "micro_group")
      .map((plan) => [plan.taskId, plan.requestedWindow])
  );
  const microGroupPlans = microGroupCandidates
    .filter((group) => serialMicroGroupWindows.has(group.groupId))
    .map((group) => createMicroGroupPlan(
      group,
      serialMicroGroupWindows.get(group.groupId) ?? policy.minRequestedWindow,
      policy.microGroupMode
    ));
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
    microGroupPlans,
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
  const microGroupMaxChildSizeBytes = Math.max(1, Math.floor(overrides.microGroupMaxChildSizeBytes ?? DEFAULT_TRANSFER_PLANNER_POLICY.microGroupMaxChildSizeBytes));
  const microGroupMaxGroupBytes = Math.max(
    microGroupMaxChildSizeBytes,
    Math.floor(overrides.microGroupMaxGroupBytes ?? DEFAULT_TRANSFER_PLANNER_POLICY.microGroupMaxGroupBytes)
  );
  const microGroupMaxGroupItems = Math.max(2, Math.floor(overrides.microGroupMaxGroupItems ?? DEFAULT_TRANSFER_PLANNER_POLICY.microGroupMaxGroupItems));

  return {
    globalWindowBudget,
    minRequestedWindow,
    maxRequestedWindow,
    safetyActiveTransferCap,
    rebalanceActiveWindows: overrides.rebalanceActiveWindows ?? DEFAULT_TRANSFER_PLANNER_POLICY.rebalanceActiveWindows,
    microGroupMode: overrides.microGroupMode === "fixed" || overrides.microGroupMode === "dynamic"
      ? overrides.microGroupMode
      : DEFAULT_TRANSFER_PLANNER_POLICY.microGroupMode,
    microGroupMaxChildSizeBytes,
    microGroupMaxGroupBytes,
    microGroupMaxGroupItems,
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

function sortAllocationCandidates(candidates: readonly ClassifiedTask[]): ClassifiedTask[] {
  return [...candidates].sort((left, right) => (
    priorityValue(right.priority) - priorityValue(left.priority) ||
    allocationWeight(right) - allocationWeight(left) ||
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

function selectRunnableCandidates(
  candidates: readonly ClassifiedTask[],
  runnableSlots: number,
  availableBudget: number,
  policy: TransferPlannerPolicy
): ClassifiedTask[] {
  const windowBoundedCount = Math.floor(Math.max(0, availableBudget) / policy.minRequestedWindow);
  const count = Math.min(candidates.length, Math.max(0, runnableSlots), windowBoundedCount);
  return sortAllocationCandidates(candidates).slice(0, count);
}

function heldReasonForSkippedRunnable(
  runnableSlots: number,
  availableBudget: number,
  policy: TransferPlannerPolicy
): TransferPlannerHeldReason {
  if (runnableSlots <= 0) {
    return "safety_cap_reached";
  }
  if (availableBudget < policy.minRequestedWindow) {
    return "global_budget_exhausted";
  }
  return "global_budget_exhausted";
}

function distributeWeightedWindows(
  candidates: readonly ClassifiedTask[],
  budget: number,
  policy: TransferPlannerPolicy
): number[] {
  if (candidates.length === 0 || budget < policy.minRequestedWindow) {
    return [];
  }

  const windowCount = Math.min(candidates.length, Math.floor(budget / policy.minRequestedWindow));
  const selected = candidates.slice(0, windowCount);
  const windows = selected.map(() => policy.minRequestedWindow);
  const remaining = Math.max(0, budget - selected.length * policy.minRequestedWindow);
  if (remaining === 0) {
    return windows;
  }

  const weights = selected.map(allocationWeight);
  const totalWeight = weights.reduce((total, weight) => total + weight, 0) || selected.length;
  const remainders: Array<{ index: number; remainder: number; weight: number }> = [];

  weights.forEach((weight, index) => {
    const capacity = maxRequestedWindowForCandidate(selected[index], policy) - windows[index];
    if (capacity <= 0) {
      remainders.push({ index, remainder: 0, weight });
      return;
    }

    const exact = remaining * weight / totalWeight;
    const additional = Math.min(capacity, Math.floor(exact));
    windows[index] += additional;
    remainders.push({ index, remainder: exact - Math.floor(exact), weight });
  });

  let allocated = windows.reduce((total, window) => total + window, 0);
  let guard = remaining + selected.length;
  while (allocated < budget && guard > 0) {
    const candidate = [...remainders]
      .filter((entry) => windows[entry.index] < maxRequestedWindowForCandidate(selected[entry.index], policy))
      .sort((left, right) => (
        right.remainder - left.remainder ||
        right.weight - left.weight ||
        left.index - right.index
      ))[0];
    if (!candidate) {
      break;
    }

    windows[candidate.index] += 1;
    allocated += 1;
    candidate.remainder = 0;
    guard -= 1;
  }

  return windows;
}

function allocationWeight(candidate: ClassifiedTask): number {
  return Math.max(1, candidate.task.sizeBytes ?? 1);
}

function maxRequestedWindowForCandidate(candidate: ClassifiedTask, policy: TransferPlannerPolicy): number {
  if (candidate.task.kind === "micro_group") {
    return policy.minRequestedWindow;
  }
  return policy.maxRequestedWindow;
}

function planMicroFlowGroups(
  candidates: readonly ClassifiedTask[],
  policy: TransferPlannerPolicy
): PlannedMicroFlowGroup[] {
  const groupsByKey = new Map<string, ClassifiedTask[]>();
  for (const candidate of sortMicroFlowGroupCandidates(candidates, policy)) {
    if (!isMicroFlowGroupEligible(candidate, policy)) {
      continue;
    }

    const key = microFlowGroupKey(candidate);
    const entries = groupsByKey.get(key) ?? [];
    entries.push(candidate);
    groupsByKey.set(key, entries);
  }

  const groups: PlannedMicroFlowGroup[] = [];
  for (const entries of groupsByKey.values()) {
    let pending: ClassifiedTask[] = [];
    let pendingBytes = 0;

    for (const candidate of entries) {
      const sizeBytes = microFlowGroupCandidateCost(candidate, policy);
      if (
        pending.length >= policy.microGroupMaxGroupItems ||
        (pending.length > 0 && pendingBytes + sizeBytes > policy.microGroupMaxGroupBytes)
      ) {
        pushMicroFlowGroup(groups, pending, policy);
        pending = [];
        pendingBytes = 0;
      }

      pending.push(candidate);
      pendingBytes += sizeBytes;
    }

    pushMicroFlowGroup(groups, pending, policy);
  }

  if (policy.microGroupMode === "dynamic") {
    return groups
      .sort((left, right) => (
        right.childTaskIds.length - left.childTaskIds.length ||
        right.totalBytes - left.totalBytes ||
        left.groupId.localeCompare(right.groupId)
      ))
      .slice(0, DYNAMIC_MICRO_GROUP_MAX_ACTIVE_WINDOWS);
  }
  return groups.sort((left, right) => left.groupId.localeCompare(right.groupId));
}

function sortMicroFlowGroupCandidates(
  candidates: readonly ClassifiedTask[],
  policy: TransferPlannerPolicy
): ClassifiedTask[] {
  if (policy.microGroupMode === "fixed") {
    return sortAllocationCandidates(candidates);
  }
  return [...candidates].sort((left, right) => (
    (left.task.sizeBytes ?? 0) - (right.task.sizeBytes ?? 0) ||
    (left.task.createdAt ?? 0) - (right.task.createdAt ?? 0) ||
    left.task.id.localeCompare(right.task.id)
  ));
}

function pushMicroFlowGroup(
  groups: PlannedMicroFlowGroup[],
  candidates: readonly ClassifiedTask[],
  policy: TransferPlannerPolicy
) {
  if (candidates.length < 2) {
    return;
  }

  const sorted = sortCandidates(candidates);
  const first = sorted[0];
  const totalBytes = sorted.reduce((total, candidate) => total + microFlowGroupCandidateCost(candidate, policy), 0);
  const childTaskIds = sorted.map((candidate) => candidate.task.id);
  const groupId = [
    "micro-group",
    first.task.roomId,
    first.lane,
    first.sizeClass,
    broadMimeFamily(first.task.mimeType),
    childTaskIds[0]
  ].join(":");
  const priority = sorted.reduce((highest, candidate) => (
    priorityValue(candidate.priority) > priorityValue(highest) ? candidate.priority : highest
  ), first.priority);
  const task: TransferPlannerTask = {
    id: groupId,
    roomId: first.task.roomId,
    kind: "micro_group",
    state: "queued",
    metadataStatus: "ready",
    sizeBytes: totalBytes,
    priority,
    latencySensitive: true,
    throughputSensitive: false,
    roomStatus: "active",
    roomAvailable: true,
    createdAt: Math.min(...sorted.map((candidate) => candidate.task.createdAt ?? 0))
  };

  groups.push({
    groupId,
    roomId: first.task.roomId,
    lane: first.lane,
    childTaskIds,
    totalBytes,
    reason: `grouped ${childTaskIds.length} file-like tasks using ${policy.microGroupMode} one-window policy`,
    candidate: {
      task,
      lane: first.lane,
      sizeClass: classifyTransferPlannerSize(totalBytes),
      priority,
      latencySensitive: true,
      throughputSensitive: false
    }
  });
}

function isMicroFlowGroupEligible(candidate: ClassifiedTask, policy: TransferPlannerPolicy): boolean {
  return isFileLikeTaskKind(candidate.task.kind) &&
    candidate.task.state === "queued" &&
    candidate.task.roomStatus === "active" &&
    candidate.task.roomAvailable !== false &&
    candidate.task.metadataStatus === "ready" &&
    typeof candidate.task.sizeBytes === "number" &&
    microFlowGroupCandidateCost(candidate, policy) <= policy.microGroupMaxChildSizeBytes &&
    (candidate.sizeClass === "tiny" || candidate.sizeClass === "small") &&
    candidate.lane === "small_file";
}

function isFileLikeTaskKind(kind: TransferPlannerTaskKind): boolean {
  return kind === "file" || kind === "image" || kind === "pasted_image";
}

function microFlowGroupKey(candidate: ClassifiedTask): string {
  return [
    candidate.task.roomId,
    candidate.lane,
    candidate.sizeClass,
    "file_like",
    broadMimeFamily(candidate.task.mimeType)
  ].join(":");
}

function broadMimeFamily(mimeType?: string | null): string {
  const trimmed = mimeType?.trim().toLowerCase();
  if (!trimmed || !trimmed.includes("/")) {
    return "unknown";
  }
  return trimmed.split("/", 1)[0] || "unknown";
}

function createMicroGroupPlan(
  group: PlannedMicroFlowGroup,
  requestedWindow: number,
  microGroupMode: MicroFlowGroupMode
): MicroFlowGroupPlan {
  return {
    groupId: group.groupId,
    roomId: group.roomId,
    lane: group.lane,
    requestedWindow,
    childTaskIds: group.childTaskIds,
    totalBytes: group.totalBytes,
    reason: group.reason,
    dispatchMode: "serial",
    microGroupMode
  };
}

function resolveLiveMicroGroupPolicy(
  candidates: readonly ClassifiedTask[],
  policy: TransferPlannerPolicy
): TransferPlannerPolicy {
  if (policy.microGroupMode === "fixed") {
    return policy;
  }

  const serviceCandidates = candidates.filter((candidate) => (
    candidate.task.state === "queued" || candidate.task.state === "active"
  ));
  const hasBulkCandidate = serviceCandidates.some((candidate) => candidate.lane === "bulk_file");
  const contention = serviceCandidates.length > policy.safetyActiveTransferCap ||
    (hasBulkCandidate && serviceCandidates.length > 1);
  if (!contention) {
    return {
      ...policy,
      microGroupMaxChildSizeBytes: 1,
      microGroupMaxGroupBytes: 1
    };
  }

  const largestReadyQueuedBytes = Math.max(0, ...serviceCandidates.map((candidate) => candidate.task.sizeBytes ?? 0));
  const oneWindowQuantumBytes = clampBytes(
    Math.floor(largestReadyQueuedBytes / policy.globalWindowBudget),
    DYNAMIC_MICRO_GROUP_MIN_WINDOW_QUANTUM_BYTES,
    DYNAMIC_MICRO_GROUP_MAX_WINDOW_QUANTUM_BYTES
  );
  const dynamicChildCapBytes = clampBytes(
    oneWindowQuantumBytes,
    DYNAMIC_MICRO_GROUP_MIN_CHILD_CAP_BYTES,
    DYNAMIC_MICRO_GROUP_MAX_CHILD_CAP_BYTES
  );
  const dynamicGroupCapBytes = clampBytes(
    oneWindowQuantumBytes,
    DYNAMIC_MICRO_GROUP_MIN_GROUP_CAP_BYTES,
    DYNAMIC_MICRO_GROUP_MAX_GROUP_CAP_BYTES
  );
  return {
    ...policy,
    microGroupMaxChildSizeBytes: dynamicChildCapBytes,
    microGroupMaxGroupBytes: dynamicGroupCapBytes
  };
}

function microFlowGroupCandidateCost(candidate: ClassifiedTask, policy: TransferPlannerPolicy): number {
  return (candidate.task.sizeBytes ?? 0) +
    (policy.microGroupMode === "dynamic" ? MICRO_GROUP_PER_FILE_OVERHEAD_BYTES : 0);
}

function clampBytes(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, Math.floor(value)));
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

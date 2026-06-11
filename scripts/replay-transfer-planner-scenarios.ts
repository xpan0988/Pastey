import {
  DEFAULT_TRANSFER_PLANNER_POLICY,
  planWeightedTransfers,
  type TransferPlannerHeldPlan,
  type TransferPlannerPolicy,
  type TransferPlannerResult,
  type TransferPlannerTask
} from "../src/lib/transferPlanner";
import {
  cancelQueueItem,
  createTransferSchedulerState,
  enqueueTransferBatch,
  summarizeMicroFlowGroupPlanning,
  type MicroFlowGroupPlanningDiagnostics,
  type TransferQueueInput,
  type TransferSchedulerState
} from "../src/lib/transferScheduler";

const KiB = 1024;
const MiB = 1024 * KiB;
const GiB = 1024 * MiB;

interface ScenarioItem {
  id: string;
  roomId?: string;
  kind?: TransferPlannerTask["kind"];
  sizeBytes?: number;
  metadataReady?: boolean;
  state?: TransferPlannerTask["state"];
  roomStatus?: TransferPlannerTask["roomStatus"];
  roomAvailable?: boolean;
  cancelRequested?: boolean;
  mimeType?: string | null;
  createdAt?: number;
}

interface Scenario {
  name: string;
  items: ScenarioItem[];
  roomStatus?: "active" | "burned" | "peer_left" | "expired" | "unavailable";
}

interface ReplayMode {
  name: "fixed" | "dynamic";
  result: TransferPlannerResult;
  diagnostics: MicroFlowGroupPlanningDiagnostics;
  policy: TransferPlannerPolicy;
}

const scenarios: Scenario[] = [
  {
    name: "two_1_2MiB_files_only",
    items: [
      item("small-a", 1.2 * MiB, 1),
      item("small-b", 1.2 * MiB, 2)
    ]
  },
  {
    name: "huge_plus_many_0_3_to_1_3MiB",
    items: [
      item("huge", 2 * GiB, 1),
      ...[0.3, 0.45, 0.7, 0.9, 1.1, 1.2, 1.3, 0.35, 0.8, 1.25].map((size, index) => (
        item(`mixed-small-${index}`, size * MiB, index + 2)
      ))
    ]
  },
  {
    name: "many_100KiB_to_900KiB_files",
    items: Array.from({ length: 20 }, (_, index) => (
      item(`tiny-${index}`, (100 + index * 40) * KiB, index + 1)
    ))
  },
  {
    name: "mixed_chaos_recent_log_shape",
    items: [
      item("large", 2 * GiB, 1),
      item("medium-a", 64 * MiB, 2),
      item("medium-b", 128 * MiB, 3),
      item("medium-c", 256 * MiB, 4),
      item("small-over-a", 1.1 * MiB, 5),
      item("small-over-b", 1.2 * MiB, 6),
      item("small-over-c", 1.3 * MiB, 7),
      item("single-sub-mib", 0.35 * MiB, 8)
    ]
  },
  {
    name: "metadata_missing_files",
    items: Array.from({ length: 6 }, (_, index) => ({
      ...item(`missing-${index}`, 300 * KiB, index + 1),
      metadataReady: false
    }))
  },
  {
    name: "cancelled_and_burned_room_files",
    items: [
      { ...item("cancelled-a", 300 * KiB, 1), cancelRequested: true },
      { ...item("cancelled-b", 400 * KiB, 2), state: "cancelled" },
      { ...item("burned-a", 300 * KiB, 3), roomId: "burned-room", roomStatus: "burned", roomAvailable: false },
      { ...item("burned-b", 400 * KiB, 4), roomId: "burned-room", roomStatus: "burned", roomAvailable: false }
    ]
  }
];

for (const scenario of scenarios) {
  replayScenario(scenario);
}

function replayScenario(scenario: Scenario) {
  const fixedPolicy: TransferPlannerPolicy = { ...DEFAULT_TRANSFER_PLANNER_POLICY, microGroupMode: "fixed" };
  const tasks = scenario.items.map(toPlannerTask);
  const state = toSchedulerState(scenario);
  const rooms = roomsForScenario(scenario);
  const fixedDiagnostics = summarizeMicroFlowGroupPlanning(state, rooms, new Set(), fixedPolicy);
  const dynamicPolicy: TransferPlannerPolicy = {
    ...fixedPolicy,
    microGroupMode: "dynamic",
    microGroupMaxChildSizeBytes: fixedDiagnostics.dynamicChildCapBytes,
    microGroupMaxGroupBytes: fixedDiagnostics.dynamicGroupCapBytes
  };
  const dynamicDiagnostics = summarizeMicroFlowGroupPlanning(state, rooms, new Set(), dynamicPolicy);
  const fixed: ReplayMode = {
    name: "fixed",
    result: planWeightedTransfers(tasks, fixedPolicy),
    diagnostics: fixedDiagnostics,
    policy: fixedPolicy
  };
  const dynamic: ReplayMode = {
    name: "dynamic",
    result: planWeightedTransfers(tasks, dynamicPolicy),
    diagnostics: dynamicDiagnostics,
    policy: dynamicPolicy
  };

  printSummary(scenario.name, fixed, dynamic, fixedDiagnostics);
}

function item(id: string, sizeBytes: number, createdAt: number): ScenarioItem {
  return {
    id,
    sizeBytes: Math.round(sizeBytes),
    metadataReady: true,
    state: "queued",
    roomStatus: "active",
    roomAvailable: true,
    mimeType: "application/octet-stream",
    createdAt
  };
}

function toPlannerTask(input: ScenarioItem): TransferPlannerTask {
  return {
    id: input.id,
    roomId: input.roomId ?? "room-1",
    kind: input.kind ?? "file",
    state: input.state ?? "queued",
    metadataStatus: input.metadataReady === false ? "unknown" : "ready",
    sizeBytes: input.metadataReady === false ? null : Math.round(input.sizeBytes ?? 0),
    roomStatus: input.roomStatus ?? "active",
    roomAvailable: input.roomAvailable,
    cancelRequested: input.cancelRequested,
    mimeType: input.mimeType,
    createdAt: input.createdAt
  };
}

function toSchedulerState(scenario: Scenario): TransferSchedulerState {
  let state = createTransferSchedulerState();
  const inputsByRoom = new Map<string, Array<{ entry: ScenarioItem; input: TransferQueueInput }>>();
  for (const entry of scenario.items) {
    const roomId = entry.roomId ?? "room-1";
    const base = {
      path: `/planner-replay/${entry.id}`,
      displayName: `${entry.id}.bin`,
      mimeType: entry.mimeType ?? "application/octet-stream"
    };
    const input = entry.metadataReady === false || typeof entry.sizeBytes !== "number"
      ? base
      : {
        ...base,
        sizeBytes: Math.round(entry.sizeBytes),
        modifiedMs: entry.createdAt ?? Math.round(entry.sizeBytes)
      };
    const inputs = inputsByRoom.get(roomId) ?? [];
    inputs.push({ entry, input });
    inputsByRoom.set(roomId, inputs);
  }

  const itemIdBySourceId = new Map<string, string>();
  for (const [roomId, entries] of inputsByRoom.entries()) {
    const before = new Set(Object.keys(state.items));
    state = enqueueTransferBatch(state, roomId, entries.map((entry) => entry.input));
    const addedItemIds = Object.keys(state.items).filter((itemId) => !before.has(itemId));
    entries.forEach(({ entry }, index) => {
      const itemId = addedItemIds[index];
      if (itemId) {
        itemIdBySourceId.set(entry.id, itemId);
      }
    });
  }

  for (const entry of scenario.items) {
    if (entry.cancelRequested || entry.state === "cancelled") {
      const itemId = itemIdBySourceId.get(entry.id);
      if (itemId) {
        state = cancelQueueItem(state, itemId);
      }
    }
  }

  return state;
}

function roomsForScenario(scenario: Scenario) {
  const roomIds = new Set(scenario.items.map((entry) => entry.roomId ?? "room-1"));
  return [...roomIds].map((id) => ({
    id,
    status: scenario.items.find((entry) => (entry.roomId ?? "room-1") === id)?.roomStatus ?? scenario.roomStatus ?? "active"
  }));
}

function printSummary(
  scenario: string,
  fixed: ReplayMode,
  dynamic: ReplayMode,
  fixedDiagnostics: MicroFlowGroupPlanningDiagnostics
) {
  const fields = {
    scenario,
    fixed_micro_group_plans: fixed.result.microGroupPlans.length,
    fixed_grouped_children: groupedChildren(fixed.result),
    fixed_requested_window_total: fixed.result.requestedWindowTotal,
    fixed_held_reasons: heldReasons(fixed.result.heldPlans),
    dynamic_micro_group_plans: dynamic.result.microGroupPlans.length,
    dynamic_grouped_children: groupedChildren(dynamic.result),
    dynamic_requested_window_total: dynamic.result.requestedWindowTotal,
    dynamic_skip_reason: liveSkipReason(dynamic),
    fixed_candidate_children: fixed.diagnostics.eligibleTinyCandidates,
    dynamic_candidate_children: dynamic.diagnostics.eligibleTinyCandidates,
    contention: fixedDiagnostics.contention,
    contention_severity: fixedDiagnostics.contentionSeverity,
    one_window_quantum_bytes: fixedDiagnostics.oneWindowQuantumBytes,
    dynamic_child_cap_bytes: fixedDiagnostics.dynamicChildCapBytes,
    dynamic_group_cap_bytes: fixedDiagnostics.dynamicGroupCapBytes,
    global_window_budget: fixed.result.globalWindowBudget
  };
  console.log(Object.entries(fields).map(([key, value]) => `${key}=${value}`).join(" "));
}

function liveSkipReason(mode: ReplayMode): string {
  if (mode.result.microGroupPlans.length > 0) {
    return "group_planned";
  }
  if (!mode.diagnostics.contention) {
    return "no_contention";
  }
  return mode.diagnostics.microGroupSkipReason;
}

function groupedChildren(result: TransferPlannerResult): number {
  return result.microGroupPlans.reduce((total, plan) => total + plan.childTaskIds.length, 0);
}

function heldReasons(heldPlans: readonly TransferPlannerHeldPlan[]): string {
  const counts = new Map<string, number>();
  for (const plan of heldPlans) {
    counts.set(plan.reason, (counts.get(plan.reason) ?? 0) + 1);
  }
  if (counts.size === 0) {
    return "none";
  }
  return [...counts.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([reason, count]) => `${reason}:${count}`)
    .join(",");
}

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { mergeRoomItems, reconcileRoomItems, reconcileRooms } from "../src/lib/authoritativeSnapshots";
import { chooseInitialBridgeId, reconcileSelectedBridgeId, visibleBridgeRooms } from "../src/lib/bridgeSelection";
import { ownAsyncDisposer } from "../src/lib/subscriptionLifecycle";
import { mergeTransferEvent } from "../src/lib/transferState";
import { uniqueNearbyDevices } from "../src/features/workspace/workspaceViewModel";
import {
  nativeAgentInterruptedRecoveryRequiresAction,
  nativeAgentMovementBlocksNewRun,
  observeNativeAgentMovementAfterReturnRetry,
  nativeAgentRecoveryRequiresAction,
  nativeAgentRemoteReconciliationRequired,
  nativeAgentConsequenceAbandonmentRequired,
  nativeAgentTaskNeedsObservation,
} from "../src/features/workspace/AgentTaskLifecycle";
import type { FileTransferProgressEvent, NearbyDevice, RoomInfo, RoomItem } from "../src/lib/types";

function room(peerConnected: boolean): RoomInfo {
  return {
    id: "room-a",
    created_at: 1,
    expires_at: 2,
    status: "active",
    local_role: "creator",
    auto_burn_after_expiry: false,
    peer_connected: peerConnected,
  };
}

function item(status: RoomItem["status"] = "sent"): RoomItem {
  return {
    id: "item-a",
    room_id: "room-a",
    direction: "outgoing",
    item_kind: "text",
    payload_type: "text",
    size_bytes: 5,
    created_at: 1,
    status,
    text: "hello",
  };
}

function transfer(status: FileTransferProgressEvent["status"] = "transferring"): FileTransferProgressEvent {
  return {
    transfer_id: "transfer-a",
    room_id: "room-a",
    item_id: "item-a",
    direction: "incoming",
    file_name: "hello.txt",
    file_size: 5,
    chunk_size: 5,
    total_chunks: 1,
    transferred_bytes: status === "completed" ? 5 : 0,
    status,
    current_speed_bps: 0,
    average_speed_bps: 0,
  };
}

test("late native-listener registration is unlistened after StrictMode cleanup", async () => {
  let resolveRegistration!: (dispose: () => void) => void;
  const registration = new Promise<() => void>((resolve) => { resolveRegistration = resolve; });
  let unlistenCount = 0;

  const dispose = ownAsyncDisposer(registration);
  dispose();
  dispose();
  resolveRegistration(() => { unlistenCount += 1; });
  await registration;
  await Promise.resolve();

  assert.equal(unlistenCount, 1);
});

test("resolved native-listener registration is cleaned up exactly once", async () => {
  let unlistenCount = 0;
  const dispose = ownAsyncDisposer(Promise.resolve(() => { unlistenCount += 1; }));
  await Promise.resolve();
  dispose();
  dispose();
  assert.equal(unlistenCount, 1);
});

test("authoritative room snapshots preserve identity until device liveness changes", () => {
  const current = [room(false)];
  assert.equal(reconcileRooms(current, [room(false)]), current);
  const connected = reconcileRooms(current, [room(true)]);
  assert.notEqual(connected, current);
  assert.equal(connected[0].peer_connected, true);
});

test("poll and event reconciliation keep each room item once", () => {
  const current = [item()];
  assert.equal(reconcileRoomItems(current, [item(), item()]), current);
  const merged = mergeRoomItems(current, [item(), { ...item(), id: "item-b", created_at: 2 }]);
  assert.deepEqual(merged.map((entry) => entry.id), ["item-b", "item-a"]);
});

test("replayed transfer progress does not create a frontend state transition", () => {
  const event = transfer();
  const first = mergeTransferEvent({}, event, new Set());
  assert.equal(mergeTransferEvent(first, { ...event }, new Set()), first);
  const completed = mergeTransferEvent(first, transfer("completed"), new Set());
  assert.equal(completed["transfer-a"].status, "completed");
  assert.equal(mergeTransferEvent(completed, transfer(), new Set()), completed);
});

test("new Bridge view consumes the real renderer-safe nearby discovery binding", () => {
  const screen = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");
  assert.match(screen, /onListNearby\(\)/);
  assert.match(screen, /onJoinNearby\(device\.device_id\)/);
  assert.match(screen, /device\.display_name/);
  assert.match(screen, /\? "Refreshing…" : "Refresh"/);
  assert.match(screen, /run\("create", onCreate\)/);
  assert.match(screen, /A new Bridge and 8-digit code are created only after this explicit action/);
  assert.doesNotMatch(screen, /useEffect[\s\S]{0,800}onCreate\(/);
  assert.doesNotMatch(screen, /Scan nearby.*disabled/);
  assert.doesNotMatch(screen, /Nearby discovery results are not exposed/);
  assert.match(app, /onListNearbyDevices=\{listNearbyDevices\}/);
  assert.match(app, /nearbyDiscoveryAvailable=\{hasTauriRuntime\(\)\}/);
  assert.match(app, /requestNearbyJoin\(deviceId\)/);
});

test("nearby refresh deduplicates one physical device without mutating Bridge state", () => {
  const device: NearbyDevice = {
    device_id: "device-a",
    display_name: "Laptop",
    platform: "macOS",
    app_version: "1.9.2",
    availability: "Available",
    capabilities: ["nearby_join"],
    last_seen_seconds_ago: 0,
    compatible: true,
  };
  const duplicate = { ...device, display_name: "Laptop refreshed", last_seen_seconds_ago: 1 };
  assert.deepEqual(uniqueNearbyDevices([device, duplicate]), [duplicate]);
});

test("Devices is inspection-only and navigation cannot create or join a Bridge", () => {
  const screens = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const workspace = readFileSync("src/features/workspace/WorkspaceV2.tsx", "utf8");
  const devicesBody = screens.slice(screens.indexOf("export function DevicesScreen"), screens.indexOf("export function NewBridgeScreen"));
  assert.doesNotMatch(devicesBody, /onCreate|onJoin|onListNearby|onJoinNearby|Add device|Find nearby/);
  assert.match(devicesBody, /Device admission is not supported from this view/);
  assert.match(workspace, /route === "devices" \? <DevicesScreen room=\{activeRoom\}/);
});

test("Bridge Device Check uses exact HostRef and remains explicitly user initiated", () => {
  const screens = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  const devicesBody = screens.slice(screens.indexOf("export function DevicesScreen"), screens.indexOf("export function NewBridgeScreen"));
  assert.match(devicesBody, /runBridgeDeviceSelfCheck\(bridgeId, hostRef\)/);
  assert.match(devicesBody, /deviceCheckKey\(bridgeId, hostRef\)/);
  assert.match(devicesBody, /inFlight\.current\.has\(checkKey\)/);
  assert.match(devicesBody, /disabled=\{loading\}/);
  assert.match(devicesBody, /Checking…/);
  assert.match(devicesBody, /Retry/);
  assert.match(devicesBody, /Not configured/);
  assert.match(devicesBody, /Managed readiness/);
  assert.match(devicesBody, /Managed E2E self-check/);
  assert.doesNotMatch(devicesBody, /useEffect[\s\S]{0,500}runBridgeDeviceSelfCheck/);
  assert.doesNotMatch(devicesBody, /bridge_peers|transport key|session_pair_ref|SQLite|reconnect:/);
  assert.match(bindings, /invoke\("run_bridge_device_diagnostics", \{ bridgeId, hostRef \}\)/);
  assert.match(bindings, /invoke\("run_bridge_device_self_check", \{ bridgeId, hostRef \}\)/);
});

test("capability acquisition confirmation binding remains display-only", () => {
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  const types = readFileSync("src/lib/types.ts", "utf8");
  assert.match(bindings, /invoke\("confirm_capability_acquisition", \{ input \}\)/);
  assert.match(types, /CapabilityAcquisitionConfirmationOutcomeV1 = "confirmed" \| "cancelled"/);
  const contract = types.slice(
    types.indexOf("export interface CapabilityAcquisitionRequestV1"),
    types.indexOf("export type BenchmarkMode")
  );
  for (const forbidden of ["path", "command", "args", "shell", "installer", "credentials", "authority"]) {
    assert.doesNotMatch(contract.toLowerCase(), new RegExp(`\\b${forbidden}\\??\\s*:`));
  }
});

test("Bridge Devices consumes the read-only NodeList without turning it into a Check or admission flow", () => {
  const screens = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  const devicesBody = screens.slice(screens.indexOf("export function DevicesScreen"), screens.indexOf("export function NewBridgeScreen"));
  assert.match(devicesBody, /getBridgeNodeListProjection\(room\.id\)/);
  assert.match(devicesBody, /nodeList\.nodes\.map/);
  assert.match(bindings, /invoke\("get_bridge_node_list_projection", \{ bridgeId \}\)/);
  assert.doesNotMatch(devicesBody, /useEffect[\s\S]{0,600}runBridgeDeviceSelfCheck/);
  assert.doesNotMatch(devicesBody, /selectHost|composeNative|approveNative|startNative/);
});

test("opening New Bridge is a choice view; creation is wired only to explicit Create", () => {
  const screens = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const workspace = readFileSync("src/features/workspace/WorkspaceV2.tsx", "utf8");
  const bridge = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.match(workspace, /route === "new-bridge" \? <NewBridgeScreen onCreate=\{createBridge\}/);
  assert.match(screens, /onClick=\{\(\) => void run\("create", onCreate\)\}/);
  assert.match(bridge, /onNewBridge/);
  assert.doesNotMatch(bridge, /onCreateBridge|createRoom/);
});

test("selection reconciliation removes a burned Bridge and ignores late snapshots", () => {
  const active = room(true);
  const other = { ...room(false), id: "room-b" };
  assert.equal(chooseInitialBridgeId([active, other], "room-a"), "room-a");
  assert.equal(reconcileSelectedBridgeId("room-a", [active, other], new Set(["room-a"])), "");
  assert.deepEqual(visibleBridgeRooms([active, other], new Set(["room-a"])).map((entry) => entry.id), ["room-b"]);
  assert.equal(reconcileSelectedBridgeId("room-a", [active], new Set(["room-a"])), "");
});

test("Developer Mode stays inside the selected Bridge and receiver observation is token-free", () => {
  const routeTypes = readFileSync("src/features/workspace/workspaceTypes.ts", "utf8");
  const workspace = readFileSync("src/features/workspace/WorkspaceV2.tsx", "utf8");
  const bridge = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.doesNotMatch(routeTypes, /"developer"/);
  assert.doesNotMatch(workspace, /route === "developer"/);
  assert.match(workspace, /bridgeMode === "developer"/);
  assert.match(bridge, /developerMode \? <DeveloperModeScreen/);
  assert.match(workspace, /getDeveloperTerminalWorkspace\(roomId\)/);
  assert.match(workspace, /Developer Mode request/);
  assert.match(workspace, />Deny</);
  assert.match(workspace, /Accept/);
});

test("Bridge send selection never falls back from a stale session to another peer", () => {
  const bridge = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.match(bridge, /peers\.find\(\(peer\) => peer\.peerSessionId === selectedPeerId\) \?\? null/);
  assert.doesNotMatch(bridge, /peerSessionId === selectedPeerId\) \?\? peers\[0\]/);
});

test("nearby polling and native subscriptions have explicit remount cleanup", () => {
  const screen = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");
  assert.equal(screen.match(/window\.setInterval/g)?.length, 1);
  assert.equal(screen.match(/window\.clearInterval/g)?.length, 1);
  assert.match(screen, /cancelled = true/);
  assert.match(app, /disposeAll\(disposers\)/);
  assert.match(app, /setWorkspaceFocusRequest/);
});

test("all workspace Tauri listeners use late-resolution-safe ownership", () => {
  for (const path of [
    "src/App.tsx",
    "src/features/workspace/AgentTaskLifecycle.tsx",
    "src/components/DeveloperTerminalViewport.tsx",
  ]) {
    const source = readFileSync(path, "utf8");
    const listenCalls = source.match(/\blisten</g)?.length ?? 0;
    const ownedCalls = source.match(/ownAsyncDisposer\(listen</g)?.length ?? 0;
    assert.equal(ownedCalls, listenCalls, path);
  }
});

test("reconciliation-required Native Agent movement remains exclusive while ordinary interruption releases", () => {
  const interrupted = {
    schemaVersion: "pastey-native-agent-workspace-movement-v1" as const,
    movementId: "movement",
    taskId: "task",
    agentId: "agent.coding.codex",
    sourceWorkspaceName: "workspace",
    targetHostRef: "host:remote",
    reviewSummary: "Review",
    state: "interrupted" as const,
    code: "native_agent_reconciliation_required",
  };
  assert.equal(nativeAgentMovementBlocksNewRun(interrupted), true);
  assert.equal(nativeAgentMovementBlocksNewRun({ ...interrupted, code: "outbound_transfer_failed" }), false);
  const captureFailure = { ...interrupted, code: "native_agent_result_snapshot_recovery_failed" };
  assert.equal(nativeAgentMovementBlocksNewRun(captureFailure), true);
  assert.equal(nativeAgentConsequenceAbandonmentRequired(captureFailure), true);
  assert.equal(nativeAgentRecoveryRequiresAction(null, captureFailure), true);
  for (const code of ["conflict_result_retention_required", "conflict_result_retention_failed"]) {
    const conflictRepair = { ...interrupted, code };
    assert.equal(nativeAgentMovementBlocksNewRun(conflictRepair), true);
    assert.equal(nativeAgentConsequenceAbandonmentRequired(conflictRepair), true);
    assert.equal(nativeAgentRecoveryRequiresAction(null, conflictRepair), true);
  }
  assert.equal(nativeAgentRecoveryRequiresAction(null, { ...interrupted, state: "returning_result", code: null }), true);
  assert.equal(nativeAgentMovementBlocksNewRun({ ...interrupted, state: "cancelled", code: null }), false);
});

test("Native Agent recovery uses the existing card with durable refresh and explicit repair actions", () => {
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  const card = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  assert.match(lifecycle, /getNativeAgentRecoveryProjection\(roomId\)/);
  assert.match(lifecycle, /await loadRecoveryProjection\(\)/);
  assert.match(lifecycle, /getNativeAgentWorkspaceMovementStatus\(movement\.movementId\)/);
  assert.match(lifecycle, /reconcileRemoteNativeAgentTask\(\s*roomId,\s*recoveryCorrelation\.targetHostRef,\s*recoveryCorrelation\.taskId/);
  assert.match(lifecycle, /stopBridgeNativeAgentTask\(roomId, recoveryCorrelation\.taskId\)/);
  assert.match(card, />Load recovery details</);
  assert.match(card, /disabled=\{agent\.busy \|\| !agent\.recoveryTaskId\} onClick=\{\(\) => void agent\.stopRecovery\(\)\}/);
  assert.match(card, /agent\.recoveryTargetHostRef \? <button[^>]*onClick=\{\(\) => void agent\.reconcileRemote\(\)\}>Reconcile<\/button> : null/);
  assert.match(card, /Pastey will not reuse this workspace until the task is reconciled or explicitly stopped/);
  assert.match(card, />Reconcile</);
  assert.match(card, />Stop</);
  assert.match(card, />Abandon recovery</);
  assert.match(card, /agent\.retryingResultReturn \? "Retrying result Return…" : "Retry result Return"/);
  assert.match(lifecycle, /setReturnRetryObservation\(\{ roomId, movement \}\)/);
  assert.match(lifecycle, /observeNativeAgentMovementAfterReturnRetry\(/);
  assert.match(lifecycle, /RETURN_RETRY_MAX_POLLS = 20/);
  assert.match(lifecycle, /nativeAgentTaskNeedsObservation\(observedStatus\)/);
  assert.match(lifecycle, /native_agent_outcome_unknown/);
  assert.match(lifecycle, /reconcileRemoteNativeAgentTask\(\s*roomId,\s*movement\.targetHostRef,\s*movement\.taskId,\s*movement\.movementId/);
  assert.match(card, /Codex completed, but Pastey could not safely capture its result for return/);
  assert.match(bindings, /invoke\("get_native_agent_recovery_projection", \{ roomId \}\)/);
  const recoveryType = bindings.slice(
    bindings.indexOf("export interface NativeAgentRecoveryProjection"),
    bindings.indexOf("export function listNativeAgentCapabilities"),
  );
  for (const privateField of ["path", "threadId", "turnId", "provider", "auth", "processId", "sessionReused"]) {
    assert.doesNotMatch(recoveryType, new RegExp(privateField, "i"));
  }
});

test("Retry result Return observes a later backend transition in the same mounted lifecycle", async () => {
  const initial = {
    schemaVersion: "pastey-native-agent-workspace-movement-v1" as const,
    movementId: "movement-apply-retry",
    taskId: "task-apply-retry",
    agentId: "agent.coding.codex",
    sourceWorkspaceName: "workspace",
    targetHostRef: "host:remote",
    reviewSummary: "Review",
    state: "interrupted" as const,
    code: "result_apply_interrupted",
  };
  const completed = { ...initial, state: "completed" as const, code: null };
  const responses = [initial, completed];
  const observed: string[] = [];
  let reads = 0;
  await observeNativeAgentMovementAfterReturnRetry(
    initial,
    async (movementId) => {
      assert.equal(movementId, initial.movementId);
      return responses[reads++];
    },
    (movement) => observed.push(movement.state),
    () => false,
    async () => {},
  );
  assert.equal(reads, 2, "an unchanged acknowledgement must not stop observation");
  assert.deepEqual(observed, ["interrupted", "completed"]);
});

test("Native Agent recovery drains only after the current item no longer requires action", () => {
  const task = {
    schemaVersion: "pastey-native-agent-task-v1" as const,
    taskId: "task-one",
    agentId: "agent.coding.codex",
    workspaceName: "workspace",
    state: "interrupted" as const,
    code: "native_agent_reconciliation_required",
  };
  assert.equal(nativeAgentRecoveryRequiresAction(task, null), true);
  assert.equal(nativeAgentInterruptedRecoveryRequiresAction(task, null), true);
  assert.equal(nativeAgentInterruptedRecoveryRequiresAction({ ...task, code: "native_agent_outcome_unknown" }, null), true);
  assert.equal(nativeAgentRemoteReconciliationRequired({ ...task, code: "native_agent_outcome_unknown" }, null), true);
  assert.equal(nativeAgentTaskNeedsObservation({ ...task, code: "native_agent_outcome_unknown" }), true);
  assert.equal(nativeAgentTaskNeedsObservation({ ...task, state: "completed", code: null }), false);
  assert.equal(nativeAgentRecoveryRequiresAction({ ...task, state: "cancelled", code: "native_agent_cancel_requested" }, null), false);
  assert.equal(nativeAgentInterruptedRecoveryRequiresAction(null, {
    schemaVersion: "pastey-native-agent-workspace-movement-v1",
    movementId: "movement-apply",
    taskId: "task-apply",
    agentId: "agent.coding.codex",
    sourceWorkspaceName: "workspace-apply",
    targetHostRef: "host:remote",
    reviewSummary: "Review",
    state: "interrupted",
    code: "result_apply_interrupted",
  }), true);
  const applyInterrupted = {
    schemaVersion: "pastey-native-agent-workspace-movement-v1" as const,
    movementId: "movement-apply",
    taskId: "task-apply",
    agentId: "agent.coding.codex",
    sourceWorkspaceName: "workspace-apply",
    targetHostRef: "host:remote",
    reviewSummary: "Review",
    state: "interrupted" as const,
    code: "result_apply_interrupted",
  };
  assert.equal(nativeAgentRemoteReconciliationRequired(null, applyInterrupted), false);
  assert.equal(nativeAgentConsequenceAbandonmentRequired(applyInterrupted), false);
  assert.equal(nativeAgentRecoveryRequiresAction(null, {
    schemaVersion: "pastey-native-agent-workspace-movement-v1",
    movementId: "movement-two",
    taskId: "task-two",
    agentId: "agent.coding.codex",
    sourceWorkspaceName: "workspace-two",
    targetHostRef: "host:remote",
    reviewSummary: "Review",
    state: "returning_result",
    code: "result_return_retry_required",
  }), true);
});

test("physical review expiry, exact approval, and uncertain status stay separate", async () => {
  const { physicalReviewFresh } = await import("../src/lib/physical");
  const review = {
    scope: { environment: { offerExpiry: 2000 }, qualification: { expiresAt: 1900 } },
    approval: { expiresAt: 1800 },
  } as import("../src/lib/physical").PhysicalReview;
  assert.equal(physicalReviewFresh(review, 1799), true);
  assert.equal(physicalReviewFresh(review, 1800), false);
  assert.equal(physicalReviewFresh(null, 1000), false);
  const panel = readFileSync("src/components/PhysicalReviewPanel.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  assert.match(bindings, /physical_product_command/);
  assert.match(panel, /r\.state !== "approved"/);
  assert.match(panel, /physicalReviewFresh\(r\)/);
  assert.match(panel, /stop acknowledgement does not prove physical rest/);
  assert.doesNotMatch(panel, /robot\.enable|setApproval|authorityToken/);
  // The renderer shows option names only, never payloads or device fields.
  assert.doesNotMatch(panel, /vxMps|vyawRadps|micro_duck|payload/i);
});

test("physical review shows a decision-stream scope, including its observation flow", async () => {
  const { physicalScopeSummary } = await import("../src/lib/physical");
  const base = {
    requester: "host:brain",
    executor: "host:body",
    environment: { environment: "environment:v1:a", evidenceClass: "simulation", offerExpiry: 2000 },
    execution: { actionDurationUs: 500000, leaseDurationUs: 3000000, totalExecutionUs: 30000000, actionCount: 60 },
    qualification: { requiredEnforcementClass: "adapter_isolation_only", expiresAt: 2000 },
    completion: { predicate: { id: "flat.in-room/v1" }, requiredWitness: "simulation_oracle", evaluationWindowUs: 3000000 },
  };
  const stream = physicalScopeSummary({
    ...base,
    stream: {
      options: ["forward", "stop", "turn_left", "turn_right"],
      minDecisionIntervalUs: 200000,
      observation: { fields: ["/heading", "/room"], minIntervalUs: 100000, destination: "host:brain" },
      onCompletion: "await_review",
      effectBound: { verification: "witnessed", predicate: { id: "flat.stays-inside/v1" }, requiredWitness: "simulation_oracle" },
      idleLeaseUs: 120000000,
      approvalLifetimeUs: 1800000000,
    },
  } as import("../src/lib/physical").PhysicalScope);
  assert.deepEqual(Object.fromEntries(stream), {
    "Executor Host": "host:body",
    "Approved options": "forward, stop, turn_left, turn_right",
    "Decision rate": "at most one decision per 0.2 s",
    "Per action": "≤ 0.5 s",
    "In total": "≤ 30 s · ≤ 60 actions",
    Completion: "flat.in-room/v1 within 3 s",
    "Witness class": "simulation oracle",
    "Observations sent": "/heading, /room to host:brain, at most one per 0.1 s",
    "Effect bound": "flat.stays-inside/v1, checked by a simulation oracle witness",
    "On completion": "Awaits a review decision",
    "Approval valid for": "1800 s after approval",
    "Idle lease": "the stream ends after 120 s without a tool call",
  });
  // A single reviewed action is a one-option stream with one action.
  const intentOnly = physicalScopeSummary({
    ...base,
    execution: { ...base.execution, actionCount: 1 },
    stream: {
      options: ["pour_5ml"],
      minDecisionIntervalUs: 500000,
      observation: { fields: [], minIntervalUs: 500000, destination: "host:brain" },
      onCompletion: "automatic",
      effectBound: { verification: "intent_only" },
      idleLeaseUs: 700000,
      approvalLifetimeUs: 30000000,
    },
  } as import("../src/lib/physical").PhysicalScope);
  assert.equal(Object.fromEntries(intentOnly)["Effect bound"], "Intent only: constrains the brain's choices, not what the body does");
  assert.equal(Object.fromEntries(intentOnly)["Observations sent"], "none");
  assert.equal(Object.fromEntries(intentOnly)["Approved options"], "pour_5ml");
  assert.equal(Object.fromEntries(intentOnly)["In total"], "≤ 30 s · ≤ 1 action");
  assert.equal(Object.fromEntries(intentOnly)["On completion"], "Accepted automatically");
  // The panel renders these rows and no longer reads scope.intent directly.
  const panel = readFileSync("src/components/PhysicalReviewPanel.tsx", "utf8");
  assert.match(panel, /physicalScopeSummary\(scope\)/);
  assert.doesNotMatch(panel, /scope\.intent\./);
});

test("physical status shows decision records and the witness verdict apart from Core's conclusion", async () => {
  const { physicalStatusRows, physicalWitnessSummary, physicalDecisionRow, physicalMcpConfig } = await import("../src/lib/physical");
  const status = {
    review: "approved", authority: "closed", installation: "quarantined", dispatch: "acknowledged",
    consequence: "verified", acceptance: "accepted", reconciliation: "pending", enforcementPending: false, quarantined: true,
    consequenceReason: "completion_held",
    witness: { witnessClass: "simulation_oracle", result: "verified", reason: "completion_held" },
    decisions: [
      { sequence: 1, proposer: "mcp:claude-code", option: "turn_right", allowed: true, reason: "" },
      { sequence: 2, proposer: "mcp:claude-code", option: "sprint", allowed: false, reason: "Option not approved" },
    ],
  } as import("../src/lib/physical").PhysicalStatus;
  assert.deepEqual(Object.fromEntries(physicalStatusRows(status)), {
    Authority: "closed", Installation: "quarantined", Dispatch: "acknowledged",
    Consequence: "verified (completion held)", Acceptance: "accepted", Reconciliation: "pending",
    "Enforcement pending": "no", Quarantined: "yes",
  });
  assert.equal(physicalWitnessSummary(status), "simulation oracle witness: verified (completion held)");
  assert.equal(physicalWitnessSummary({ ...status, witness: null }), "No witness verdict yet");
  assert.deepEqual(status.decisions!.map(physicalDecisionRow), [
    ["1", "mcp:claude-code", "turn_right", "allowed"],
    ["2", "mcp:claude-code", "sprint", "refused: Option not approved"],
  ]);
  assert.deepEqual(JSON.parse(physicalMcpConfig({ command: "/Applications/Pastey", args: ["--physical-mcp", "/tmp/g.json"], grantPath: "/tmp/g.json" })),
    { mcpServers: { "pastey-physical": { command: "/Applications/Pastey", args: ["--physical-mcp", "/tmp/g.json"] } } });
  const panel = readFileSync("src/components/PhysicalReviewPanel.tsx", "utf8");
  assert.match(panel, /physicalMcpConnection\(roomId, host, start\)/);
  assert.match(panel, /physicalDecisionRow\(d\)/);
  assert.doesNotMatch(panel, /Object\.entries\(status\)/);
});

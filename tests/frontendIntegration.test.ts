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
  nativeAgentAwaitingNativeDecision,
  nativeAgentDisplayName,
  nativeAgentLifecycleLabel,
  nativeAgentResponse,
  nativeAgentTargetAvailability,
  nativeAgentConversationLabel,
  nativeAgentConversationUnavailable,
} from "../src/features/workspace/AgentTaskLifecycle";
import type { FileTransferProgressEvent, NearbyDevice, ReceivedRoomControlEvent, RoomInfo, RoomItem } from "../src/lib/types";
import { availableRevisionId, FINDER_SCOPES, remoteFindBlocks, remoteFindProgress } from "../src/lib/remoteFileFinder";
import { bridgeConnectionErrorMessage, bridgeReachability, peerAddressProblem } from "../src/lib/bridgeAddress";
import { expiryLabel, grantableDevices, grantFormProblem, grantStateLabel } from "../src/lib/unattendedAccess";
import { nativeDecisionPresentation, nativeDecisionsAwaitUser, nativeFileChangeLabel } from "../src/lib/nativeDecisions";
import type { NativeDecisionReview } from "../src/lib/tauri";

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

test("a nearby join pairs only with the code the accepting device shows", () => {
  const screen = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  // Accepting yields only a pending request; the code is typed, never received.
  assert.match(bindings, /requestNearbyJoin\(deviceId: string\): Promise<NearbyJoinPending>/);
  assert.match(screen, /onCompleteNearby\(nearbyPending\.requestId, nearbyCode\)/);
  assert.match(app, /completeNearbyJoin\(requestId, code\)/);
  assert.doesNotMatch(app, /room_code/);
  // The accepting device shows its code until the requester has paired.
  assert.match(app, /Enter \{acceptedPairing\.code\} on \{acceptedPairing\.deviceName\}/);
});

test("a Bridge without a verified device offers only explicit re-pairing", () => {
  const panel = readFileSync("src/features/workspace/BridgeNetworkPanel.tsx", "utf8");
  assert.match(panel, /openBridgePairing\(room\.id\)/);
  assert.match(panel, /pairing === "pairing_required" \? "Pairing required"/);
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

test("native capability Review offers only Accept or Deny and never shows invocation input", () => {
  const workspace = readFileSync("src/features/workspace/WorkspaceV2.tsx", "utf8");
  const tauri = readFileSync("src/lib/tauri.ts", "utf8");
  assert.match(workspace, /listNativeInvocationReviews\(\)/);
  assert.match(workspace, /acceptNativeInvocationReview\(pendingReview\.taskId\)/);
  assert.match(workspace, /denyNativeInvocationReview\(pendingReview\.taskId\)/);
  assert.match(workspace, /Agent request/);
  const review = tauri.match(/export interface NativeInvocationReview \{([^}]*)\}/)?.[1] ?? "";
  assert.deepEqual(
    [...review.matchAll(/^\s*(\w+):/gm)].map((field) => field[1]).sort(),
    ["bridgeId", "capabilityDisplayName", "expiresAt", "peerHostRef", "requestingPeerSessionId", "taskId"],
  );
  assert.doesNotMatch(review, /input|path|command|args|provider/i);
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

test("Native Agent responses are shown apart from lifecycle and only as escaped text", () => {
  const completed = {
    schemaVersion: "pastey-native-agent-task-v1" as const,
    taskId: "task-response",
    agentId: "agent.coding.codex",
    workspaceName: "project",
    state: "completed" as const,
    result: "Codex completed its native task.",
    code: null,
  };
  const hostile = "<img src=x onerror=alert(1)> [link](https://example.invalid) $(rm -rf ~) C:\\Windows";
  const output = { schemaVersion: "pastey-native-text-output-v1", text: hostile, truncated: false };
  assert.deepEqual(nativeAgentResponse({ ...completed, output }), { kind: "text", text: hostile, truncated: false });
  assert.deepEqual(nativeAgentResponse({ ...completed, output: { ...output, truncated: true } }), { kind: "text", text: hostile, truncated: true });
  // Completion without a displayable response stays a completion.
  assert.deepEqual(nativeAgentResponse({ ...completed, output: undefined }), { kind: "none" });
  assert.deepEqual(nativeAgentResponse({ ...completed, output: null }), { kind: "none" });
  for (const malformed of [
    "plain string",
    ["text"],
    { schemaVersion: "other", text: "x", truncated: false },
    { schemaVersion: "pastey-native-text-output-v1", text: 7, truncated: false },
    { schemaVersion: "pastey-native-text-output-v1", text: "x" },
    { payload: { text: "fake.longjob" }, steps: 3 },
  ]) {
    assert.deepEqual(nativeAgentResponse({ ...completed, output: malformed }), { kind: "undisplayable" });
  }
  // Only a completed task has a response; no other state shows output.
  for (const state of ["queued", "running", "failed", "cancelled", "interrupted"] as const) {
    assert.equal(nativeAgentResponse({ ...completed, state, output }), null);
  }
  assert.equal(nativeAgentResponse(null), null);
  assert.equal(nativeAgentDisplayName("agent.coding.codex"), "Codex");
  assert.equal(nativeAgentDisplayName("fake.longjob.v1"), "fake.longjob.v1");

  const card = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  assert.doesNotMatch(card, /dangerouslySetInnerHTML|innerHTML/);
  assert.match(card, /<pre className="v2-agent-response-text">\{response\.text\}<\/pre>/);
  const responseCard = card.slice(card.indexOf("function NativeAgentResponse"), card.indexOf("export function MessageCard"));
  assert.doesNotMatch(responseCard, /<a\b|href=|onClick|open\(|invoke\(/);
  assert.match(bindings, /output\?: unknown;/);
});

test("Native approval waits stay running and are explained, not hidden", () => {
  const running = {
    schemaVersion: "pastey-native-agent-task-v1" as const,
    taskId: "task-approval",
    agentId: "agent.coding.codex",
    workspaceName: "project",
    state: "running" as const,
    code: "native_agent_awaiting_native_decision",
  };
  assert.equal(nativeAgentAwaitingNativeDecision(running), true);
  assert.equal(nativeAgentTaskNeedsObservation(running), true);
  assert.equal(nativeAgentLifecycleLabel(running), "running · waiting for a native approval");
  assert.equal(nativeAgentAwaitingNativeDecision({ ...running, code: null }), false);
  // A cancelled task is never shown as waiting, whatever its last code was.
  assert.equal(nativeAgentAwaitingNativeDecision({ ...running, state: "cancelled" }), false);
  assert.equal(nativeAgentLifecycleLabel({ ...running, state: "failed", code: "native_agent_start_failed" }), "failed · did not start");
  const card = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.match(card, /waiting for a native request that cannot be answered from here/);
  assert.match(card, /agent\.canCancel \? <button[\s\S]*?>Stop Codex<\/button>/);
});

test("A selected remote Host shows its own advertised Codex availability", () => {
  const observed = (state: "available" | "incompatible" | "unavailable") => ({
    roomId: "room",
    hostRef: "host:windows",
    state: "observed" as const,
    capabilities: [{ agentId: "agent.coding.codex", displayName: "Codex", state }],
  });
  assert.deepEqual(nativeAgentTargetAvailability("agent.coding.codex", null), { state: "checking" });
  assert.deepEqual(nativeAgentTargetAvailability("agent.coding.codex", { roomId: "room", hostRef: "host:windows", state: "checking" }), { state: "checking" });
  for (const state of ["available", "incompatible", "unavailable"] as const) {
    assert.deepEqual(nativeAgentTargetAvailability("agent.coding.codex", observed(state)), { state });
  }
  assert.deepEqual(
    nativeAgentTargetAvailability("agent.coding.codex", { roomId: "room", hostRef: "host:windows", state: "observed", capabilities: [] }),
    { state: "unavailable" },
  );
  assert.deepEqual(
    nativeAgentTargetAvailability("agent.coding.codex", { roomId: "room", hostRef: "host:windows", state: "error", message: "no response" }),
    { state: "unknown", message: "no response" },
  );
  const card = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  // The remote target never falls back to this device's local Codex state.
  assert.match(card, /const availability: NativeAgentTargetAvailability = remote\s*\? nativeAgentTargetAvailability\("agent\.coding\.codex", remoteCapabilities\.observation\)\s*: localAvailability\(codex\);/);
  assert.match(card, /availability\.state !== "available"/);
  assert.match(lifecycle, /listRemoteNativeAgentCapabilities\(roomId, hostRef\)/);
  assert.match(bindings, /invoke\("list_remote_native_agent_capabilities", \{ roomId, targetHostRef \}\)/);
  // A finished task stays visible (with its response) when recovery drains.
  assert.match(lifecycle, /loadRecoveryProjection\(undefined, true\)/);
});

test("Continued and new Agent conversations are shown only once confirmed", () => {
  const status = {
    schemaVersion: "pastey-native-agent-task-v1" as const,
    taskId: "task-conversation",
    agentId: "agent.coding.codex",
    workspaceName: "repz",
    sessionReused: false,
    state: "running" as const,
    code: null,
  };
  // Not yet confirmed: never claimed either way, whatever sessionReused says.
  assert.equal(nativeAgentConversationLabel(status), "opening conversation…");
  assert.equal(nativeAgentConversationLabel({ ...status, sessionReused: true }), "opening conversation…");
  assert.equal(nativeAgentConversationLabel({ ...status, conversation: "continued" }), "continued conversation");
  assert.equal(nativeAgentConversationLabel({ ...status, state: "completed", conversation: "new" }), "new conversation");
  assert.equal(nativeAgentConversationLabel({ ...status, state: "completed" }), null);
  const unavailable = { ...status, state: "failed" as const, code: "native_agent_conversation_unavailable" };
  assert.equal(nativeAgentConversationUnavailable(unavailable), true);
  assert.equal(nativeAgentLifecycleLabel(unavailable), "failed · conversation unavailable");
  assert.equal(nativeAgentConversationUnavailable({ ...unavailable, code: "native_agent_start_failed" }), false);

  const card = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  assert.doesNotMatch(card, /resumed native session/);
  assert.match(card, /Start a new conversation/);
  assert.match(card, /could not reopen your earlier conversation, so nothing ran/);
  // A new conversation is only ever an explicit choice, sent as such.
  assert.match(lifecycle, /startNativeCodexTask\(workspace\.trim\(\), taskText\.trim\(\), newConversation\)/);
  assert.match(lifecycle, /startRemoteNativeCodexTask\(roomId, peerSessionId, hostRef, workspace\.trim\(\), taskText\.trim\(\), !newConversation\)/);
  assert.match(bindings, /invoke\("start_native_codex_task", \{ workspace, task, newConversation \}\)/);
  assert.match(bindings, /conversation\?: "new" \| "continued" \| null;/);
  assert.doesNotMatch(bindings, /threadId/);
});

function planEvent(kind: string, payload: Record<string, unknown>): ReceivedRoomControlEvent {
  return {
    eventId: `event-${Math.random()}`,
    kind,
    roomRef: "room",
    sourceDeviceRef: "windows",
    targetPeerRef: "mac",
    createdAt: "2026-10-10T00:00:00Z",
    expiresAt: "2026-10-10T00:10:00Z",
    receivedAt: "2026-10-10T00:00:01Z",
    event: { payload },
  };
}

test("Find file composes only a Search on the other device and a Transfer of the chosen file to this Inbox", () => {
  const found = remoteFindBlocks({ filenameHint: " validation ", extension: ".MP4", scopes: ["documents", "authorized_folders"] });
  assert.equal(found.error, null);
  // Only shared folders: the other device refuses its built-in places.
  assert.deepEqual(found.blocks, [
    { primitive: "Search", executionDevice: "selected_device", filenameHint: "validation", extension: "mp4", safeScopes: ["authorized_folders"] },
    { primitive: "Transfer", source: "selected_device", destination: "requesting_device", landingMode: "final_delivery" },
  ]);
  // A path (for example one an Agent mentioned) is never a search target.
  for (const filenameHint of ["C:\\REPZ\\Saved\\clip.mp4", "/Users/me/REPZ/clip.mp4", "../clip.mp4", "", "   ", "..."]) {
    assert.equal(remoteFindBlocks({ filenameHint, extension: "mp4", scopes: ["authorized_folders"] }).blocks, null, filenameHint);
  }
  assert.equal(remoteFindBlocks({ filenameHint: "clip", extension: "mp4/../x", scopes: ["authorized_folders"] }).blocks, null);
  assert.equal(remoteFindBlocks({ filenameHint: "clip", extension: "mp4", scopes: [] }).blocks, null);
  // Only the finder's own places can be named.
  assert.equal(remoteFindBlocks({ filenameHint: "clip", extension: "", scopes: ["pastey_shared"] }).blocks, null);
  for (const builtIn of ["downloads", "desktop", "documents"] as const) {
    assert.equal(remoteFindBlocks({ filenameHint: "clip", extension: "", scopes: [builtIn] }).blocks, null, builtIn);
  }
  assert.deepEqual(FINDER_SCOPES.map((scope) => scope.value), ["authorized_folders"]);
  assert.match(readFileSync("src/features/workspace/SettingsScreens.tsx", "utf8"), /never searchable until you share a specific folder here for one Bridge/);
  assert.equal(availableRevisionId({ plans: [], revisions: [{ revision: { revision_id: "old" }, state: "withdrawn" }, { revision: { revision_id: "r2" }, state: "available" }], approvals: [], attempts: [], activities: [], results: [] }), "r2");
});

test("Find file follows exactly its own attempt from search to Inbox delivery", () => {
  const candidate = (attemptId: string, candidateId: string, displayName: string) => planEvent("bridge_plan.step_result", {
    attemptId,
    stepId: "search",
    safeResult: { summary: "found", candidates: [{ candidateId, displayName, extension: "mp4", mimeFamily: "media", sizeBytes: 1024, matchReason: "filename_substring_match", redactedLocation: "ValidationCaptures/.../walk.mp4", modifiedAt: "2026-10-10T00:00:00Z", confidence: "medium" }] },
  });
  const workspace = (state: string, results: unknown[] = []) => ({ plans: [], revisions: [], approvals: [], attempts: [{ attempt: { attempt_id: "mine" }, state }], activities: [], results });
  assert.deepEqual(remoteFindProgress("mine", workspace("running"), [], null), { phase: "searching" });
  const events = [candidate("other", "c-other", "other.mp4"), candidate("mine", "c-mine", "walk.mp4"), candidate("mine", "c-bad", "../../etc/passwd")];
  const listed = remoteFindProgress("mine", workspace("running"), events, null);
  assert.equal(listed.phase, "candidates");
  assert.deepEqual(listed.phase === "candidates" ? listed.candidates.map((entry) => entry.candidateId) : [], ["c-mine"]);
  assert.deepEqual(remoteFindProgress("mine", workspace("running"), [planEvent("bridge_plan.step_result", { attemptId: "mine", stepId: "search", safeResult: { summary: "none", candidates: [] } })], null), { phase: "no_matches" });
  const failed = remoteFindProgress("mine", workspace("failed"), [planEvent("bridge_plan.step_failed", { attemptId: "mine", stepId: "search", failureCode: "no_searchable_scopes" })], null);
  assert.equal(failed.phase, "search_failed");
  assert.match(failed.phase === "search_failed" ? failed.message : "", /Shared search folders/);
  assert.deepEqual(remoteFindProgress("mine", workspace("running"), events, "c-mine"), { phase: "sending" });
  // Delivered only once the attempt completed with its Transfer result.
  assert.deepEqual(remoteFindProgress("mine", workspace("running", [{ attempt_id: "mine", step_id: "transfer", summary: "Transfer completed." }]), events, "c-mine"), { phase: "sending" });
  assert.deepEqual(remoteFindProgress("mine", workspace("completed", [{ attempt_id: "mine", step_id: "search", summary: "found" }, { attempt_id: "mine", step_id: "transfer", summary: "Transfer completed." }]), events, "c-mine"), { phase: "delivered", summary: "Transfer completed." });
  assert.deepEqual(remoteFindProgress("mine", workspace("completed", [{ attempt_id: "other", step_id: "transfer", summary: "x" }]), events, "c-mine"), { phase: "sending" });
  assert.equal(remoteFindProgress("mine", workspace("interrupted"), events, "c-mine").phase, "transfer_failed");
});

test("Find file and shared folders keep paths and authority on the searched device", () => {
  const finder = readFileSync("src/features/workspace/RemoteFileFinder.tsx", "utf8");
  const settings = readFileSync("src/features/workspace/SettingsScreens.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  const card = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.doesNotMatch(finder, /innerHTML|sourcePath|createDirectFileTransferBridgePlan|propose.*Movement/);
  assert.match(finder, /selectBridgePlanSearchCandidate\(roomId, attemptId, candidateId, controlRoute\)/);
  assert.match(finder, /approveBridgePlan\(revisionId, approvalId\)/);
  assert.match(card, /<option value="find">Find file<\/option>/);
  // A folder is shared only from this device's own folder picker.
  assert.match(settings, /open\(\{ directory: true, multiple: false \}\)/);
  assert.match(bindings, /invoke\("authorize_search_folder", \{ path, roomId \}\)/);
  // A folder is shared with exactly one Bridge chosen by this device's user.
  assert.match(settings, /authorizeSearchFolder\(chosen, bridgeId\)/);
  assert.match(settings, /Other Bridges cannot/);
  const folderType = bindings.slice(bindings.indexOf("export interface AuthorizedSearchFolder"), bindings.indexOf("export function listAuthorizedSearchFolders"));
  assert.doesNotMatch(folderType, /path/i);
});

test("An overlay address is checked like Rust checks it and is only a place to look", () => {
  for (const accepted of ["100.101.102.103", "192.168.1.20", "10.0.0.7", "172.20.4.9", "home-pc", "Windows-PC.tail1234.TS.net", " home-pc "]) {
    assert.equal(peerAddressProblem(accepted), null, accepted);
  }
  for (const refused of ["", "8.8.8.8", "203.0.113.5", "127.0.0.1", "0.0.0.0", "255.255.255.255", "1.2.3", "300.1.1.1", "https://home-pc", "home-pc:48392", "home-pc/rooms", "user@home-pc", "home pc", "-bad.example", "bad..example", "fd7a:115c:a1e0::1"]) {
    assert.notEqual(peerAddressProblem(refused), null, refused);
  }
  assert.match(peerAddressProblem("8.8.8.8") ?? "", /must not be exposed directly to the internet/);
  // Numeric forms a resolver reads as addresses are not names.
  for (const numeric of ["0x08080808", "0X7f000001", "0x7f.1"]) assert.notEqual(peerAddressProblem(numeric), null, numeric);
  assert.equal(peerAddressProblem("0xbox"), null);
});

test("Join and reconnect failures say what the user can do", () => {
  const lan = bridgeConnectionErrorMessage(new Error("timed out while waiting for room discovery"), false);
  assert.match(lan, /enter its overlay address/);
  assert.match(lan, /expired or been burned/);
  const remote = bridgeConnectionErrorMessage(new Error("The other device did not answer. Check that it is running Pastey"), true);
  assert.match(remote, /same overlay network/);
  assert.match(bridgeConnectionErrorMessage(new Error("Discovered Bridge identity does not match the preserved Bridge."), true), /different or ended Bridge/);
  assert.match(bridgeConnectionErrorMessage(new Error("Use an address on a private or overlay network (for example Tailscale or WireGuard). Pastey must not be exposed directly to the internet."), true), /must not be exposed/);
  assert.match(bridgeConnectionErrorMessage(new Error("Room control delivery failed."), true), /firewall/);
  const room = (liveness: string[]) => ({ id: "r", created_at: 0, expires_at: 0, status: "active", local_role: "creator", auto_burn_after_expiry: false, peer_connected: false, peers: liveness.map((value) => ({ liveness: value })) }) as unknown as RoomInfo;
  assert.equal(bridgeReachability(room(["connected", "reconnecting"])), "connected");
  assert.equal(bridgeReachability(room(["reconnecting"])), "reconnecting");
  assert.equal(bridgeReachability(room(["disconnected", "left"])), "disconnected");
  assert.equal(bridgeReachability(room([])), "none");

  const app = readFileSync("src/App.tsx", "utf8");
  const screens = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const panel = readFileSync("src/features/workspace/BridgeNetworkPanel.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  // LAN join is unchanged; the address is used only when given.
  assert.match(app, /address \? await joinRoomAt\(code, address\) : await joinRoom\(code\)/);
  assert.match(screens, /onJoin\(code, remote \? address\.trim\(\) : undefined\)/);
  assert.match(panel, /setBridgePeerAddress\(room\.id/);
  assert.match(panel, /gives the other device no permissions/);
  assert.match(bindings, /invoke\("join_room_at", \{ code, address \}\)/);
});

test("Unattended Codex permissions are narrow, explained and only created on the executing device", () => {
  const seen = [
    { fingerprint: "device:v1:a", displayFingerprint: "AAAA", bridgeId: "b", peerName: "School Mac", trusted: true },
    { fingerprint: "device:v1:z", displayFingerprint: "ZZZZ", bridgeId: "b", peerName: "Unknown", trusted: false },
  ];
  assert.deepEqual(grantableDevices(seen).map((device) => device.peerName), ["School Mac"]);
  const complete = { deviceFingerprint: "device:v1:a", roomId: "b", workspacePath: "C:\\Users\\me\\REPZ", durationHours: 24, maxRuns: 10 };
  assert.equal(grantFormProblem(complete), null);
  assert.notEqual(grantFormProblem({ ...complete, workspacePath: "" }), null);
  assert.notEqual(grantFormProblem({ ...complete, durationHours: 24 * 365 }), null);
  assert.notEqual(grantFormProblem({ ...complete, maxRuns: 1000 }), null);
  assert.notEqual(grantFormProblem({ ...complete, deviceFingerprint: "" }), null);
  const grant = { grantId: "g", deviceLabel: "School Mac", displayFingerprint: "AAAA", bridgeId: "b", capabilityId: "agent.coding.codex", workspace: "C:\\REPZ", createdAt: 0, expiresAt: 100, maxRuns: 10, runs: 3, state: "active" as const };
  assert.equal(grantStateLabel(grant), "Active · 3 of 10 runs used");
  assert.equal(grantStateLabel({ ...grant, state: "used_up" }), "All 10 runs used");
  assert.match(grantStateLabel({ ...grant, state: "unavailable" }), /folder moved/);
  assert.equal(expiryLabel(100, 200), "expired");
  assert.equal(expiryLabel(200 + 2 * 3600, 200), "expires in 2 h");

  const settings = readFileSync("src/features/workspace/UnattendedAccessSettings.tsx", "utf8");
  const panel = readFileSync("src/features/workspace/BridgeNetworkPanel.tsx", "utf8");
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  // No blanket permission exists anywhere.
  for (const file of [settings, panel, lifecycle, readFileSync("src/lib/tauri.ts", "utf8")]) {
    assert.doesNotMatch(file, /always allow|allowAll|trust_all/i);
  }
  // Trust requires comparing the fingerprint; permissions say what they do not cover.
  assert.match(settings, /Only continue if \$\{device\.peerName\} shows exactly this identity/);
  assert.match(settings, /Codex's own approval requests are never answered by it/);
  assert.match(settings, /must stay on, awake, signed in and running Pastey/);
  // The requesting device can only show itself; it cannot create permissions.
  assert.match(panel, /presentDeviceIdentity\(room\.id, peerHostRef\)/);
  assert.doesNotMatch(panel, /createUnattendedGrant|trustPeerDevice/);
  assert.match(lifecycle, /waiting for the other device to accept \(no unattended permission applied\)/);
});

test("Codex requests are shown as untrusted text and answered only once, by the requesting device", () => {
  const review: NativeDecisionReview = {
    reviewRef: "native-decision:r1",
    kind: "command_execution",
    state: "pending",
    canAccept: true,
    canDecline: true,
    content: { command: "cargo test", cwd: "C:\\REPZ", reason: "<img src=x onerror=alert(1)>" },
    contentDigest: "a".repeat(64),
    requestedAt: 0,
    expiresAt: 1000,
  };
  const remote = { now: 10, remote: true, hostName: "Windows" };
  const pending = nativeDecisionPresentation(review, remote);
  assert.equal(pending.title, "Codex asks to run a command");
  assert.equal(pending.stateLabel, "Waiting for your decision.");
  assert.equal(pending.canAccept && pending.canDecline, true);
  // A task running on this device is never answered from here.
  const local = nativeDecisionPresentation(review, { ...remote, remote: false });
  assert.equal(local.canAccept || local.canDecline, false);
  // Sending, sent, submitted and resolved never offer a second answer, and
  // nothing claims the operation ran.
  for (const view of [
    nativeDecisionPresentation(review, { ...remote, delivery: "sending" }),
    nativeDecisionPresentation(review, { ...remote, delivery: "sent" }),
    nativeDecisionPresentation({ ...review, state: "submitted", submitted: "accept", canAccept: false, canDecline: false }, remote),
    nativeDecisionPresentation({ ...review, state: "resolved", submitted: "accept", canAccept: false, canDecline: false }, remote),
    nativeDecisionPresentation({ ...review, state: "resolved", canAccept: false, canDecline: false }, remote),
  ]) {
    assert.equal(view.canAccept || view.canDecline, false, view.stateLabel);
    assert.doesNotMatch(view.stateLabel, /\b(ran|executed|applied|completed)\b/i, view.stateLabel);
  }
  assert.match(nativeDecisionPresentation({ ...review, state: "submitted", submitted: "accept" }, remote).stateLabel, /Waiting for Codex to confirm/);
  assert.match(nativeDecisionPresentation({ ...review, state: "resolved", submitted: "decline" }, remote).stateLabel, /denied/);
  // Expired requests offer nothing.
  const expired = nativeDecisionPresentation(review, { ...remote, now: 1000 });
  assert.equal(expired.canAccept || expired.canDecline, false);
  assert.match(expired.stateLabel, /expired/);
  // Approval withheld: only what Rust offers is offered, with the reason.
  const withheld = nativeDecisionPresentation({ ...review, canAccept: false, unavailableReason: "native_decision_widens_permissions" }, remote);
  assert.equal(withheld.canAccept, false);
  assert.equal(withheld.canDecline, true);
  assert.match(withheld.note ?? "", /wider sandbox permissions/);
  const unsupported = nativeDecisionPresentation({ ...review, kind: "unsupported", content: null, canAccept: false, canDecline: false, unavailableReason: "native_decision_unsupported" }, remote);
  assert.equal(unsupported.canAccept || unsupported.canDecline, false);
  assert.match(unsupported.stateLabel, /Stop the task/);
  assert.equal(nativeFileChangeLabel({ change: "move", path: "a.rs", moveTo: "b.rs" }), "Move a.rs → b.rs");

  const status = { schemaVersion: "pastey-native-agent-task-v1" as const, taskId: "t", agentId: "agent.coding.codex", workspaceName: "REPZ", state: "running" as const, code: "native_agent_awaiting_native_decision", nativeDecisions: [review] };
  assert.equal(nativeDecisionsAwaitUser(status, true, 10), true);
  assert.equal(nativeDecisionsAwaitUser(status, false, 10), false);
  assert.equal(nativeDecisionsAwaitUser({ ...status, state: "cancelled" }, true, 10), false);
  assert.equal(nativeDecisionsAwaitUser(status, true, 1000), false);

  const panel = readFileSync("src/features/workspace/NativeDecisionPanel.tsx", "utf8");
  const helpers = readFileSync("src/lib/nativeDecisions.ts", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  // Untrusted content is only ever text.
  assert.doesNotMatch(panel, /dangerouslySetInnerHTML|innerHTML|href=|<a /);
  // Only one-time answers exist; nothing approves for a session or by policy.
  for (const file of [panel, helpers, bindings]) {
    assert.doesNotMatch(file, /acceptForSession|"cancel"|always allow|allowAll|approveAll/i);
  }
  assert.match(panel, />Approve once</);
  assert.match(panel, /respondRemoteNativeDecision\(roomId, status\.taskId, review\.reviewRef, review\.contentDigest, decision\)/);
  assert.match(bindings, /invoke\("respond_remote_native_decision", \{ roomId, taskId, reviewRef, contentDigest, decision \}\)/);
  // A permission never approves; the settings say where requests go instead.
  const settings = readFileSync("src/features/workspace/UnattendedAccessSettings.tsx", "utf8");
  assert.match(settings, /shown on the device that started the task, where its user may approve it once or deny it/);
});

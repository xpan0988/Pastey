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
  observationWhileRefreshing,
  REMOTE_CAPABILITY_REFRESH_MS,
} from "../src/features/workspace/AgentTaskLifecycle";
import { codexRunReadiness, looksLikeFullPath, type CodexRunInput } from "../src/lib/nativeAgentReadiness";
import { remoteAccessGuidance, type RemoteAccessFacts } from "../src/lib/accessGuidance";
import { benchmarkFacts, benchmarkScope, deviceProfileFacts, measuredAgo, runtimeFacts } from "../src/lib/deviceDiagnostics";
import { rememberedNativeTask, rememberStartedNativeTask } from "../src/features/workspace/AgentTaskLifecycle";
import type { FileTransferProgressEvent, NearbyDevice, ReceivedRoomControlEvent, RoomInfo, RoomItem } from "../src/lib/types";
import { availableRevisionId, FINDER_SCOPES, remoteFindBlocks, remoteFindProgress } from "../src/lib/remoteFileFinder";
import { bridgeConnectionErrorMessage, bridgeReachability, peerAddressProblem } from "../src/lib/bridgeAddress";
import { expiryLabel, grantableDevices, grantFormProblem, grantStateLabel } from "../src/lib/unattendedAccess";
import { nativeDecisionPresentation, nativeDecisionsAwaitUser, nativeFileChangeLabel } from "../src/lib/nativeDecisions";
import { continuationAvailability, evidenceGroups, localSessionView, readGrantFormProblem, readGrantStateLabel, remoteOutcomeMessage, resumeLabel, sessionFilters, sessionStatusPresentation, type SessionView } from "../src/lib/nativeSessions";
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
  // Accepting yields a code step (or a proven known device); the code is
  // typed, never received.
  assert.match(bindings, /requestNearbyJoin\(deviceId: string\): Promise<NearbyJoinOutcome>/);
  assert.match(screen, /onCompleteNearby\(nearbyPending\.requestId, nearbyCode\)/);
  assert.match(app, /completeNearbyJoin\(requestId, code\)/);
  assert.doesNotMatch(app, /room_code/);
  // The accepting device shows its code until the requester has paired.
  assert.match(app, /Enter \{acceptedPairing\.code\} on \{acceptedPairing\.deviceName\}/);
});

test("a Bridge without a verified device offers only explicit re-pairing", () => {
  const panel = readFileSync("src/features/workspace/BridgeNetworkPanel.tsx", "utf8");
  assert.match(panel, /openBridgePairing\(room\.id\)/);
  // The other device's code re-pairs exactly this Bridge, never through New Bridge.
  assert.match(panel, /repairBridge\(room\.id, pairCode\)/);
  assert.match(panel, /Only the code decides which device is trusted/);
  assert.doesNotMatch(panel, /same (other )?device/i);
  assert.match(panel, /pairing === "pairing_required" \? "Pairing required"/);
});

test("the local Codex card says when the desktop app's Codex is used", () => {
  const card = readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8");
  assert.match(card, /availability\.backend === "desktop"/);
  assert.match(card, /No separate Codex CLI is needed/);
});

test("Codex availability names the failing layer and keeps local detail local", () => {
  const card = readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8");
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  for (const reason of ["native_agent_not_installed", "native_agent_launch_failed", "native_agent_probe_failed", "native_agent_probe_timeout", "native_agent_incompatible"]) {
    assert.match(card, new RegExp(`case "${reason}"`));
  }
  assert.match(lifecycle, /\{ state, reason: capability\.reason \}/);
  assert.doesNotMatch(lifecycle, /detail: capability/);
});

test("a device paired before connects from Nearby without a code and can be forgotten", () => {
  const screen = readFileSync("src/features/workspace/WorkspaceScreens.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");
  const settings = readFileSync("src/features/workspace/KnownDevicesSettings.tsx", "utf8");
  assert.match(screen, /device\.availability === "Available" \|\| device\.known === true/);
  assert.match(screen, /device\.known \? "Connect" : "Join"/);
  assert.match(app, /outcome\.status === "connected"/);
  assert.match(app, /pastey:\/\/known-device-connect/);
  assert.match(settings, /forgetKnownDevice\(device\.fingerprint\)/);
  assert.match(settings, /Other devices on the network always need a code/);
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

test("Bridge devices are inspection-only and navigation cannot create or join a Bridge", () => {
  const details = readFileSync("src/features/workspace/BridgeDetails.tsx", "utf8");
  const workspace = readFileSync("src/features/workspace/WorkspaceV2.tsx", "utf8");
  const bridge = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  const devicesBody = details.slice(details.indexOf("export function BridgeDevices"), details.indexOf("function deviceCheckKey"));
  assert.doesNotMatch(devicesBody, /onCreate|onJoin|onListNearby|onJoinNearby|Add device|Find nearby/);
  assert.match(devicesBody, /Devices join a Bridge only by pairing/);
  // Devices are a part of the selected Bridge, not a global route.
  assert.doesNotMatch(readFileSync("src/features/workspace/workspaceTypes.ts", "utf8"), /"devices"|"activity"/);
  assert.doesNotMatch(workspace, /DevicesScreen|ActivityScreen/);
  assert.match(details, /<BridgeDevices room=\{room\} onOpenDevice=\{onOpenDevice\} \/>/);
  assert.match(bridge, /<BridgeDetails room=\{room\}/);
});

test("Device checks use the exact HostRef and run only when the user asks", () => {
  const device = readFileSync("src/features/workspace/DeviceDetails.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  const performance = device.slice(device.indexOf("function PerformanceSection"), device.indexOf("function AgentsSection"));
  const advanced = device.slice(device.indexOf("function AdvancedSection"), device.indexOf("function DiagnosticGroup"));
  // The read-only connection test and the heavier managed self-check are
  // separate, explicit actions against this exact device.
  assert.match(performance, /runBridgeDeviceDiagnostics\(room\.id, hostRef\)/);
  assert.match(performance, /if \(inFlight\.current\) return;/);
  assert.match(performance, /disabled=\{running \|\| !connected\}/);
  assert.match(advanced, /runBridgeDeviceSelfCheck\(room\.id, hostRef\)/);
  assert.match(advanced, /if \(inFlight\.current\) return;/);
  assert.match(advanced, /disabled=\{loading \|\| !connected\}/);
  assert.match(advanced, /Checking…/);
  assert.match(advanced, /Run again/);
  assert.match(device, /Not configured/);
  assert.match(advanced, /Managed readiness/);
  assert.match(advanced, /Managed E2E self-check/);
  // No effect starts a check: each runs from its own button.
  for (const body of [performance, advanced]) {
    const effects = body.match(/useEffect\(\(\) => \{[\s\S]*?\}, \[[^\]]*\]\);/g) ?? [];
    for (const effect of effects) assert.doesNotMatch(effect, /runBridgeDevice/);
  }
  assert.doesNotMatch(device, /bridge_peers|transport key|session_pair_ref|SQLite|reconnect:/);
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
  const details = readFileSync("src/features/workspace/BridgeDetails.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  const devicesBody = details.slice(details.indexOf("export function BridgeDevices"), details.indexOf("function BridgePermissions"));
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
  assert.match(bridge, /developerMode \? <div[^>]*><div[^>]*><DeveloperModeScreen/);
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
  const card = readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8");
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
  assert.equal(nativeAgentDisplayName("agent.coding.codex_thread"), "Codex");
  assert.equal(nativeAgentDisplayName("agent.coding.claude_code"), "Claude Code");

  const card = readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  assert.doesNotMatch(card, /dangerouslySetInnerHTML|innerHTML/);
  assert.match(card, /<pre className="agent-response-text">\{response\.text\}<\/pre>/);
  const responseCard = card.slice(card.indexOf("function NativeAgentResponse"));
  assert.ok(responseCard.length > 0);
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
  const card = readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8");
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
  const card = readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8");
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  const bindings = readFileSync("src/lib/tauri.ts", "utf8");
  // A chosen remote target never falls back to this device's local Codex
  // state, even while it is disconnected (it is chosen by HostRef).
  assert.match(card, /const availability: NativeAgentTargetAvailability = targetHostRef\s*\? nativeAgentTargetAvailability\("agent\.coding\.codex", remoteCapabilities\.observation\)\s*: localAvailability\(codex\);/);
  assert.match(card, /disabled=\{!readiness\.ready \|\| taskActive\}/);
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

  const card = readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8");
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
  // Find file is the Share composer's "get a file" action, opened in a dialog.
  assert.match(card, /Get a file from \{peerName \?\? "the other device"\}…/);
  assert.match(card, /<RemoteFileFinder roomId=\{room\.id\} peerName=\{peerName\} route=\{selectedRoute\} \/>/);
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
  const trust = readFileSync("src/features/workspace/DeviceTrustSettings.tsx", "utf8");
  const panel = readFileSync("src/features/workspace/BridgeNetworkPanel.tsx", "utf8");
  const details = readFileSync("src/features/workspace/BridgeDetails.tsx", "utf8");
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  // No blanket permission exists anywhere.
  for (const file of [settings, trust, panel, details, lifecycle, readFileSync("src/lib/tauri.ts", "utf8")]) {
    assert.doesNotMatch(file, /always allow|allowAll|trust_all/i);
  }
  // Trust requires comparing the fingerprint; permissions say what they do not cover.
  assert.match(trust, /Only continue if \$\{device\.peerName\} shows exactly this identity/);
  assert.match(settings, /Codex's own approval requests are never answered by it/);
  assert.match(settings, /must stay on, awake, signed in and running Pastey/);
  // The requesting device can only show itself; it cannot create permissions.
  assert.match(panel, /presentDeviceIdentity\(room\.id, peerHostRef\)/);
  assert.doesNotMatch(panel, /createUnattendedGrant|trustPeerDevice/);
  // A Bridge's details can revoke what it is allowed here, never create it.
  assert.doesNotMatch(details, /createUnattendedGrant|createSessionReadGrant|authorizeSearchFolder|trustPeerDevice/);
  assert.match(details, /revokeUnattendedGrant\(grant\.grantId\)/);
  assert.match(details, /revokeSessionReadGrant\(grant\.grantId\)/);
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

function sessionView(overrides: Partial<SessionView>): SessionView {
  return {
    provider: "claude_code", sessionId: "77777777-7777-4777-8777-777777777777", scopeRef: "scope:0123456789abcdef", workspaceLabel: "Pastey",
    title: "Fix the retry bug", origin: "desktop", createdAtMs: 1, lastActivityAtMs: 2, gitBranch: "main",
    live: { state: "not_running", source: "claude agents --json", observedAtMs: 3, detail: null },
    lastTurn: { outcome: "completed", atMs: 2, source: "transcript" },
    status: "completed", statusBasis: "fact", resume: { verdict: "safe", reasons: [] },
    ...overrides,
  };
}

test("a session's status comes from Rust's live facts, never from its last message", () => {
  // History says the last turn ended, but a live source says it runs.
  assert.equal(sessionStatusPresentation(sessionView({ status: "running", statusBasis: "fact", live: { state: "running", source: "claude agents --json", observedAtMs: 3, detail: null } })).label, "Running");
  // History says the last turn ended and nothing runs it: not "done".
  const ended = sessionStatusPresentation(sessionView({}));
  assert.equal(ended.label, "Last turn ended");
  assert.match(ended.explanation, /not proof the task is done/);
  // History says it was mid-turn, but no live source can tell: unknown.
  const unknown = sessionStatusPresentation(sessionView({ status: "unknown", statusBasis: "unknown", lastTurn: { outcome: "in_progress", atMs: 2, source: "transcript" }, live: { state: "unknown", source: "process list", observedAtMs: 3, detail: "a desktop app may hold it" } }));
  assert.equal(unknown.label, "Unknown");
  assert.match(unknown.explanation, /a desktop app may hold it/);
  assert.equal(sessionStatusPresentation(sessionView({ status: "interrupted", statusBasis: "inference" })).label, "Interrupted (inferred)");
  assert.equal(resumeLabel(sessionView({ resume: { verdict: "unknown", reasons: [] } })), "Continuing is not shown to be safe");
});

test("a Host that has not shared its sessions says where to allow it", () => {
  assert.match(remoteOutcomeMessage({ outcome: "not_authorized" }, "Windows PC") ?? "", /On Windows PC, open Settings → Session reading/);
  assert.equal(remoteOutcomeMessage({ outcome: "ok" }, "Windows PC"), null);
  assert.match(remoteOutcomeMessage({ outcome: "busy" }, "Windows PC") ?? "", /still answering/);
});

test("session filters, folder labels and evidence groups are normalized for display", () => {
  assert.deepEqual(sessionFilters({ provider: "gemini", text: "  ", activeWithinSecs: 3600, scopeRef: "" }), { provider: null, text: null, activeWithinSecs: 3600, scopeRef: null });
  assert.deepEqual(sessionFilters({ provider: "codex", text: " retry ", activeWithinSecs: null, scopeRef: "scope:0123456789abcdef" }), { provider: "codex", text: "retry", activeWithinSecs: null, scopeRef: "scope:0123456789abcdef" });
  const local = { ...sessionView({}), workspace: "C:\\Users\\me\\Pastey", originRaw: "cli" } as Parameters<typeof localSessionView>[0];
  assert.equal(localSessionView(local).workspaceLabel, "Pastey");
  assert.equal(localSessionView({ ...local, workspace: "/Users/me/Projects/Site" }).workspaceLabel, "Site");
  const groups = evidenceGroups([{ basis: "fact", statement: "a", source: "s" }, { basis: "unknown", statement: "b", source: "s" }, { basis: "inference", statement: "c", source: "s" }]);
  assert.deepEqual([groups.facts.length, groups.inferences.length, groups.unknowns.length], [1, 1, 1]);
});

test("a session reading permission names a device, a folder and a length", () => {
  assert.match(readGrantFormProblem({}) ?? "", /trusted device/);
  assert.match(readGrantFormProblem({ deviceFingerprint: "d", roomId: "r" }) ?? "", /folder/);
  assert.match(readGrantFormProblem({ deviceFingerprint: "d", roomId: "r", workspacePath: "/p", durationHours: 5 }) ?? "", /how long/);
  assert.equal(readGrantFormProblem({ deviceFingerprint: "d", roomId: "r", workspacePath: "/p", durationHours: 720, shareExcerpts: false }), null);
  assert.equal(readGrantStateLabel({ grantId: "g", deviceLabel: "Mac", displayFingerprint: "f", bridgeId: "b", workspace: "/p", shareExcerpts: false, createdAt: 1, expiresAt: 2, state: "active" }), "Active · statuses only");
});

test("the sessions card continues only this device's own sessions and never grants anything", () => {
  const card = readFileSync("src/features/workspace/NativeSessionsCard.tsx", "utf8");
  const settings = readFileSync("src/features/workspace/SessionReadingSettings.tsx", "utf8");
  assert.match(card, /discoverRemoteNativeSessions\(roomId, remote\.hostRef!/);
  assert.doesNotMatch(card, /startClaude|createSessionReadGrant|startRemoteNative/);
  // One explicit continuation, as the user saw it: here, or on the chosen
  // device by the scope its read permission showed, never a path.
  assert.equal(card.match(/continueNativeSession\(/g)?.length, 1);
  assert.equal(card.match(/continueRemoteNativeSession\(/g)?.length, 1);
  assert.match(card, /continueNativeSession\(\{ workspaces: \[folder\], provider: session\.provider, sessionId: session\.sessionId, prompt: prompt\.trim\(\), expectedMarker: session\.lastTurn\.marker \?\? null \}\)/);
  assert.match(card, /continueRemoteNativeSession\(\{ roomId: route\.roomId, peerSessionId: route\.peerSessionId, targetHostRef: route\.hostRef, provider: session\.provider, sessionId: session\.sessionId, scopeRef: session\.scopeRef, prompt: prompt\.trim\(\), expectedMarker: session\.lastTurn\.marker \?\? null \}\)/);
  assert.match(card, /continuationAvailability\(session, !remote, hostName\)/);
  assert.match(card, /onContinued\?\.\(status, route\?\.hostRef \?\? null\)/);
  assert.match(card, /disabled=\{busy \|\| started \|\| !prompt\.trim\(\)\}/);
  assert.match(card, /Pastey does not pick for you/);
  assert.match(card, /the agent's own account, not verified/);
  assert.match(settings, /trusting a device does not let it read anything/);
});

function codexInput(overrides: Partial<CodexRunInput> = {}): CodexRunInput {
  return {
    target: { kind: "remote", name: "DESKTOP-DMI2L9P", connected: true },
    availability: { state: "available" },
    workspaceSource: "on_target",
    workspace: "C:\\Users\\me\\Pastey",
    taskText: "Run the tests",
    busy: false,
    movementBlocksNewRun: false,
    ...overrides,
  };
}

test("Run with Codex on an available remote Host is ready once its folder path and task are given", () => {
  const ready = codexRunReadiness(codexInput());
  assert.equal(ready.ready, true);
  assert.equal(ready.blocker, null);
  assert.equal(ready.action, "run_remote");
  assert.equal(ready.actionLabel, "Run with Codex");
  assert.match(ready.next, /asks its user to accept/);
  assert.ok(ready.requirements.every((requirement) => requirement.met));

  // The reported case: Codex is available on Windows and a folder was
  // chosen there (for Session reading), but this device was never told the
  // path. The button used to stay disabled silently; now it says why.
  const noPath = codexRunReadiness(codexInput({ workspace: "  " }));
  assert.equal(noPath.ready, false);
  assert.match(noPath.blocker ?? "", /full path of the folder on DESKTOP-DMI2L9P/);
  assert.deepEqual(noPath.requirements.filter((requirement) => !requirement.met).map((requirement) => requirement.id), ["workspace"]);

  assert.equal(codexRunReadiness(codexInput({ taskText: "" })).blocker, "Describe what Codex should do.");
  assert.equal(codexRunReadiness(codexInput({ busy: true })).ready, false);
  assert.equal(codexRunReadiness(codexInput({ busy: true })).actionLabel, "Starting…");
});

test("Run with Codex names each unmet requirement in order, by target and folder location", () => {
  const disconnected = codexRunReadiness(codexInput({ target: { kind: "remote", name: "DESKTOP-DMI2L9P", connected: false }, workspace: "" }));
  assert.equal(disconnected.requirements[0].id, "device");
  assert.match(disconnected.blocker ?? "", /not connected right now/);
  assert.match(codexRunReadiness(codexInput({ availability: { state: "checking" } })).blocker ?? "", /Checking Codex on DESKTOP-DMI2L9P/);
  assert.match(codexRunReadiness(codexInput({ availability: { state: "unknown", message: "timeout" } })).blocker ?? "", /could not check Codex/);
  assert.match(codexRunReadiness(codexInput({ availability: { state: "unavailable", reason: "native_agent_not_installed" } })).blocker ?? "", /not available on DESKTOP-DMI2L9P/);
  assert.match(codexRunReadiness(codexInput({ movementBlocksNewRun: true })).blocker ?? "", /earlier workspace movement is still unresolved/);

  // A folder on this device is reviewed and moved, never run in place.
  const movement = codexRunReadiness(codexInput({ workspaceSource: "send_from_here", workspace: "/Users/me/app" }));
  assert.equal(movement.action, "review_movement");
  assert.equal(movement.actionLabel, "Review movement");
  assert.match(movement.next, /Nothing moves until you approve/);
  assert.match(codexRunReadiness(codexInput({ workspaceSource: "send_from_here", workspace: "" })).blocker ?? "", /Choose the folder on this device to send/);

  const local = codexRunReadiness(codexInput({ target: { kind: "local" } }));
  assert.equal(local.action, "run_local");
  assert.ok(!local.requirements.some((requirement) => requirement.id === "device"));

  assert.equal(looksLikeFullPath("C:\\Users\\me\\app"), true);
  assert.equal(looksLikeFullPath("/Users/me/app"), true);
  assert.equal(looksLikeFullPath("\\\\server\\share"), true);
  assert.equal(looksLikeFullPath("Pastey"), false);
});

test("Native execution readiness never depends on Session reading permissions", () => {
  const readiness = readFileSync("src/lib/nativeAgentReadiness.ts", "utf8");
  const panel = readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8");
  for (const source of [readiness, panel]) {
    assert.doesNotMatch(source, /SessionReadGrant|listSessionReadGrants|readGrant|discoverRemoteNativeSessions/);
  }
  // The button is disabled only by the readiness model, which shows its reason.
  assert.match(panel, /disabled=\{!readiness\.ready \|\| taskActive\} title=\{blocker \?\? undefined\}/);
  assert.match(panel, /<Checklist items=\{readiness\.requirements\} \/>/);
  assert.match(panel, /\{blocker \?\? readiness\.next\}/);
});

test("A chosen Codex Host is kept by HostRef across reconnects and never falls back to this device", () => {
  const panel = readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8");
  assert.match(panel, /const remote = targetHostRef \? peers\.find\(\(peer\) => peer\.hostRef === targetHostRef\) \?\? null : null;/);
  assert.doesNotMatch(panel, /peerSessionId === target\b/);
  // Stop and Approve route to the task's own Host, not whatever is selected now.
  assert.match(panel, /peers\.find\(\(peer\) => peer\.hostRef === agent\.taskHostRef\)/);
  assert.match(panel, /peers\.find\(\(peer\) => peer\.hostRef === agent\.movement\?\.targetHostRef\)/);
  const sessions = readFileSync("src/features/workspace/NativeSessionsCard.tsx", "utf8");
  assert.match(sessions, /peers\.find\(\(peer\) => peer\.hostRef === target\)/);
  // A just-started remote task keeps its Host even when recovery is drained.
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  assert.match(lifecycle, /if \(!projection\) \{ setStatus\(started\); setTaskHostRef\(hostRef\); \}/);
});

test("A remote Host's capability is observed again in the background without flashing back to checking", () => {
  const available = { roomId: "room", hostRef: "host:windows", state: "observed" as const, capabilities: [{ agentId: "agent.coding.codex", displayName: "Codex", state: "available" as const }] };
  assert.equal(observationWhileRefreshing(available, "room", "host:windows", true), available);
  assert.deepEqual(observationWhileRefreshing(available, "room", "host:windows", false), { roomId: "room", hostRef: "host:windows", state: "checking" });
  assert.deepEqual(observationWhileRefreshing(available, "room", "host:other", true), { roomId: "room", hostRef: "host:other", state: "checking" });
  assert.deepEqual(observationWhileRefreshing(available, "room-b", "host:windows", true), { roomId: "room-b", hostRef: "host:windows", state: "checking" });
  assert.deepEqual(observationWhileRefreshing(null, "room", "host:windows", true), { roomId: "room", hostRef: "host:windows", state: "checking" });
  const failed = { roomId: "room", hostRef: "host:windows", state: "error" as const, message: "no answer" };
  assert.equal(observationWhileRefreshing(failed, "room", "host:windows", true), failed);
  assert.ok(REMOTE_CAPABILITY_REFRESH_MS >= 5_000 && REMOTE_CAPABILITY_REFRESH_MS <= 60_000);

  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  assert.match(lifecycle, /window\.setInterval\(\(\) => void query\(true\), REMOTE_CAPABILITY_REFRESH_MS\)/);
  assert.match(lifecycle, /window\.addEventListener\("focus", onFocus\)/);
  assert.match(lifecycle, /window\.clearInterval\(timer\)/);
  // A reconnecting Host is queried again as soon as it is routeable.
  assert.match(readFileSync("src/features/workspace/NativeAgentPanel.tsx", "utf8"), /useRemoteNativeCapabilities\(roomId, remote \? targetHostRef : null\)/);
});

test("Settings are device-wide sections and every permission can be found and revoked", () => {
  const settings = readFileSync("src/features/workspace/SettingsScreens.tsx", "utf8");
  for (const label of ["General", "Devices & trust", "Permissions", "Agents", "Advanced", "About"]) {
    assert.match(settings, new RegExp(`label: "${label}"`));
  }
  const permissions = settings.slice(settings.indexOf("function PermissionsSettings"), settings.indexOf("function SharedSearchFolders"));
  assert.match(permissions, /<UnattendedAccessSettings \/>/);
  assert.match(permissions, /<SessionReadingSettings \/>/);
  assert.match(permissions, /<SharedSearchFolders \/>/);
  // Stored but never read by the Host: not offered as if it did something.
  assert.doesNotMatch(settings, /auto_burn_after_download/);
  // Read by the Host when it chooses where a received item lands.
  assert.match(settings, /toggle\("save_received_files_to_inbox", value\)/);
  assert.match(settings, /toggle\("save_received_images_to_inbox", value\)/);
  // No placeholder rows that look like settings.
  assert.doesNotMatch(settings, /value="Unavailable"|Not projected|Not renderer-exposed|UnavailableRows/);
  // Each Bridge shows and revokes what it is allowed, and links here.
  const details = readFileSync("src/features/workspace/BridgeDetails.tsx", "utf8");
  assert.match(details, /grant\.bridgeId === room\.id/);
  assert.match(details, /folder\.bridgeId === room\.id/);
  assert.match(readFileSync("src/features/workspace/WorkspaceV2.tsx", "utf8"), /onOpenSettings=\{\(next\) => navigate\(next \?\? "settings-permissions"\)\}/);
  assert.match(readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8"), /onOpenSettings=\{\(\) => onOpenSettings\("settings-permissions"\)\}/);
  // Grants list first; the add form is one explicit step away.
  for (const file of ["UnattendedAccessSettings.tsx", "SessionReadingSettings.tsx"]) {
    const source = readFileSync(`src/features/workspace/${file}`, "utf8");
    assert.ok(source.indexOf("ordered.map") < source.indexOf("add-form"), file);
    assert.match(source, /Revoke<\/button>/);
  }
});

test("Burn, Forget and identity replacement each need an explicit confirmation", () => {
  const details = readFileSync("src/features/workspace/BridgeDetails.tsx", "utf8");
  assert.equal(details.match(/onBurn\(room\)/g)?.length, 1);
  const dialog = details.slice(details.indexOf("{confirmBurn ? <Dialog"));
  assert.match(dialog, /onBurn\(room\)/);
  assert.match(dialog, /permanently removes the Bridge content, membership, authority, and current-session state/);
  assert.doesNotMatch(readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8"), /onBurn\(/);
  const known = readFileSync("src/features/workspace/KnownDevicesSettings.tsx", "utf8");
  assert.match(known.slice(known.indexOf("{confirm ? <Dialog")), /forget\(confirm\)/);
  const trust = readFileSync("src/features/workspace/DeviceTrustSettings.tsx", "utf8");
  assert.match(trust.slice(trust.indexOf("{confirmRotate ? <Dialog")), /act\(rotateDeviceIdentity\)/);
  assert.equal(trust.match(/trustPeerDevice\(/g)?.length, 1);
});

test("Execution, observation and continuation stay separate in the Bridge", () => {
  const bridge = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  assert.match(bridge, /\{ value: "share", label: "Share" \}/);
  assert.match(bridge, /value: "agents", label: "Agents"/);
  assert.doesNotMatch(bridge, /value: "sessions"/);
  // One Agents workspace, two separately labelled authorities.
  const workspace = readFileSync("src/features/workspace/AgentWorkspace.tsx", "utf8");
  assert.match(workspace, /Run a task <Badge tone="live">Execution<\/Badge>/);
  assert.match(workspace, /Sessions on \{targetName\} <Badge>Look or continue<\/Badge>/);
  assert.match(workspace, /onContinued=\{agent\.adoptTask\}/);
  assert.match(workspace, /<NativeAgentPanel [^>]*target=\{target\} \/>/);
  assert.match(workspace, /<NativeSessionsCard [^>]*target=\{target\}/);
  const sessions = readFileSync("src/features/workspace/NativeSessionsCard.tsx", "utf8");
  // Continuing never starts a new run in place of the session.
  assert.doesNotMatch(sessions, /startRemoteNativeCodexTask|startNativeCodexTask/);
});

function accessFacts(overrides: Partial<RemoteAccessFacts> = {}): RemoteAccessFacts {
  return {
    remoteName: "DESKTOP-DMI2L9P",
    thisDeviceName: "MacBook Pro",
    thisFingerprint: "C0FF EE12 3456",
    paired: true,
    connected: true,
    lastRunWaitedForAccept: null,
    sessionReading: { state: "unknown" },
    ...overrides,
  };
}

test("Bridge access guidance keeps each authority separate and says where it is granted", () => {
  const lines = remoteAccessGuidance(accessFacts());
  assert.deepEqual(lines.map((line) => line.id), ["pairing", "run", "decisions", "reading", "continue"]);
  // Continuing is its own authority: it needs reading there and an Accept.
  assert.equal(lines.find((line) => line.id === "continue")!.state, "decided_there");
  assert.equal(remoteAccessGuidance(accessFacts({ sessionReading: { state: "not_allowed" } })).find((line) => line.id === "continue")!.state, "needs_action");
  // No single trust switch: running is decided on the executing device.
  const run = lines.find((line) => line.id === "run")!;
  assert.equal(run.state, "decided_there");
  assert.match(run.detail, /waits for someone on DESKTOP-DMI2L9P to click Accept/);
  assert.match(run.steps.join(" "), /On DESKTOP-DMI2L9P: Settings → Devices & trust[\s\S]*C0FF EE12 3456/);
  assert.match(run.steps.join(" "), /On DESKTOP-DMI2L9P: Settings → Permissions → Unattended Codex access/);
  // Codex's own prompts need no trust or permission.
  const decisions = lines.find((line) => line.id === "decisions")!;
  assert.equal(decisions.state, "ok");
  assert.deepEqual(decisions.steps, []);
  assert.match(decisions.detail, /needs no trust or permission/);
  // Reading is unknown until asked, then reported as the other device said.
  assert.equal(lines.find((line) => line.id === "reading")!.state, "unknown");
  const denied = remoteAccessGuidance(accessFacts({ sessionReading: { state: "not_allowed" } })).find((line) => line.id === "reading")!;
  assert.equal(denied.state, "needs_action");
  assert.match(denied.steps.join(" "), /Settings → Permissions → Session reading/);
  const allowed = remoteAccessGuidance(accessFacts({ sessionReading: { state: "allowed", folders: ["Pastey"] } })).find((line) => line.id === "reading")!;
  assert.equal(allowed.state, "ok");
  assert.match(allowed.detail, /Pastey/);
  // A run that waited in Review is reported, not guessed beforehand.
  assert.match(remoteAccessGuidance(accessFacts({ lastRunWaitedForAccept: true })).find((line) => line.id === "run")!.detail, /no unattended permission for this device/);
  assert.equal(remoteAccessGuidance(accessFacts({ paired: false }))[0].state, "needs_action");
  // Nothing here can grant anything.
  const source = readFileSync("src/lib/accessGuidance.ts", "utf8") + readFileSync("src/features/workspace/AgentWorkspace.tsx", "utf8");
  assert.doesNotMatch(source, /createUnattendedGrant|createSessionReadGrant|trustPeerDevice|authorizeSearchFolder/);
});

test("Measurements are shown with what they measured and when, never estimated", () => {
  const result = { average_MBps: 112.34, peak_MBps: 140, latency_ms: 2.5, duration_ms: 1000, total_bytes: 117_000_000, failed_chunks: 0, duplicate_chunks: 0, benchmark_mode: "pastey_pipeline" as const, link_quality: "Excellent" as const, timestamp: Date.now() / 1000 - 30 };
  const shown = benchmarkFacts(result);
  assert.equal(shown.headline, "112.3 MB/s average");
  assert.deepEqual(shown.facts.map(([label]) => label), ["Peak", "Latency", "Quality", "Measured for", "Measured"]);
  assert.match(benchmarkScope("pastey_pipeline", true), /over this Bridge's current connection/);
  assert.match(benchmarkScope("raw_memory", false), /no network/);
  assert.equal(measuredAgo(1000, 1000 * 1000 + 30_000), "just now");
  const rows = deviceProfileFacts({ device_id: "d", device_name: "DESKTOP", platform: "Windows", os_version: "11", arch: "x86_64", gpu_names: [], power_state: "plugged_in", updated_at: 0 });
  assert.deepEqual(rows.map(([label]) => label), ["Name", "System", "Architecture", "Power"]);
  assert.deepEqual(runtimeFacts({ runtimes: [{ name: "Node.js", available: false, source: "path" }, { name: "Python", available: true, version: "3.12", source: "path" }], gpu_acceleration: { cuda_available: false, metal_available: false, gpu_names: [] }, updated_at: 0 }), [["Python", "3.12"], ["Node.js", "Not found"]]);
});

test("A started task stays observable after the Agents view remounts", () => {
  rememberStartedNativeTask("room-x", { taskId: "native-agent:1", hostRef: "host:windows" });
  assert.deepEqual(rememberedNativeTask("room-x"), { taskId: "native-agent:1", hostRef: "host:windows" });
  assert.equal(rememberedNativeTask("room-other"), null);
  const lifecycle = readFileSync("src/features/workspace/AgentTaskLifecycle.tsx", "utf8");
  // Re-observed only by exact identity, only when nothing needs resolution.
  assert.match(lifecycle, /if \(cancelled \|\| projection\) return;[\s\S]{0,200}rememberedNativeTask\(roomId\)/);
  assert.match(lifecycle, /getNativeAgentTaskStatus\(remembered\.taskId\)/);
  assert.match(lifecycle, /rememberStartedNativeTask\(roomId, \{ taskId: started\.taskId, hostRef \}\)/);
  assert.match(lifecycle, /rememberStartedNativeTask\(roomId, \{ taskId: proposed\.taskId, hostRef, movementId: proposed\.movementId \}\)/);
  // Nothing re-runs a task: only its status is read.
  const remount = lifecycle.slice(lifecycle.indexOf("void loadRecoveryProjection(() => !cancelled)"), lifecycle.indexOf("return () => { cancelled = true; };"));
  assert.doesNotMatch(remount, /start(Remote)?NativeCodexTask|approveRemote|retryNativeAgent/);
});

test("Device details, network tests and this device's diagnostics are reachable from the UI", () => {
  const bridge = readFileSync("src/features/workspace/BridgeWorkspace.tsx", "utf8");
  const details = readFileSync("src/features/workspace/BridgeDetails.tsx", "utf8");
  const agents = readFileSync("src/features/workspace/AgentWorkspace.tsx", "utf8");
  const device = readFileSync("src/features/workspace/DeviceDetails.tsx", "utf8");
  const thisDevice = readFileSync("src/features/workspace/ThisDeviceSettings.tsx", "utf8");
  const settings = readFileSync("src/features/workspace/SettingsScreens.tsx", "utf8");
  // Three ways in: the Bridge title, Bridge details' devices, the Agents device bar.
  assert.match(bridge, /onClick=\{\(\) => setDeviceOpen\(members\[0\]\.hostRef!\)\}/);
  assert.match(bridge, /onOpenDevice=\{\(hostRef\) => hostRef \? setDeviceOpen\(hostRef\) : onOpenSettings\("settings-this-device"\)\}/);
  assert.match(details, /onClick=\{\(\) => onOpenDevice\(row\.hostRef\)\}/);
  assert.match(agents, /onClick=\{\(\) => onOpenDevice\(targetHostRef\)\}/);
  assert.match(bridge, /<DeviceDetails room=\{room\} hostRef=\{deviceOpen\}/);
  for (const section of ["Overview", "Connection", "Performance", "Agents", "Trust & permissions", "Advanced"]) {
    assert.match(device, new RegExp(`label: "${section}"`));
  }
  assert.match(device, /Test connection/);
  assert.match(device, /getLastBenchmarkResults\(\)/);
  assert.match(device, /No measurement for this Bridge yet\. Pastey does not estimate speed/);
  // This device: identity, profile, runtimes and a local-only speed test.
  assert.match(settings, /\{ route: "settings-this-device", label: "This device", icon: "device" \}/);
  assert.match(thisDevice, /<DeviceIdentitySettings \/>/);
  assert.match(thisDevice, /getDeviceProfile\(\{ forceRefresh: true \}\)/);
  assert.match(thisDevice, /getDeviceCapabilities\(\{ forceRefresh: true, probeMode: mode \}\)/);
  assert.match(thisDevice, /runLoopbackBenchmark\(\{ mode, durationSeconds: Number\(duration\) \}\)/);
  // Restored Settings controls the Host acts on.
  assert.match(settings, /save\(\{ \.\.\.config, inbox_dir: chosen \}\)/);
  assert.match(settings, /save\(\{ \.\.\.config, inbox_dir: null \}\)/);
  assert.match(settings, /micro_flow_group_mode: event\.target\.value/);
  assert.match(settings, /transfer_window_override: event\.target\.value === "auto" \? null : Number\(event\.target\.value\)/);
  assert.match(settings, /openLogsFolder\(\)/);
  assert.match(settings, /copyLastError\(\)/);
  assert.match(settings, /checkForUpdates\(\)/);
});

test("Sessions follow the Agents device and mark the ones Pastey started", () => {
  const sessions = readFileSync("src/features/workspace/NativeSessionsCard.tsx", "utf8");
  assert.doesNotMatch(sessions, /aria-label="Device"/);
  assert.match(sessions, /session\.origin === "pastey" \? <Badge tone="accent">Started by Pastey<\/Badge>/);
  assert.match(sessions, /response\.outcome === "not_authorized"\) onReading\?\.\(\{ state: "not_allowed" \}\)/);
});

test("A session is offered for continuation only where the Host would accept it", () => {
  const base = sessionView({ provider: "codex", resume: { verdict: "safe", reasons: [] } });
  assert.equal(continuationAvailability(base, true).allowed, true);
  // Another device: a Codex thread only with the marker to check against,
  // and only after that device's Accept; Claude Code never.
  const remoteCodex = continuationAvailability({ ...base, lastTurn: { ...base.lastTurn, marker: "turn-9" } }, false, "DESKTOP");
  assert.equal(remoteCodex.allowed, true);
  assert.match(remoteCodex.reason, /Someone on DESKTOP must Accept/);
  assert.equal(continuationAvailability({ ...base, lastTurn: { ...base.lastTurn, marker: null } }, false, "DESKTOP").allowed, false);
  const remoteClaude = continuationAvailability(sessionView({ provider: "claude_code" }), false, "DESKTOP");
  assert.equal(remoteClaude.allowed, false);
  assert.match(remoteClaude.reason, /outlive Pastey on DESKTOP/);
  // Held by the Codex app or a terminal.
  const held = continuationAvailability({ ...base, resume: { verdict: "unsafe", reasons: [] } }, true);
  assert.equal(held.allowed, false);
  assert.match(held.reason, /open in another Codex client/);
  // Codex enforces its own single writer, so an unreadable lock is no refusal...
  assert.equal(continuationAvailability({ ...base, resume: { verdict: "unknown", reasons: ["lock unreadable"] } }, true).allowed, true);
  // ...but an unreadable history is.
  assert.equal(continuationAvailability({ ...base, lastTurn: { ...base.lastTurn, outcome: "unreadable" } }, true).allowed, false);
  // Claude Code needs a shown-safe state.
  const claude = sessionView({ provider: "claude_code", resume: { verdict: "unknown", reasons: ["no live source"] } });
  assert.equal(continuationAvailability(claude, true).allowed, false);
  assert.match(continuationAvailability(claude, true).reason, /no live source/);
  assert.equal(continuationAvailability({ ...claude, resume: { verdict: "safe", reasons: [] } }, true).allowed, true);
});

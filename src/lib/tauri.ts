import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  BenchmarkMode,
  BridgeDeviceDiagnostics,
  CapabilityProbeMode,
  CapabilityAcquisitionConfirmationInputV1,
  CapabilityAcquisitionConfirmationV1,
  DeviceCapabilities,
  DeviceProfile,
  BridgeNodeListProjectionV1,
  DeveloperModeUiSession,
  DeveloperTerminalWorkspace,
  JoinRequestPrompt,
  LinkBenchmarkResult,
  NearbyDevice,
  KnownDevice,
  NearbyJoinOutcome,
  ReceivedRoomControlEvent,
  RoomControlDeliveryReceipt,
  RoomControlSessionContext,
  RoomInfo,
  RoomItem
} from "./types";
import type {
  ControlBridgeRoutePayload,
  FileBridgeRoutePayload,
  TextBridgeRoutePayload,
} from "./bridgeRoutingRuntime";
import type { ComposerBlock } from "./bridgePlanComposer";
import type { CandidateSemanticPlanV2, NaturalV2Operation } from "./ai/naturalV2Plan";

export interface NaturalV2HostSelection {
  alias: string;
  hostRef: string;
  displayName: string;
  capabilityFacts: string[];
}

export interface NaturalV2RootSelection {
  rootAlias: string;
  objectAlias: string;
  logicalObjectId: string;
  revision: number;
  hostAlias: string;
  displayName: string;
}

export interface NaturalV2ComposeCandidateRequest {
  planId: string;
  revisionId: string;
  revisionNumber: number;
  bridgeId: string;
  requesterHostAlias: string;
  originalUserGoal: string;
  context: {
    hosts: NaturalV2HostSelection[];
    roots: NaturalV2RootSelection[];
    allowedOperations: NaturalV2Operation[];
    allowedTransferRoutes: Array<{ sourceHostAlias: string; destinationHostAlias: string }>;
    allowedScopeLabels: string[];
  };
  candidate: CandidateSemanticPlanV2;
}

export interface NaturalV2CandidateReview {
  schemaVersion: "pastey-natural-v2-review-v1";
  title: string;
  draft: {
    schemaVersion: string;
    planId: string;
    revisionId: string;
    revisionHash: string;
    approvalId?: string | null;
    attemptId?: string | null;
    state: "draft";
    currentStepId?: string | null;
    completedSteps: number;
    totalSteps: number;
    readyHosts: number;
    totalHosts: number;
    code?: string | null;
    updatedAt: number;
  };
  affectedHosts: Array<{ hostAlias: string; displayName: string }>;
  topology: Array<{
    stepAlias: string;
    operation: NaturalV2Operation;
    hostAliases: string[];
    dependsOn: string[];
    inputAlias?: string | null;
    outputAlias?: string | null;
  }>;
  movements: Array<{
    stepAlias: string;
    objectAlias: string;
    sourceHostAlias: string;
    destinationHostAlias: string;
  }>;
}

/** Proposal lowering only. A successful response is still an unapproved Draft. */
export function composeNaturalV2Candidate(request: NaturalV2ComposeCandidateRequest): Promise<NaturalV2CandidateReview> {
  return invoke<NaturalV2CandidateReview>("compose_natural_v2_candidate", { request });
}

/** Public workspace projection. It contains reviewed plan semantics and safe
 * history only; execution grants and receiver-local resolution remain Rust
 * private. */
export interface BridgePlanWorkspace {
  plans: unknown[];
  revisions: unknown[];
  approvals: unknown[];
  attempts: unknown[];
  activities: unknown[];
  results: unknown[];
}

/** Authoritative, Host-owned lifecycle projection for a native-v2 Plan. */
export type NativeV2ProductState =
  | "draft"
  | "approved"
  | "checking_readiness"
  | "preparing"
  | "running"
  | "completed"
  | "failed"
  | "interrupted"
  | "cancelled";

export interface NativeV2PlanStatus {
  schemaVersion: "pastey-native-v2-product-v1";
  planId: string;
  revisionId: string;
  revisionHash: string;
  approvalId?: string | null;
  attemptId?: string | null;
  state: NativeV2ProductState;
  currentStepId?: string | null;
  completedSteps: number;
  totalSteps: number;
  readyHosts: number;
  totalHosts: number;
  code?: string | null;
  updatedAt: number;
}

export function getNativeV2PlanStatus(revisionId: string): Promise<NativeV2PlanStatus> {
  return invoke("get_native_v2_plan_status", { revisionId });
}

export function approveNativeV2Plan(
  revisionId: string,
  approvalId: string,
  expiresAt: number,
): Promise<NativeV2PlanStatus> {
  return invoke("approve_native_v2_plan", { revisionId, approvalId, expiresAt });
}

export function startNativeV2PlanAttempt(
  approvalId: string,
  attemptId: string,
  expiresAt: number,
): Promise<NativeV2PlanStatus> {
  return invoke("start_native_v2_plan_attempt", { approvalId, attemptId, expiresAt });
}

export function cancelNativeV2PlanAttempt(attemptId: string): Promise<NativeV2PlanStatus> {
  return invoke("cancel_native_v2_plan_attempt", { attemptId });
}

/** A mature Agent is a Host capability, not a managed Worker/provider. */
export interface NativeAgentCapability {
  agentId: string;
  displayName: string;
  state: "available" | "incompatible" | "unavailable";
  /** Why it is not available, when the Host can tell (a reason code). */
  reason?: string | null;
  /** This device only: what its probe found, possibly naming local paths. */
  detail?: string | null;
  /** This device only: the installation in use, `cli` or `desktop` (the
   * Codex bundled with the ChatGPT desktop app). */
  backend?: "cli" | "desktop" | null;
}

export type NativeAgentTaskState = "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted";

export interface NativeAgentTaskStatus {
  schemaVersion: "pastey-native-agent-task-v1";
  taskId: string;
  agentId: string;
  workspaceName: string;
  sessionReused?: boolean;
  state: NativeAgentTaskState;
  result?: string | null;
  code?: string | null;
  /** Capability-owned terminal output, possibly from another Host.
   * Untrusted data: displayed only as escaped text, never interpreted as
   * markup, a command, a path, or a link. */
  output?: unknown;
  /** Set once the Agent confirms whether the task continued this owner's
   * earlier conversation or began a new one. Never a native identifier. */
  conversation?: "new" | "continued" | null;
  /** Native requests of this running task awaiting the user who started it.
   * Their content is the native Agent's own, untrusted and possibly remote:
   * shown only as escaped text. */
  nativeDecisions?: NativeDecisionReview[];
}

export type NativeDecision = "accept" | "decline";

export interface NativeDecisionReview {
  reviewRef: string;
  kind: "command_execution" | "file_change" | "unsupported";
  state: "pending" | "submitted" | "resolved";
  submitted?: NativeDecision | null;
  canAccept: boolean;
  canDecline: boolean;
  unavailableReason?: string | null;
  content?: {
    command?: string;
    cwd?: string;
    reason?: string;
    networkHost?: string;
    networkProtocol?: string;
    fileChanges?: { path: string; change: string; moveTo?: string; diff: string }[];
  } | null;
  contentDigest: string;
  requestedAt: number;
  expiresAt: number;
}

export type NativeAgentWorkspaceMovementState =
  | "review" | "awaiting_approval" | "transferring_to_agent" | "agent_running"
  | "returning_result" | "applying_result" | "completed"
  | "conflict_recovery_required" | "failed" | "cancelled" | "interrupted";

export interface NativeAgentWorkspaceMovement {
  schemaVersion: "pastey-native-agent-workspace-movement-v1";
  movementId: string;
  taskId: string;
  agentId: string;
  sourceWorkspaceName: string;
  targetHostRef: string;
  reviewSummary: string;
  state: NativeAgentWorkspaceMovementState;
  code?: string | null;
}

export interface NativeAgentRecoveryTaskStatus {
  schemaVersion: "pastey-native-agent-task-v1";
  taskId: string;
  agentId: string;
  workspaceName: string;
  state: NativeAgentTaskState;
  code?: string | null;
}

export interface NativeAgentRecoveryProjection {
  task: NativeAgentRecoveryTaskStatus;
  movement?: NativeAgentWorkspaceMovement | null;
  targetHostRef?: string | null;
}

export function listNativeAgentCapabilities(): Promise<NativeAgentCapability[]> {
  return invoke("list_native_agent_capabilities");
}

/** A folder this device's user shared for other Bridge devices' bounded
 * file search. Never a physical path. */
export interface AuthorizedSearchFolder {
  folderId: string;
  displayName: string;
  /** The one Bridge whose devices may search it; null for a folder shared
   * before Bridge scoping, which is not searched until shared again. */
  bridgeId: string | null;
  /** Moved, deleted, or replaced since it was shared; search skips it. */
  unavailable: boolean;
}

export function listAuthorizedSearchFolders(): Promise<AuthorizedSearchFolder[]> {
  return invoke("list_authorized_search_folders");
}

/** Only this device's own user, choosing a folder with the system picker,
 * can share it, and only with one Bridge. */
export function authorizeSearchFolder(path: string, roomId: string): Promise<AuthorizedSearchFolder> {
  return invoke("authorize_search_folder", { path, roomId });
}

export function revokeAuthorizedSearchFolder(folderId: string): Promise<AuthorizedSearchFolder[]> {
  return invoke("revoke_authorized_search_folder", { folderId });
}

/** A fresh observation of what one exact current Bridge Host advertises.
 * Presentation only: it neither selects the Host nor grants invocation. */
export function listRemoteNativeAgentCapabilities(roomId: string, targetHostRef: string): Promise<NativeAgentCapability[]> {
  return invoke("list_remote_native_agent_capabilities", { roomId, targetHostRef });
}

/** Continues this device's Codex conversation in the workspace (or begins
 * one); `newConversation` explicitly begins a new one instead. */
export function startNativeCodexTask(workspace: string, task: string, newConversation = false): Promise<NativeAgentTaskStatus> {
  return invoke("start_native_codex_task", { workspace, task, newConversation });
}

export function getNativeAgentTaskStatus(taskId: string): Promise<NativeAgentTaskStatus> {
  return invoke("get_native_agent_task_status", { taskId });
}

export function cancelNativeAgentTask(taskId: string): Promise<NativeAgentTaskStatus> {
  return invoke("cancel_native_agent_task", { taskId });
}

/** A remote invocation waiting for this Host's Review. Never its input. */
export interface NativeInvocationReview {
  taskId: string;
  bridgeId: string;
  peerHostRef: string;
  requestingPeerSessionId: string;
  capabilityDisplayName: string;
  expiresAt: number;
  /** This device's own shared folder the request names, by its name. */
  sharedFolder?: string | null;
}

export function listNativeInvocationReviews(): Promise<NativeInvocationReview[]> {
  return invoke("list_native_invocation_reviews");
}

export function acceptNativeInvocationReview(taskId: string): Promise<NativeAgentTaskStatus> {
  return invoke("accept_native_invocation_review", { taskId });
}

export function denyNativeInvocationReview(taskId: string): Promise<NativeAgentTaskStatus> {
  return invoke("deny_native_invocation_review", { taskId });
}

/** `workspace` is a full path on that Host, or the reference of a folder it
 * shares with this device (`SharedWorkspace.scopeRef`), which that Host
 * resolves itself; `workspaceLabel` names such a folder until it answers. */
export function startRemoteNativeCodexTask(
  roomId: string, peerSessionId: string, targetHostRef: string, workspace: string, task: string, resume: boolean, workspaceLabel: string | null = null,
): Promise<NativeAgentTaskStatus> {
  return invoke("start_remote_native_codex_task", { roomId, peerSessionId, targetHostRef, workspace, task, resume, workspaceLabel });
}

export function proposeRemoteNativeCodexWorkspaceMovement(
  targetHostRef: string, sourceWorkspace: string, task: string, roomId: string,
): Promise<NativeAgentWorkspaceMovement> {
  return invoke("propose_remote_native_codex_workspace_movement", { targetHostRef, sourceWorkspace, task, roomId });
}

export function approveRemoteNativeCodexWorkspaceMovement(
  movementId: string, roomId: string, peerSessionId: string,
): Promise<NativeAgentWorkspaceMovement> {
  return invoke("approve_remote_native_codex_workspace_movement", { movementId, roomId, peerSessionId });
}

export function getNativeAgentWorkspaceMovementStatus(
  movementId: string,
): Promise<NativeAgentWorkspaceMovement> {
  return invoke("get_native_agent_workspace_movement_status", { movementId });
}

export function getNativeAgentRecoveryProjection(
  roomId: string,
): Promise<NativeAgentRecoveryProjection | null> {
  return invoke("get_native_agent_recovery_projection", { roomId });
}

export function reconcileRemoteNativeAgentTask(
  roomId: string, targetHostRef: string, taskId: string, movementId?: string | null,
): Promise<void> {
  return invoke("reconcile_remote_native_agent_task", { roomId, targetHostRef, taskId, movementId });
}

export function retryNativeAgentWorkspaceResultReturn(
  movementId: string, roomId: string,
): Promise<NativeAgentWorkspaceMovement> {
  return invoke("retry_native_agent_workspace_result_return", { movementId, roomId });
}

export function revealNativeAgentConflictResult(movementId: string): Promise<void> {
  return invoke("reveal_native_agent_conflict_result", { movementId });
}

export function discardNativeAgentConflictResult(
  movementId: string,
): Promise<NativeAgentWorkspaceMovement> {
  return invoke("discard_native_agent_conflict_result", { movementId });
}

export function cancelRemoteNativeAgentTask(
  roomId: string, peerSessionId: string, targetHostRef: string, taskId: string,
): Promise<NativeAgentTaskStatus> {
  return invoke("cancel_remote_native_agent_task", { roomId, peerSessionId, targetHostRef, taskId });
}

/** Sends this user's explicit decision on one native request of their task
 * running on another device. Delivery is not application: the task's status
 * reports what the native Agent confirmed. */
export function respondRemoteNativeDecision(
  roomId: string, taskId: string, reviewRef: string, contentDigest: string, decision: NativeDecision,
): Promise<void> {
  return invoke("respond_remote_native_decision", { roomId, taskId, reviewRef, contentDigest, decision });
}

export function stopBridgeNativeAgentTask(
  roomId: string, taskId: string,
): Promise<NativeAgentTaskStatus> {
  return invoke("stop_bridge_native_agent_task", { roomId, taskId });
}

export interface ManagedRuntimeOption {
  runtimeId: "python" | "node";
  available: boolean;
  selected: boolean;
  selectionReady: boolean;
}

/** Host-local settings projection. Physical executable paths and identities
 * remain inside Rust and are never returned to the renderer. */
export function listManagedRuntimeOptions(): Promise<ManagedRuntimeOption[]> {
  return invoke("list_managed_runtime_options");
}

/** Selects one discovered logical runtime identity on this Host. */
export function selectManagedExecuteRuntime(runtimeId: ManagedRuntimeOption["runtimeId"]): Promise<ManagedRuntimeOption[]> {
  return invoke("select_managed_execute_runtime", { runtimeId });
}

export interface WorkerProviderConfigRef {
  providerId: string;
  generation: number;
  configDigest: string;
}

export type WorkerProviderHealth = "unknown" | "healthy" | "unhealthy";
export type ManagedWorkerProviderSelectionState = "not_configured" | "selected" | "stale";

/** Safe local settings data. Credentials, encrypted rows, bindings, and
 * revocation handles are intentionally absent. */
export interface ManagedWorkerProviderSettings {
  configRef: WorkerProviderConfigRef;
  providerKind: "openai_compatible";
  baseUrl: string;
  model: string;
  timeoutMillis: number;
  maxOutputTokens: number;
  selected: boolean;
  health: WorkerProviderHealth;
  lastHealthCheckAt?: number | null;
  updatedAt: number;
}

export interface ManagedWorkerProviderSettingsSnapshot {
  providers: ManagedWorkerProviderSettings[];
  managedSelection: ManagedWorkerProviderSelectionState;
}

export interface CreateManagedWorkerProviderRequest {
  providerId: string;
  baseUrl: string;
  model: string;
  apiKey: string;
  timeoutMillis: number;
  maxOutputTokens: number;
}

export interface UpdateManagedWorkerProviderRequest {
  expectedRef: WorkerProviderConfigRef;
  baseUrl: string;
  model: string;
  replacementApiKey?: string | null;
  timeoutMillis: number;
  maxOutputTokens: number;
}

export function getManagedWorkerProviderSettings(): Promise<ManagedWorkerProviderSettingsSnapshot> {
  return invoke("get_managed_worker_provider_settings");
}

export function createManagedWorkerProvider(request: CreateManagedWorkerProviderRequest): Promise<ManagedWorkerProviderSettingsSnapshot> {
  return invoke("create_managed_worker_provider", { request });
}

export function updateManagedWorkerProvider(request: UpdateManagedWorkerProviderRequest): Promise<ManagedWorkerProviderSettingsSnapshot> {
  return invoke("update_managed_worker_provider", { request });
}

export function deleteManagedWorkerProvider(expectedRef: WorkerProviderConfigRef): Promise<ManagedWorkerProviderSettingsSnapshot> {
  return invoke("delete_managed_worker_provider", { expectedRef });
}

export function selectManagedWorkerProvider(selection: { configRef: WorkerProviderConfigRef; model: string }): Promise<ManagedWorkerProviderSettingsSnapshot> {
  return invoke("select_managed_worker_provider", { selection });
}

export function checkManagedWorkerProviderHealth(configRef: WorkerProviderConfigRef): Promise<ManagedWorkerProviderSettingsSnapshot> {
  return invoke("check_managed_worker_provider_health", { configRef });
}

export interface ComposedFileBridgePlanRequest {
  roomId: string;
  originalUserGoal: string;
  blocks: ComposerBlock[];
}

export function createComposedFileBridgePlan(request: ComposedFileBridgePlanRequest): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("create_composed_file_bridge_plan", { request });
}

export interface DirectFileTransferBridgePlanRequest {
  roomId: string;
  originalUserGoal: string;
  sourcePath: string;
}

export function createDirectFileTransferBridgePlan(request: DirectFileTransferBridgePlanRequest): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("create_direct_file_transfer_bridge_plan", { request });
}

export function refreshSelectedPeerCapabilities(roomId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("refresh_selected_peer_capabilities", { roomId, bridgeRoute });
}

export function listBridgePlanWorkspace(roomId: string): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("list_bridge_plan_workspace", { roomId });
}

export function approveBridgePlan(revisionId: string, approvalId: string): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("approve_bridge_plan", { revisionId, approvalId });
}

export function withdrawBridgePlanRevision(roomId: string, revisionId: string): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("withdraw_bridge_plan_revision", { roomId, revisionId });
}

export function bindBridgePlanToSession(approvalId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("bind_bridge_plan_to_session", { approvalId, bridgeRoute });
}

export function startBridgePlanAttempt(approvalId: string, attemptId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("start_bridge_plan_attempt", { approvalId, attemptId, bridgeRoute });
}

export function selectBridgePlanSearchCandidate(roomId: string, attemptId: string, candidateId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("select_bridge_plan_search_candidate", { roomId, attemptId, candidateId, bridgeRoute });
}


interface SendFileOptions {
  displayName?: string;
  mimeType?: string | null;
  queueItemId?: string | null;
  requestedWindow?: number | null;
  bridgeRoute?: FileBridgeRoutePayload;
}

interface CancelTransferOptions {
  source?: string;
  queueItemId?: string | null;
  batchId?: string | null;
  roomId?: string | null;
}

export interface FileTransferMetadata {
  path: string;
  display_name: string;
  mime_type?: string | null;
  size_bytes: number;
  modified_ms: number;
}

export interface UpdateTransferWindowResult {
  updated: boolean;
  transfer_id: string;
  previous_window?: number | null;
  effective_window?: number | null;
  requested_window: number;
  reason: "updated" | "unchanged" | "not_active" | "receiver_transfer" | "unsupported_protocol" | "override_active";
}

export async function createRoom(expiryMinutes = 15): Promise<RoomInfo> {
  return invoke("create_room", { expiryMinutes });
}

export async function joinRoom(code: string): Promise<RoomInfo> {
  return invoke("join_room", { code });
}

export async function listNearbyDevices(): Promise<NearbyDevice[]> {
  return invoke("list_nearby_devices");
}

export async function requestNearbyJoin(deviceId: string): Promise<NearbyJoinOutcome> {
  return invoke("request_nearby_join", { deviceId });
}

export function listKnownDevices(): Promise<KnownDevice[]> {
  return invoke("list_known_devices");
}

/** Stops trusting a device: it needs a code again, and its Bridges end
 * their sessions until paired again. */
export function forgetKnownDevice(fingerprint: string): Promise<KnownDevice[]> {
  return invoke("forget_known_device", { fingerprint });
}

/** Pairs with the nearby device that accepted, using the code its screen
 * shows. */
export async function completeNearbyJoin(requestId: string, code: string): Promise<RoomInfo> {
  return invoke("complete_nearby_join", { requestId, code });
}

/** Shows a fresh one-time code for pairing this Bridge again. The previous
 * code stops working. */
export function openBridgePairing(roomId: string): Promise<RoomInfo> {
  return invoke("open_bridge_pairing", { roomId });
}

/** Pairs this existing Bridge again with the code shown on the other
 * device. The code must belong to this Bridge. Pairing a device other than
 * the one this Bridge last verified is fresh trust: nothing granted before
 * carries over. */
export function repairBridge(roomId: string, code: string): Promise<RoomInfo> {
  return invoke("repair_bridge", { roomId, code });
}

export async function acceptNearbyJoin(requestId: string): Promise<RoomInfo> {
  return invoke("accept_nearby_join", { requestId });
}

export async function rejectNearbyJoin(requestId: string): Promise<boolean> {
  return invoke("reject_nearby_join", { requestId });
}

export async function pendingJoinRequests(): Promise<JoinRequestPrompt[]> {
  return invoke("pending_join_requests");
}

export async function markJoinPromptRendered(): Promise<boolean> {
  return invoke("mark_join_prompt_rendered");
}

/** Joins a Bridge whose device is on another network, reached at its
 * private or overlay (Tailscale/WireGuard) address or name. The address only
 * says where to look; the Bridge code and session keys decide the join. */
export function joinRoomAt(code: string, address: string): Promise<RoomInfo> {
  return invoke("join_room_at", { code, address });
}

export function getBridgePeerAddress(roomId: string): Promise<string | null> {
  return invoke("get_bridge_peer_address", { roomId });
}

/** Remembers (or with null forgets) where this Bridge's peer can be reached
 * from another network, for reconnects. Grants nothing. */
export function setBridgePeerAddress(roomId: string, address: string | null): Promise<string | null> {
  return invoke("set_bridge_peer_address", { roomId, address });
}

/** This device's internet connectivity: whether it is on, which relay it
 * uses (null: n0's public relays), and whether it runs now. */
export interface InternetConnectivity {
  enabled: boolean;
  relayUrl: string | null;
  running: boolean;
  /** Why it could not start, when it is on but not running. */
  problem: string | null;
}

export function getInternetConnectivity(): Promise<InternetConnectivity> {
  return invoke("get_internet_connectivity");
}

/** Turns internet connectivity on or off. It changes only how this device
 * can be reached; every Bridge still authenticates and authorizes as on a
 * LAN. */
export function setInternetConnectivity(enabled: boolean, relayUrl: string | null): Promise<InternetConnectivity> {
  return invoke("set_internet_connectivity", { enabled, relayUrl });
}

export type InternetPath = "direct" | "relayed" | "unknown";

/** How one Bridge's peer is reached across the internet now. */
export interface BridgeInternetStatus {
  /** This device has internet connectivity on and running. */
  carrierRunning: boolean;
  /** The other device said how to reach it across the internet. */
  peerReachableByCarrier: boolean;
  /** The session runs over the internet now, and on which path; null when
   * it does not (on the LAN, or not connected). */
  path: InternetPath | null;
}

export function getBridgeInternetStatus(roomId: string): Promise<BridgeInternetStatus> {
  return invoke("get_bridge_internet_status", { roomId });
}

/** This device's durable identity, to compare on another device. */
export interface DeviceIdentity {
  fingerprint: string;
  displayFingerprint: string;
}

export function getDeviceIdentity(): Promise<DeviceIdentity> {
  return invoke("get_device_identity");
}

/** Replaces this device's identity key; devices that trusted it must trust
 * the new one again. */
export function rotateDeviceIdentity(): Promise<DeviceIdentity> {
  return invoke("rotate_device_identity");
}

/** Proves this device's identity to one Bridge Host for the current session
 * only, so its user can choose to trust it. Grants nothing. */
export function presentDeviceIdentity(roomId: string, targetHostRef: string): Promise<void> {
  return invoke("present_device_identity", { roomId, targetHostRef });
}

/** A device that proved its identity in a current session on this Host. */
export interface SeenPeerDevice {
  fingerprint: string;
  displayFingerprint: string;
  bridgeId: string;
  peerName: string;
  trusted: boolean;
}

export interface TrustedDevice {
  fingerprint: string;
  displayFingerprint: string;
  label: string;
  enrolledAt: number;
  revoked: boolean;
}

/** Full Access that this device gave the device one Bridge is paired with:
 * it may control this device through Pastey without anyone here accepting
 * each request. Only this device's user gives or revokes it. */
export interface FullAccessGrant {
  grantId: string;
  bridgeId: string;
  deviceLabel: string;
  displayFingerprint: string;
  grantedAt: number;
  /** Folders that device named here by path, shown only on this device. */
  folders: string[];
}

/** Full Access for one Bridge, as this device's user sees it. */
export interface BridgeFullAccess {
  paired: boolean;
  /** The rights everything Full Access starts here runs with. */
  processRights: "standard" | "administrator" | "unknown";
  /** The paired device's fingerprint, to compare before giving it. */
  displayFingerprint: string | null;
  grant: FullAccessGrant | null;
}

export function listSeenPeerDevices(): Promise<SeenPeerDevice[]> {
  return invoke("list_seen_peer_devices");
}

export function trustPeerDevice(fingerprint: string): Promise<TrustedDevice[]> {
  return invoke("trust_peer_device", { fingerprint });
}

export function listTrustedDevices(): Promise<TrustedDevice[]> {
  return invoke("list_trusted_devices");
}

export function revokeTrustedDevice(fingerprint: string): Promise<TrustedDevice[]> {
  return invoke("revoke_trusted_device", { fingerprint });
}

export function getBridgeFullAccess(roomId: string): Promise<BridgeFullAccess> {
  return invoke("get_bridge_full_access", { roomId });
}

export function listFullAccess(): Promise<FullAccessGrant[]> {
  return invoke("list_full_access");
}

/** Gives the device this Bridge is paired with Full Access to this device.
 * `understood`: the user confirmed what it allows. */
export function grantFullAccess(roomId: string, understood: boolean): Promise<BridgeFullAccess> {
  return invoke("grant_full_access", { request: { roomId, understood } });
}

/** Ends it; a terminal it has open here closes. */
export function revokeFullAccess(roomId: string): Promise<BridgeFullAccess> {
  return invoke("revoke_full_access", { roomId });
}

export function removeFullAccessFolder(grantId: string, workspace: string): Promise<FullAccessGrant[]> {
  return invoke("remove_full_access_folder", { grantId, workspace });
}

/** Native session discovery (read only). Rust owns every rule: live state
 * comes only from a live source, history only explains how a stopped
 * session ended, and an agent's own words are claims, not proof. */
export type NativeSessionProvider = "claude_code" | "codex";
export type EvidenceBasis = "fact" | "inference" | "unknown";
export type NativeSessionLiveState = "running" | "attached" | "not_running" | "unknown";
export type NativeSessionLastTurn = "completed" | "interrupted" | "failed" | "in_progress" | "none" | "unreadable";
export type NativeSessionStatus = "running" | "idle" | "completed" | "interrupted" | "failed" | "unknown";
export type NativeSessionResumeVerdict = "safe" | "unsafe" | "unknown";

/** One session as a peer Host describes it: no folder paths, a workspace
 * only by its folder name and an opaque scope reference. */
export interface RemoteNativeSession {
  provider: NativeSessionProvider;
  sessionId: string;
  scopeRef: string;
  workspaceLabel: string;
  title: string | null;
  origin: string;
  createdAtMs: number | null;
  lastActivityAtMs: number | null;
  gitBranch: string | null;
  live: { state: NativeSessionLiveState; source: string; observedAtMs: number; detail: string | null };
  /** `marker`: the provider's id of the last entry. Continuing requires the
   * session to still end with it. */
  lastTurn: { outcome: NativeSessionLastTurn; atMs: number | null; source: string; marker?: string | null };
  status: NativeSessionStatus;
  statusBasis: EvidenceBasis;
  resume: { verdict: NativeSessionResumeVerdict; reasons: string[] };
}

export interface RemoteNativeSessionSnapshot {
  session: RemoteNativeSession;
  activity: { userTurns: number; agentReplies: number; toolCalls: number; commands: number; failedCommands: number; changedFiles: string[]; changedElsewhere: number; errors: number };
  historyComplete: boolean;
  excerptsShared: boolean;
  recent: Array<{ role: "user" | "agent_claim"; atMs: number | null; text: string }>;
  workspace: { exists: boolean; gitBranch: string | null; head: string | null; headCommittedAtMs: number | null; uncommittedPaths: number | null; commitsSinceLastActivity: number | null };
  evidence: Array<{ basis: EvidenceBasis; statement: string; source: string }>;
}

export type RemoteNativeSessionOutcome = "ok" | "not_authorized" | "not_found" | "busy" | "unavailable" | "folder_refused";

export interface RemoteNativeSessionResponse {
  schemaVersion: string;
  requestId: string;
  sourceHostRef: string;
  outcome: RemoteNativeSessionOutcome;
  scopes: SharedWorkspace[];
  discovery?: {
    observedAtMs: number;
    providers: Array<{ provider: NativeSessionProvider; historyReadable: boolean; liveSource: string | null; note: string | null }>;
    sessions: RemoteNativeSession[];
    truncated: boolean;
    resolution: "unique" | "ambiguous" | "none";
    sessionId: string | null;
  } | null;
  snapshot?: RemoteNativeSessionSnapshot | null;
  /** That Host gave this device Full Access (absent: it did not, or it is
   * older than Full Access). */
  fullAccess?: boolean;
  /** Why it would not open a folder (`folder_refused`). */
  detail?: string | null;
}

/** A folder another Bridge Host shares with this device, as that Host
 * describes it: its name and an opaque reference, never its path, and what
 * its user allows there. */
export interface SharedWorkspace {
  scopeRef: string;
  label: string;
  readSessions: boolean;
  shareExcerpts: boolean;
  /** Runs there wait for that Host's Accept unless it gave this device
   * Full Access. */
  agentRequests: boolean;
  /** None for a folder this device opened under Full Access. */
  expiresAt: number | null;
  /** Allowed by this device's Full Access there. */
  fullAccess?: boolean;
}

export function listRemoteSharedWorkspaces(roomId: string, targetHostRef: string): Promise<RemoteNativeSessionResponse> {
  return invoke("list_remote_shared_workspaces", { roomId, targetHostRef });
}

/** Under the Full Access another Bridge Host gave this device: adds one of
 * its folders, by full path, to the workspaces this device may use there. */
export function openRemoteFolder(roomId: string, targetHostRef: string, folder: string): Promise<RemoteNativeSessionResponse> {
  return invoke("open_remote_folder", { roomId, targetHostRef, folder });
}

export interface NativeSessionFilters {
  provider?: NativeSessionProvider | null;
  text?: string | null;
  activeWithinSecs?: number | null;
  scopeRef?: string | null;
}

export function discoverRemoteNativeSessions(roomId: string, targetHostRef: string, filters: NativeSessionFilters): Promise<RemoteNativeSessionResponse> {
  return invoke("discover_remote_native_sessions", { roomId, targetHostRef, provider: filters.provider ?? null, text: filters.text ?? null, activeWithinSecs: filters.activeWithinSecs ?? null, scopeRef: filters.scopeRef ?? null });
}

export function inspectRemoteNativeSession(roomId: string, targetHostRef: string, provider: NativeSessionProvider, sessionId: string): Promise<RemoteNativeSessionResponse> {
  return invoke("inspect_remote_native_session", { roomId, targetHostRef, provider, sessionId });
}

/** This device's own sessions, in folders its user names here. */
export interface LocalNativeSession extends Omit<RemoteNativeSession, "scopeRef" | "workspaceLabel"> {
  workspace: string;
  originRaw: string | null;
}

export interface LocalNativeSessionDiscovery {
  observedAtMs: number;
  providers: Array<{ provider: NativeSessionProvider; historyReadable: boolean; liveSource: string | null; note: string | null }>;
  sessions: LocalNativeSession[];
  resolution: "unique" | "ambiguous" | "none";
  sessionId: string | null;
}

export interface LocalNativeSessionSnapshot extends Omit<RemoteNativeSessionSnapshot, "session" | "activity" | "excerptsShared"> {
  session: LocalNativeSession;
  activity: Omit<RemoteNativeSessionSnapshot["activity"], "changedElsewhere">;
}

export function discoverNativeSessions(workspaces: string[], filters: NativeSessionFilters): Promise<LocalNativeSessionDiscovery> {
  return invoke("discover_native_sessions", { query: { workspaces, provider: filters.provider ?? null, text: filters.text ?? null, activeWithinSecs: filters.activeWithinSecs ?? null } });
}

export function inspectNativeSession(workspaces: string[], provider: NativeSessionProvider, sessionId: string): Promise<LocalNativeSessionSnapshot> {
  return invoke("inspect_native_session", { request: { workspaces, provider, sessionId } });
}

/** Continues one of this device's own sessions in the session itself, with
 * a new prompt, only while it still ends with `expectedMarker`. The task then
 * follows the ordinary Native Agent status, Stop and recovery commands. */
export function continueNativeSession(request: { workspaces: string[]; provider: NativeSessionProvider; sessionId: string; prompt: string; expectedMarker: string | null }): Promise<NativeAgentTaskStatus> {
  return invoke("continue_native_session", { request });
}

/** Asks another current Bridge Host to continue one of its Codex threads,
 * named by the scope its Session reading permission showed this device. The
 * Host waits for its user's Accept; Codex there refuses a second writer. */
export function continueRemoteNativeSession(request: { roomId: string; peerSessionId: string; targetHostRef: string; provider: NativeSessionProvider; sessionId: string; scopeRef: string; prompt: string; expectedMarker: string | null }): Promise<NativeAgentTaskStatus> {
  return invoke("continue_remote_native_session", request);
}

export type SessionReadGrantState = "active" | "expired" | "revoked" | "unavailable";

/** One folder this device shares with one trusted device over one Bridge,
 * and what it allows there. Host-local; trusting a device shares nothing. */
export interface SessionReadGrant {
  grantId: string;
  deviceLabel: string;
  deviceFingerprint: string;
  displayFingerprint: string;
  bridgeId: string;
  /** This device's own path, shown only here. */
  workspace: string;
  /** The folder's name, which the other device sees. */
  label: string;
  readSessions: boolean;
  shareExcerpts: boolean;
  agentRequests: boolean;
  createdAt: number;
  expiresAt: number;
  state: SessionReadGrantState;
}

export type WorkspaceShare = SessionReadGrant;

export interface ShareWorkspaceRequest {
  roomId: string;
  /** The Bridge device this device's user saw and trusted. */
  deviceFingerprint?: string | null;
  workspacePath: string;
  durationHours: number;
  readSessions: boolean;
  shareExcerpts: boolean;
  agentRequests: boolean;
}

export function listBridgeWorkspaceShares(roomId: string): Promise<WorkspaceShare[]> {
  return invoke("list_bridge_workspace_shares", { roomId });
}

/** Shares a folder with this Bridge's own, already trusted device. */
export function shareBridgeWorkspace(request: ShareWorkspaceRequest): Promise<WorkspaceShare> {
  return invoke("share_bridge_workspace", { request });
}

export function updateWorkspaceShare(request: { grantId: string; readSessions: boolean; shareExcerpts: boolean; agentRequests: boolean; durationHours?: number | null }): Promise<WorkspaceShare> {
  return invoke("update_workspace_share", { request: { ...request, durationHours: request.durationHours ?? null } });
}

/** Ends the share of that folder with that device over its Bridge. */
export function stopSharingWorkspace(grantId: string): Promise<void> {
  return invoke("stop_sharing_workspace", { grantId });
}

export function listSessionReadGrants(): Promise<SessionReadGrant[]> {
  return invoke("list_session_read_grants");
}

export async function listRooms(): Promise<RoomInfo[]> {
  return invoke("list_rooms");
}

export async function getRoom(roomId: string): Promise<RoomInfo> {
  return invoke("get_room", { roomId });
}

export async function listRoomItems(roomId: string): Promise<RoomItem[]> {
  return invoke("list_room_items", { roomId });
}

export async function sendTextToRoom(
  roomId: string,
  text: string,
  bridgeRoute?: TextBridgeRoutePayload,
): Promise<RoomItem> {
  return invoke("send_text_to_room", {
    roomId,
    text,
    bridgeRoute: bridgeRoute ?? null,
  });
}

export async function getRoomControlSessionContext(
  roomId: string,
): Promise<RoomControlSessionContext> {
  return invoke("get_room_control_session_context", { roomId });
}

export type ControlClass = "capability_query" | "capability_answer" | "native_agent" | "bridge_plan" | "membership";

export interface ControlRefusal { class: ControlClass; reason: string; at: number }

/** Room Control on one Bridge as this device sees it: counts, classes,
 * reasons and times only, never an event or its content. */
export interface RoomControlDiagnostics {
  observedAt: number;
  historyDepth: number;
  historyLimit: number;
  historyDropped: number;
  historyStale: number;
  replayWindow: number;
  replayLimit: number;
  classes: Array<{
    class: ControlClass;
    accepted: number;
    refused: Array<[string, number]>;
    lastMinute: number;
    perMinute: number;
    burst: number;
    backpressureSince: number | null;
    backpressureRefused: number;
  }>;
  lastRefusal: ControlRefusal | null;
  outboundFailures: Array<{ class: ControlClass; failure: string; count: number }>;
  lastOutboundFailure: ControlRefusal | null;
  nativeReviews: { pending: number; limit: number; started: number; denied: number; expired: number; revoked: number };
}

export function getRoomControlDiagnostics(roomId: string): Promise<RoomControlDiagnostics> {
  return invoke("get_room_control_diagnostics", { roomId });
}

export async function listReceivedRoomControlEvents(
  roomId: string,
): Promise<ReceivedRoomControlEvent[]> {
  return invoke("list_received_room_control_events", { roomId });
}

export function enterDeveloperMode(roomId: string): Promise<DeveloperModeUiSession> {
  return invoke("enter_developer_mode", { roomId });
}

export function getDeveloperTerminalWorkspace(roomId: string): Promise<DeveloperTerminalWorkspace> {
  return invoke("get_developer_terminal_workspace", { roomId });
}

export function requestDeveloperTerminal(
  roomId: string,
  peerSessionId: string,
  developerUiToken: string,
): Promise<DeveloperTerminalWorkspace> {
  return invoke("request_developer_terminal", { roomId, peerSessionId, developerUiToken });
}

export function acceptDeveloperTerminal(
  roomId: string,
  terminalSessionId: string,
  developerUiToken: string,
  cols: number,
  rows: number,
): Promise<boolean> {
  return invoke("accept_developer_terminal", {
    roomId,
    terminalSessionId,
    developerUiToken,
    cols,
    rows,
  });
}

export function denyDeveloperTerminal(
  terminalSessionId: string,
  developerUiToken: string,
): Promise<boolean> {
  return invoke("deny_developer_terminal", { terminalSessionId, developerUiToken });
}

export function sendDeveloperTerminalInput(
  terminalSessionId: string,
  developerUiToken: string,
  bytes: number[],
): Promise<boolean> {
  return invoke("send_developer_terminal_input", { terminalSessionId, developerUiToken, bytes });
}

export function resizeDeveloperTerminal(
  terminalSessionId: string,
  developerUiToken: string,
  cols: number,
  rows: number,
): Promise<boolean> {
  return invoke("resize_developer_terminal", {
    terminalSessionId,
    developerUiToken,
    cols,
    rows,
  });
}

export function closeDeveloperTerminal(
  terminalSessionId: string,
  developerUiToken: string,
): Promise<boolean> {
  return invoke("close_developer_terminal", { terminalSessionId, developerUiToken });
}

/** This device's user closes a terminal another device has open here. */
export function closeHostedDeveloperTerminal(roomId: string, terminalSessionId: string): Promise<DeveloperTerminalWorkspace> {
  return invoke("close_hosted_developer_terminal", { roomId, terminalSessionId });
}

export async function sendFileToRoom(roomId: string, path: string, options?: SendFileOptions): Promise<RoomItem> {
  return invoke("send_file_to_room", {
    roomId,
    path,
    displayName: options?.displayName ?? null,
    mimeType: options?.mimeType ?? null,
    queueItemId: options?.queueItemId ?? null,
    requestedWindow: options?.requestedWindow ?? null,
    bridgeRoute: options?.bridgeRoute ?? null
  });
}

export async function cancelTransfer(transferId: string, options?: CancelTransferOptions): Promise<boolean> {
  return invoke("cancel_transfer", {
    transferId,
    cancelSource: options?.source ?? null,
    queueItemId: options?.queueItemId ?? null,
    batchId: options?.batchId ?? null,
    roomId: options?.roomId ?? null
  });
}

export async function updateTransferWindow(
  transferId: string,
  requestedWindow: number
): Promise<UpdateTransferWindowResult> {
  return invoke("update_transfer_window", { transferId, requestedWindow });
}

export async function writeTempFile(fileName: string, bytes: number[]): Promise<string> {
  return invoke("write_temp_file", { fileName, bytes });
}

export async function getFileTransferMetadata(path: string): Promise<FileTransferMetadata> {
  return invoke("get_file_transfer_metadata", { path });
}

export async function deleteTempFile(path: string): Promise<boolean> {
  return invoke("delete_temp_file", { path });
}

export async function burnRoom(roomId: string): Promise<boolean> {
  return invoke("burn_room", { roomId });
}

export async function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

export async function getDeviceProfile(options?: { forceRefresh?: boolean }): Promise<DeviceProfile> {
  return invoke("get_device_profile", {
    forceRefresh: options?.forceRefresh ?? false
  });
}

export async function getDeviceCapabilities(options?: { forceRefresh?: boolean; probeMode?: CapabilityProbeMode }): Promise<DeviceCapabilities> {
  return invoke("get_device_capabilities", {
    forceRefresh: options?.forceRefresh ?? false,
    probeMode: options?.probeMode ?? null
  });
}

/** Records only a user's decision for a future AI-side acquisition action. */
export async function confirmCapabilityAcquisition(
  input: CapabilityAcquisitionConfirmationInputV1
): Promise<CapabilityAcquisitionConfirmationV1> {
  return invoke("confirm_capability_acquisition", { input });
}

export async function getBridgeNodeListProjection(bridgeId: string): Promise<BridgeNodeListProjectionV1> {
  return invoke("get_bridge_node_list_projection", { bridgeId });
}

export async function runLoopbackBenchmark(options?: {
  mode?: BenchmarkMode;
  durationSeconds?: number;
  windowSize?: number | null;
}): Promise<LinkBenchmarkResult> {
  return invoke("run_loopback_benchmark", {
    mode: options?.mode ?? null,
    durationSeconds: options?.durationSeconds ?? null,
    windowSize: options?.windowSize ?? null
  });
}

export async function runBridgeDeviceDiagnostics(
  bridgeId: string,
  hostRef: string
): Promise<BridgeDeviceDiagnostics> {
  return invoke("run_bridge_device_diagnostics", { bridgeId, hostRef });
}

export async function runBridgeDeviceSelfCheck(
  bridgeId: string,
  hostRef: string
): Promise<BridgeDeviceDiagnostics> {
  return invoke("run_bridge_device_self_check", { bridgeId, hostRef });
}

export async function getLastBenchmarkResults(): Promise<LinkBenchmarkResult[]> {
  return invoke("get_last_benchmark_results");
}

export async function updateConfig(config: AppConfig): Promise<AppConfig> {
  return invoke("update_config", { configValue: config });
}

export async function revealInFolder(path: string): Promise<void> {
  return invoke("reveal_in_folder", { path });
}

export async function openLogsFolder(): Promise<void> {
  return invoke("open_logs_folder");
}

export async function copyLastError(): Promise<string | null> {
  return invoke("copy_last_error");
}

export async function checkForUpdates(): Promise<void> {
  return invoke("check_for_updates");
}

export async function copyTextToClipboard(text: string): Promise<void> {
  return invoke("copy_text_to_clipboard", { text });
}

export async function logFrontendDiagnostic(line: string): Promise<boolean> {
  return invoke("log_frontend_diagnostic", { line });
}

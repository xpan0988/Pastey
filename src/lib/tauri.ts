import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  BenchmarkMode,
  CapabilityProbeMode,
  DeviceCapabilities,
  DeviceProfile,
  DeveloperModeUiSession,
  DeveloperTerminalWorkspace,
  JoinRequestPrompt,
  LinkBenchmarkResult,
  NearbyDevice,
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

export async function requestNearbyJoin(deviceId: string): Promise<RoomInfo> {
  return invoke("request_nearby_join", { deviceId });
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

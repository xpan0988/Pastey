import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  BenchmarkMode,
  CapabilityProbeMode,
  DeviceCapabilities,
  DeviceProfile,
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

export interface FileSearchBridgePlanRequest {
  roomId: string;
  originalUserGoal: string;
  filenameHint: string;
  extensions: string[];
  safeScopes: Array<"downloads" | "desktop" | "documents" | "pastey_shared">;
  transferToRequester: boolean;
  transferDestination?: "requesting_device" | "selected_device";
}

export function createFileSearchBridgePlan(request: FileSearchBridgePlanRequest): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("create_file_search_bridge_plan", { request });
}

export interface DirectFileTransferBridgePlanRequest {
  roomId: string;
  originalUserGoal: string;
  sourcePath: string;
}

export function createDirectFileTransferBridgePlan(request: DirectFileTransferBridgePlanRequest): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("create_direct_file_transfer_bridge_plan", { request });
}

export interface FileTransformAlternativeBridgePlanRequest extends FileSearchBridgePlanRequest {
  transformIntent: string;
}

export function createFileTransformBridgePlan(request: FileTransformAlternativeBridgePlanRequest): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("create_file_transform_bridge_plan", { request });
}

export function proposeBridgePlanTransformFallback(revisionId: string): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("propose_bridge_plan_transform_fallback", { revisionId });
}

export function listBridgePlanWorkspace(roomId: string): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("list_bridge_plan_workspace", { roomId });
}

export function approveBridgePlan(revisionId: string, approvalId: string, receiverRequired: boolean): Promise<BridgePlanWorkspace> {
  return invoke<BridgePlanWorkspace>("approve_bridge_plan", { revisionId, approvalId, receiverRequired });
}

export function sendBridgePlanReviewRequest(approvalId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("send_bridge_plan_review_request", { approvalId, bridgeRoute });
}

export function decideBridgePlanReview(roomId: string, approvalId: string, allow: boolean, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("decide_bridge_plan_review", { roomId, approvalId, allow, bridgeRoute });
}

export function bridgePlanReceiverReviewStatus(roomId: string, approvalId: string): Promise<"allow" | "deny" | null> {
  return invoke<"allow" | "deny" | null>("bridge_plan_receiver_review_status", { roomId, approvalId });
}

export function startBridgePlanAttempt(approvalId: string, attemptId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("start_bridge_plan_attempt", { approvalId, attemptId, bridgeRoute });
}

export function selectBridgePlanSearchCandidate(roomId: string, attemptId: string, candidateId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("select_bridge_plan_search_candidate", { roomId, attemptId, candidateId, bridgeRoute });
}

export function startBridgePlanTransferAttempt(roomId: string, attemptId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("start_bridge_plan_transfer_attempt", { roomId, attemptId, bridgeRoute });
}

export function executeBridgePlanTransferAttempt(roomId: string, attemptId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<boolean> {
  return invoke<boolean>("execute_bridge_plan_transfer_attempt", { roomId, attemptId, bridgeRoute });
}

export function executeDirectBridgePlanTransferAttempt(roomId: string, attemptId: string): Promise<boolean> {
  return invoke<boolean>("execute_direct_bridge_plan_transfer_attempt", { roomId, attemptId });
}

export function startBridgePlanTransformAttempt(roomId: string, attemptId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<RoomControlDeliveryReceipt> {
  return invoke<RoomControlDeliveryReceipt>("start_bridge_plan_transform_attempt", { roomId, attemptId, bridgeRoute });
}

export function executeBridgePlanTransformAttempt(roomId: string, attemptId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<boolean> {
  return invoke<boolean>("execute_bridge_plan_transform_attempt", { roomId, attemptId, bridgeRoute });
}

export function executeBridgePlanSearchAttempt(roomId: string, attemptId: string, bridgeRoute: ControlBridgeRoutePayload): Promise<void> {
  return invoke<void>("execute_bridge_plan_search_attempt", { roomId, attemptId, bridgeRoute });
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

export async function leaveRoom(roomId: string): Promise<boolean> {
  return invoke("leave_room", { roomId });
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

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
import {
  validateCandidatePayloadExecutionRequest,
  validateArtifactTransformExecutionRequest,
  type ArtifactTransformExecutionResult,
  validateFileCandidateExecutionRequest,
  type ArtifactTransformExecutionRequest,
  type CandidatePayloadExecutionRequest,
  type CandidatePayloadResolution,
  type FileCandidateExecutionRequest,
  type FileCandidateExecutionResult,
} from "./ai";
import {
  validateHelloStdoutExecutionRequest,
  validateRoomControlEvent,
  type CandidatePayloadLocalResolution,
  type HelloStdoutExecutionRequest,
  type HelloStdoutExecutionResult,
  type RoomControlEvent
} from "./agentBridge";
import type {
  ControlBridgeRoutePayload,
  FileBridgeRoutePayload,
  TextBridgeRoutePayload,
} from "./bridgeRoutingRuntime";

export interface TransformConsentPromptInfo {
  pendingConsentPromptId: string;
  consentId: string;
  roomRef: string;
  sourcePreviewEventId: string;
  expiresAt: string;
  status: "pending" | "allowed_once" | "denied" | "expired";
  decidedAt?: string;
}

export interface ArtifactTransformRawExecutorResult {
  status: "completed" | "failed" | "timed_out" | "rejected";
  result?: ArtifactTransformExecutionResult["result"];
  errorCode?: "executor_failed" | "invalid_executor_result" | "policy_rejected" | "timed_out";
}

export interface TransformFinalizationDelivery {
  terminalCategory: "completed" | "failed" | "timed_out" | "rejected";
  sent: boolean;
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

export async function sendRoomControlEvent(
  roomId: string,
  event: RoomControlEvent,
  bridgeRoute?: ControlBridgeRoutePayload,
): Promise<RoomControlDeliveryReceipt> {
  const validation = validateRoomControlEvent(event, {
    expectedRoomRef: roomId,
  });
  if (!validation.valid) {
    throw new Error(validation.errors.join(" "));
  }
  return invoke("send_room_control_event", {
    roomId,
    event: validation.value,
    bridgeRoute: bridgeRoute ?? null,
  });
}

export async function executeHelloStdoutCapability(
  request: HelloStdoutExecutionRequest,
): Promise<HelloStdoutExecutionResult> {
  const errors = validateHelloStdoutExecutionRequest(request);
  if (errors.length > 0) {
    throw new Error(errors.join(" "));
  }
  return invoke("execute_hello_stdout_capability", { request });
}

export async function executeFileCandidateSearchCapability(
  request: FileCandidateExecutionRequest,
): Promise<FileCandidateExecutionResult> {
  const validation = validateFileCandidateExecutionRequest(request);
  if (!validation.valid) {
    throw new Error(validation.errors.join(" "));
  }
  return invoke("execute_file_candidate_search_capability", { request });
}

export async function resolveCandidatePayloadCapability(
  request: CandidatePayloadExecutionRequest,
): Promise<CandidatePayloadLocalResolution> {
  const validation = validateCandidatePayloadExecutionRequest(request);
  if (!validation.valid) {
    throw new Error(validation.errors.join(" "));
  }
  return invoke("resolve_candidate_payload_capability", { request });
}

export async function beginTransformOperation(
  request: ArtifactTransformExecutionRequest,
): Promise<"leased" | "already_leased" | "candidate_not_found" | "candidate_expired" | "candidate_changed" | "candidate_claimed"> {
  const validation = validateArtifactTransformExecutionRequest(request);
  if (!validation.valid) throw new Error(validation.errors.join(" "));
  const result = await invoke<{ status: "leased" | "already_leased" | "candidate_not_found" | "candidate_expired" | "candidate_changed" | "candidate_claimed" }>("begin_transform_operation", {
    request,
  });
  return result.status;
}

export async function revalidateTransformOperation(
  request: ArtifactTransformExecutionRequest,
): Promise<"revalidated" | "candidate_not_found" | "candidate_expired" | "candidate_changed" | "candidate_claimed" | "invalid_consent"> {
  const validation = validateArtifactTransformExecutionRequest(request);
  if (!validation.valid) throw new Error(validation.errors.join(" "));
  const result = await invoke<{ status: "revalidated" | "candidate_not_found" | "candidate_expired" | "candidate_changed" | "candidate_claimed" | "invalid_consent" }>("revalidate_transform_operation", { request });
  return result.status;
}

/** Pre-start cleanup only: the receiver host releases the exact request lease and preserves a still-valid grant for retry. */
export async function abortTransformOperation(request: ArtifactTransformExecutionRequest): Promise<"released" | "candidate_not_found" | "candidate_claimed"> {
  const validation = validateArtifactTransformExecutionRequest(request);
  if (!validation.valid) throw new Error(validation.errors.join(" "));
  const result = await invoke<{ status: "released" | "candidate_not_found" | "candidate_claimed" }>("abort_transform_operation", { request });
  return result.status;
}

/** Rust validates and sanitizes before this result can become room-control or UI state. */
/** The only frontend-callable path that can ask Rust to finalize and transport a Transform result. */
export async function finalizeAndSendTransformResult(
  request: ArtifactTransformExecutionRequest,
  rawResult: ArtifactTransformRawExecutorResult,
): Promise<TransformFinalizationDelivery> {
  const validation = validateArtifactTransformExecutionRequest(request);
  if (!validation.valid) throw new Error(validation.errors.join(" "));
  return invoke<TransformFinalizationDelivery>("finalize_and_send_transform_result", { request, rawResult });
}

export async function getTransformOperationStatus(request: ArtifactTransformExecutionRequest): Promise<string> {
  const validation = validateArtifactTransformExecutionRequest(request);
  if (!validation.valid) throw new Error(validation.errors.join(" "));
  const result = await invoke<{ status: string }>("get_transform_operation_status", { request });
  return result.status;
}

export async function createTransformConsentPrompt(
  roomId: string,
  sourcePreviewEventId: string,
): Promise<TransformConsentPromptInfo> {
  return invoke<TransformConsentPromptInfo>("create_transform_consent_prompt", { roomId, sourcePreviewEventId });
}

export async function resolveTransformConsentPrompt(
  roomId: string,
  pendingConsentPromptId: string,
  decision: "allow_once" | "deny",
): Promise<TransformConsentPromptInfo> {
  return invoke<TransformConsentPromptInfo>("resolve_transform_consent_prompt", { roomId, pendingConsentPromptId, decision });
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

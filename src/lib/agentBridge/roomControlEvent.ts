import { isRecord } from "../ai/actionPlanValidator";
import {
  validateCapabilityRequestPreviewEnvelope,
  type CapabilityRequestPreviewEnvelope
} from "../ai/capabilityPreviewEnvelope";
import {
  CANDIDATE_PAYLOAD_CAPABILITY,
  FILE_CANDIDATES_CAPABILITY,
  getAgentBridgeCapabilityContract,
  getAgentBridgeCapabilityContractByConsentGrantSchema,
  getAgentBridgeCapabilityContractByExecutionRequestSchema,
  getAgentBridgeCapabilityContractByExecutionResultSchema,
  HELLO_STDOUT_CAPABILITY,
  HELLO_STDOUT_EXPECTED_STDOUT,
  HELLO_TEMPLATE_CAPABILITY,
  HELLO_TEMPLATE_MESSAGE,
  normalizeCapabilityFieldName
} from "../ai/capabilityRegistry";
import {
  validateFileCandidateExecutionRequest,
  validateFileCandidateExecutionResult,
  type FileCandidateExecutionRequest,
  type FileCandidateExecutionResult,
} from "../ai/fileCandidateRequest";
import {
  CANDIDATE_PAYLOAD_CONSENT_GRANT_SCHEMA,
  validateCandidatePayloadExecutionRequest,
  validateCandidatePayloadExecutionResult,
  type CandidatePayloadExecutionRequest,
  type CandidatePayloadExecutionResult,
} from "../ai/candidatePayloadRequest";
import {
  ARTIFACT_TRANSFORM_CAPABILITY,
  ARTIFACT_TRANSFORM_CONSENT_GRANT_SCHEMA,
  ARTIFACT_TRANSFORM_RESULT_SCHEMA,
  validateArtifactTransformExecutionRequest,
  validateArtifactTransformExecutionResult,
  type ArtifactTransformExecutionRequest,
  type ArtifactTransformExecutionResult,
} from "../ai/artifactTransformRequest";

export type RoomControlEventKind =
  | "capability_preview"
  | "capability_preview_ack"
  | "capability_preview_deny"
  | "capability_preview_invalid"
  | "capability_preview_expired"
  | "capability_execute_request"
  | "capability_execution_result";

export type CapabilityPreviewControlStatus =
  | "acknowledged_preview_only"
  | "denied"
  | "invalid"
  | "expired";

export interface RoomControlEventBase {
  schemaVersion: "pastey-room-control-event-v1";
  eventId: string;
  kind: RoomControlEventKind;
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef?: string;
  createdAt: string;
  expiresAt: string;
  previewOnly: boolean;
}

export interface CapabilityPreviewRoomControlEvent extends RoomControlEventBase {
  kind: "capability_preview";
  targetPeerRef: string;
  previewOnly: true;
  payload: CapabilityRequestPreviewEnvelope;
}

export interface CapabilityPreviewStatusPayload {
  envelopeId: string;
  requestId: string;
  status: CapabilityPreviewControlStatus;
  reason?: string;
}

export interface CapabilityPreviewAckRoomControlEvent extends RoomControlEventBase {
  kind: "capability_preview_ack";
  previewOnly: true;
  payload: CapabilityPreviewStatusPayload & {
    status: "acknowledged_preview_only";
    consent?: CapabilityConsentGrant;
  };
}

export interface CapabilityPreviewDenyRoomControlEvent extends RoomControlEventBase {
  kind: "capability_preview_deny";
  previewOnly: true;
  payload: CapabilityPreviewStatusPayload & {
    status: "denied";
  };
}

export interface CapabilityPreviewInvalidRoomControlEvent extends RoomControlEventBase {
  kind: "capability_preview_invalid";
  previewOnly: true;
  payload: CapabilityPreviewStatusPayload & {
    status: "invalid";
  };
}

export interface CapabilityPreviewExpiredRoomControlEvent extends RoomControlEventBase {
  kind: "capability_preview_expired";
  previewOnly: true;
  payload: CapabilityPreviewStatusPayload & {
    status: "expired";
  };
}

export type CapabilityPreviewStatusRoomControlEvent =
  | CapabilityPreviewAckRoomControlEvent
  | CapabilityPreviewDenyRoomControlEvent
  | CapabilityPreviewInvalidRoomControlEvent
  | CapabilityPreviewExpiredRoomControlEvent;

export interface HelloPeerConsentGrant {
  schemaVersion: "pastey-hello-peer-consent-grant-v1";
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  capability: "runtime.execute_hello_template";
  exactMessage: "hello peer!";
  expiresAt: string;
}

export interface HelloStdoutConsentGrant {
  schemaVersion: "pastey-runtime-hello-stdout-consent-grant-v1";
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  capability: "runtime.hello_stdout";
  expectedStdout: "hello peer";
  expiresAt: string;
}

export interface FileCandidateConsentGrant {
  schemaVersion: "filesystem-find-file-candidates-consent-grant-v1";
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  capability: "filesystem.find_file_candidates";
  filenameHint: string;
  searchMode: "filename_metadata_only";
  expiresAt: string;
}

export interface CandidatePayloadConsentGrant {
  schemaVersion: "transfer-request-candidate-payload-consent-grant-v1";
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  capability: "transfer.request_candidate_payload";
  sourceCapability: "filesystem.find_file_candidates";
  sourceRequestId: string;
  candidateId: string;
  candidateKind: "filesystem_file";
  candidateDisplayName: string;
  expiresAt: string;
}

export interface ArtifactTransformConsentGrant {
  schemaVersion: typeof ARTIFACT_TRANSFORM_CONSENT_GRANT_SCHEMA;
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  capability: typeof ARTIFACT_TRANSFORM_CAPABILITY;
  sourceCapability: "filesystem.find_file_candidates";
  sourceRequestId: string;
  candidateId: string;
  candidateKind: "filesystem_file";
  resultContract: "typed_transform_result";
  expiresAt: string;
}

export type CapabilityConsentGrant =
  | HelloPeerConsentGrant
  | HelloStdoutConsentGrant
  | FileCandidateConsentGrant
  | CandidatePayloadConsentGrant
  | ArtifactTransformConsentGrant;

export interface HelloPeerExecutionRequest {
  schemaVersion: "pastey-hello-peer-execution-request-v1";
  executionId: string;
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: "runtime.execute_hello_template";
  exactMessage: "hello peer!";
  createdAt: string;
  expiresAt: string;
}

export interface HelloStdoutExecutionRequest {
  schemaVersion: "pastey-runtime-hello-stdout-execution-request-v1";
  executionId: string;
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: "runtime.hello_stdout";
  expectedStdout: "hello peer";
  createdAt: string;
  expiresAt: string;
}

export type HelloPeerExecutionResultStatus =
  | "succeeded"
  | "rejected"
  | "expired"
  | "already_consumed"
  | "failed";

export interface HelloPeerExecutionResult {
  schemaVersion: "pastey-hello-peer-execution-result-v1";
  executionId: string;
  requestId: string;
  consentId: string;
  status: HelloPeerExecutionResultStatus;
  output?: "hello peer!";
  errorCode?: string;
  createdAt: string;
}

export type HelloStdoutExecutionResultStatus = HelloPeerExecutionResultStatus;
export type HelloStdoutRuntimeKind = "rust_host_helper";

export interface HelloStdoutExecutionResult {
  schemaVersion: "pastey-runtime-hello-stdout-execution-result-v1";
  executionId: string;
  requestId: string;
  consentId: string;
  capability: "runtime.hello_stdout";
  runtimeKind: HelloStdoutRuntimeKind;
  status: HelloStdoutExecutionResultStatus;
  stdout: string;
  stderr: string;
  exitCode: number;
  durationMs: number;
  timedOut: boolean;
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
  errorCode?: string;
  createdAt: string;
}

export type CapabilityExecutionRequest =
  | HelloPeerExecutionRequest
  | HelloStdoutExecutionRequest
  | FileCandidateExecutionRequest
  | CandidatePayloadExecutionRequest
  | ArtifactTransformExecutionRequest;
export type CapabilityExecutionResult =
  | HelloPeerExecutionResult
  | HelloStdoutExecutionResult
  | FileCandidateExecutionResult
  | CandidatePayloadExecutionResult
  | ArtifactTransformExecutionResult;

export interface CapabilityExecuteRequestRoomControlEvent extends RoomControlEventBase {
  kind: "capability_execute_request";
  targetPeerRef: string;
  previewOnly: false;
  payload: CapabilityExecutionRequest;
}

export interface CapabilityExecutionResultRoomControlEvent extends RoomControlEventBase {
  kind: "capability_execution_result";
  targetPeerRef: string;
  previewOnly: false;
  payload: CapabilityExecutionResult;
}

export type RoomControlEvent =
  | CapabilityPreviewRoomControlEvent
  | CapabilityPreviewStatusRoomControlEvent
  | CapabilityExecuteRequestRoomControlEvent
  | CapabilityExecutionResultRoomControlEvent;

export type RoomControlEventBuildResult =
  | { ok: true; event: RoomControlEvent }
  | { ok: false; errors: string[] };

export type RoomControlEventValidationResult =
  | { valid: true; value: RoomControlEvent; errors: [] }
  | { valid: false; errors: string[] };

export interface RoomControlEventSessionState {
  seenEventIds: string[];
  seenEnvelopeIds: string[];
  seenRequestIds: string[];
  seenExecutionRequestIds: string[];
  seenExecutionResultIds: string[];
}

export type RoomControlEventReplayResult =
  | { ok: true; state: RoomControlEventSessionState }
  | {
      ok: false;
      reason: "duplicate_event" | "duplicate_envelope" | "duplicate_request" | "expired";
      errors: string[];
      state: RoomControlEventSessionState;
    };

export interface ControlLaneBudget {
  totalWindows: number;
  controlWindows: number;
  dataWindows: number;
  controlBacklog: boolean;
}

interface BuildCapabilityPreviewControlEventOptions {
  roomRef: string;
  sourceDeviceRef?: string;
  targetPeerRef?: string;
  now?: Date;
  ttlMs?: number;
  eventId?: string;
}

interface BuildCapabilityPreviewStatusControlEventOptions {
  now?: Date;
  ttlMs?: number;
  eventId?: string;
  reason?: string;
  consent?: CapabilityConsentGrant;
}

interface BuildExecutionControlEventOptions {
  now?: Date;
  ttlMs?: number;
  eventId?: string;
}

interface ValidateRoomControlEventOptions {
  now?: Date;
  expectedRoomRef?: string;
  expectedTargetPeerRef?: string;
  expectedSourceDeviceRef?: string;
}

interface CheckRoomControlEventOptions {
  now?: Date;
}

const ROOM_CONTROL_SCHEMA_VERSION = "pastey-room-control-event-v1";
const DEFAULT_CONTROL_EVENT_TTL_MS = 2 * 60 * 1_000;
const MAX_ROOM_CONTROL_EVENT_BYTES = 64 * 1024;
const MAX_IDENTIFIER_LENGTH = 256;
const MAX_REASON_LENGTH = 512;
const ROOM_CONTROL_EVENT_REQUIRED_FIELDS = [
  "schemaVersion",
  "eventId",
  "kind",
  "roomRef",
  "sourceDeviceRef",
  "createdAt",
  "expiresAt",
  "previewOnly",
  "payload"
];
const ROOM_CONTROL_EVENT_OPTIONAL_FIELDS = ["targetPeerRef"];
const STATUS_PAYLOAD_REQUIRED_FIELDS = ["envelopeId", "requestId", "status"];
const STATUS_PAYLOAD_OPTIONAL_FIELDS = ["reason", "consent"];
const CONSENT_GRANT_FIELDS = [
  "schemaVersion",
  "consentId",
  "sourcePreviewEventId",
  "envelopeId",
  "requestId",
  "requestPayloadHash",
  "capability",
  "exactMessage",
  "expiresAt"
];
const HELLO_STDOUT_CONSENT_GRANT_FIELDS = [
  "schemaVersion",
  "consentId",
  "sourcePreviewEventId",
  "envelopeId",
  "requestId",
  "requestPayloadHash",
  "capability",
  "expectedStdout",
  "expiresAt"
];
const FILE_CANDIDATE_CONSENT_GRANT_FIELDS = [
  "schemaVersion",
  "consentId",
  "sourcePreviewEventId",
  "envelopeId",
  "requestId",
  "requestPayloadHash",
  "capability",
  "filenameHint",
  "searchMode",
  "expiresAt"
];
const CANDIDATE_PAYLOAD_CONSENT_GRANT_FIELDS = [
  "schemaVersion",
  "consentId",
  "sourcePreviewEventId",
  "envelopeId",
  "requestId",
  "requestPayloadHash",
  "capability",
  "sourceCapability",
  "sourceRequestId",
  "candidateId",
  "candidateKind",
  "candidateDisplayName",
  "expiresAt"
];
const ARTIFACT_TRANSFORM_CONSENT_GRANT_FIELDS = [
  "schemaVersion", "consentId", "sourcePreviewEventId", "envelopeId", "requestId", "requestPayloadHash",
  "capability", "sourceCapability", "sourceRequestId", "candidateId", "candidateKind", "resultContract", "expiresAt",
];
const EXECUTION_REQUEST_FIELDS = [
  "schemaVersion",
  "executionId",
  "consentId",
  "sourcePreviewEventId",
  "envelopeId",
  "requestId",
  "requestPayloadHash",
  "roomRef",
  "sourceDeviceRef",
  "targetPeerRef",
  "capability",
  "exactMessage",
  "createdAt",
  "expiresAt"
];
const HELLO_STDOUT_EXECUTION_REQUEST_FIELDS = [
  "schemaVersion",
  "executionId",
  "consentId",
  "sourcePreviewEventId",
  "envelopeId",
  "requestId",
  "requestPayloadHash",
  "roomRef",
  "sourceDeviceRef",
  "targetPeerRef",
  "capability",
  "expectedStdout",
  "createdAt",
  "expiresAt"
];
const EXECUTION_RESULT_REQUIRED_FIELDS = [
  "schemaVersion",
  "executionId",
  "requestId",
  "consentId",
  "status",
  "createdAt"
];
const EXECUTION_RESULT_OPTIONAL_FIELDS = ["output", "errorCode"];
const HELLO_STDOUT_EXECUTION_RESULT_REQUIRED_FIELDS = [
  "schemaVersion",
  "executionId",
  "requestId",
  "consentId",
  "capability",
  "runtimeKind",
  "status",
  "stdout",
  "stderr",
  "exitCode",
  "durationMs",
  "timedOut",
  "stdoutTruncated",
  "stderrTruncated",
  "createdAt"
];
const HELLO_STDOUT_EXECUTION_RESULT_OPTIONAL_FIELDS = ["errorCode"];
const ROOM_CONTROL_EVENT_KINDS = new Set<RoomControlEventKind>([
  "capability_preview",
  "capability_preview_ack",
  "capability_preview_deny",
  "capability_preview_invalid",
  "capability_preview_expired",
  "capability_execute_request",
  "capability_execution_result"
]);
const STATUS_BY_KIND: Record<CapabilityPreviewStatusRoomControlEvent["kind"], CapabilityPreviewControlStatus> = {
  capability_preview_ack: "acknowledged_preview_only",
  capability_preview_deny: "denied",
  capability_preview_invalid: "invalid",
  capability_preview_expired: "expired"
};
const KIND_BY_STATUS: Record<CapabilityPreviewControlStatus, Exclude<RoomControlEventKind, "capability_preview">> = {
  acknowledged_preview_only: "capability_preview_ack",
  denied: "capability_preview_deny",
  invalid: "capability_preview_invalid",
  expired: "capability_preview_expired"
};
const UNSAFE_OR_EXECUTION_FIELDS = new Set([
  "command",
  "cmd",
  "shell",
  "script",
  "code",
  "args",
  "arguments",
  "argv",
  "stdin",
  "workingDirectory",
  "runtime",
  "interpreter",
  "compiler",
  "env",
  "environment",
  "proxy",
  "path",
  "absolutePath",
  "filePath",
  "filesystemTree",
  "rawLogs",
  "secret",
  "token",
  "apiKey",
  "roomKey",
  "roomCode",
  "transportKey",
  "hiddenTransfer",
  "peerFilesystemSearch",
  "contents",
  "fileContents",
  "transferQueueId",
  "transferQueueItemId",
  "autoSend",
  "sendFile",
  "stdout",
  "stderr",
  "exitCode",
  "process",
  "spawn"
].map(normalizeCapabilityFieldName));
const CONSENT_GRANT_SCHEMA = "pastey-hello-peer-consent-grant-v1";
const HELLO_STDOUT_CONSENT_GRANT_SCHEMA = "pastey-runtime-hello-stdout-consent-grant-v1";
const EXECUTION_REQUEST_SCHEMA = "pastey-hello-peer-execution-request-v1";
const HELLO_STDOUT_EXECUTION_REQUEST_SCHEMA = "pastey-runtime-hello-stdout-execution-request-v1";
const EXECUTION_RESULT_SCHEMA = "pastey-hello-peer-execution-result-v1";
const HELLO_STDOUT_EXECUTION_RESULT_SCHEMA = "pastey-runtime-hello-stdout-execution-result-v1";
const FILE_CANDIDATE_CONSENT_GRANT_SCHEMA = "filesystem-find-file-candidates-consent-grant-v1";
const MAX_EXECUTION_ERROR_CODE_LENGTH = 64;
const MAX_HELLO_STDOUT_STDOUT_BYTES = 64;
const MAX_HELLO_STDOUT_STDERR_BYTES = 256;
const MAX_HELLO_STDOUT_DURATION_MS = 60_000;
let eventSequence = 0;

export function buildCapabilityPreviewControlEvent(
  envelope: CapabilityRequestPreviewEnvelope,
  options: BuildCapabilityPreviewControlEventOptions
): RoomControlEventBuildResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_CONTROL_EVENT_TTL_MS;
  const envelopeValidation = validateCapabilityRequestPreviewEnvelope(envelope, {
    now,
    expectedRoomRef: options.roomRef,
    expectedTargetPeerRef: options.targetPeerRef ?? envelope.targetPeerRef
  });
  if (!envelopeValidation.valid) {
    errors.push(...envelopeValidation.errors.map((error) => `Embedded preview envelope: ${error}`));
  }

  const sourceDeviceRef = options.sourceDeviceRef ?? envelope.sourceDeviceRef;
  const targetPeerRef = options.targetPeerRef ?? envelope.targetPeerRef;
  validateBuilderInputs(now, ttlMs, options.roomRef, sourceDeviceRef, targetPeerRef, errors);
  if (sourceDeviceRef !== envelope.sourceDeviceRef) {
    errors.push("Room control event source must match the embedded preview envelope source.");
  }
  if (targetPeerRef !== envelope.targetPeerRef) {
    errors.push("Room control event target must match the embedded preview envelope target.");
  }
  if (options.roomRef !== envelope.roomRef) {
    errors.push("Room control event room must match the embedded preview envelope room.");
  }
  if (errors.length > 0) {
    return { ok: false, errors: unique(errors) };
  }

  const event: CapabilityPreviewRoomControlEvent = {
    schemaVersion: ROOM_CONTROL_SCHEMA_VERSION,
    eventId: options.eventId ?? createEventId(now),
    kind: "capability_preview",
    roomRef: options.roomRef,
    sourceDeviceRef,
    targetPeerRef,
    createdAt: now.toISOString(),
    expiresAt: new Date(Math.min(now.getTime() + ttlMs, new Date(envelope.expiresAt).getTime())).toISOString(),
    previewOnly: true,
    payload: envelope
  };
  return validatedBuildResult(event, now);
}

export function buildCapabilityPreviewStatusControlEvent(
  sourceEvent: CapabilityPreviewRoomControlEvent,
  status: CapabilityPreviewControlStatus,
  options: BuildCapabilityPreviewStatusControlEventOptions = {}
): RoomControlEventBuildResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_CONTROL_EVENT_TTL_MS;
  const sourceValidation = validateRoomControlEvent(sourceEvent, { now });
  if (!sourceValidation.valid) {
    errors.push(...sourceValidation.errors.map((error) => `Source preview event: ${error}`));
  }
  validateBuilderInputs(
    now,
    ttlMs,
    sourceEvent.roomRef,
    sourceEvent.targetPeerRef,
    sourceEvent.sourceDeviceRef,
    errors
  );
  validateOptionalBoundedString(options.reason, "reason", MAX_REASON_LENGTH, errors);
  if (errors.length > 0) {
    return { ok: false, errors: unique(errors) };
  }

  const event: CapabilityPreviewStatusRoomControlEvent = {
    schemaVersion: ROOM_CONTROL_SCHEMA_VERSION,
    eventId: options.eventId ?? createEventId(now),
    kind: KIND_BY_STATUS[status],
    roomRef: sourceEvent.roomRef,
    sourceDeviceRef: sourceEvent.targetPeerRef,
    targetPeerRef: sourceEvent.sourceDeviceRef,
    createdAt: now.toISOString(),
    expiresAt: new Date(Math.min(now.getTime() + ttlMs, new Date(sourceEvent.expiresAt).getTime())).toISOString(),
    previewOnly: true,
    payload: {
      envelopeId: sourceEvent.payload.envelopeId,
      requestId: sourceEvent.payload.request.requestId,
      status,
      ...(options.reason ? { reason: options.reason } : {}),
      ...(options.consent ? { consent: options.consent } : {})
    }
  } as CapabilityPreviewStatusRoomControlEvent;
  return validatedBuildResult(event, now);
}

export function buildCapabilityExecuteRequestControlEvent(
  request: CapabilityExecutionRequest,
  options: BuildExecutionControlEventOptions = {}
): RoomControlEventBuildResult {
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_CONTROL_EVENT_TTL_MS;
  const errors = validateCapabilityExecutionRequest(request, now);
  validateBuilderInputs(now, ttlMs, request.roomRef, request.sourceDeviceRef, request.targetPeerRef, errors);
  if (errors.length > 0) {
    return { ok: false, errors: unique(errors) };
  }
  const event: CapabilityExecuteRequestRoomControlEvent = {
    schemaVersion: ROOM_CONTROL_SCHEMA_VERSION,
    eventId: options.eventId ?? createEventId(now),
    kind: "capability_execute_request",
    roomRef: request.roomRef,
    sourceDeviceRef: request.sourceDeviceRef,
    targetPeerRef: request.targetPeerRef,
    createdAt: now.toISOString(),
    expiresAt: new Date(Math.min(
      now.getTime() + ttlMs,
      Date.parse(request.expiresAt)
    )).toISOString(),
    previewOnly: false,
    payload: request
  };
  return validatedBuildResult(event, now);
}

export function buildCapabilityExecutionResultControlEvent(
  result: CapabilityExecutionResult,
  requestEvent: CapabilityExecuteRequestRoomControlEvent,
  options: BuildExecutionControlEventOptions = {}
): RoomControlEventBuildResult {
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_CONTROL_EVENT_TTL_MS;
  const errors = validateCapabilityExecutionResult(result);
  validateBuilderInputs(
    now,
    ttlMs,
    requestEvent.roomRef,
    requestEvent.targetPeerRef,
    requestEvent.sourceDeviceRef,
    errors
  );
  if (
    result.executionId !== requestEvent.payload.executionId ||
    result.requestId !== requestEvent.payload.requestId ||
    result.consentId !== requestEvent.payload.consentId
  ) {
    errors.push("Execution result must match the exact execution request IDs.");
  }
  if (errors.length > 0) {
    return { ok: false, errors: unique(errors) };
  }
  const event: CapabilityExecutionResultRoomControlEvent = {
    schemaVersion: ROOM_CONTROL_SCHEMA_VERSION,
    eventId: options.eventId ?? createEventId(now),
    kind: "capability_execution_result",
    roomRef: requestEvent.roomRef,
    sourceDeviceRef: requestEvent.targetPeerRef,
    targetPeerRef: requestEvent.sourceDeviceRef,
    createdAt: now.toISOString(),
    expiresAt: new Date(now.getTime() + ttlMs).toISOString(),
    previewOnly: false,
    payload: result
  };
  return validatedBuildResult(event, now);
}

export function validateRoomControlEvent(
  value: unknown,
  options: ValidateRoomControlEventOptions = {}
): RoomControlEventValidationResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  if (!isRecord(value)) {
    return { valid: false, errors: ["Room control event must be an object."] };
  }

  requireExactFields(
    value,
    ROOM_CONTROL_EVENT_REQUIRED_FIELDS,
    ROOM_CONTROL_EVENT_OPTIONAL_FIELDS,
    "Room control event",
    errors
  );
  for (const path of findUnsafeOrExecutionFieldPaths(value, "$", [], value)) {
    errors.push(`Unsafe or execution-like field is not allowed in room control event: ${path}.`);
  }
  if (serializedByteLength(value) > MAX_ROOM_CONTROL_EVENT_BYTES) {
    errors.push(`Room control event exceeds ${MAX_ROOM_CONTROL_EVENT_BYTES} bytes.`);
  }
  if (value.schemaVersion !== ROOM_CONTROL_SCHEMA_VERSION) {
    errors.push(`Room control event schemaVersion must be ${ROOM_CONTROL_SCHEMA_VERSION}.`);
  }
  requireBoundedString(value.eventId, "eventId", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.roomRef, "roomRef", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.sourceDeviceRef, "sourceDeviceRef", MAX_IDENTIFIER_LENGTH, errors);
  validateOptionalBoundedString(value.targetPeerRef, "targetPeerRef", MAX_IDENTIFIER_LENGTH, errors);
  validateDates(value.createdAt, value.expiresAt, now, errors);
  const kind = typeof value.kind === "string" && ROOM_CONTROL_EVENT_KINDS.has(value.kind as RoomControlEventKind)
    ? value.kind as RoomControlEventKind
    : null;
  if (!kind) {
    errors.push("Room control event contains an unknown or unsupported kind.");
  } else if (kind === "capability_preview") {
    validateCapabilityPreviewEvent(value, now, errors);
  } else if (kind === "capability_execute_request") {
    validateExecutionRequestEvent(value, now, errors);
  } else if (kind === "capability_execution_result") {
    validateExecutionResultEvent(value, errors);
  } else {
    validateStatusEvent(value, kind as CapabilityPreviewStatusRoomControlEvent["kind"], errors);
  }

  if (options.expectedRoomRef && value.roomRef !== options.expectedRoomRef) {
    errors.push("Room control event does not match the expected room.");
  }
  if (options.expectedTargetPeerRef && value.targetPeerRef !== options.expectedTargetPeerRef) {
    errors.push("Room control event does not target the expected peer.");
  }
  if (options.expectedSourceDeviceRef && value.sourceDeviceRef !== options.expectedSourceDeviceRef) {
    errors.push("Room control event does not match the expected source device.");
  }

  return errors.length === 0
    ? { valid: true, value: value as unknown as RoomControlEvent, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function createRoomControlEventSessionState(): RoomControlEventSessionState {
  return {
    seenEventIds: [],
    seenEnvelopeIds: [],
    seenRequestIds: [],
    seenExecutionRequestIds: [],
    seenExecutionResultIds: []
  };
}

export function checkAndRecordRoomControlEvent(
  event: RoomControlEvent,
  state: RoomControlEventSessionState,
  options: CheckRoomControlEventOptions = {}
): RoomControlEventReplayResult {
  const now = options.now ?? new Date();
  if (new Date(event.expiresAt).getTime() <= now.getTime()) {
    return { ok: false, reason: "expired", errors: ["Room control event is expired."], state };
  }
  if (state.seenEventIds.includes(event.eventId)) {
    return { ok: false, reason: "duplicate_event", errors: ["Room control event ID is a duplicate."], state };
  }
  if (event.kind === "capability_preview") {
    if (state.seenEnvelopeIds.includes(event.payload.envelopeId)) {
      return { ok: false, reason: "duplicate_envelope", errors: ["Room control preview envelope ID is a duplicate."], state };
    }
    if (state.seenRequestIds.includes(event.payload.request.requestId)) {
      return { ok: false, reason: "duplicate_request", errors: ["Room control preview request ID is a duplicate."], state };
    }
    return {
      ok: true,
      state: {
        seenEventIds: [...state.seenEventIds, event.eventId],
        seenEnvelopeIds: [...state.seenEnvelopeIds, event.payload.envelopeId],
        seenRequestIds: [...state.seenRequestIds, event.payload.request.requestId],
        seenExecutionRequestIds: state.seenExecutionRequestIds,
        seenExecutionResultIds: state.seenExecutionResultIds
      }
    };
  }
  if (event.kind === "capability_execute_request") {
    if (state.seenExecutionRequestIds.includes(event.payload.executionId)) {
      return { ok: false, reason: "duplicate_request", errors: ["Execution request ID is a duplicate."], state };
    }
    return {
      ok: true,
      state: {
        ...state,
        seenEventIds: [...state.seenEventIds, event.eventId],
        seenExecutionRequestIds: [...state.seenExecutionRequestIds, event.payload.executionId]
      }
    };
  }
  if (event.kind === "capability_execution_result") {
    if (state.seenExecutionResultIds.includes(event.payload.executionId)) {
      return { ok: false, reason: "duplicate_request", errors: ["Execution result ID is a duplicate."], state };
    }
    return {
      ok: true,
      state: {
        ...state,
        seenEventIds: [...state.seenEventIds, event.eventId],
        seenExecutionResultIds: [...state.seenExecutionResultIds, event.payload.executionId]
      }
    };
  }
  return {
    ok: true,
    state: {
      ...state,
      seenEventIds: [...state.seenEventIds, event.eventId]
    }
  };
}

// Pure CL-1 feasibility helper only. It is not wired into the transfer scheduler.
export function computeControlLaneBudget(options: {
  totalWindows?: number;
  controlBacklog: boolean;
}): ControlLaneBudget {
  const totalWindows = normalizeTotalWindows(options.totalWindows);
  const controlWindows = options.controlBacklog ? Math.min(1, totalWindows) : 0;
  return {
    totalWindows,
    controlWindows,
    dataWindows: totalWindows - controlWindows,
    controlBacklog: options.controlBacklog
  };
}

function validateCapabilityPreviewEvent(
  value: Record<string, unknown>,
  now: Date,
  errors: string[]
) {
  if (value.previewOnly !== true) {
    errors.push("Capability preview event requires previewOnly true.");
  }
  requireBoundedString(value.targetPeerRef, "targetPeerRef", MAX_IDENTIFIER_LENGTH, errors);
  const envelopeValidation = validateCapabilityRequestPreviewEnvelope(value.payload, {
    now,
    expectedRoomRef: typeof value.roomRef === "string" ? value.roomRef : undefined,
    expectedTargetPeerRef: typeof value.targetPeerRef === "string" ? value.targetPeerRef : undefined
  });
  if (!envelopeValidation.valid) {
    errors.push(...envelopeValidation.errors.map((error) => `Embedded preview envelope: ${error}`));
    return;
  }
  if (value.sourceDeviceRef !== envelopeValidation.value.sourceDeviceRef) {
    errors.push("Room control event source must match the embedded preview envelope source.");
  }
  const eventExpiry = typeof value.expiresAt === "string" ? new Date(value.expiresAt).getTime() : Number.NaN;
  if (Number.isFinite(eventExpiry) && eventExpiry > new Date(envelopeValidation.value.expiresAt).getTime()) {
    errors.push("Room control event expiry must not exceed embedded preview envelope expiry.");
  }
}

function validateStatusEvent(
  value: Record<string, unknown>,
  kind: CapabilityPreviewStatusRoomControlEvent["kind"],
  errors: string[]
) {
  if (value.previewOnly !== true) {
    errors.push("Capability preview status event requires previewOnly true.");
  }
  if (!isRecord(value.payload)) {
    errors.push("Room control status event payload must be an object.");
    return;
  }
  requireExactFields(
    value.payload,
    STATUS_PAYLOAD_REQUIRED_FIELDS,
    STATUS_PAYLOAD_OPTIONAL_FIELDS,
    "Room control status payload",
    errors
  );
  requireBoundedString(value.payload.envelopeId, "payload.envelopeId", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.payload.requestId, "payload.requestId", MAX_IDENTIFIER_LENGTH, errors);
  validateOptionalBoundedString(value.payload.reason, "payload.reason", MAX_REASON_LENGTH, errors);
  if (value.payload.consent !== undefined) {
    if (kind !== "capability_preview_ack") {
      errors.push("Only a capability preview acknowledgement may contain a consent grant.");
    }
    errors.push(...validateCapabilityConsentGrant(value.payload.consent));
  }
  if (value.payload.status !== STATUS_BY_KIND[kind]) {
    errors.push(`Room control event kind ${kind} requires status ${STATUS_BY_KIND[kind]}.`);
  }
}

function validateExecutionRequestEvent(
  value: Record<string, unknown>,
  now: Date,
  errors: string[]
) {
  if (value.previewOnly !== false) {
    errors.push("Execution request event requires previewOnly false.");
  }
  const requestErrors = validateCapabilityExecutionRequest(value.payload, now);
  errors.push(...requestErrors);
  if (!isRecord(value.payload)) {
    return;
  }
  if (
    value.roomRef !== value.payload.roomRef ||
    value.sourceDeviceRef !== value.payload.sourceDeviceRef ||
    value.targetPeerRef !== value.payload.targetPeerRef
  ) {
    errors.push("Execution request event must match the exact room/source/target request bindings.");
  }
  const eventExpiry = typeof value.expiresAt === "string" ? Date.parse(value.expiresAt) : Number.NaN;
  const requestExpiry = typeof value.payload.expiresAt === "string"
    ? Date.parse(value.payload.expiresAt)
    : Number.NaN;
  if (Number.isFinite(eventExpiry) && Number.isFinite(requestExpiry) && eventExpiry > requestExpiry) {
    errors.push("Execution request event expiry must not exceed request expiry.");
  }
}

function validateExecutionResultEvent(
  value: Record<string, unknown>,
  errors: string[]
) {
  if (value.previewOnly !== false) {
    errors.push("Execution result event requires previewOnly false.");
  }
  errors.push(...validateCapabilityExecutionResult(value.payload));
}

export function validateCapabilityConsentGrant(value: unknown): string[] {
  if (!isRecord(value)) {
    return ["Capability consent grant must be an object."];
  }
  const contract = getAgentBridgeCapabilityContract(value.capability)
    ?? getAgentBridgeCapabilityContractByConsentGrantSchema(value.schemaVersion);
  if (contract?.capability === FILE_CANDIDATES_CAPABILITY) {
    return validateFileCandidateConsentGrant(value);
  }
  if (contract?.capability === CANDIDATE_PAYLOAD_CAPABILITY) {
    return validateCandidatePayloadConsentGrant(value);
  }
  if (contract?.capability === ARTIFACT_TRANSFORM_CAPABILITY) return validateArtifactTransformConsentGrant(value);
  if (contract?.capability === HELLO_STDOUT_CAPABILITY) {
    return validateHelloStdoutConsentGrant(value);
  }
  if (contract?.capability === HELLO_TEMPLATE_CAPABILITY) {
    return validateHelloPeerConsentGrant(value);
  }
  return ["Capability consent grant capability is not registered."];
}

export function validateHelloPeerConsentGrant(value: unknown): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return ["Hello Peer consent grant must be an object."];
  }
  requireExactFields(value, CONSENT_GRANT_FIELDS, [], "Hello Peer consent grant", errors);
  if (value.schemaVersion !== CONSENT_GRANT_SCHEMA) {
    errors.push(`Hello Peer consent grant schemaVersion must be ${CONSENT_GRANT_SCHEMA}.`);
  }
  for (const field of [
    "consentId",
    "sourcePreviewEventId",
    "envelopeId",
    "requestId",
    "requestPayloadHash"
  ]) {
    requireBoundedString(value[field], `consent.${field}`, MAX_IDENTIFIER_LENGTH, errors);
  }
  validateFixedHelloCapabilityAndMessage(value, "Hello Peer consent grant", errors);
  requireDateString(value.expiresAt, "consent.expiresAt", errors);
  return unique(errors);
}

export function validateHelloStdoutConsentGrant(value: unknown): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return ["Hello Stdout consent grant must be an object."];
  }
  requireExactFields(value, HELLO_STDOUT_CONSENT_GRANT_FIELDS, [], "Hello Stdout consent grant", errors);
  if (value.schemaVersion !== HELLO_STDOUT_CONSENT_GRANT_SCHEMA) {
    errors.push(`Hello Stdout consent grant schemaVersion must be ${HELLO_STDOUT_CONSENT_GRANT_SCHEMA}.`);
  }
  for (const field of [
    "consentId",
    "sourcePreviewEventId",
    "envelopeId",
    "requestId",
    "requestPayloadHash"
  ]) {
    requireBoundedString(value[field], `consent.${field}`, MAX_IDENTIFIER_LENGTH, errors);
  }
  validateFixedHelloStdoutCapabilityAndExpectedStdout(value, "Hello Stdout consent grant", errors);
  requireDateString(value.expiresAt, "consent.expiresAt", errors);
  return unique(errors);
}

export function validateFileCandidateConsentGrant(value: unknown): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return ["File candidate consent grant must be an object."];
  }
  requireExactFields(value, FILE_CANDIDATE_CONSENT_GRANT_FIELDS, [], "File candidate consent grant", errors);
  if (value.schemaVersion !== FILE_CANDIDATE_CONSENT_GRANT_SCHEMA) {
    errors.push(`File candidate consent grant schemaVersion must be ${FILE_CANDIDATE_CONSENT_GRANT_SCHEMA}.`);
  }
  for (const field of [
    "consentId",
    "sourcePreviewEventId",
    "envelopeId",
    "requestId",
    "requestPayloadHash"
  ]) {
    requireBoundedString(value[field], `consent.${field}`, MAX_IDENTIFIER_LENGTH, errors);
  }
  if (value.capability !== FILE_CANDIDATES_CAPABILITY) {
    errors.push(`File candidate consent grant capability must be exactly ${FILE_CANDIDATES_CAPABILITY}.`);
  }
  requireBoundedString(value.filenameHint, "consent.filenameHint", 128, errors);
  if (value.searchMode !== "filename_metadata_only") {
    errors.push("File candidate consent grant searchMode must be filename_metadata_only.");
  }
  requireDateString(value.expiresAt, "consent.expiresAt", errors);
  return unique(errors);
}

export function validateCandidatePayloadConsentGrant(value: unknown): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return ["Candidate payload consent grant must be an object."];
  }
  requireExactFields(value, CANDIDATE_PAYLOAD_CONSENT_GRANT_FIELDS, [], "Candidate payload consent grant", errors);
  if (value.schemaVersion !== CANDIDATE_PAYLOAD_CONSENT_GRANT_SCHEMA) {
    errors.push(`Candidate payload consent grant schemaVersion must be ${CANDIDATE_PAYLOAD_CONSENT_GRANT_SCHEMA}.`);
  }
  for (const field of [
    "consentId",
    "sourcePreviewEventId",
    "envelopeId",
    "requestId",
    "requestPayloadHash",
    "sourceRequestId",
    "candidateId",
    "candidateDisplayName"
  ]) {
    requireBoundedString(value[field], `consent.${field}`, MAX_IDENTIFIER_LENGTH, errors);
  }
  if (value.capability !== CANDIDATE_PAYLOAD_CAPABILITY) {
    errors.push(`Candidate payload consent grant capability must be exactly ${CANDIDATE_PAYLOAD_CAPABILITY}.`);
  }
  if (value.sourceCapability !== FILE_CANDIDATES_CAPABILITY) {
    errors.push(`Candidate payload consent grant sourceCapability must be exactly ${FILE_CANDIDATES_CAPABILITY}.`);
  }
  if (value.candidateKind !== "filesystem_file") {
    errors.push("Candidate payload consent grant candidateKind must be filesystem_file.");
  }
  if (typeof value.candidateId === "string" && looksLikePath(value.candidateId)) {
    errors.push("Candidate payload consent grant candidateId must be opaque and not path-like.");
  }
  requireDateString(value.expiresAt, "consent.expiresAt", errors);
  return unique(errors);
}

export function validateArtifactTransformConsentGrant(value: unknown): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) return ["Artifact Transform consent grant must be an object."];
  requireExactFields(value, ARTIFACT_TRANSFORM_CONSENT_GRANT_FIELDS, [], "Artifact Transform consent grant", errors);
  if (value.schemaVersion !== ARTIFACT_TRANSFORM_CONSENT_GRANT_SCHEMA || value.capability !== ARTIFACT_TRANSFORM_CAPABILITY) errors.push("Artifact Transform consent grant has an invalid fixed contract.");
  for (const field of ["consentId", "sourcePreviewEventId", "envelopeId", "requestId", "requestPayloadHash", "sourceRequestId", "candidateId"]) requireBoundedString(value[field], `consent.${field}`, MAX_IDENTIFIER_LENGTH, errors);
  if (value.sourceCapability !== FILE_CANDIDATES_CAPABILITY || value.candidateKind !== "filesystem_file" || value.resultContract !== "typed_transform_result") errors.push("Artifact Transform consent grant has an invalid source or result contract.");
  if (typeof value.candidateId === "string" && looksLikePath(value.candidateId)) errors.push("Artifact Transform consent grant candidateId must be opaque.");
  requireDateString(value.expiresAt, "consent.expiresAt", errors);
  return unique(errors);
}

export function validateCapabilityExecutionRequest(value: unknown, now = new Date()): string[] {
  if (!isRecord(value)) {
    return ["Capability execution request must be an object."];
  }
  const contract = getAgentBridgeCapabilityContract(value.capability)
    ?? getAgentBridgeCapabilityContractByExecutionRequestSchema(value.schemaVersion);
  if (contract?.capability === FILE_CANDIDATES_CAPABILITY) {
    return validateFileCandidateExecutionRequest(value, { now }).errors;
  }
  if (contract?.capability === CANDIDATE_PAYLOAD_CAPABILITY) {
    return validateCandidatePayloadExecutionRequest(value, { now }).errors;
  }
  if (contract?.capability === ARTIFACT_TRANSFORM_CAPABILITY) return validateArtifactTransformExecutionRequest(value, { now }).errors;
  if (contract?.capability === HELLO_STDOUT_CAPABILITY) {
    return validateHelloStdoutExecutionRequest(value, now);
  }
  if (contract?.capability === HELLO_TEMPLATE_CAPABILITY) {
    return validateHelloPeerExecutionRequest(value, now);
  }
  return ["Capability execution request capability is not registered."];
}

export function validateHelloPeerExecutionRequest(value: unknown, now = new Date()): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return ["Hello Peer execution request must be an object."];
  }
  requireExactFields(value, EXECUTION_REQUEST_FIELDS, [], "Hello Peer execution request", errors);
  if (value.schemaVersion !== EXECUTION_REQUEST_SCHEMA) {
    errors.push(`Hello Peer execution request schemaVersion must be ${EXECUTION_REQUEST_SCHEMA}.`);
  }
  for (const field of [
    "executionId",
    "consentId",
    "sourcePreviewEventId",
    "envelopeId",
    "requestId",
    "requestPayloadHash",
    "roomRef",
    "sourceDeviceRef",
    "targetPeerRef"
  ]) {
    requireBoundedString(value[field], field, MAX_IDENTIFIER_LENGTH, errors);
  }
  validateFixedHelloCapabilityAndMessage(value, "Hello Peer execution request", errors);
  validateDates(value.createdAt, value.expiresAt, now, errors);
  return unique(errors);
}

export function validateHelloStdoutExecutionRequest(value: unknown, now = new Date()): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return ["Hello Stdout execution request must be an object."];
  }
  requireExactFields(value, HELLO_STDOUT_EXECUTION_REQUEST_FIELDS, [], "Hello Stdout execution request", errors);
  if (value.schemaVersion !== HELLO_STDOUT_EXECUTION_REQUEST_SCHEMA) {
    errors.push(`Hello Stdout execution request schemaVersion must be ${HELLO_STDOUT_EXECUTION_REQUEST_SCHEMA}.`);
  }
  for (const field of [
    "executionId",
    "consentId",
    "sourcePreviewEventId",
    "envelopeId",
    "requestId",
    "requestPayloadHash",
    "roomRef",
    "sourceDeviceRef",
    "targetPeerRef"
  ]) {
    requireBoundedString(value[field], field, MAX_IDENTIFIER_LENGTH, errors);
  }
  validateFixedHelloStdoutCapabilityAndExpectedStdout(value, "Hello Stdout execution request", errors);
  validateDates(value.createdAt, value.expiresAt, now, errors);
  return unique(errors);
}

export function validateCapabilityExecutionResult(value: unknown): string[] {
  if (!isRecord(value)) {
    return ["Capability execution result must be an object."];
  }
  const contract = getAgentBridgeCapabilityContractByExecutionResultSchema(value.schemaVersion);
  if (contract?.capability === FILE_CANDIDATES_CAPABILITY) {
    return validateFileCandidateExecutionResult(value).errors;
  }
  if (contract?.capability === CANDIDATE_PAYLOAD_CAPABILITY) {
    return validateCandidatePayloadExecutionResult(value).errors;
  }
  if (contract?.capability === ARTIFACT_TRANSFORM_CAPABILITY) return validateArtifactTransformExecutionResult(value).errors;
  if (contract?.capability === HELLO_STDOUT_CAPABILITY) {
    return validateHelloStdoutExecutionResult(value);
  }
  if (contract?.capability === HELLO_TEMPLATE_CAPABILITY) {
    return validateHelloPeerExecutionResult(value);
  }
  return ["Capability execution result schema is not registered."];
}

export function validateHelloPeerExecutionResult(value: unknown): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return ["Hello Peer execution result must be an object."];
  }
  requireExactFields(
    value,
    EXECUTION_RESULT_REQUIRED_FIELDS,
    EXECUTION_RESULT_OPTIONAL_FIELDS,
    "Hello Peer execution result",
    errors
  );
  if (value.schemaVersion !== EXECUTION_RESULT_SCHEMA) {
    errors.push(`Hello Peer execution result schemaVersion must be ${EXECUTION_RESULT_SCHEMA}.`);
  }
  for (const field of ["executionId", "requestId", "consentId"]) {
    requireBoundedString(value[field], field, MAX_IDENTIFIER_LENGTH, errors);
  }
  if (!["succeeded", "rejected", "expired", "already_consumed", "failed"].includes(String(value.status))) {
    errors.push("Hello Peer execution result contains an unsupported status.");
  }
  requireDateString(value.createdAt, "createdAt", errors);
  if (value.status === "succeeded") {
    if (value.output !== HELLO_TEMPLATE_MESSAGE) {
      errors.push(`Successful Hello Peer execution output must be exactly ${HELLO_TEMPLATE_MESSAGE}.`);
    }
    if (value.errorCode !== undefined) {
      errors.push("Successful Hello Peer execution result must not contain errorCode.");
    }
  } else {
    if (value.output !== undefined) {
      errors.push("Rejected or failed Hello Peer execution result must not contain output.");
    }
    requireBoundedString(value.errorCode, "errorCode", MAX_EXECUTION_ERROR_CODE_LENGTH, errors);
  }
  if (serializedByteLength(value) > 1024) {
    errors.push("Hello Peer execution result exceeds 1024 bytes.");
  }
  return unique(errors);
}

export function validateHelloStdoutExecutionResult(value: unknown): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return ["Hello Stdout execution result must be an object."];
  }
  requireExactFields(
    value,
    HELLO_STDOUT_EXECUTION_RESULT_REQUIRED_FIELDS,
    HELLO_STDOUT_EXECUTION_RESULT_OPTIONAL_FIELDS,
    "Hello Stdout execution result",
    errors
  );
  if (value.schemaVersion !== HELLO_STDOUT_EXECUTION_RESULT_SCHEMA) {
    errors.push(`Hello Stdout execution result schemaVersion must be ${HELLO_STDOUT_EXECUTION_RESULT_SCHEMA}.`);
  }
  for (const field of ["executionId", "requestId", "consentId"]) {
    requireBoundedString(value[field], field, MAX_IDENTIFIER_LENGTH, errors);
  }
  if (value.capability !== HELLO_STDOUT_CAPABILITY) {
    errors.push(`Hello Stdout execution result capability must be exactly ${HELLO_STDOUT_CAPABILITY}.`);
  }
  if (value.runtimeKind !== "rust_host_helper") {
    errors.push("Hello Stdout execution result runtimeKind must be rust_host_helper.");
  }
  if (!["succeeded", "rejected", "expired", "already_consumed", "failed"].includes(String(value.status))) {
    errors.push("Hello Stdout execution result contains an unsupported status.");
  }
  requireDateString(value.createdAt, "createdAt", errors);
  validateBoundedStringBytes(value.stdout, "stdout", MAX_HELLO_STDOUT_STDOUT_BYTES, errors);
  validateBoundedStringBytes(value.stderr, "stderr", MAX_HELLO_STDOUT_STDERR_BYTES, errors);
  validateNonNegativeInteger(value.exitCode, "exitCode", errors);
  validateNonNegativeInteger(value.durationMs, "durationMs", errors, MAX_HELLO_STDOUT_DURATION_MS);
  for (const field of ["timedOut", "stdoutTruncated", "stderrTruncated"]) {
    if (typeof value[field] !== "boolean") {
      errors.push(`Hello Stdout execution result requires boolean ${field}.`);
    }
  }
  if (value.status === "succeeded") {
    if (value.stdout !== HELLO_STDOUT_EXPECTED_STDOUT) {
      errors.push(`Successful Hello Stdout execution stdout must be exactly ${HELLO_STDOUT_EXPECTED_STDOUT}.`);
    }
    if (value.stderr !== "" || value.exitCode !== 0 || value.timedOut !== false) {
      errors.push("Successful Hello Stdout execution result must have empty stderr, exitCode 0, and timedOut false.");
    }
    if (value.errorCode !== undefined) {
      errors.push("Successful Hello Stdout execution result must not contain errorCode.");
    }
  } else {
    requireBoundedString(value.errorCode, "errorCode", MAX_EXECUTION_ERROR_CODE_LENGTH, errors);
  }
  if (serializedByteLength(value) > 2048) {
    errors.push("Hello Stdout execution result exceeds 2048 bytes.");
  }
  return unique(errors);
}

function validateFixedHelloCapabilityAndMessage(
  value: Record<string, unknown>,
  label: string,
  errors: string[]
) {
  if (value.capability !== HELLO_TEMPLATE_CAPABILITY) {
    errors.push(`${label} capability must be exactly ${HELLO_TEMPLATE_CAPABILITY}.`);
  }
  if (value.exactMessage !== HELLO_TEMPLATE_MESSAGE) {
    errors.push(`${label} message must be exactly ${HELLO_TEMPLATE_MESSAGE}.`);
  }
}

function validateFixedHelloStdoutCapabilityAndExpectedStdout(
  value: Record<string, unknown>,
  label: string,
  errors: string[]
) {
  if (value.capability !== HELLO_STDOUT_CAPABILITY) {
    errors.push(`${label} capability must be exactly ${HELLO_STDOUT_CAPABILITY}.`);
  }
  if (value.expectedStdout !== HELLO_STDOUT_EXPECTED_STDOUT) {
    errors.push(`${label} expectedStdout must be exactly ${HELLO_STDOUT_EXPECTED_STDOUT}.`);
  }
}

function validatedBuildResult(event: RoomControlEvent, now: Date): RoomControlEventBuildResult {
  const validation = validateRoomControlEvent(event, { now });
  return validation.valid
    ? { ok: true, event: validation.value }
    : { ok: false, errors: validation.errors };
}

function validateBuilderInputs(
  now: Date,
  ttlMs: number,
  roomRef: unknown,
  sourceDeviceRef: unknown,
  targetPeerRef: unknown,
  errors: string[]
) {
  if (!Number.isFinite(now.getTime()) || !Number.isFinite(ttlMs) || ttlMs <= 0) {
    errors.push("Room control event requires a valid time and positive finite TTL.");
  }
  requireBoundedString(roomRef, "roomRef", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(sourceDeviceRef, "sourceDeviceRef", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(targetPeerRef, "targetPeerRef", MAX_IDENTIFIER_LENGTH, errors);
}

function validateDates(createdAt: unknown, expiresAt: unknown, now: Date, errors: string[]) {
  const createdAtMs = typeof createdAt === "string" ? new Date(createdAt).getTime() : Number.NaN;
  const expiresAtMs = typeof expiresAt === "string" ? new Date(expiresAt).getTime() : Number.NaN;
  if (!Number.isFinite(createdAtMs)) {
    errors.push("Room control event requires a valid createdAt.");
  }
  if (!Number.isFinite(expiresAtMs)) {
    errors.push("Room control event requires a valid expiresAt.");
  } else if (expiresAtMs <= now.getTime()) {
    errors.push("Room control event is expired.");
  }
  if (Number.isFinite(createdAtMs) && Number.isFinite(expiresAtMs) && expiresAtMs <= createdAtMs) {
    errors.push("Room control event expiresAt must be after createdAt.");
  }
}

function requireDateString(value: unknown, label: string, errors: string[]) {
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value))) {
    errors.push(`Room control event requires a valid ${label}.`);
  }
}

function requireExactFields(
  value: Record<string, unknown>,
  requiredFields: string[],
  optionalFields: string[],
  label: string,
  errors: string[]
) {
  const actual = Object.keys(value);
  const allowed = new Set([...requiredFields, ...optionalFields]);
  if (requiredFields.some((field) => !actual.includes(field)) || actual.some((field) => !allowed.has(field))) {
    errors.push(`${label} contains missing or unsupported fields.`);
  }
}

function requireBoundedString(value: unknown, label: string, maxLength: number, errors: string[]) {
  if (typeof value !== "string" || value.trim().length === 0) {
    errors.push(`Room control event requires ${label}.`);
  } else if (value.length > maxLength) {
    errors.push(`Room control event ${label} exceeds ${maxLength} characters.`);
  }
}

function validateOptionalBoundedString(value: unknown, label: string, maxLength: number, errors: string[]) {
  if (typeof value === "undefined") {
    return;
  }
  requireBoundedString(value, label, maxLength, errors);
}

function validateBoundedStringBytes(value: unknown, label: string, maxBytes: number, errors: string[]) {
  if (typeof value !== "string") {
    errors.push(`Hello Stdout execution result requires string ${label}.`);
    return;
  }
  if (new TextEncoder().encode(value).byteLength > maxBytes) {
    errors.push(`Hello Stdout execution result ${label} exceeds ${maxBytes} bytes.`);
  }
}

function validateNonNegativeInteger(value: unknown, label: string, errors: string[], max?: number) {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || (max !== undefined && value > max)) {
    errors.push(`Hello Stdout execution result requires bounded non-negative integer ${label}.`);
  }
}

function findUnsafeOrExecutionFieldPaths(
  value: unknown,
  path = "$",
  found: string[] = [],
  root: unknown = value
): string[] {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => findUnsafeOrExecutionFieldPaths(entry, `${path}[${index}]`, found, root));
  } else if (isRecord(value)) {
    for (const [key, entry] of Object.entries(value)) {
      const entryPath = `${path}.${key}`;
      if (
        UNSAFE_OR_EXECUTION_FIELDS.has(normalizeCapabilityFieldName(key))
        && !isAllowedExecutionResultField(root, entryPath, key)
      ) {
        found.push(entryPath);
      }
      findUnsafeOrExecutionFieldPaths(entry, entryPath, found, root);
    }
  }
  return found;
}

function isAllowedExecutionResultField(root: unknown, path: string, key: string): boolean {
  if (!isRecord(root) || root.kind !== "capability_execution_result" || !isRecord(root.payload)) {
    return false;
  }
  if (root.payload.schemaVersion === HELLO_STDOUT_EXECUTION_RESULT_SCHEMA) {
    return path === `$.payload.${key}` && ["stdout", "stderr", "exitCode"].includes(key);
  }
  return root.payload.schemaVersion === ARTIFACT_TRANSFORM_RESULT_SCHEMA
    && path === `$.payload.result.output.${key}`
    && ["stdout", "stderr", "exitCode"].includes(key);
}

function looksLikePath(value: string): boolean {
  return value.startsWith("/") || /^[A-Za-z]:[\\/]/.test(value) || value.includes("/") || value.includes("\\");
}

function normalizeTotalWindows(value?: number): number {
  if (!Number.isFinite(value)) {
    return 8;
  }
  return Math.max(1, Math.floor(value as number));
}

function serializedByteLength(value: unknown): number {
  try {
    return new TextEncoder().encode(JSON.stringify(value)).byteLength;
  } catch {
    return Number.POSITIVE_INFINITY;
  }
}

function createEventId(now: Date): string {
  eventSequence += 1;
  const randomPart = globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${eventSequence}`;
  return `room-control-event-${randomPart}`;
}

function unique(errors: string[]): string[] {
  return [...new Set(errors)];
}

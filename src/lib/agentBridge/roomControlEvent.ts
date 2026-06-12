import { isRecord } from "../ai/actionPlanValidator";
import {
  validateCapabilityRequestPreviewEnvelope,
  type CapabilityRequestPreviewEnvelope
} from "../ai/capabilityPreviewEnvelope";

export type RoomControlEventKind =
  | "capability_preview"
  | "capability_preview_ack"
  | "capability_preview_deny"
  | "capability_preview_invalid"
  | "capability_preview_expired";

export type CapabilityPreviewControlStatus =
  | "acknowledged_preview_only"
  | "denied"
  | "invalid"
  | "expired";

export interface RoomControlEventBase {
  schemaVersion: "pastey-room-control-event/v1";
  eventId: string;
  kind: RoomControlEventKind;
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef?: string;
  createdAt: string;
  expiresAt: string;
  previewOnly: true;
}

export interface CapabilityPreviewRoomControlEvent extends RoomControlEventBase {
  kind: "capability_preview";
  targetPeerRef: string;
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
  payload: CapabilityPreviewStatusPayload & {
    status: "acknowledged_preview_only";
  };
}

export interface CapabilityPreviewDenyRoomControlEvent extends RoomControlEventBase {
  kind: "capability_preview_deny";
  payload: CapabilityPreviewStatusPayload & {
    status: "denied";
  };
}

export interface CapabilityPreviewInvalidRoomControlEvent extends RoomControlEventBase {
  kind: "capability_preview_invalid";
  payload: CapabilityPreviewStatusPayload & {
    status: "invalid";
  };
}

export interface CapabilityPreviewExpiredRoomControlEvent extends RoomControlEventBase {
  kind: "capability_preview_expired";
  payload: CapabilityPreviewStatusPayload & {
    status: "expired";
  };
}

export type CapabilityPreviewStatusRoomControlEvent =
  | CapabilityPreviewAckRoomControlEvent
  | CapabilityPreviewDenyRoomControlEvent
  | CapabilityPreviewInvalidRoomControlEvent
  | CapabilityPreviewExpiredRoomControlEvent;

export type RoomControlEvent =
  | CapabilityPreviewRoomControlEvent
  | CapabilityPreviewStatusRoomControlEvent;

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

const ROOM_CONTROL_SCHEMA_VERSION = "pastey-room-control-event/v1";
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
const STATUS_PAYLOAD_OPTIONAL_FIELDS = ["reason"];
const ROOM_CONTROL_EVENT_KINDS = new Set<RoomControlEventKind>([
  "capability_preview",
  "capability_preview_ack",
  "capability_preview_deny",
  "capability_preview_invalid",
  "capability_preview_expired"
]);
const STATUS_BY_KIND: Record<Exclude<RoomControlEventKind, "capability_preview">, CapabilityPreviewControlStatus> = {
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
  "stdout",
  "stderr",
  "exitCode",
  "process",
  "spawn"
].map(normalizeFieldName));
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
      ...(options.reason ? { reason: options.reason } : {})
    }
  } as CapabilityPreviewStatusRoomControlEvent;
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
  for (const path of findUnsafeOrExecutionFieldPaths(value)) {
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
  if (value.previewOnly !== true) {
    errors.push("Room control event requires previewOnly true.");
  }

  const kind = typeof value.kind === "string" && ROOM_CONTROL_EVENT_KINDS.has(value.kind as RoomControlEventKind)
    ? value.kind as RoomControlEventKind
    : null;
  if (!kind) {
    errors.push("Room control event contains an unknown or unsupported kind.");
  } else if (kind === "capability_preview") {
    validateCapabilityPreviewEvent(value, now, errors);
  } else {
    validateStatusEvent(value, kind, errors);
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
    seenRequestIds: []
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
        seenRequestIds: [...state.seenRequestIds, event.payload.request.requestId]
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
  kind: Exclude<RoomControlEventKind, "capability_preview">,
  errors: string[]
) {
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
  if (value.payload.status !== STATUS_BY_KIND[kind]) {
    errors.push(`Room control event kind ${kind} requires status ${STATUS_BY_KIND[kind]}.`);
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

function findUnsafeOrExecutionFieldPaths(value: unknown, path = "$", found: string[] = []): string[] {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => findUnsafeOrExecutionFieldPaths(entry, `${path}[${index}]`, found));
  } else if (isRecord(value)) {
    for (const [key, entry] of Object.entries(value)) {
      const entryPath = `${path}.${key}`;
      if (UNSAFE_OR_EXECUTION_FIELDS.has(normalizeFieldName(key))) {
        found.push(entryPath);
      }
      findUnsafeOrExecutionFieldPaths(entry, entryPath, found);
    }
  }
  return found;
}

function normalizeTotalWindows(value?: number): number {
  if (!Number.isFinite(value)) {
    return 8;
  }
  return Math.max(1, Math.floor(value as number));
}

function normalizeFieldName(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, "").toLowerCase();
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

import { isRecord } from "../ai/actionPlanValidator";
import {
  buildCapabilityPreviewStatusControlEvent,
  validateRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type CapabilityPreviewStatusRoomControlEvent,
  type HelloPeerConsentGrant,
  type RoomControlEvent,
} from "./roomControlEvent";
import {
  markControlQueueItemStatus,
  type ControlQueueState,
  type ControlQueueTransitionResult,
} from "./controlQueue";

export type PeerConsentDecision = "allow_once" | "deny";
export type PeerConsentStatus = "allowed_once" | "denied" | "expired" | "invalid";

export interface PeerConsentBinding {
  readonly schemaVersion: "pastey-peer-consent-binding/v1";
  readonly consentId: string;
  readonly sourceEventId: string;
  readonly envelopeId: string;
  readonly requestId: string;
  readonly requestPayloadHash: string;
  readonly roomRef: string;
  readonly sourceDeviceRef: string;
  readonly targetPeerRef: string;
  readonly capability: "runtime.execute_hello_template";
  readonly exactMessage: "hello peer!";
  readonly previewOnly: true;
  readonly createdAt: string;
  readonly expiresAt: string;
}

export interface PeerConsentRecord {
  readonly binding: PeerConsentBinding;
  readonly decision: PeerConsentDecision;
  readonly decidedAt: string;
  readonly status: PeerConsentStatus;
  readonly reason?: string;
}

export interface PeerConsentSessionState {
  decidedRequestIds: string[];
  decidedEnvelopeIds: string[];
  decidedEventIds: string[];
  consentIds: string[];
}

export type PeerPolicyResult =
  | { status: "reviewable"; binding: PeerConsentBinding; warnings: string[] }
  | { status: "rejected"; errors: string[] };

export type PeerConsentDecisionResult =
  | { ok: true; state: PeerConsentSessionState; record: PeerConsentRecord }
  | { ok: false; state: PeerConsentSessionState; errors: string[] };

export type PeerConsentValidationResult =
  | { valid: true; value: PeerConsentRecord; errors: [] }
  | { valid: false; errors: string[] };

export type PeerStatusEventBuildResult =
  | { ok: true; event: CapabilityPreviewStatusRoomControlEvent; record: PeerConsentRecord }
  | { ok: false; errors: string[] };

export interface PeerPolicyContext {
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  session: PeerConsentSessionState;
  now?: Date;
  consentId?: string;
  maxConsentLifetimeMs?: number;
}

const CONSENT_BINDING_SCHEMA = "pastey-peer-consent-binding/v1";
const HELLO_CAPABILITY = "runtime.execute_hello_template";
const HELLO_MESSAGE = "hello peer!";
const DEFAULT_MAX_CONSENT_LIFETIME_MS = 2 * 60 * 1_000;
const MAX_IDENTIFIER_LENGTH = 256;
const MAX_REASON_LENGTH = 512;
const BINDING_FIELDS = [
  "schemaVersion",
  "consentId",
  "sourceEventId",
  "envelopeId",
  "requestId",
  "requestPayloadHash",
  "roomRef",
  "sourceDeviceRef",
  "targetPeerRef",
  "capability",
  "exactMessage",
  "previewOnly",
  "createdAt",
  "expiresAt",
];
let consentSequence = 0;

export function createPeerConsentSessionState(): PeerConsentSessionState {
  return {
    decidedRequestIds: [],
    decidedEnvelopeIds: [],
    decidedEventIds: [],
    consentIds: [],
  };
}

export function preservePeerConsentSessionState(
  state: PeerConsentSessionState,
  previousSessionIdentity: string | null,
  nextSessionIdentity: string | null,
): PeerConsentSessionState {
  return previousSessionIdentity === nextSessionIdentity
    ? state
    : createPeerConsentSessionState();
}

export function evaluatePeerCapabilityPreview(
  event: RoomControlEvent,
  context: PeerPolicyContext,
): PeerPolicyResult {
  const now = context.now ?? new Date();
  const validation = validateRoomControlEvent(event, {
    now,
    expectedRoomRef: context.roomRef,
    expectedSourceDeviceRef: context.sourceDeviceRef,
    expectedTargetPeerRef: context.targetPeerRef,
  });
  if (!validation.valid) {
    return { status: "rejected", errors: validation.errors };
  }
  if (validation.value.kind !== "capability_preview") {
    return { status: "rejected", errors: ["Peer PolicyGate accepts only capability_preview events."] };
  }

  const preview = validation.value;
  const request = preview.payload.request;
  const errors: string[] = [];
  if (request.capability !== HELLO_CAPABILITY) {
    errors.push(`Peer PolicyGate capability must be exactly ${HELLO_CAPABILITY}.`);
  }
  if (request.input.message !== HELLO_MESSAGE) {
    errors.push(`Peer PolicyGate message must be exactly ${HELLO_MESSAGE}.`);
  }
  if (preview.previewOnly !== true || preview.payload.previewOnly !== true) {
    errors.push("Peer PolicyGate requires previewOnly true.");
  }
  if (
    preview.sourceDeviceRef !== preview.payload.sourceDeviceRef ||
    preview.sourceDeviceRef !== request.sourceDeviceRef
  ) {
    errors.push("Peer PolicyGate source binding is inconsistent.");
  }
  if (
    preview.targetPeerRef !== preview.payload.targetPeerRef ||
    preview.targetPeerRef !== request.targetPeerRef
  ) {
    errors.push("Peer PolicyGate target binding is inconsistent.");
  }
  if (preview.roomRef !== preview.payload.roomRef) {
    errors.push("Peer PolicyGate room binding is inconsistent.");
  }
  if (context.session.decidedEventIds.includes(preview.eventId)) {
    errors.push("Peer PolicyGate event has already been decided.");
  }
  if (context.session.decidedEnvelopeIds.includes(preview.payload.envelopeId)) {
    errors.push("Peer PolicyGate envelope has already been decided.");
  }
  if (context.session.decidedRequestIds.includes(request.requestId)) {
    errors.push("Peer PolicyGate request has already been decided.");
  }
  const maxLifetimeMs = context.maxConsentLifetimeMs ?? DEFAULT_MAX_CONSENT_LIFETIME_MS;
  if (!Number.isFinite(maxLifetimeMs) || maxLifetimeMs <= 0) {
    errors.push("Peer PolicyGate requires a positive bounded consent lifetime.");
  }
  if (errors.length > 0) {
    return { status: "rejected", errors: unique(errors) };
  }

  const expiresAtMs = Math.min(
    Date.parse(preview.expiresAt),
    Date.parse(preview.payload.expiresAt),
    Date.parse(request.expiresAt),
    now.getTime() + maxLifetimeMs,
  );
  const binding: PeerConsentBinding = Object.freeze({
    schemaVersion: CONSENT_BINDING_SCHEMA,
    consentId: context.consentId ?? createConsentId(now),
    sourceEventId: preview.eventId,
    envelopeId: preview.payload.envelopeId,
    requestId: request.requestId,
    requestPayloadHash: request.requestPayloadHash,
    roomRef: preview.roomRef,
    sourceDeviceRef: preview.sourceDeviceRef,
    targetPeerRef: preview.targetPeerRef,
    capability: HELLO_CAPABILITY,
    exactMessage: HELLO_MESSAGE,
    previewOnly: true,
    createdAt: now.toISOString(),
    expiresAt: new Date(expiresAtMs).toISOString(),
  });
  const bindingErrors = validatePeerConsentBinding(binding, now);
  return bindingErrors.length === 0
    ? {
        status: "reviewable",
        binding,
        warnings: [
          "Trusted room membership is not approval.",
          "Allow once applies only to this exact request and does not execute it.",
        ],
      }
    : { status: "rejected", errors: bindingErrors };
}

export function allowPeerCapabilityOnce(
  binding: PeerConsentBinding,
  state: PeerConsentSessionState,
  options: { now?: Date; reason?: string } = {},
): PeerConsentDecisionResult {
  return decidePeerCapability(binding, "allow_once", state, options);
}

export function denyPeerCapability(
  binding: PeerConsentBinding,
  state: PeerConsentSessionState,
  options: { now?: Date; reason?: string } = {},
): PeerConsentDecisionResult {
  return decidePeerCapability(binding, "deny", state, options);
}

export function expirePeerConsent(record: PeerConsentRecord, now = new Date()): PeerConsentRecord {
  return Date.parse(record.binding.expiresAt) <= now.getTime()
    ? { ...record, status: "expired", reason: "One-time peer consent expired unused." }
    : record;
}

export function validatePeerConsentRecord(
  value: unknown,
  options: { now?: Date; expectedBinding?: PeerConsentBinding } = {},
): PeerConsentValidationResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  if (!isRecord(value)) {
    return { valid: false, errors: ["Peer consent record must be an object."] };
  }
  requireExactFields(
    value,
    ["binding", "decision", "decidedAt", "status"],
    ["reason"],
    "Peer consent record",
    errors,
  );
  const bindingErrors = validatePeerConsentBinding(value.binding, now);
  errors.push(...bindingErrors);
  if (value.decision !== "allow_once" && value.decision !== "deny") {
    errors.push("Peer consent record contains an unsupported decision.");
  }
  if (!["allowed_once", "denied", "expired", "invalid"].includes(String(value.status))) {
    errors.push("Peer consent record contains an unsupported status.");
  }
  requireDate(value.decidedAt, "decidedAt", errors);
  if (typeof value.reason === "string" && value.reason.length > MAX_REASON_LENGTH) {
    errors.push(`Peer consent reason exceeds ${MAX_REASON_LENGTH} characters.`);
  }
  if (options.expectedBinding && stableBinding(value.binding) !== stableBinding(options.expectedBinding)) {
    errors.push("Peer consent record does not match the exact expected binding.");
  }
  return errors.length === 0
    ? { valid: true, value: value as unknown as PeerConsentRecord, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function buildPeerConsentStatusEvent(
  sourceEvent: CapabilityPreviewRoomControlEvent,
  record: PeerConsentRecord,
  options: { now?: Date; eventId?: string } = {},
): PeerStatusEventBuildResult {
  const now = options.now ?? new Date();
  const validation = validatePeerConsentRecord(record, { now });
  if (!validation.valid) {
    return { ok: false, errors: validation.errors };
  }
  const binding = validation.value.binding;
  if (
    binding.sourceEventId !== sourceEvent.eventId ||
    binding.envelopeId !== sourceEvent.payload.envelopeId ||
    binding.requestId !== sourceEvent.payload.request.requestId ||
    binding.requestPayloadHash !== sourceEvent.payload.request.requestPayloadHash ||
    binding.roomRef !== sourceEvent.roomRef ||
    binding.sourceDeviceRef !== sourceEvent.sourceDeviceRef ||
    binding.targetPeerRef !== sourceEvent.targetPeerRef
  ) {
    return { ok: false, errors: ["Peer consent record does not bind to the source preview event."] };
  }
  const status = record.decision === "allow_once" ? "acknowledged_preview_only" : "denied";
  const result = buildCapabilityPreviewStatusControlEvent(sourceEvent, status, {
    now,
    eventId: options.eventId,
    ...(record.decision === "allow_once" ? { consent: consentGrantFromRecord(record) } : {}),
    reason: record.decision === "allow_once"
      ? "Receiver allowed this exact preview once. No execution occurred."
      : "Receiver denied this preview. No retry requested.",
  });
  if (!result.ok) {
    return result;
  }
  if (
    result.event.kind === "capability_preview"
    || result.event.kind === "capability_execute_request"
    || result.event.kind === "capability_execution_result"
  ) {
    return { ok: false, errors: ["Peer consent status builder produced an invalid preview event."] };
  }
  return { ok: true, event: result.event, record: validation.value };
}

function consentGrantFromRecord(record: PeerConsentRecord): HelloPeerConsentGrant {
  return {
    schemaVersion: "pastey-hello-peer-consent-grant/v1",
    consentId: record.binding.consentId,
    sourcePreviewEventId: record.binding.sourceEventId,
    envelopeId: record.binding.envelopeId,
    requestId: record.binding.requestId,
    requestPayloadHash: record.binding.requestPayloadHash,
    capability: record.binding.capability,
    exactMessage: record.binding.exactMessage,
    expiresAt: record.binding.expiresAt,
  };
}

export function applyInboundPeerStatusToOutboundQueue(
  state: ControlQueueState,
  statusEvent: CapabilityPreviewStatusRoomControlEvent,
  options: { now?: Date } = {},
): ControlQueueTransitionResult {
  const matching = state.outbound.find((item) =>
    item.event.kind === "capability_preview" &&
    item.event.payload.envelopeId === statusEvent.payload.envelopeId &&
    item.event.payload.request.requestId === statusEvent.payload.requestId &&
    item.event.roomRef === statusEvent.roomRef &&
    item.event.sourceDeviceRef === statusEvent.targetPeerRef &&
    item.event.targetPeerRef === statusEvent.sourceDeviceRef
  );
  if (!matching) {
    return { ok: false, state, errors: ["No matching outbound capability preview was found."] };
  }
  return markControlQueueItemStatus(state, matching.queueId, statusEvent.payload.status, {
    now: options.now,
    reason: statusEvent.kind === "capability_preview_ack"
      ? "Peer allowed this exact preview once. No execution has occurred."
      : statusEvent.kind === "capability_preview_deny"
        ? "Peer denied the preview. No retry will be attempted."
        : `Peer returned ${statusEvent.payload.status}. No execution occurred.`,
  });
}

function decidePeerCapability(
  binding: PeerConsentBinding,
  decision: PeerConsentDecision,
  state: PeerConsentSessionState,
  options: { now?: Date; reason?: string },
): PeerConsentDecisionResult {
  const now = options.now ?? new Date();
  const errors = validatePeerConsentBinding(binding, now);
  if (state.decidedEventIds.includes(binding.sourceEventId)) {
    errors.push("A decision already exists for this event.");
  }
  if (state.decidedEnvelopeIds.includes(binding.envelopeId)) {
    errors.push("A decision already exists for this envelope.");
  }
  if (state.decidedRequestIds.includes(binding.requestId)) {
    errors.push("A decision already exists for this request.");
  }
  if (state.consentIds.includes(binding.consentId)) {
    errors.push("Peer consent ID has already been used.");
  }
  if (options.reason !== undefined && options.reason.length > MAX_REASON_LENGTH) {
    errors.push(`Peer consent reason exceeds ${MAX_REASON_LENGTH} characters.`);
  }
  if (errors.length > 0) {
    return { ok: false, state, errors: unique(errors) };
  }
  const record: PeerConsentRecord = Object.freeze({
    binding: Object.freeze({ ...binding }),
    decision,
    decidedAt: now.toISOString(),
    status: decision === "allow_once" ? "allowed_once" : "denied",
    ...(options.reason ? { reason: options.reason } : {}),
  });
  return {
    ok: true,
    record,
    state: {
      decidedRequestIds: [...state.decidedRequestIds, binding.requestId],
      decidedEnvelopeIds: [...state.decidedEnvelopeIds, binding.envelopeId],
      decidedEventIds: [...state.decidedEventIds, binding.sourceEventId],
      consentIds: [...state.consentIds, binding.consentId],
    },
  };
}

function validatePeerConsentBinding(value: unknown, now: Date): string[] {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return ["Peer consent binding must be an object."];
  }
  requireExactFields(value, BINDING_FIELDS, [], "Peer consent binding", errors);
  if (value.schemaVersion !== CONSENT_BINDING_SCHEMA) {
    errors.push(`Peer consent binding schemaVersion must be ${CONSENT_BINDING_SCHEMA}.`);
  }
  for (const field of [
    "consentId",
    "sourceEventId",
    "envelopeId",
    "requestId",
    "requestPayloadHash",
    "roomRef",
    "sourceDeviceRef",
    "targetPeerRef",
  ]) {
    requireBoundedString(value[field], field, errors);
  }
  if (value.capability !== HELLO_CAPABILITY) {
    errors.push(`Peer consent capability must be exactly ${HELLO_CAPABILITY}.`);
  }
  if (value.exactMessage !== HELLO_MESSAGE) {
    errors.push(`Peer consent message must be exactly ${HELLO_MESSAGE}.`);
  }
  if (value.previewOnly !== true) {
    errors.push("Peer consent binding requires previewOnly true.");
  }
  requireDate(value.createdAt, "createdAt", errors);
  requireDate(value.expiresAt, "expiresAt", errors);
  const expiresAt = typeof value.expiresAt === "string" ? Date.parse(value.expiresAt) : Number.NaN;
  if (Number.isFinite(expiresAt) && expiresAt <= now.getTime()) {
    errors.push("Peer consent binding is expired.");
  }
  return errors;
}

function requireExactFields(
  value: Record<string, unknown>,
  required: string[],
  optional: string[],
  label: string,
  errors: string[],
) {
  const allowed = new Set([...required, ...optional]);
  if (required.some((field) => !(field in value)) || Object.keys(value).some((field) => !allowed.has(field))) {
    errors.push(`${label} contains missing or unsupported fields.`);
  }
}

function requireBoundedString(value: unknown, label: string, errors: string[]) {
  if (typeof value !== "string" || value.length === 0 || value.length > MAX_IDENTIFIER_LENGTH) {
    errors.push(`Peer consent binding requires bounded ${label}.`);
  }
}

function requireDate(value: unknown, label: string, errors: string[]) {
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value))) {
    errors.push(`Peer consent requires a valid ${label}.`);
  }
}

function stableBinding(value: unknown): string {
  return JSON.stringify(value, Object.keys(isRecord(value) ? value : {}).sort());
}

function createConsentId(now: Date): string {
  consentSequence += 1;
  return `peer-consent-${globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${consentSequence}`}`;
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

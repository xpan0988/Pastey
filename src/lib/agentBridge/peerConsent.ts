import { isRecord } from "../ai/actionPlanValidator";
import {
  FILE_CANDIDATES_CAPABILITY,
  getAgentBridgeCapabilityContract,
  HELLO_STDOUT_CAPABILITY,
  HELLO_STDOUT_EXPECTED_STDOUT,
  HELLO_TEMPLATE_CAPABILITY,
  HELLO_TEMPLATE_MESSAGE,
  type AgentBridgeCapabilityContract
} from "../ai/capabilityRegistry";
import { validateFileCandidateAdvisoryInput } from "../ai/fileCandidateAdvisory";
import {
  buildCapabilityPreviewStatusControlEvent,
  validateRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type CapabilityPreviewStatusRoomControlEvent,
  type CapabilityConsentGrant,
  type FileCandidateConsentGrant,
  type HelloPeerConsentGrant,
  type HelloStdoutConsentGrant,
  type RoomControlEvent,
} from "./roomControlEvent";
import {
  markControlQueueItemStatus,
  type ControlQueueState,
  type ControlQueueTransitionResult,
} from "./controlQueue";

export type PeerConsentDecision = "allow_once" | "deny";
export type PeerConsentStatus = "allowed_once" | "denied" | "expired" | "invalid";

interface PeerConsentBindingBase {
  readonly schemaVersion: "pastey-peer-consent-binding/v1";
  readonly consentId: string;
  readonly sourceEventId: string;
  readonly envelopeId: string;
  readonly requestId: string;
  readonly requestPayloadHash: string;
  readonly roomRef: string;
  readonly sourceDeviceRef: string;
  readonly targetPeerRef: string;
  readonly previewOnly: true;
  readonly createdAt: string;
  readonly expiresAt: string;
}

export interface HelloPeerConsentBinding extends PeerConsentBindingBase {
  readonly capability: "runtime.execute_hello_template";
  readonly exactMessage: "hello peer!";
}

export interface HelloStdoutConsentBinding extends PeerConsentBindingBase {
  readonly capability: "runtime.hello_stdout/v1";
  readonly expectedStdout: "hello peer";
}

export interface FileCandidateConsentBinding extends PeerConsentBindingBase {
  readonly capability: "filesystem.find_file_candidates/v1";
  readonly filenameHint: string;
  readonly searchMode: "filename_metadata_only";
}

export type PeerConsentBinding = HelloPeerConsentBinding | HelloStdoutConsentBinding | FileCandidateConsentBinding;

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
const HELLO_STDOUT_BINDING_FIELDS = [
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
  "expectedStdout",
  "previewOnly",
  "createdAt",
  "expiresAt",
];
const FILE_CANDIDATE_BINDING_FIELDS = [
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
  "filenameHint",
  "searchMode",
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
  const fileCandidateRequest = request.capability === FILE_CANDIDATES_CAPABILITY ? request : null;
  const errors: string[] = [];
  const contract = getAgentBridgeCapabilityContract(request.capability);
  if (!contract) {
    errors.push("Peer PolicyGate capability is not supported.");
  } else if (!requestInputMatchesContract(request.input, contract)) {
    errors.push(`Peer PolicyGate ${contract.typedBindingField} must be exactly ${contract.typedBindingValue}.`);
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
  if (!contract) {
    return { status: "rejected", errors: ["Peer PolicyGate capability is not supported."] };
  }

  const expiresAtMs = Math.min(
    Date.parse(preview.expiresAt),
    Date.parse(preview.payload.expiresAt),
    Date.parse(request.expiresAt),
    now.getTime() + maxLifetimeMs,
  );
  const baseBinding = {
    schemaVersion: CONSENT_BINDING_SCHEMA,
    consentId: context.consentId ?? createConsentId(now),
    sourceEventId: preview.eventId,
    envelopeId: preview.payload.envelopeId,
    requestId: request.requestId,
    requestPayloadHash: request.requestPayloadHash,
    roomRef: preview.roomRef,
    sourceDeviceRef: preview.sourceDeviceRef,
    targetPeerRef: preview.targetPeerRef,
    previewOnly: true,
    createdAt: now.toISOString(),
    expiresAt: new Date(expiresAtMs).toISOString(),
  } as const;
  const binding: PeerConsentBinding = Object.freeze(
    contract.capability === FILE_CANDIDATES_CAPABILITY && fileCandidateRequest
      ? {
          ...baseBinding,
          capability: FILE_CANDIDATES_CAPABILITY,
          filenameHint: fileCandidateRequest.input.query.filenameHint,
          searchMode: fileCandidateRequest.input.query.searchMode,
        }
      : contract.capability === HELLO_STDOUT_CAPABILITY
      ? {
          ...baseBinding,
          capability: HELLO_STDOUT_CAPABILITY,
          expectedStdout: contract.typedBindingValue as "hello peer",
        }
      : {
          ...baseBinding,
          capability: HELLO_TEMPLATE_CAPABILITY,
          exactMessage: contract.typedBindingValue as "hello peer!",
        }
  );
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

function consentGrantFromRecord(record: PeerConsentRecord): CapabilityConsentGrant {
  const base = {
    consentId: record.binding.consentId,
    sourcePreviewEventId: record.binding.sourceEventId,
    envelopeId: record.binding.envelopeId,
    requestId: record.binding.requestId,
    requestPayloadHash: record.binding.requestPayloadHash,
    expiresAt: record.binding.expiresAt,
  };
  if (record.binding.capability === HELLO_STDOUT_CAPABILITY) {
    const grant: HelloStdoutConsentGrant = {
      ...base,
      schemaVersion: "pastey-runtime-hello-stdout-consent-grant/v1",
      capability: HELLO_STDOUT_CAPABILITY,
      expectedStdout: record.binding.expectedStdout,
    };
    return grant;
  }
  if (record.binding.capability === FILE_CANDIDATES_CAPABILITY) {
    const grant: FileCandidateConsentGrant = {
      ...base,
      schemaVersion: "filesystem-find-file-candidates-consent-grant/v1",
      capability: FILE_CANDIDATES_CAPABILITY,
      filenameHint: record.binding.filenameHint,
      searchMode: record.binding.searchMode,
    };
    return grant;
  }
  const grant: HelloPeerConsentGrant = {
    ...base,
    schemaVersion: "pastey-hello-peer-consent-grant/v1",
    capability: HELLO_TEMPLATE_CAPABILITY,
    exactMessage: record.binding.exactMessage,
  };
  return grant;
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
  const isHelloStdout = value.capability === HELLO_STDOUT_CAPABILITY;
  const isFileCandidate = value.capability === FILE_CANDIDATES_CAPABILITY;
  requireExactFields(
    value,
    isFileCandidate ? FILE_CANDIDATE_BINDING_FIELDS : isHelloStdout ? HELLO_STDOUT_BINDING_FIELDS : BINDING_FIELDS,
    [],
    "Peer consent binding",
    errors
  );
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
  if (isFileCandidate) {
    if (value.filenameHint === undefined || typeof value.filenameHint !== "string" || value.filenameHint.trim().length === 0 || value.filenameHint.length > 128) {
      errors.push("Peer consent file candidate filenameHint must be bounded.");
    }
    if (value.searchMode !== "filename_metadata_only") {
      errors.push("Peer consent file candidate searchMode must be filename_metadata_only.");
    }
  } else if (isHelloStdout) {
    if (value.expectedStdout !== HELLO_STDOUT_EXPECTED_STDOUT) {
      errors.push(`Peer consent expectedStdout must be exactly ${HELLO_STDOUT_EXPECTED_STDOUT}.`);
    }
  } else {
    if (value.capability !== HELLO_TEMPLATE_CAPABILITY) {
      errors.push(`Peer consent capability must be exactly ${HELLO_TEMPLATE_CAPABILITY}.`);
    }
    if (value.exactMessage !== HELLO_TEMPLATE_MESSAGE) {
      errors.push(`Peer consent message must be exactly ${HELLO_TEMPLATE_MESSAGE}.`);
    }
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

function requestInputMatchesContract(
  input: unknown,
  contract: AgentBridgeCapabilityContract,
): boolean {
  if (!isRecord(input)) {
    return false;
  }
  if (contract.capability === FILE_CANDIDATES_CAPABILITY) {
    return validateFileCandidateAdvisoryInput(input).valid;
  }
  if (contract.typedBindingField === "exactMessage") {
    return input.message === contract.typedBindingValue;
  }
  return input.expectedStdout === contract.typedBindingValue;
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

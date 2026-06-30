import { isRecord } from "./actionPlanValidator";
import {
  findForbiddenProviderFieldPaths,
  getAgentBridgeCapabilityContract,
  getAgentBridgeCapabilityContractByPreviewSchema,
  SHARED_CAPABILITY_ENVELOPE_SCHEMA,
  type AgentBridgeCapabilityEnvelope
} from "./capabilityRegistry";
import {
  validateHelloPeerRequest,
  type HelloPeerRequest
} from "./helloPeerRequest";
import {
  validateHelloStdoutRequest,
  type HelloStdoutRequest
} from "./helloStdoutRequest";
import {
  validateFileCandidateRequest,
  type FileCandidateRequest
} from "./fileCandidateRequest";

export type CapabilityRequest = HelloPeerRequest | HelloStdoutRequest | FileCandidateRequest;
export type CapabilitySharedPreviewEnvelope = AgentBridgeCapabilityEnvelope<CapabilityRequest>;

export type CapabilityPreviewStatus =
  | "outbound_preview"
  | "sent_preview"
  | "received_preview"
  | "acknowledged_preview_only"
  | "denied"
  | "expired"
  | "invalid";

export interface CapabilityRequestPreviewEnvelope {
  schemaVersion: "pastey-capability-preview-v1";
  envelopeId: string;
  createdAt: string;
  expiresAt: string;
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  request: CapabilityRequest;
  previewOnly: true;
  status: CapabilityPreviewStatus;
}

export type CapabilityPreviewBuildResult =
  | { ok: true; envelope: CapabilityRequestPreviewEnvelope }
  | { ok: false; errors: string[] };

export type CapabilityPreviewValidationResult =
  | { valid: true; value: CapabilityRequestPreviewEnvelope; errors: [] }
  | { valid: false; errors: string[] };

export interface CapabilityPreviewSessionState {
  seenEnvelopeIds: string[];
  seenRequestIds: string[];
}

export type CapabilityPreviewReplayResult =
  | { ok: true; state: CapabilityPreviewSessionState }
  | { ok: false; reason: "duplicate_envelope" | "duplicate_request" | "expired"; errors: string[]; state: CapabilityPreviewSessionState };

interface BuildCapabilityPreviewOptions {
  roomRef: string;
  sourceDeviceRef?: string;
  targetPeerRef?: string;
  now?: Date;
  ttlMs?: number;
  envelopeId?: string;
}

interface ValidateCapabilityPreviewOptions {
  now?: Date;
  expectedTargetPeerRef?: string;
  expectedRoomRef?: string;
}

const DEFAULT_ENVELOPE_TTL_MS = 2 * 60 * 1_000;
const PREVIEW_STATUSES = new Set<CapabilityPreviewStatus>([
  "outbound_preview",
  "sent_preview",
  "received_preview",
  "acknowledged_preview_only",
  "denied",
  "expired",
  "invalid"
]);
const ENVELOPE_FIELDS = [
  "schemaVersion",
  "envelopeId",
  "createdAt",
  "expiresAt",
  "roomRef",
  "sourceDeviceRef",
  "targetPeerRef",
  "request",
  "previewOnly",
  "status"
];
let envelopeSequence = 0;

export function buildCapabilityRequestPreviewEnvelope(
  request: CapabilityRequest,
  options: BuildCapabilityPreviewOptions
): CapabilityPreviewBuildResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_ENVELOPE_TTL_MS;
  const requestValidation = validateCapabilityRequest(request, { now });
  if (!requestValidation.valid) {
    errors.push(...requestValidation.errors);
  }
  if (!isNonEmptyString(options.roomRef)) {
    errors.push("Capability preview envelope requires a roomRef.");
  }
  if (!Number.isFinite(now.getTime()) || !Number.isFinite(ttlMs) || ttlMs <= 0) {
    errors.push("Capability preview envelope requires a valid time and positive finite TTL.");
  }

  const sourceDeviceRef = options.sourceDeviceRef ?? request.sourceDeviceRef;
  const targetPeerRef = options.targetPeerRef ?? request.targetPeerRef;
  if (sourceDeviceRef !== request.sourceDeviceRef) {
    errors.push("Capability preview envelope source must match the embedded request source.");
  }
  if (targetPeerRef !== request.targetPeerRef) {
    errors.push("Capability preview envelope target must match the embedded request target.");
  }
  if (errors.length > 0) {
    return { ok: false, errors: [...new Set(errors)] };
  }

  const requestExpiry = new Date(request.expiresAt).getTime();
  const envelope: CapabilityRequestPreviewEnvelope = {
    schemaVersion: "pastey-capability-preview-v1",
    envelopeId: options.envelopeId ?? createEnvelopeId(now),
    createdAt: now.toISOString(),
    expiresAt: new Date(Math.min(now.getTime() + ttlMs, requestExpiry)).toISOString(),
    roomRef: options.roomRef,
    sourceDeviceRef,
    targetPeerRef,
    request,
    previewOnly: true,
    status: "outbound_preview"
  };
  const validation = validateCapabilityRequestPreviewEnvelope(envelope, { now });
  return validation.valid
    ? { ok: true, envelope: validation.value }
    : { ok: false, errors: validation.errors };
}

export function validateCapabilityRequestPreviewEnvelope(
  value: unknown,
  options: ValidateCapabilityPreviewOptions = {}
): CapabilityPreviewValidationResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  if (!isRecord(value)) {
    return { valid: false, errors: ["Capability preview envelope must be an object."] };
  }

  requireExactFields(value, ENVELOPE_FIELDS, "Capability preview envelope", errors);
  for (const path of findUnsafeOrExecutionFieldPaths(value)) {
    errors.push(`Unsafe or execution-like field is not allowed in capability preview envelope: ${path}.`);
  }
  if (value.schemaVersion !== "pastey-capability-preview-v1") {
    errors.push("Capability preview envelope schemaVersion must be pastey-capability-preview-v1.");
  }
  requireNonEmptyString(value.envelopeId, "envelopeId", errors);
  requireNonEmptyString(value.roomRef, "roomRef", errors);
  requireNonEmptyString(value.sourceDeviceRef, "sourceDeviceRef", errors);
  requireNonEmptyString(value.targetPeerRef, "targetPeerRef", errors);
  if (value.previewOnly !== true) {
    errors.push("Capability preview envelope requires previewOnly true.");
  }
  if (typeof value.status !== "string" || !PREVIEW_STATUSES.has(value.status as CapabilityPreviewStatus)) {
    errors.push("Capability preview envelope contains an invalid preview-only status.");
  }
  validateDates(value.createdAt, value.expiresAt, now, errors);

  const requestValidation = validateCapabilityRequest(value.request, { now });
  if (!requestValidation.valid) {
    errors.push(...requestValidation.errors.map((error) => `Embedded request: ${error}`));
  } else {
    const request = requestValidation.value;
    if (request.transportStatus !== "preview_only") {
      errors.push("Embedded capability request must remain preview_only.");
    }
    if (value.targetPeerRef !== request.targetPeerRef) {
      errors.push("Capability preview envelope target must match the embedded request target.");
    }
    if (value.sourceDeviceRef !== request.sourceDeviceRef) {
      errors.push("Capability preview envelope source must match the embedded request source.");
    }
    const envelopeExpiry = typeof value.expiresAt === "string" ? new Date(value.expiresAt).getTime() : Number.NaN;
    if (Number.isFinite(envelopeExpiry) && envelopeExpiry > new Date(request.expiresAt).getTime()) {
      errors.push("Capability preview envelope expiry must not exceed embedded request expiry.");
    }
  }

  if (options.expectedTargetPeerRef && value.targetPeerRef !== options.expectedTargetPeerRef) {
    errors.push("Capability preview envelope does not target the expected peer.");
  }
  if (options.expectedRoomRef && value.roomRef !== options.expectedRoomRef) {
    errors.push("Capability preview envelope does not match the expected room.");
  }

  return errors.length === 0
    ? { valid: true, value: value as unknown as CapabilityRequestPreviewEnvelope, errors: [] }
    : { valid: false, errors: [...new Set(errors)] };
}

function validateCapabilityRequest(
  value: unknown,
  options: { now?: Date }
) {
  if (!isRecord(value)) {
    return validateHelloPeerRequest(value, options);
  }
  const contract = getAgentBridgeCapabilityContract(value.capability)
    ?? getAgentBridgeCapabilityContractByPreviewSchema(value.schemaVersion);
  if (contract?.capability === "filesystem.find_file_candidates") {
    return validateFileCandidateRequest(value, options);
  }
  if (contract?.capability === "runtime.hello_stdout") {
    return validateHelloStdoutRequest(value, options);
  }
  if (contract?.capability === "runtime.execute_hello_template") {
    return validateHelloPeerRequest(value, options);
  }
  return {
    valid: false as const,
    errors: ["Embedded request capability is not registered."]
  };
}

export function deriveCapabilitySharedPreviewEnvelope(
  envelope: CapabilityRequestPreviewEnvelope
): CapabilitySharedPreviewEnvelope {
  const contract = getAgentBridgeCapabilityContract(envelope.request.capability);
  if (!contract) {
    throw new Error("Cannot derive shared envelope for an unregistered capability.");
  }
  return {
    schemaVersion: SHARED_CAPABILITY_ENVELOPE_SCHEMA,
    capability: contract.capability,
    capabilityVersion: contract.version,
    requestId: envelope.request.requestId,
    roomRef: envelope.roomRef,
    sourceDeviceRef: envelope.sourceDeviceRef,
    targetPeerRef: envelope.targetPeerRef,
    routePolicy: contract.routePolicy,
    consentPolicy: contract.consentPolicy,
    createdAt: envelope.createdAt,
    expiresAt: envelope.expiresAt,
    payloadHash: envelope.request.requestPayloadHash,
    typedPayload: envelope.request,
    transport: {
      kind: "room-control",
      route: contract.routePolicy,
      previewOnly: envelope.previewOnly,
      maxPayloadBytes: 64 * 1024,
    },
  };
}

export function createCapabilityPreviewSessionState(): CapabilityPreviewSessionState {
  return {
    seenEnvelopeIds: [],
    seenRequestIds: []
  };
}

export function checkAndRecordCapabilityPreview(
  envelope: CapabilityRequestPreviewEnvelope,
  state: CapabilityPreviewSessionState,
  now = new Date()
): CapabilityPreviewReplayResult {
  if (new Date(envelope.expiresAt).getTime() <= now.getTime()) {
    return { ok: false, reason: "expired", errors: ["Capability preview envelope is expired."], state };
  }
  if (state.seenEnvelopeIds.includes(envelope.envelopeId)) {
    return { ok: false, reason: "duplicate_envelope", errors: ["Capability preview envelope ID is a duplicate."], state };
  }
  if (state.seenRequestIds.includes(envelope.request.requestId)) {
    return { ok: false, reason: "duplicate_request", errors: ["Capability preview request ID is a duplicate."], state };
  }
  return {
    ok: true,
    state: {
      seenEnvelopeIds: [...state.seenEnvelopeIds, envelope.envelopeId],
      seenRequestIds: [...state.seenRequestIds, envelope.request.requestId]
    }
  };
}

export function markCapabilityPreviewReceived(
  envelope: CapabilityRequestPreviewEnvelope
): CapabilityRequestPreviewEnvelope {
  return { ...envelope, status: "received_preview" };
}

export function acknowledgeCapabilityPreview(
  envelope: CapabilityRequestPreviewEnvelope
): CapabilityRequestPreviewEnvelope {
  return { ...envelope, status: "acknowledged_preview_only" };
}

export function denyCapabilityPreview(
  envelope: CapabilityRequestPreviewEnvelope
): CapabilityRequestPreviewEnvelope {
  return { ...envelope, status: "denied" };
}

function validateDates(createdAt: unknown, expiresAt: unknown, now: Date, errors: string[]) {
  const createdAtMs = typeof createdAt === "string" ? new Date(createdAt).getTime() : Number.NaN;
  const expiresAtMs = typeof expiresAt === "string" ? new Date(expiresAt).getTime() : Number.NaN;
  if (!Number.isFinite(createdAtMs)) errors.push("Capability preview envelope requires a valid createdAt.");
  if (!Number.isFinite(expiresAtMs)) {
    errors.push("Capability preview envelope requires a valid expiresAt.");
  } else if (expiresAtMs <= now.getTime()) {
    errors.push("Capability preview envelope is expired.");
  }
  if (Number.isFinite(createdAtMs) && Number.isFinite(expiresAtMs) && expiresAtMs <= createdAtMs) {
    errors.push("Capability preview envelope expiresAt must be after createdAt.");
  }
}

function findUnsafeOrExecutionFieldPaths(value: unknown): string[] {
  return findForbiddenProviderFieldPaths(value);
}

function requireExactFields(value: Record<string, unknown>, expectedFields: string[], label: string, errors: string[]) {
  const actual = Object.keys(value).sort();
  const expected = [...expectedFields].sort();
  if (actual.length !== expected.length || actual.some((field, index) => field !== expected[index])) {
    errors.push(`${label} contains missing or unsupported fields.`);
  }
}

function requireNonEmptyString(value: unknown, label: string, errors: string[]) {
  if (!isNonEmptyString(value)) errors.push(`Capability preview envelope requires ${label}.`);
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function createEnvelopeId(now: Date): string {
  envelopeSequence += 1;
  const randomPart = globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${envelopeSequence}`;
  return `capability-preview-${randomPart}`;
}

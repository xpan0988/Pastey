import { isRecord } from "./actionPlanValidator";
import {
  CANDIDATE_PAYLOAD_CAPABILITY,
  CANDIDATE_PAYLOAD_EXECUTOR_KIND,
  FILE_CANDIDATES_CAPABILITY,
  findForbiddenProviderFieldPaths,
} from "./capabilityRegistry";
import {
  buildCandidatePayloadRequestInput,
  validateCandidatePayloadAdvisoryInput,
  validateCandidatePayloadRequestInput,
  type CandidatePayloadAdvisoryInput,
  type CandidatePayloadKind,
  type CandidatePayloadRequestInput,
} from "./candidatePayloadAdvisory";
import {
  buildPendingAiActionCanonicalPayload,
  hashDeterministicString,
  hashPendingAiActionPayload,
  isPendingAiActionExpired,
  stableSerialize,
} from "./pendingAction";
import type { FileCandidateMimeFamily } from "./fileCandidateRequest";
import type { PendingAiAction } from "./types";

export const CANDIDATE_PAYLOAD_REQUEST_SCHEMA = "transfer-request-candidate-payload-request-v1";
export const CANDIDATE_PAYLOAD_CONSENT_GRANT_SCHEMA = "transfer-request-candidate-payload-consent-grant-v1";
export const CANDIDATE_PAYLOAD_EXECUTION_REQUEST_SCHEMA = "transfer-request-candidate-payload-execution-request-v1";
export const CANDIDATE_PAYLOAD_RESULT_SCHEMA = "transfer-request-candidate-payload-result-v1";

export type CandidatePayloadExecutionStatus =
  | "handoff_queued"
  | "handoff_failed"
  | "candidate_resolved_handoff_not_implemented"
  | "candidate_not_found"
  | "candidate_expired"
  | "candidate_changed"
  | "handoff_not_implemented"
  | "rejected"
  | "expired"
  | "already_consumed"
  | "failed";

export type CandidatePayloadErrorCode =
  | "missing_consent"
  | "consent_not_allowed_once"
  | "consent_expired"
  | "invalid_consent"
  | "consent_binding_mismatch"
  | "already_consumed"
  | "malformed_request"
  | "unsupported_route"
  | "unsafe_request_rejected"
  | "handoff_not_implemented"
  | "handoff_failed"
  | "policy_rejected";

export interface CandidatePayloadRequest {
  schemaVersion: typeof CANDIDATE_PAYLOAD_REQUEST_SCHEMA;
  requestId: string;
  nonce: string;
  createdAt: string;
  expiresAt: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: typeof CANDIDATE_PAYLOAD_CAPABILITY;
  executorKind: typeof CANDIDATE_PAYLOAD_EXECUTOR_KIND;
  input: CandidatePayloadRequestInput;
  pendingPayloadHash: string;
  requestPayloadHash: string;
  transportStatus: "preview_only";
}

export interface CandidatePayloadExecutionRequest {
  schemaVersion: typeof CANDIDATE_PAYLOAD_EXECUTION_REQUEST_SCHEMA;
  executionId: string;
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: typeof CANDIDATE_PAYLOAD_CAPABILITY;
  executorKind: typeof CANDIDATE_PAYLOAD_EXECUTOR_KIND;
  sourceCapability: typeof FILE_CANDIDATES_CAPABILITY;
  sourceRequestId: string;
  candidateId: string;
  candidateKind: CandidatePayloadKind;
  candidateDisplayName: string;
  createdAt: string;
  expiresAt: string;
}

export interface CandidatePayloadResultCandidate {
  candidateId: string;
  candidateKind: CandidatePayloadKind;
  candidateDisplayName: string;
  sizeBytes?: number;
  mimeFamily?: FileCandidateMimeFamily;
  extension?: string;
}

export type CandidatePayloadResolutionReason =
  | "resolved"
  | "not_found"
  | "expired"
  | "changed"
  | "binding_mismatch"
  | "unsupported_kind";

export interface CandidatePayloadResolution {
  sourceCapability: typeof FILE_CANDIDATES_CAPABILITY;
  sourceRequestId: string;
  candidateId: string;
  candidateKind: CandidatePayloadKind;
  resolved: boolean;
  reason: CandidatePayloadResolutionReason;
  displayName?: string;
  sizeBytes?: number;
  modifiedAt?: string;
  mimeFamily?: FileCandidateMimeFamily;
  extension?: string;
}

export interface CandidatePayloadExecutionResult {
  schemaVersion: typeof CANDIDATE_PAYLOAD_RESULT_SCHEMA;
  capability: typeof CANDIDATE_PAYLOAD_CAPABILITY;
  executionId: string;
  requestId: string;
  consentId: string;
  status: CandidatePayloadExecutionStatus;
  candidate: CandidatePayloadResultCandidate;
  candidateResolution?: CandidatePayloadResolution;
  transferredBytes: 0;
  handoffQueued: boolean;
  transferStatus?: "queued";
  errorCode: CandidatePayloadErrorCode | null;
  createdAt: string;
}

export type CandidatePayloadRequestBuildResult =
  | { ok: true; request: CandidatePayloadRequest }
  | { ok: false; errors: string[] };

export type CandidatePayloadRequestValidationResult =
  | { valid: true; value: CandidatePayloadRequest; errors: [] }
  | { valid: false; errors: string[] };

export type CandidatePayloadExecutionRequestValidationResult =
  | { valid: true; value: CandidatePayloadExecutionRequest; errors: [] }
  | { valid: false; errors: string[] };

export type CandidatePayloadExecutionResultValidationResult =
  | { valid: true; value: CandidatePayloadExecutionResult; errors: [] }
  | { valid: false; errors: string[] };

interface BuildCandidatePayloadRequestOptions {
  now?: Date;
  ttlMs?: number;
  sourceDeviceRef?: string;
  nonce?: string;
  requestId?: string;
}

const DEFAULT_REQUEST_TTL_MS = 2 * 60 * 1_000;
const MAX_IDENTIFIER_LENGTH = 256;
const REQUEST_FIELDS = [
  "schemaVersion",
  "requestId",
  "nonce",
  "createdAt",
  "expiresAt",
  "sourceDeviceRef",
  "targetPeerRef",
  "capability",
  "executorKind",
  "input",
  "pendingPayloadHash",
  "requestPayloadHash",
  "transportStatus",
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
  "executorKind",
  "sourceCapability",
  "sourceRequestId",
  "candidateId",
  "candidateKind",
  "candidateDisplayName",
  "createdAt",
  "expiresAt",
];
const RESULT_FIELDS = [
  "schemaVersion",
  "capability",
  "executionId",
  "requestId",
  "consentId",
  "status",
  "candidate",
  "transferredBytes",
  "handoffQueued",
  "errorCode",
  "createdAt",
];
const RESULT_OPTIONAL_FIELDS = ["candidateResolution", "transferStatus"];
const RESULT_CANDIDATE_REQUIRED_FIELDS = ["candidateId", "candidateKind", "candidateDisplayName"];
const RESULT_CANDIDATE_OPTIONAL_FIELDS = ["sizeBytes", "mimeFamily", "extension"];
const RESULT_RESOLUTION_REQUIRED_FIELDS = [
  "sourceCapability",
  "sourceRequestId",
  "candidateId",
  "candidateKind",
  "resolved",
  "reason",
];
const RESULT_RESOLUTION_OPTIONAL_FIELDS = ["displayName", "sizeBytes", "modifiedAt", "mimeFamily", "extension"];
const STATUSES = new Set<CandidatePayloadExecutionStatus>([
  "handoff_queued",
  "handoff_failed",
  "candidate_resolved_handoff_not_implemented",
  "candidate_not_found",
  "candidate_expired",
  "candidate_changed",
  "handoff_not_implemented",
  "rejected",
  "expired",
  "already_consumed",
  "failed",
]);
const ERROR_CODES = new Set<CandidatePayloadErrorCode>([
  "missing_consent",
  "consent_not_allowed_once",
  "consent_expired",
  "invalid_consent",
  "consent_binding_mismatch",
  "already_consumed",
  "malformed_request",
  "unsupported_route",
  "unsafe_request_rejected",
  "handoff_not_implemented",
  "handoff_failed",
  "policy_rejected",
]);
const MIME_FAMILIES = new Set<FileCandidateMimeFamily>(["document", "image", "archive", "media", "code", "unknown"]);
const RESOLUTION_REASONS = new Set<CandidatePayloadResolutionReason>([
  "resolved",
  "not_found",
  "expired",
  "changed",
  "binding_mismatch",
  "unsupported_kind",
]);
let requestSequence = 0;

export function buildCandidatePayloadRequestFromPendingAction(
  pending: PendingAiAction,
  options: BuildCandidatePayloadRequestOptions = {},
): CandidatePayloadRequestBuildResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_REQUEST_TTL_MS;

  if (pending.status !== "confirmed_local_only") {
    errors.push("Candidate payload request preview requires a confirmed_local_only pending action.");
  }
  if (pending.policyResult.status !== "accepted" || !pending.policyResult.requiresUserConfirmation) {
    errors.push("Candidate payload request preview requires the accepted confirmation-bound PolicyGate result.");
  }
  if (!Number.isFinite(now.getTime()) || !Number.isFinite(ttlMs) || ttlMs <= 0) {
    errors.push("Candidate payload request preview requires a valid time and positive finite TTL.");
  }
  if (isPendingAiActionExpired(pending, now)) {
    errors.push("Candidate payload request preview cannot be built from an expired pending action.");
  }
  if (pending.actionPlan.kind !== "request_peer_candidate_payload") {
    errors.push("Candidate payload request preview requires request_peer_candidate_payload.");
  }

  let rebuiltPendingPayload;
  try {
    rebuiltPendingPayload = buildPendingAiActionCanonicalPayload(
      pending.actionPlan,
      pending.pendingId,
      pending.expiresAt,
    );
    if (stableSerialize(rebuiltPendingPayload) !== stableSerialize(pending.canonicalPayload)) {
      errors.push("Pending action canonical payload does not match the confirmed action plan.");
    }
    if (hashPendingAiActionPayload(pending.canonicalPayload) !== pending.payloadHash) {
      errors.push("Pending action payload hash does not match the confirmed canonical payload.");
    }
  } catch (error) {
    errors.push(error instanceof Error ? error.message : "Pending action payload validation failed.");
  }

  const inputValidation = validateCandidatePayloadAdvisoryInput(pending.actionPlan.proposedInput);
  if (!inputValidation.valid) {
    errors.push(...inputValidation.errors);
  }
  if (errors.length > 0 || !rebuiltPendingPayload || !inputValidation.valid) {
    return { ok: false, errors: unique(errors) };
  }

  const createdAt = now.toISOString();
  const expiresAtMs = Math.min(now.getTime() + ttlMs, new Date(pending.expiresAt).getTime());
  const requestId = options.requestId ?? createPreviewIdentifier("candidate-payload-request", now);
  const nonce = options.nonce ?? createPreviewIdentifier("candidate-payload-nonce", now);
  const sourceDeviceRef = options.sourceDeviceRef ?? "local-device-preview";
  const requestWithoutHash: Omit<CandidatePayloadRequest, "requestPayloadHash"> = {
    schemaVersion: CANDIDATE_PAYLOAD_REQUEST_SCHEMA,
    requestId,
    nonce,
    createdAt,
    expiresAt: new Date(expiresAtMs).toISOString(),
    sourceDeviceRef,
    targetPeerRef: rebuiltPendingPayload.targetPeerRef,
    capability: CANDIDATE_PAYLOAD_CAPABILITY,
    executorKind: CANDIDATE_PAYLOAD_EXECUTOR_KIND,
    input: buildCandidatePayloadRequestInput(inputValidation.value),
    pendingPayloadHash: pending.payloadHash,
    transportStatus: "preview_only",
  };
  const request: CandidatePayloadRequest = {
    ...requestWithoutHash,
    requestPayloadHash: hashCandidatePayloadRequestPayload(requestWithoutHash),
  };
  const validation = validateCandidatePayloadRequest(request, { now });
  return validation.valid
    ? { ok: true, request: validation.value }
    : { ok: false, errors: validation.errors };
}

export function hashCandidatePayloadRequestPayload(
  request: Omit<CandidatePayloadRequest, "requestPayloadHash">,
): string {
  return hashDeterministicString(stableSerialize(request));
}

export function validateCandidatePayloadRequest(
  value: unknown,
  options: { now?: Date } = {},
): CandidatePayloadRequestValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Candidate payload request must be an object."] };
  }
  requireExactFields(value, REQUEST_FIELDS, "Candidate payload request", errors);
  for (const path of findUnsafeFieldPaths(value)) {
    errors.push(`Unsafe field is not allowed in candidate payload request: ${path}.`);
  }
  validateCommonRequestFields(value, CANDIDATE_PAYLOAD_REQUEST_SCHEMA, options.now ?? new Date(), errors);
  requireBoundedString(value.nonce, "nonce", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.pendingPayloadHash, "pendingPayloadHash", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.requestPayloadHash, "requestPayloadHash", MAX_IDENTIFIER_LENGTH, errors);
  if (value.transportStatus !== "preview_only") {
    errors.push("Candidate payload request transportStatus must be preview_only.");
  }
  const inputValidation = validateCandidatePayloadRequestInput(value.input);
  if (!inputValidation.valid) {
    errors.push(...inputValidation.errors);
  }
  if (errors.length === 0) {
    const { requestPayloadHash, ...requestWithoutHash } = value;
    const expectedHash = hashCandidatePayloadRequestPayload(requestWithoutHash as Omit<CandidatePayloadRequest, "requestPayloadHash">);
    if (requestPayloadHash !== expectedHash) {
      errors.push("Candidate payload request payload hash does not match the canonical preview payload.");
    }
  }
  return errors.length === 0
    ? { valid: true, value: value as unknown as CandidatePayloadRequest, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function validateCandidatePayloadExecutionRequest(
  value: unknown,
  options: { now?: Date } = {},
): CandidatePayloadExecutionRequestValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Candidate payload execution request must be an object."] };
  }
  requireExactFields(value, EXECUTION_REQUEST_FIELDS, "Candidate payload execution request", errors);
  for (const path of findUnsafeFieldPaths(value)) {
    errors.push(`Unsafe field is not allowed in candidate payload execution request: ${path}.`);
  }
  validateCommonRequestFields(value, CANDIDATE_PAYLOAD_EXECUTION_REQUEST_SCHEMA, options.now ?? new Date(), errors);
  for (const field of [
    "executionId",
    "consentId",
    "sourcePreviewEventId",
    "envelopeId",
    "requestId",
    "requestPayloadHash",
    "sourceRequestId",
    "candidateId",
    "candidateDisplayName",
  ]) {
    requireBoundedString(value[field], field, MAX_IDENTIFIER_LENGTH, errors);
  }
  if (typeof value.candidateId === "string" && looksLikePath(value.candidateId)) {
    errors.push("Candidate payload execution candidateId must be opaque and not path-like.");
  }
  if (value.sourceCapability !== FILE_CANDIDATES_CAPABILITY) {
    errors.push(`Candidate payload execution sourceCapability must be exactly ${FILE_CANDIDATES_CAPABILITY}.`);
  }
  if (value.candidateKind !== "filesystem_file") {
    errors.push("Candidate payload execution candidateKind must be filesystem_file.");
  }
  return errors.length === 0
    ? { valid: true, value: value as unknown as CandidatePayloadExecutionRequest, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function validateCandidatePayloadExecutionResult(
  value: unknown,
): CandidatePayloadExecutionResultValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Candidate payload execution result must be an object."] };
  }
  requireExactFields(value, RESULT_FIELDS, "Candidate payload execution result", errors, RESULT_OPTIONAL_FIELDS);
  if (value.schemaVersion !== CANDIDATE_PAYLOAD_RESULT_SCHEMA) {
    errors.push(`Candidate payload execution result schemaVersion must be ${CANDIDATE_PAYLOAD_RESULT_SCHEMA}.`);
  }
  if (value.capability !== CANDIDATE_PAYLOAD_CAPABILITY) {
    errors.push(`Candidate payload execution result capability must be exactly ${CANDIDATE_PAYLOAD_CAPABILITY}.`);
  }
  for (const field of ["executionId", "requestId", "consentId"]) {
    requireBoundedString(value[field], field, MAX_IDENTIFIER_LENGTH, errors);
  }
  if (typeof value.status !== "string" || !STATUSES.has(value.status as CandidatePayloadExecutionStatus)) {
    errors.push("Candidate payload execution result contains an unsupported status.");
  }
  validateResultCandidate(value.candidate, errors);
  if (value.candidateResolution !== undefined) {
    validateCandidateResolution(value.candidateResolution, errors);
  }
  if (value.transferredBytes !== 0) {
    errors.push("Candidate payload scaffold result must report transferredBytes 0.");
  }
  if (value.status === "handoff_queued") {
    if (value.handoffQueued !== true || value.transferStatus !== "queued") {
      errors.push("Candidate payload handoff result must report queued handoff status.");
    }
  } else {
    if (value.handoffQueued !== false) {
      errors.push("Candidate payload non-handoff result must report handoffQueued false.");
    }
    if (value.transferStatus !== undefined) {
      errors.push("Candidate payload transferStatus is only allowed for queued handoff results.");
    }
  }
  if (value.status === "handoff_queued") {
    if (value.errorCode !== null) {
      errors.push("Candidate payload queued handoff result must have null errorCode.");
    }
  } else if (
    value.status === "candidate_resolved_handoff_not_implemented" ||
    value.status === "candidate_not_found" ||
    value.status === "candidate_expired" ||
    value.status === "candidate_changed" ||
    value.status === "handoff_not_implemented"
  ) {
    if (value.errorCode !== null) {
      errors.push("Candidate payload resolution/handoff scaffold result must have null errorCode.");
    }
  } else if (typeof value.errorCode !== "string" || !ERROR_CODES.has(value.errorCode as CandidatePayloadErrorCode)) {
    errors.push("Candidate payload non-success result requires a bounded errorCode.");
  }
  requireDateString(value.createdAt, "createdAt", errors);
  if (serializedByteLength(value) > 4096) {
    errors.push("Candidate payload execution result exceeds 4096 bytes.");
  }
  return errors.length === 0
    ? { valid: true, value: value as unknown as CandidatePayloadExecutionResult, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function buildCandidatePayloadResultCandidate(
  input: CandidatePayloadAdvisoryInput | CandidatePayloadRequestInput | CandidatePayloadExecutionRequest,
): CandidatePayloadResultCandidate {
  return {
    candidateId: input.candidateId,
    candidateKind: input.candidateKind,
    candidateDisplayName: input.candidateDisplayName,
    ...("sizeBytes" in input && input.sizeBytes !== undefined ? { sizeBytes: input.sizeBytes } : {}),
    ...("mimeFamily" in input && input.mimeFamily !== undefined ? { mimeFamily: input.mimeFamily } : {}),
    ...("extension" in input && input.extension !== undefined ? { extension: input.extension } : {}),
  };
}

function validateCommonRequestFields(
  value: Record<string, unknown>,
  schemaVersion: string,
  now: Date,
  errors: string[],
) {
  if (value.schemaVersion !== schemaVersion) {
    errors.push(`Candidate payload request schemaVersion must be ${schemaVersion}.`);
  }
  if (value.capability !== CANDIDATE_PAYLOAD_CAPABILITY) {
    errors.push(`Candidate payload request capability must be exactly ${CANDIDATE_PAYLOAD_CAPABILITY}.`);
  }
  if (value.executorKind !== CANDIDATE_PAYLOAD_EXECUTOR_KIND) {
    errors.push(`Candidate payload request executorKind must be exactly ${CANDIDATE_PAYLOAD_EXECUTOR_KIND}.`);
  }
  requireBoundedString(value.roomRef, "roomRef", MAX_IDENTIFIER_LENGTH, errors, true);
  requireBoundedString(value.sourceDeviceRef, "sourceDeviceRef", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.targetPeerRef, "targetPeerRef", MAX_IDENTIFIER_LENGTH, errors);
  validateDates(value.createdAt, value.expiresAt, now, errors);
}

function validateResultCandidate(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("Candidate payload execution result requires candidate.");
    return;
  }
  requireExactFields(
    value,
    RESULT_CANDIDATE_REQUIRED_FIELDS,
    "Candidate payload result candidate",
    errors,
    RESULT_CANDIDATE_OPTIONAL_FIELDS,
  );
  requireBoundedString(value.candidateId, "candidate.candidateId", MAX_IDENTIFIER_LENGTH, errors);
  if (typeof value.candidateId === "string" && looksLikePath(value.candidateId)) {
    errors.push("Candidate payload result candidateId must be opaque and not path-like.");
  }
  requireBoundedString(value.candidateDisplayName, "candidate.candidateDisplayName", 255, errors);
  if (typeof value.candidateDisplayName === "string" && looksLikePath(value.candidateDisplayName)) {
    errors.push("Candidate payload result candidateDisplayName must be display-only, not a path.");
  }
  if (value.candidateKind !== "filesystem_file") {
    errors.push("Candidate payload result candidateKind must be filesystem_file.");
  }
  if (value.sizeBytes !== undefined && (typeof value.sizeBytes !== "number" || !Number.isInteger(value.sizeBytes) || value.sizeBytes < 0)) {
    errors.push("Candidate payload result sizeBytes must be a non-negative integer when present.");
  }
  if (value.mimeFamily !== undefined && (typeof value.mimeFamily !== "string" || !MIME_FAMILIES.has(value.mimeFamily as FileCandidateMimeFamily))) {
    errors.push("Candidate payload result mimeFamily is unsupported.");
  }
  if (value.extension !== undefined && (typeof value.extension !== "string" || !/^[a-z0-9]{0,16}$/i.test(value.extension))) {
    errors.push("Candidate payload result extension must be bounded.");
  }
}

function validateCandidateResolution(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("Candidate payload execution result candidateResolution must be an object.");
    return;
  }
  requireExactFields(
    value,
    RESULT_RESOLUTION_REQUIRED_FIELDS,
    "Candidate payload resolution",
    errors,
    RESULT_RESOLUTION_OPTIONAL_FIELDS,
  );
  if (value.sourceCapability !== FILE_CANDIDATES_CAPABILITY) {
    errors.push(`Candidate payload resolution sourceCapability must be exactly ${FILE_CANDIDATES_CAPABILITY}.`);
  }
  requireBoundedString(value.sourceRequestId, "candidateResolution.sourceRequestId", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.candidateId, "candidateResolution.candidateId", MAX_IDENTIFIER_LENGTH, errors);
  if (typeof value.candidateId === "string" && looksLikePath(value.candidateId)) {
    errors.push("Candidate payload resolution candidateId must be opaque and not path-like.");
  }
  if (value.candidateKind !== "filesystem_file") {
    errors.push("Candidate payload resolution candidateKind must be filesystem_file.");
  }
  if (typeof value.resolved !== "boolean") {
    errors.push("Candidate payload resolution requires boolean resolved.");
  }
  if (typeof value.reason !== "string" || !RESOLUTION_REASONS.has(value.reason as CandidatePayloadResolutionReason)) {
    errors.push("Candidate payload resolution reason is unsupported.");
  }
  if (value.displayName !== undefined) {
    requireBoundedString(value.displayName, "candidateResolution.displayName", 255, errors);
    if (typeof value.displayName === "string" && looksLikePath(value.displayName)) {
      errors.push("Candidate payload resolution displayName must be display-only, not a path.");
    }
  }
  if (value.sizeBytes !== undefined && (typeof value.sizeBytes !== "number" || !Number.isInteger(value.sizeBytes) || value.sizeBytes < 0)) {
    errors.push("Candidate payload resolution sizeBytes must be a non-negative integer when present.");
  }
  if (value.modifiedAt !== undefined && (typeof value.modifiedAt !== "string" || !Number.isFinite(Date.parse(value.modifiedAt)))) {
    errors.push("Candidate payload resolution modifiedAt must be a valid timestamp when present.");
  }
  if (value.mimeFamily !== undefined && (typeof value.mimeFamily !== "string" || !MIME_FAMILIES.has(value.mimeFamily as FileCandidateMimeFamily))) {
    errors.push("Candidate payload resolution mimeFamily is unsupported.");
  }
  if (value.extension !== undefined && (typeof value.extension !== "string" || !/^[a-z0-9]{0,16}$/i.test(value.extension))) {
    errors.push("Candidate payload resolution extension must be bounded.");
  }
}

function requireExactFields(
  value: Record<string, unknown>,
  expectedFields: string[],
  label: string,
  errors: string[],
  optionalFields: string[] = [],
) {
  const actual = Object.keys(value);
  const allowed = new Set([...expectedFields, ...optionalFields]);
  if (expectedFields.some((field) => !actual.includes(field)) || actual.some((field) => !allowed.has(field))) {
    errors.push(`${label} contains missing or unsupported fields.`);
  }
}

function requireBoundedString(
  value: unknown,
  label: string,
  maxLength: number,
  errors: string[],
  optional = false,
) {
  if (optional && typeof value === "undefined") {
    return;
  }
  if (typeof value !== "string" || value.trim().length === 0 || value.length > maxLength) {
    errors.push(`Candidate payload request requires bounded ${label}.`);
  }
}

function requireDateString(value: unknown, label: string, errors: string[]) {
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value))) {
    errors.push(`Candidate payload execution result requires a valid ${label}.`);
  }
}

function validateDates(createdAt: unknown, expiresAt: unknown, now: Date, errors: string[]) {
  const createdAtMs = typeof createdAt === "string" ? Date.parse(createdAt) : Number.NaN;
  const expiresAtMs = typeof expiresAt === "string" ? Date.parse(expiresAt) : Number.NaN;
  if (!Number.isFinite(createdAtMs)) {
    errors.push("Candidate payload request requires a valid createdAt timestamp.");
  }
  if (!Number.isFinite(expiresAtMs)) {
    errors.push("Candidate payload request requires a valid expiresAt timestamp.");
  } else if (expiresAtMs <= now.getTime()) {
    errors.push("Candidate payload request is expired.");
  }
  if (Number.isFinite(createdAtMs) && Number.isFinite(expiresAtMs) && expiresAtMs <= createdAtMs) {
    errors.push("Candidate payload request expiresAt must be after createdAt.");
  }
}

function findUnsafeFieldPaths(value: unknown): string[] {
  return findForbiddenProviderFieldPaths(value);
}

function isAbsolutePathLike(value: string): boolean {
  return value.startsWith("/") || /^[A-Za-z]:[\\/]/.test(value);
}

function looksLikePath(value: string): boolean {
  return isAbsolutePathLike(value) || value.includes("/") || value.includes("\\");
}

function serializedByteLength(value: unknown): number {
  try {
    return new TextEncoder().encode(JSON.stringify(value)).byteLength;
  } catch {
    return Number.POSITIVE_INFINITY;
  }
}

function createPreviewIdentifier(prefix: string, now: Date): string {
  requestSequence += 1;
  const randomPart = globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${requestSequence}`;
  return `${prefix}-${randomPart}`;
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

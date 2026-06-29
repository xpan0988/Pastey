import { isRecord } from "./actionPlanValidator";
import {
  FILE_CANDIDATES_CAPABILITY,
  FILE_CANDIDATES_EXECUTOR_KIND,
  findForbiddenProviderFieldPaths,
} from "./capabilityRegistry";
import {
  validateFileCandidateAdvisoryInput,
  type FileCandidateAdvisoryInput,
} from "./fileCandidateAdvisory";
import {
  buildPendingAiActionCanonicalPayload,
  hashDeterministicString,
  hashPendingAiActionPayload,
  isPendingAiActionExpired,
  stableSerialize,
} from "./pendingAction";
import type { PendingAiAction } from "./types";

export const FILE_CANDIDATES_REQUEST_SCHEMA = "filesystem-find-file-candidates-request/v1";
export const FILE_CANDIDATES_EXECUTION_REQUEST_SCHEMA = "filesystem-find-file-candidates-execution-request/v1";
export const FILE_CANDIDATES_RESULT_SCHEMA = "filesystem-find-file-candidates-result/v1";
export const FILE_CANDIDATES_CONSENT_GRANT_SCHEMA = "filesystem-find-file-candidates-consent-grant/v1";

export type FileCandidateMatchReason =
  | "filename_exact_match"
  | "filename_case_insensitive_match"
  | "filename_substring_match";
export type FileCandidateConfidence = "high" | "medium" | "low";
export type FileCandidateMimeFamily = "document" | "image" | "archive" | "media" | "code" | "unknown";
export type FileCandidateExecutionStatus =
  | "completed"
  | "rejected"
  | "expired"
  | "already_consumed"
  | "failed";
export type FileCandidateErrorCode =
  | "missing_consent"
  | "consent_not_allowed_once"
  | "consent_expired"
  | "invalid_consent"
  | "consent_binding_mismatch"
  | "already_consumed"
  | "malformed_request"
  | "unsupported_route"
  | "invalid_scope"
  | "no_searchable_scopes"
  | "search_timeout"
  | "result_truncated"
  | "executor_unavailable"
  | "unsafe_request_rejected"
  | "internal_filesystem_error"
  | "policy_rejected";

export interface FileCandidateRequest {
  schemaVersion: typeof FILE_CANDIDATES_REQUEST_SCHEMA;
  requestId: string;
  nonce: string;
  createdAt: string;
  expiresAt: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: typeof FILE_CANDIDATES_CAPABILITY;
  executorKind: typeof FILE_CANDIDATES_EXECUTOR_KIND;
  input: FileCandidateAdvisoryInput;
  pendingPayloadHash: string;
  requestPayloadHash: string;
  transportStatus: "preview_only";
}

export interface FileCandidateExecutionRequest {
  schemaVersion: typeof FILE_CANDIDATES_EXECUTION_REQUEST_SCHEMA;
  executionId: string;
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: typeof FILE_CANDIDATES_CAPABILITY;
  executorKind: typeof FILE_CANDIDATES_EXECUTOR_KIND;
  input: FileCandidateAdvisoryInput;
  createdAt: string;
  expiresAt: string;
}

export interface FileCandidateMetadata {
  candidateId: string;
  displayName: string;
  redactedLocation: string;
  extension: string;
  mimeFamily: FileCandidateMimeFamily;
  sizeBytes: number;
  modifiedAt: string;
  matchReason: FileCandidateMatchReason;
  confidence: FileCandidateConfidence;
}

export interface FileCandidateExecutionResult {
  schemaVersion: typeof FILE_CANDIDATES_RESULT_SCHEMA;
  capability: typeof FILE_CANDIDATES_CAPABILITY;
  executionId: string;
  requestId: string;
  consentId: string;
  status: FileCandidateExecutionStatus;
  queryEcho: {
    filenameHint: string;
    extensions: string[];
    searchMode: "filename_metadata_only";
  };
  candidates: FileCandidateMetadata[];
  omitted: {
    tooManyMatches: boolean;
    hiddenFilesSkipped: boolean;
    symlinksSkipped: boolean;
    scopesSkipped: string[];
  };
  durationMs: number;
  truncated: boolean;
  errorCode: FileCandidateErrorCode | null;
  createdAt: string;
}

export type FileCandidateRequestBuildResult =
  | { ok: true; request: FileCandidateRequest }
  | { ok: false; errors: string[] };

export type FileCandidateRequestValidationResult =
  | { valid: true; value: FileCandidateRequest; errors: [] }
  | { valid: false; errors: string[] };

export type FileCandidateExecutionRequestValidationResult =
  | { valid: true; value: FileCandidateExecutionRequest; errors: [] }
  | { valid: false; errors: string[] };

export type FileCandidateExecutionResultValidationResult =
  | { valid: true; value: FileCandidateExecutionResult; errors: [] }
  | { valid: false; errors: string[] };

interface BuildFileCandidateRequestOptions {
  now?: Date;
  ttlMs?: number;
  sourceDeviceRef?: string;
  nonce?: string;
  requestId?: string;
}

const DEFAULT_REQUEST_TTL_MS = 2 * 60 * 1_000;
const MAX_IDENTIFIER_LENGTH = 256;
const MAX_LOCATION_LENGTH = 512;
const MAX_DISPLAY_NAME_LENGTH = 255;
const MAX_DURATION_MS = 60_000;
const MAX_SCOPES_SKIPPED = 8;
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
  "input",
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
  "queryEcho",
  "candidates",
  "omitted",
  "durationMs",
  "truncated",
  "errorCode",
  "createdAt",
];
const QUERY_ECHO_FIELDS = ["filenameHint", "extensions", "searchMode"];
const OMITTED_FIELDS = ["tooManyMatches", "hiddenFilesSkipped", "symlinksSkipped", "scopesSkipped"];
const CANDIDATE_FIELDS = [
  "candidateId",
  "displayName",
  "redactedLocation",
  "extension",
  "mimeFamily",
  "sizeBytes",
  "modifiedAt",
  "matchReason",
  "confidence",
];
const MATCH_REASONS = new Set<FileCandidateMatchReason>([
  "filename_exact_match",
  "filename_case_insensitive_match",
  "filename_substring_match",
]);
const CONFIDENCE_VALUES = new Set<FileCandidateConfidence>(["high", "medium", "low"]);
const MIME_FAMILIES = new Set<FileCandidateMimeFamily>(["document", "image", "archive", "media", "code", "unknown"]);
const STATUSES = new Set<FileCandidateExecutionStatus>(["completed", "rejected", "expired", "already_consumed", "failed"]);
const ERROR_CODES = new Set<FileCandidateErrorCode>([
  "missing_consent",
  "consent_not_allowed_once",
  "consent_expired",
  "invalid_consent",
  "consent_binding_mismatch",
  "already_consumed",
  "malformed_request",
  "unsupported_route",
  "invalid_scope",
  "no_searchable_scopes",
  "search_timeout",
  "result_truncated",
  "executor_unavailable",
  "unsafe_request_rejected",
  "internal_filesystem_error",
  "policy_rejected",
]);
let requestSequence = 0;

export function buildFileCandidateRequestFromPendingAction(
  pending: PendingAiAction,
  options: BuildFileCandidateRequestOptions = {},
): FileCandidateRequestBuildResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_REQUEST_TTL_MS;

  if (pending.status !== "confirmed_local_only") {
    errors.push("File candidate request preview requires a confirmed_local_only pending action.");
  }
  if (pending.policyResult.status !== "accepted" || !pending.policyResult.requiresUserConfirmation) {
    errors.push("File candidate request preview requires the accepted confirmation-bound PolicyGate result.");
  }
  if (!Number.isFinite(now.getTime()) || !Number.isFinite(ttlMs) || ttlMs <= 0) {
    errors.push("File candidate request preview requires a valid time and positive finite TTL.");
  }
  if (isPendingAiActionExpired(pending, now)) {
    errors.push("File candidate request preview cannot be built from an expired pending action.");
  }
  if (pending.actionPlan.kind !== "request_peer_file_candidates") {
    errors.push("File candidate request preview requires request_peer_file_candidates.");
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

  const inputValidation = validateFileCandidateAdvisoryInput(pending.actionPlan.proposedInput);
  if (!inputValidation.valid) {
    errors.push(...inputValidation.errors);
  }
  if (errors.length > 0 || !rebuiltPendingPayload || !inputValidation.valid) {
    return { ok: false, errors: unique(errors) };
  }

  const createdAt = now.toISOString();
  const expiresAtMs = Math.min(now.getTime() + ttlMs, new Date(pending.expiresAt).getTime());
  const requestId = options.requestId ?? createPreviewIdentifier("file-candidates-request", now);
  const nonce = options.nonce ?? createPreviewIdentifier("file-candidates-nonce", now);
  const sourceDeviceRef = options.sourceDeviceRef ?? "local-device-preview";
  const requestWithoutHash: Omit<FileCandidateRequest, "requestPayloadHash"> = {
    schemaVersion: FILE_CANDIDATES_REQUEST_SCHEMA,
    requestId,
    nonce,
    createdAt,
    expiresAt: new Date(expiresAtMs).toISOString(),
    sourceDeviceRef,
    targetPeerRef: rebuiltPendingPayload.targetPeerRef,
    capability: FILE_CANDIDATES_CAPABILITY,
    executorKind: FILE_CANDIDATES_EXECUTOR_KIND,
    input: inputValidation.value,
    pendingPayloadHash: pending.payloadHash,
    transportStatus: "preview_only",
  };
  const request: FileCandidateRequest = {
    ...requestWithoutHash,
    requestPayloadHash: hashFileCandidateRequestPayload(requestWithoutHash),
  };
  const validation = validateFileCandidateRequest(request, { now });
  return validation.valid
    ? { ok: true, request: validation.value }
    : { ok: false, errors: validation.errors };
}

export function hashFileCandidateRequestPayload(
  request: Omit<FileCandidateRequest, "requestPayloadHash">,
): string {
  return hashDeterministicString(stableSerialize(request));
}

export function validateFileCandidateRequest(
  value: unknown,
  options: { now?: Date } = {},
): FileCandidateRequestValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["File candidate request must be an object."] };
  }
  requireExactFields(value, REQUEST_FIELDS, "File candidate request", errors);
  for (const path of findUnsafeFieldPaths(value)) {
    errors.push(`Unsafe field is not allowed in file candidate request: ${path}.`);
  }
  validateCommonRequestFields(value, FILE_CANDIDATES_REQUEST_SCHEMA, options.now ?? new Date(), errors);
  requireBoundedString(value.nonce, "nonce", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.pendingPayloadHash, "pendingPayloadHash", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.requestPayloadHash, "requestPayloadHash", MAX_IDENTIFIER_LENGTH, errors);
  if (value.transportStatus !== "preview_only") {
    errors.push("File candidate request transportStatus must be preview_only.");
  }
  const inputValidation = validateFileCandidateAdvisoryInput(value.input);
  if (!inputValidation.valid) {
    errors.push(...inputValidation.errors);
  } else if (inputValidation.value.targetPeerRef !== value.targetPeerRef) {
    errors.push("File candidate request targetPeerRef must match input targetPeerRef.");
  }
  if (errors.length === 0) {
    const { requestPayloadHash, ...requestWithoutHash } = value;
    const expectedHash = hashFileCandidateRequestPayload(requestWithoutHash as Omit<FileCandidateRequest, "requestPayloadHash">);
    if (requestPayloadHash !== expectedHash) {
      errors.push("File candidate request payload hash does not match the canonical preview payload.");
    }
  }
  return errors.length === 0
    ? { valid: true, value: value as unknown as FileCandidateRequest, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function validateFileCandidateExecutionRequest(
  value: unknown,
  options: { now?: Date } = {},
): FileCandidateExecutionRequestValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["File candidate execution request must be an object."] };
  }
  requireExactFields(value, EXECUTION_REQUEST_FIELDS, "File candidate execution request", errors);
  for (const path of findUnsafeFieldPaths(value)) {
    errors.push(`Unsafe field is not allowed in file candidate execution request: ${path}.`);
  }
  validateCommonRequestFields(value, FILE_CANDIDATES_EXECUTION_REQUEST_SCHEMA, options.now ?? new Date(), errors);
  for (const field of ["executionId", "consentId", "sourcePreviewEventId", "envelopeId", "requestId", "requestPayloadHash"]) {
    requireBoundedString(value[field], field, MAX_IDENTIFIER_LENGTH, errors);
  }
  const inputValidation = validateFileCandidateAdvisoryInput(value.input);
  if (!inputValidation.valid) {
    errors.push(...inputValidation.errors);
  } else if (inputValidation.value.targetPeerRef !== value.targetPeerRef) {
    errors.push("File candidate execution request targetPeerRef must match input targetPeerRef.");
  }
  return errors.length === 0
    ? { valid: true, value: value as unknown as FileCandidateExecutionRequest, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function validateFileCandidateExecutionResult(
  value: unknown,
): FileCandidateExecutionResultValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["File candidate execution result must be an object."] };
  }
  requireExactFields(value, RESULT_FIELDS, "File candidate execution result", errors);
  if (value.schemaVersion !== FILE_CANDIDATES_RESULT_SCHEMA) {
    errors.push(`File candidate execution result schemaVersion must be ${FILE_CANDIDATES_RESULT_SCHEMA}.`);
  }
  if (value.capability !== FILE_CANDIDATES_CAPABILITY) {
    errors.push(`File candidate execution result capability must be exactly ${FILE_CANDIDATES_CAPABILITY}.`);
  }
  for (const field of ["executionId", "requestId", "consentId"]) {
    requireBoundedString(value[field], field, MAX_IDENTIFIER_LENGTH, errors);
  }
  if (typeof value.status !== "string" || !STATUSES.has(value.status as FileCandidateExecutionStatus)) {
    errors.push("File candidate execution result contains an unsupported status.");
  }
  validateQueryEcho(value.queryEcho, errors);
  validateCandidates(value.candidates, errors);
  validateOmitted(value.omitted, errors);
  validateNonNegativeInteger(value.durationMs, "durationMs", errors, MAX_DURATION_MS);
  if (typeof value.truncated !== "boolean") {
    errors.push("File candidate execution result requires boolean truncated.");
  }
  if (value.status === "completed") {
    if (value.errorCode !== null) {
      errors.push("Completed file candidate result must have null errorCode.");
    }
  } else if (typeof value.errorCode !== "string" || !ERROR_CODES.has(value.errorCode as FileCandidateErrorCode)) {
    errors.push("Failed file candidate result requires a bounded errorCode.");
  }
  requireDateString(value.createdAt, "createdAt", errors);
  if (serializedByteLength(value) > 16 * 1024) {
    errors.push("File candidate execution result exceeds 16384 bytes.");
  }
  return errors.length === 0
    ? { valid: true, value: value as unknown as FileCandidateExecutionResult, errors: [] }
    : { valid: false, errors: unique(errors) };
}

function validateCommonRequestFields(
  value: Record<string, unknown>,
  schemaVersion: string,
  now: Date,
  errors: string[],
) {
  if (value.schemaVersion !== schemaVersion) {
    errors.push(`File candidate request schemaVersion must be ${schemaVersion}.`);
  }
  if (value.capability !== FILE_CANDIDATES_CAPABILITY) {
    errors.push(`File candidate request capability must be exactly ${FILE_CANDIDATES_CAPABILITY}.`);
  }
  if (value.executorKind !== FILE_CANDIDATES_EXECUTOR_KIND) {
    errors.push(`File candidate request executorKind must be exactly ${FILE_CANDIDATES_EXECUTOR_KIND}.`);
  }
  requireBoundedString(value.roomRef, "roomRef", MAX_IDENTIFIER_LENGTH, errors, true);
  requireBoundedString(value.sourceDeviceRef, "sourceDeviceRef", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.targetPeerRef, "targetPeerRef", MAX_IDENTIFIER_LENGTH, errors);
  validateDates(value.createdAt, value.expiresAt, now, errors);
}

function validateQueryEcho(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("File candidate execution result requires queryEcho.");
    return;
  }
  requireExactFields(value, QUERY_ECHO_FIELDS, "File candidate queryEcho", errors);
  requireBoundedString(value.filenameHint, "queryEcho.filenameHint", 128, errors);
  if (!Array.isArray(value.extensions) || value.extensions.some((entry) => typeof entry !== "string" || !/^[a-z0-9]{0,16}$/i.test(entry))) {
    errors.push("File candidate queryEcho extensions must be bounded extension labels.");
  }
  if (value.searchMode !== "filename_metadata_only") {
    errors.push("File candidate queryEcho searchMode must be filename_metadata_only.");
  }
}

function validateCandidates(value: unknown, errors: string[]) {
  if (!Array.isArray(value)) {
    errors.push("File candidate execution result requires candidates array.");
    return;
  }
  if (value.length > 20) {
    errors.push("File candidate execution result candidates exceed maxCandidates.");
  }
  value.forEach((candidate, index) => validateCandidate(candidate, index, errors));
}

function validateCandidate(value: unknown, index: number, errors: string[]) {
  if (!isRecord(value)) {
    errors.push(`File candidate ${index} must be an object.`);
    return;
  }
  requireExactFields(value, CANDIDATE_FIELDS, `File candidate ${index}`, errors);
  requireBoundedString(value.candidateId, `candidates[${index}].candidateId`, MAX_IDENTIFIER_LENGTH, errors);
  if (typeof value.candidateId === "string" && looksLikePath(value.candidateId)) {
    errors.push(`File candidate ${index} candidateId must be opaque and not path-like.`);
  }
  requireBoundedString(value.displayName, `candidates[${index}].displayName`, MAX_DISPLAY_NAME_LENGTH, errors);
  requireBoundedString(value.redactedLocation, `candidates[${index}].redactedLocation`, MAX_LOCATION_LENGTH, errors);
  if (typeof value.redactedLocation === "string" && isAbsolutePathLike(value.redactedLocation)) {
    errors.push(`File candidate ${index} redactedLocation must not be an absolute path.`);
  }
  if (typeof value.extension !== "string" || !/^[a-z0-9]{0,16}$/i.test(value.extension)) {
    errors.push(`File candidate ${index} extension must be bounded.`);
  }
  if (typeof value.mimeFamily !== "string" || !MIME_FAMILIES.has(value.mimeFamily as FileCandidateMimeFamily)) {
    errors.push(`File candidate ${index} mimeFamily is unsupported.`);
  }
  validateNonNegativeInteger(value.sizeBytes, `candidates[${index}].sizeBytes`, errors, Number.MAX_SAFE_INTEGER);
  requireDateString(value.modifiedAt, `candidates[${index}].modifiedAt`, errors);
  if (typeof value.matchReason !== "string" || !MATCH_REASONS.has(value.matchReason as FileCandidateMatchReason)) {
    errors.push(`File candidate ${index} matchReason is unsupported.`);
  }
  if (typeof value.confidence !== "string" || !CONFIDENCE_VALUES.has(value.confidence as FileCandidateConfidence)) {
    errors.push(`File candidate ${index} confidence is unsupported.`);
  }
}

function validateOmitted(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("File candidate execution result requires omitted.");
    return;
  }
  requireExactFields(value, OMITTED_FIELDS, "File candidate omitted", errors);
  for (const field of ["tooManyMatches", "hiddenFilesSkipped", "symlinksSkipped"]) {
    if (typeof value[field] !== "boolean") {
      errors.push(`File candidate omitted requires boolean ${field}.`);
    }
  }
  if (!Array.isArray(value.scopesSkipped) || value.scopesSkipped.length > MAX_SCOPES_SKIPPED) {
    errors.push("File candidate omitted scopesSkipped must be bounded.");
  } else if (value.scopesSkipped.some((entry) => typeof entry !== "string" || entry.length === 0 || entry.length > 64)) {
    errors.push("File candidate omitted scopesSkipped entries must be bounded.");
  }
}

function validateDates(createdAt: unknown, expiresAt: unknown, now: Date, errors: string[]) {
  const createdAtMs = typeof createdAt === "string" ? Date.parse(createdAt) : Number.NaN;
  const expiresAtMs = typeof expiresAt === "string" ? Date.parse(expiresAt) : Number.NaN;
  if (!Number.isFinite(createdAtMs)) {
    errors.push("File candidate request requires a valid createdAt timestamp.");
  }
  if (!Number.isFinite(expiresAtMs)) {
    errors.push("File candidate request requires a valid expiresAt timestamp.");
  } else if (expiresAtMs <= now.getTime()) {
    errors.push("File candidate request is expired.");
  }
  if (Number.isFinite(createdAtMs) && Number.isFinite(expiresAtMs) && expiresAtMs <= createdAtMs) {
    errors.push("File candidate request expiresAt must be after createdAt.");
  }
}

function requireExactFields(value: Record<string, unknown>, expectedFields: string[], label: string, errors: string[]) {
  const actual = Object.keys(value).sort();
  const expected = [...expectedFields].sort();
  if (actual.length !== expected.length || actual.some((field, index) => field !== expected[index])) {
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
    errors.push(`File candidate request requires bounded ${label}.`);
  }
}

function requireDateString(value: unknown, label: string, errors: string[]) {
  if (typeof value !== "string" || !Number.isFinite(Date.parse(value))) {
    errors.push(`File candidate execution result requires a valid ${label}.`);
  }
}

function validateNonNegativeInteger(value: unknown, label: string, errors: string[], max: number) {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || value > max) {
    errors.push(`File candidate execution result requires bounded non-negative integer ${label}.`);
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

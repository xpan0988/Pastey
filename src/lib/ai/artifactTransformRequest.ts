import { isRecord } from "./actionPlanValidator";
import { hashDeterministicString, stableSerialize } from "./pendingAction";

import { ARTIFACT_TRANSFORM_CAPABILITY } from "./capabilityRegistry";
export { ARTIFACT_TRANSFORM_CAPABILITY } from "./capabilityRegistry";
export const ARTIFACT_TRANSFORM_REQUEST_SCHEMA = "artifact-transform-selected-request-v1" as const;
export const ARTIFACT_TRANSFORM_EXECUTION_REQUEST_SCHEMA = "artifact-transform-selected-execution-request-v1" as const;
export const ARTIFACT_TRANSFORM_RESULT_SCHEMA = "artifact-transform-selected-result-v1" as const;
export const ARTIFACT_TRANSFORM_CONSENT_GRANT_SCHEMA = "artifact-transform-selected-consent-grant-v1" as const;

export type ArtifactTransformResultContract = "typed_transform_result";
export type ArtifactTransformStatus = "completed" | "failed" | "timed_out" | "rejected" | "expired" | "already_consumed";
export type ArtifactTransformErrorCode =
  | "sandbox_unavailable" | "malformed_request" | "missing_consent" | "consent_not_allowed_once"
  | "consent_expired" | "invalid_consent" | "consent_binding_mismatch" | "already_consumed"
  | "candidate_not_found" | "candidate_expired" | "candidate_changed" | "candidate_claimed"
  | "policy_rejected" | "executor_failed" | "invalid_executor_result" | "timed_out";

export interface ArtifactTransformRequestInput {
  sourceCapability: "filesystem.find_file_candidates";
  sourceRequestId: string;
  candidateId: string;
  candidateKind: "filesystem_file";
  resultContract: ArtifactTransformResultContract;
}

export interface ArtifactTransformRequest extends ArtifactTransformRequestInput {
  schemaVersion: typeof ARTIFACT_TRANSFORM_REQUEST_SCHEMA;
  requestId: string;
  nonce: string;
  createdAt: string;
  expiresAt: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: typeof ARTIFACT_TRANSFORM_CAPABILITY;
  requestPayloadHash: string;
  transportStatus: "preview_only";
}

export interface ArtifactTransformExecutionRequest extends ArtifactTransformRequestInput {
  schemaVersion: typeof ARTIFACT_TRANSFORM_EXECUTION_REQUEST_SCHEMA;
  executionId: string;
  consentId: string;
  sourcePreviewEventId: string;
  envelopeId: string;
  requestId: string;
  requestPayloadHash: string;
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: typeof ARTIFACT_TRANSFORM_CAPABILITY;
  createdAt: string;
  expiresAt: string;
}

export interface TypedTransformResult {
  kind: "typed_transform_result";
  output: {
    kind: "process_output";
    stdout: string;
    stderr: string;
    exitCode: number;
    durationMs: number;
    timedOut: boolean;
    stdoutTruncated: boolean;
    stderrTruncated: boolean;
  };
}

export interface ArtifactTransformExecutionResult {
  schemaVersion: typeof ARTIFACT_TRANSFORM_RESULT_SCHEMA;
  capability: typeof ARTIFACT_TRANSFORM_CAPABILITY;
  executionId: string;
  requestId: string;
  consentId: string;
  status: ArtifactTransformStatus;
  result?: TypedTransformResult;
  errorCode?: ArtifactTransformErrorCode;
  createdAt: string;
}

const MAX_ID = 256;
const MAX_OUTPUT_BYTES = 16 * 1024;
const MAX_DURATION_MS = 60_000;
const REQUEST_FIELDS = ["schemaVersion", "requestId", "nonce", "createdAt", "expiresAt", "sourceDeviceRef", "targetPeerRef", "capability", "sourceCapability", "sourceRequestId", "candidateId", "candidateKind", "resultContract", "requestPayloadHash", "transportStatus"];
const EXECUTION_FIELDS = ["schemaVersion", "executionId", "consentId", "sourcePreviewEventId", "envelopeId", "requestId", "requestPayloadHash", "roomRef", "sourceDeviceRef", "targetPeerRef", "capability", "sourceCapability", "sourceRequestId", "candidateId", "candidateKind", "resultContract", "createdAt", "expiresAt"];
const RESULT_FIELDS = ["schemaVersion", "capability", "executionId", "requestId", "consentId", "status", "createdAt"];
const ERROR_CODES = new Set<ArtifactTransformErrorCode>(["sandbox_unavailable", "malformed_request", "missing_consent", "consent_not_allowed_once", "consent_expired", "invalid_consent", "consent_binding_mismatch", "already_consumed", "candidate_not_found", "candidate_expired", "candidate_changed", "candidate_claimed", "policy_rejected", "executor_failed", "invalid_executor_result", "timed_out"]);
let sequence = 0;

export function buildArtifactTransformRequest(input: ArtifactTransformRequestInput & { targetPeerRef: string; sourceDeviceRef: string; now?: Date; ttlMs?: number; requestId?: string; nonce?: string }): { ok: true; request: ArtifactTransformRequest } | { ok: false; errors: string[] } {
  const now = input.now ?? new Date();
  const ttlMs = input.ttlMs ?? 120_000;
  const requestWithoutHash: Omit<ArtifactTransformRequest, "requestPayloadHash"> = {
    schemaVersion: ARTIFACT_TRANSFORM_REQUEST_SCHEMA,
    requestId: input.requestId ?? identifier("artifact-transform-request", now),
    nonce: input.nonce ?? identifier("artifact-transform-nonce", now),
    createdAt: now.toISOString(),
    expiresAt: new Date(now.getTime() + ttlMs).toISOString(),
    sourceDeviceRef: input.sourceDeviceRef,
    targetPeerRef: input.targetPeerRef,
    capability: ARTIFACT_TRANSFORM_CAPABILITY,
    sourceCapability: input.sourceCapability,
    sourceRequestId: input.sourceRequestId,
    candidateId: input.candidateId,
    candidateKind: input.candidateKind,
    resultContract: input.resultContract,
    transportStatus: "preview_only" as const,
  };
  const request: ArtifactTransformRequest = { ...requestWithoutHash, requestPayloadHash: hashArtifactTransformRequestPayload(requestWithoutHash) };
  const validation = validateArtifactTransformRequest(request, { now });
  return validation.valid ? { ok: true, request: validation.value } : { ok: false, errors: validation.errors };
}

export function hashArtifactTransformRequestPayload(request: Omit<ArtifactTransformRequest, "requestPayloadHash">): string {
  return hashDeterministicString(stableSerialize(request));
}

export function validateArtifactTransformRequestInput(value: unknown): { valid: true; value: ArtifactTransformRequestInput; errors: [] } | { valid: false; errors: string[] } {
  const errors: string[] = [];
  if (!isRecord(value)) return { valid: false, errors: ["Artifact Transform input must be an object."] };
  exact(value, ["sourceCapability", "sourceRequestId", "candidateId", "candidateKind", "resultContract"], errors, "Artifact Transform input");
  validateInput(value, errors);
  return errors.length ? { valid: false, errors: unique(errors) } : { valid: true, value: value as unknown as ArtifactTransformRequestInput, errors: [] };
}

export function validateArtifactTransformRequest(value: unknown, options: { now?: Date } = {}): { valid: true; value: ArtifactTransformRequest; errors: [] } | { valid: false; errors: string[] } {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  if (!isRecord(value)) return { valid: false, errors: ["Artifact Transform request must be an object."] };
  exact(value, REQUEST_FIELDS, errors, "Artifact Transform request");
  if (value.schemaVersion !== ARTIFACT_TRANSFORM_REQUEST_SCHEMA || value.capability !== ARTIFACT_TRANSFORM_CAPABILITY || value.transportStatus !== "preview_only") errors.push("Artifact Transform request has an invalid fixed contract.");
  for (const field of ["requestId", "nonce", "sourceDeviceRef", "targetPeerRef", "requestPayloadHash"]) bounded(value[field], field, MAX_ID, errors);
  validateInput(value, errors); dates(value.createdAt, value.expiresAt, now, errors);
  if (!errors.length) { const { requestPayloadHash, ...raw } = value; if (requestPayloadHash !== hashArtifactTransformRequestPayload(raw as Omit<ArtifactTransformRequest, "requestPayloadHash">)) errors.push("Artifact Transform request payload hash does not match."); }
  return errors.length ? { valid: false, errors: unique(errors) } : { valid: true, value: value as unknown as ArtifactTransformRequest, errors: [] };
}

export function validateArtifactTransformExecutionRequest(value: unknown, options: { now?: Date } = {}): { valid: true; value: ArtifactTransformExecutionRequest; errors: [] } | { valid: false; errors: string[] } {
  const errors: string[] = []; const now = options.now ?? new Date();
  if (!isRecord(value)) return { valid: false, errors: ["Artifact Transform execution request must be an object."] };
  exact(value, EXECUTION_FIELDS, errors, "Artifact Transform execution request");
  if (value.schemaVersion !== ARTIFACT_TRANSFORM_EXECUTION_REQUEST_SCHEMA || value.capability !== ARTIFACT_TRANSFORM_CAPABILITY) errors.push("Artifact Transform execution request has an invalid fixed contract.");
  for (const field of ["executionId", "consentId", "sourcePreviewEventId", "envelopeId", "requestId", "requestPayloadHash", "roomRef", "sourceDeviceRef", "targetPeerRef"]) bounded(value[field], field, MAX_ID, errors);
  validateInput(value, errors); dates(value.createdAt, value.expiresAt, now, errors);
  return errors.length ? { valid: false, errors: unique(errors) } : { valid: true, value: value as unknown as ArtifactTransformExecutionRequest, errors: [] };
}

export function validateArtifactTransformExecutionResult(value: unknown): { valid: true; value: ArtifactTransformExecutionResult; errors: [] } | { valid: false; errors: string[] } {
  const errors: string[] = [];
  if (!isRecord(value)) return { valid: false, errors: ["Artifact Transform result must be an object."] };
  exact(value, RESULT_FIELDS, errors, "Artifact Transform result", ["result", "errorCode"]);
  if (value.schemaVersion !== ARTIFACT_TRANSFORM_RESULT_SCHEMA || value.capability !== ARTIFACT_TRANSFORM_CAPABILITY) errors.push("Artifact Transform result has an invalid fixed contract.");
  for (const field of ["executionId", "requestId", "consentId"]) bounded(value[field], field, MAX_ID, errors);
  if (!isDate(value.createdAt)) errors.push("Artifact Transform result createdAt is invalid.");
  const status = value.status;
  if (!["completed", "failed", "timed_out", "rejected", "expired", "already_consumed"].includes(String(status))) errors.push("Artifact Transform result status is invalid.");
  if (status === "completed") { if (value.errorCode !== undefined) errors.push("Completed Artifact Transform result must omit errorCode."); validateTypedResult(value.result, errors); }
  else { if (value.result !== undefined) errors.push("Non-completed Artifact Transform result must omit result."); if (typeof value.errorCode !== "string" || !ERROR_CODES.has(value.errorCode as ArtifactTransformErrorCode)) errors.push("Non-completed Artifact Transform result requires a bounded errorCode."); }
  return errors.length ? { valid: false, errors: unique(errors) } : { valid: true, value: value as unknown as ArtifactTransformExecutionResult, errors: [] };
}

function validateInput(value: Record<string, unknown>, errors: string[]) { if (value.sourceCapability !== "filesystem.find_file_candidates" || value.candidateKind !== "filesystem_file" || value.resultContract !== "typed_transform_result") errors.push("Artifact Transform input has an invalid fixed source or result contract."); for (const field of ["sourceRequestId", "candidateId"]) bounded(value[field], field, MAX_ID, errors); if (typeof value.candidateId === "string" && /[\\/]|^file:/i.test(value.candidateId)) errors.push("Artifact Transform candidateId must be opaque."); }
function validateTypedResult(value: unknown, errors: string[]) { if (!isRecord(value)) { errors.push("Completed Artifact Transform result requires result."); return; } exact(value, ["kind", "output"], errors, "Typed Transform result"); if (value.kind !== "typed_transform_result" || !isRecord(value.output)) { errors.push("Typed Transform result is invalid."); return; } const output = value.output; exact(output, ["kind", "stdout", "stderr", "exitCode", "durationMs", "timedOut", "stdoutTruncated", "stderrTruncated"], errors, "Typed Transform output"); if (output.kind !== "process_output") errors.push("Typed Transform output kind is invalid."); for (const field of ["stdout", "stderr"]) if (typeof output[field] !== "string" || byteLength(output[field] as string) > MAX_OUTPUT_BYTES) errors.push(`Typed Transform ${field} is invalid.`); if (!Number.isInteger(output.exitCode) || (output.exitCode as number) < 0) errors.push("Typed Transform exitCode is invalid."); if (!Number.isInteger(output.durationMs) || (output.durationMs as number) < 0 || (output.durationMs as number) > MAX_DURATION_MS) errors.push("Typed Transform durationMs is invalid."); for (const field of ["timedOut", "stdoutTruncated", "stderrTruncated"]) if (typeof output[field] !== "boolean") errors.push(`Typed Transform ${field} is invalid.`); if (output.timedOut !== false) errors.push("Completed Artifact Transform result cannot be timed out."); }
function exact(value: Record<string, unknown>, fields: string[], errors: string[], label: string, optional: string[] = []) { const allowed = new Set([...fields, ...optional]); if (fields.some((field) => !(field in value)) || Object.keys(value).some((field) => !allowed.has(field))) errors.push(`${label} contains missing or unsupported fields.`); }
function bounded(value: unknown, label: string, max: number, errors: string[]) { if (typeof value !== "string" || !value.trim() || value.length > max) errors.push(`${label} must be a bounded string.`); }
function dates(created: unknown, expires: unknown, now: Date, errors: string[]) { const createdMs = typeof created === "string" ? Date.parse(created) : NaN; const expiresMs = typeof expires === "string" ? Date.parse(expires) : NaN; if (!Number.isFinite(createdMs) || !Number.isFinite(expiresMs) || expiresMs <= createdMs || expiresMs <= now.getTime()) errors.push("Artifact Transform request expiry is invalid."); }
function isDate(value: unknown) { return typeof value === "string" && Number.isFinite(Date.parse(value)); }
function byteLength(value: string) { return new TextEncoder().encode(value).byteLength; }
function identifier(prefix: string, now: Date) { sequence += 1; return `${prefix}-${globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${sequence}`}`; }
function unique(errors: string[]) { return [...new Set(errors)]; }

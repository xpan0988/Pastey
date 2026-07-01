import { isRecord } from "./actionPlanValidator";
import {
  CANDIDATE_PAYLOAD_CAPABILITY,
  FILE_CANDIDATES_CAPABILITY,
} from "./capabilityRegistry";
import type { FileCandidateMimeFamily } from "./fileCandidateRequest";

export type CandidatePayloadKind = "filesystem_file";

export interface CandidatePayloadAdvisoryInput {
  capability: typeof CANDIDATE_PAYLOAD_CAPABILITY;
  targetPeerRef: string;
  sourceCapability: typeof FILE_CANDIDATES_CAPABILITY;
  sourceRequestId: string;
  candidateId: string;
  candidateDisplayName: string;
  candidateKind: CandidatePayloadKind;
  redactedLocation?: string;
  sizeBytes?: number;
  modifiedAt?: string;
  mimeFamily?: FileCandidateMimeFamily;
  extension?: string;
}

export interface CandidatePayloadRequestInput {
  sourceCapability: typeof FILE_CANDIDATES_CAPABILITY;
  sourceRequestId: string;
  candidateId: string;
  candidateDisplayName: string;
  candidateKind: CandidatePayloadKind;
  redactedLocation?: string;
  sizeBytes?: number;
  modifiedAt?: string;
  mimeFamily?: FileCandidateMimeFamily;
  extension?: string;
}

export type CandidatePayloadAdvisoryValidationResult =
  | { valid: true; value: CandidatePayloadAdvisoryInput; errors: [] }
  | { valid: false; errors: string[] };

export type CandidatePayloadRequestInputValidationResult =
  | { valid: true; value: CandidatePayloadRequestInput; errors: [] }
  | { valid: false; errors: string[] };

const REQUIRED_ADVISORY_FIELDS = [
  "capability",
  "targetPeerRef",
  "sourceCapability",
  "sourceRequestId",
  "candidateId",
  "candidateDisplayName",
  "candidateKind",
];
const REQUIRED_REQUEST_INPUT_FIELDS = [
  "sourceCapability",
  "sourceRequestId",
  "candidateId",
  "candidateDisplayName",
  "candidateKind",
];
const OPTIONAL_CANDIDATE_FIELDS = [
  "redactedLocation",
  "sizeBytes",
  "modifiedAt",
  "mimeFamily",
  "extension",
];
const MIME_FAMILIES = new Set<FileCandidateMimeFamily>([
  "document",
  "image",
  "archive",
  "media",
  "code",
  "unknown",
]);
const MAX_IDENTIFIER_LENGTH = 256;
const MAX_DISPLAY_NAME_LENGTH = 255;
const MAX_LOCATION_LENGTH = 512;

export function validateCandidatePayloadAdvisoryInput(
  value: unknown,
): CandidatePayloadAdvisoryValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Candidate payload advisory input must be an object."] };
  }
  requireExactFields(
    value,
    REQUIRED_ADVISORY_FIELDS,
    OPTIONAL_CANDIDATE_FIELDS,
    "Candidate payload advisory input",
    errors,
  );
  if (value.capability !== CANDIDATE_PAYLOAD_CAPABILITY) {
    errors.push(`Candidate payload advisory capability must be exactly ${CANDIDATE_PAYLOAD_CAPABILITY}.`);
  }
  requireBoundedString(value.targetPeerRef, "targetPeerRef", MAX_IDENTIFIER_LENGTH, errors);
  validateCandidatePayloadCore(value, errors);

  return errors.length === 0
    ? { valid: true, value: value as unknown as CandidatePayloadAdvisoryInput, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function validateCandidatePayloadRequestInput(
  value: unknown,
): CandidatePayloadRequestInputValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Candidate payload request input must be an object."] };
  }
  requireExactFields(
    value,
    REQUIRED_REQUEST_INPUT_FIELDS,
    OPTIONAL_CANDIDATE_FIELDS,
    "Candidate payload request input",
    errors,
  );
  validateCandidatePayloadCore(value, errors);

  return errors.length === 0
    ? { valid: true, value: value as unknown as CandidatePayloadRequestInput, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function buildCandidatePayloadRequestInput(
  input: CandidatePayloadAdvisoryInput,
): CandidatePayloadRequestInput {
  return {
    sourceCapability: input.sourceCapability,
    sourceRequestId: input.sourceRequestId,
    candidateId: input.candidateId,
    candidateDisplayName: input.candidateDisplayName,
    candidateKind: input.candidateKind,
    ...(input.redactedLocation !== undefined ? { redactedLocation: input.redactedLocation } : {}),
    ...(input.sizeBytes !== undefined ? { sizeBytes: input.sizeBytes } : {}),
    ...(input.modifiedAt !== undefined ? { modifiedAt: input.modifiedAt } : {}),
    ...(input.mimeFamily !== undefined ? { mimeFamily: input.mimeFamily } : {}),
    ...(input.extension !== undefined ? { extension: input.extension } : {}),
  };
}

export function buildCandidatePayloadCanonicalConstraints(
  input: CandidatePayloadAdvisoryInput,
): Record<string, unknown> {
  return {
    sourceCapability: input.sourceCapability,
    candidateKind: input.candidateKind,
    metadataOnly: true,
    noPathAuthority: true,
    noAutoTransfer: true,
    selectedPeerOnly: true,
  };
}

function validateCandidatePayloadCore(value: Record<string, unknown>, errors: string[]) {
  if (value.sourceCapability !== FILE_CANDIDATES_CAPABILITY) {
    errors.push(`Candidate payload sourceCapability must be exactly ${FILE_CANDIDATES_CAPABILITY}.`);
  }
  requireBoundedString(value.sourceRequestId, "sourceRequestId", MAX_IDENTIFIER_LENGTH, errors);
  requireBoundedString(value.candidateId, "candidateId", MAX_IDENTIFIER_LENGTH, errors);
  if (typeof value.candidateId === "string" && looksLikePath(value.candidateId)) {
    errors.push("Candidate payload candidateId must be opaque and not path-like.");
  }
  requireBoundedString(value.candidateDisplayName, "candidateDisplayName", MAX_DISPLAY_NAME_LENGTH, errors);
  if (typeof value.candidateDisplayName === "string" && looksLikePath(value.candidateDisplayName)) {
    errors.push("Candidate payload candidateDisplayName must be display-only, not a path.");
  }
  if (value.candidateKind !== "filesystem_file") {
    errors.push("Candidate payload candidateKind must be filesystem_file.");
  }
  if (value.redactedLocation !== undefined) {
    requireBoundedString(value.redactedLocation, "redactedLocation", MAX_LOCATION_LENGTH, errors);
    if (typeof value.redactedLocation === "string" && isAbsolutePathLike(value.redactedLocation)) {
      errors.push("Candidate payload redactedLocation must not be an absolute path.");
    }
  }
  if (value.sizeBytes !== undefined) {
    validateNonNegativeInteger(value.sizeBytes, "sizeBytes", errors);
  }
  if (value.modifiedAt !== undefined && (typeof value.modifiedAt !== "string" || !Number.isFinite(Date.parse(value.modifiedAt)))) {
    errors.push("Candidate payload modifiedAt must be a valid timestamp when present.");
  }
  if (value.mimeFamily !== undefined && (typeof value.mimeFamily !== "string" || !MIME_FAMILIES.has(value.mimeFamily as FileCandidateMimeFamily))) {
    errors.push("Candidate payload mimeFamily is unsupported.");
  }
  if (value.extension !== undefined && (typeof value.extension !== "string" || !/^[a-z0-9]{0,16}$/i.test(value.extension))) {
    errors.push("Candidate payload extension must be a bounded extension label.");
  }
}

function requireExactFields(
  value: Record<string, unknown>,
  requiredFields: string[],
  optionalFields: string[],
  label: string,
  errors: string[],
) {
  const allowed = new Set([...requiredFields, ...optionalFields]);
  if (requiredFields.some((field) => !(field in value)) || Object.keys(value).some((field) => !allowed.has(field))) {
    errors.push(`${label} contains missing or unsupported fields.`);
  }
}

function requireBoundedString(value: unknown, label: string, maxLength: number, errors: string[]) {
  if (typeof value !== "string" || value.trim().length === 0 || value.length > maxLength) {
    errors.push(`Candidate payload requires bounded ${label}.`);
  }
}

function validateNonNegativeInteger(value: unknown, label: string, errors: string[]) {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || value > Number.MAX_SAFE_INTEGER) {
    errors.push(`Candidate payload requires bounded non-negative integer ${label}.`);
  }
}

function isAbsolutePathLike(value: string): boolean {
  return value.startsWith("/") || /^[A-Za-z]:[\\/]/.test(value);
}

function looksLikePath(value: string): boolean {
  return isAbsolutePathLike(value) || value.includes("/") || value.includes("\\");
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

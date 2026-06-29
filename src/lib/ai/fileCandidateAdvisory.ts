import { isRecord } from "./actionPlanValidator";
import { FILE_CANDIDATES_CAPABILITY } from "./capabilityRegistry";

export type FileCandidateAllowedScope = "downloads" | "desktop" | "documents" | "pastey_shared";
export type FileCandidateSearchMode = "filename_metadata_only";

export interface FileCandidateAdvisoryInput {
  capability: typeof FILE_CANDIDATES_CAPABILITY;
  targetPeerRef: string;
  query: {
    rawUserRequest: string;
    filenameHint: string;
    extensions: string[];
    searchMode: FileCandidateSearchMode;
  };
  scopePolicy: {
    allowedScopes: FileCandidateAllowedScope[];
    allowFullDisk: false;
    includeFileContents: false;
    includeAbsolutePaths: false;
    includeHiddenFiles: false;
  };
  limits: {
    maxCandidates: number;
    maxSearchMs: number;
    maxDepth: number;
  };
  safety: {
    returnRedactedPaths: true;
    noAutoTransfer: true;
    requireReceiverConsent: true;
    selectedPeerOnly: true;
  };
}

export type FileCandidateAdvisoryValidationResult =
  | { valid: true; value: FileCandidateAdvisoryInput; errors: [] }
  | { valid: false; errors: string[] };

const INPUT_FIELDS = ["capability", "targetPeerRef", "query", "scopePolicy", "limits", "safety"];
const QUERY_FIELDS = ["rawUserRequest", "filenameHint", "extensions", "searchMode"];
const SCOPE_POLICY_FIELDS = [
  "allowedScopes",
  "allowFullDisk",
  "includeFileContents",
  "includeAbsolutePaths",
  "includeHiddenFiles",
];
const LIMIT_FIELDS = ["maxCandidates", "maxSearchMs", "maxDepth"];
const SAFETY_FIELDS = ["returnRedactedPaths", "noAutoTransfer", "requireReceiverConsent", "selectedPeerOnly"];
const ALLOWED_SCOPES = new Set<FileCandidateAllowedScope>(["downloads", "desktop", "documents", "pastey_shared"]);

export function validateFileCandidateAdvisoryInput(value: unknown): FileCandidateAdvisoryValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["File candidate advisory input must be an object."] };
  }

  requireExactFields(value, INPUT_FIELDS, "File candidate advisory input", errors);
  if (value.capability !== FILE_CANDIDATES_CAPABILITY) {
    errors.push(`File candidate advisory capability must be exactly ${FILE_CANDIDATES_CAPABILITY}.`);
  }
  requireNonEmptyString(value.targetPeerRef, "targetPeerRef", errors);
  validateQuery(value.query, errors);
  validateScopePolicy(value.scopePolicy, errors);
  validateLimits(value.limits, errors);
  validateSafety(value.safety, errors);

  return errors.length === 0
    ? { valid: true, value: value as unknown as FileCandidateAdvisoryInput, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function buildFileCandidateCanonicalConstraints(input: FileCandidateAdvisoryInput): Record<string, unknown> {
  return {
    searchMode: input.query.searchMode,
    allowedScopes: [...input.scopePolicy.allowedScopes],
    includeFileContents: input.scopePolicy.includeFileContents,
    includeAbsolutePaths: input.scopePolicy.includeAbsolutePaths,
    noAutoTransfer: input.safety.noAutoTransfer,
    selectedPeerOnly: input.safety.selectedPeerOnly,
    maxCandidates: input.limits.maxCandidates,
    maxSearchMs: input.limits.maxSearchMs,
    maxDepth: input.limits.maxDepth,
  };
}

function validateQuery(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("File candidate advisory requires a query object.");
    return;
  }
  requireExactFields(value, QUERY_FIELDS, "File candidate query", errors);
  requireBoundedString(value.rawUserRequest, "rawUserRequest", 1, 512, errors);
  requireBoundedString(value.filenameHint, "filenameHint", 1, 128, errors);
  if (typeof value.filenameHint === "string" && !/[a-zA-Z0-9]/.test(value.filenameHint)) {
    errors.push("File candidate filenameHint must contain at least one alphanumeric character.");
  }
  if (!Array.isArray(value.extensions)) {
    errors.push("File candidate extensions must be an array.");
  } else if (value.extensions.length > 10) {
    errors.push("File candidate extensions must contain at most 10 entries.");
  } else {
    for (const extension of value.extensions) {
      if (typeof extension !== "string" || !/^[a-z0-9]{1,16}$/i.test(extension)) {
        errors.push("File candidate extensions must be simple extension labels without dots or paths.");
        break;
      }
    }
  }
  if (value.searchMode !== "filename_metadata_only") {
    errors.push("File candidate searchMode must be filename_metadata_only.");
  }
}

function validateScopePolicy(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("File candidate advisory requires a scopePolicy object.");
    return;
  }
  requireExactFields(value, SCOPE_POLICY_FIELDS, "File candidate scopePolicy", errors);
  if (!Array.isArray(value.allowedScopes) || value.allowedScopes.length === 0) {
    errors.push("File candidate allowedScopes must be a non-empty array.");
  } else {
    const seen = new Set<string>();
    for (const scope of value.allowedScopes) {
      if (typeof scope !== "string" || !ALLOWED_SCOPES.has(scope as FileCandidateAllowedScope)) {
        errors.push("File candidate allowedScopes contains an unsupported scope.");
        break;
      }
      if (seen.has(scope)) {
        errors.push("File candidate allowedScopes must not contain duplicates.");
        break;
      }
      seen.add(scope);
    }
  }
  if (value.allowFullDisk !== false) errors.push("File candidate advisory must set allowFullDisk false.");
  if (value.includeFileContents !== false) errors.push("File candidate advisory must set includeFileContents false.");
  if (value.includeAbsolutePaths !== false) errors.push("File candidate advisory must set includeAbsolutePaths false.");
  if (value.includeHiddenFiles !== false) errors.push("File candidate advisory must set includeHiddenFiles false.");
}

function validateLimits(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("File candidate advisory requires a limits object.");
    return;
  }
  requireExactFields(value, LIMIT_FIELDS, "File candidate limits", errors);
  requireIntegerInRange(value.maxCandidates, "maxCandidates", 1, 20, errors);
  requireIntegerInRange(value.maxSearchMs, "maxSearchMs", 500, 10_000, errors);
  requireIntegerInRange(value.maxDepth, "maxDepth", 1, 8, errors);
}

function validateSafety(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("File candidate advisory requires a safety object.");
    return;
  }
  requireExactFields(value, SAFETY_FIELDS, "File candidate safety", errors);
  if (value.returnRedactedPaths !== true) errors.push("File candidate advisory must set returnRedactedPaths true.");
  if (value.noAutoTransfer !== true) errors.push("File candidate advisory must set noAutoTransfer true.");
  if (value.requireReceiverConsent !== true) errors.push("File candidate advisory must set requireReceiverConsent true.");
  if (value.selectedPeerOnly !== true) errors.push("File candidate advisory must set selectedPeerOnly true.");
}

function requireExactFields(value: Record<string, unknown>, expectedFields: string[], label: string, errors: string[]) {
  const actual = Object.keys(value).sort();
  const expected = [...expectedFields].sort();
  if (actual.length !== expected.length || actual.some((field, index) => field !== expected[index])) {
    errors.push(`${label} contains missing or unsupported fields.`);
  }
}

function requireNonEmptyString(value: unknown, label: string, errors: string[]) {
  requireBoundedString(value, label, 1, 256, errors);
}

function requireBoundedString(value: unknown, label: string, min: number, max: number, errors: string[]) {
  if (typeof value !== "string" || value.trim().length < min || value.length > max) {
    errors.push(`File candidate advisory requires bounded ${label}.`);
  }
}

function requireIntegerInRange(value: unknown, label: string, min: number, max: number, errors: string[]) {
  if (typeof value !== "number" || !Number.isInteger(value) || value < min || value > max) {
    errors.push(`File candidate ${label} must be an integer from ${min} to ${max}.`);
  }
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

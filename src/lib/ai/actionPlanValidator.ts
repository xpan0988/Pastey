import type {
  AiActionKind,
  AiActionPlan,
  AiActionReference,
  AiConfidence,
  ValidationResult
} from "./types";
import {
  AGENT_BRIDGE_CAPABILITY_ACTION_KINDS,
  findForbiddenProviderFieldPaths
} from "./capabilityRegistry";

const ACTION_KINDS = new Set<AiActionKind>([
  "explain_status",
  "summarize_room_state",
  "summarize_diagnostics",
  "explain_transfer_failure",
  "suggest_retry",
  "suggest_benchmark",
  "suggest_transfer",
  "draft_text_message",
  "explain_microflowgroup_mode",
  ...AGENT_BRIDGE_CAPABILITY_ACTION_KINDS
]);

const CONFIDENCE_VALUES = new Set<AiConfidence>(["low", "medium", "high"]);
const REFERENCE_KINDS = new Set<AiActionReference["kind"]>(["room", "peer", "transfer", "diagnostic", "scheduler"]);
const TOP_LEVEL_FIELDS = new Set([
  "schemaVersion",
  "kind",
  "title",
  "explanation",
  "confidence",
  "requiresUserConfirmation",
  "references",
  "proposedInput"
]);
const REFERENCE_FIELDS = new Set(["kind", "ref"]);
const PROVIDER_FORBIDDEN_MANIFEST_FIELDS = new Set([
  "manifest",
  "templateKind",
  "providerActionKind",
  "executorKind",
  "routePolicy",
  "consentPolicy",
  "dataExposurePolicy",
  "auditRedactionPolicy",
  "schemaVersions",
  "autonomySupport",
  "approvalRequirements",
  "approvalPolicy",
  "approvalReviewer",
].map(normalizeFieldName));

export function validateAiActionPlan(value: unknown): ValidationResult<AiActionPlan> {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Action plan must be an object."] };
  }

  for (const key of Object.keys(value)) {
    if (!TOP_LEVEL_FIELDS.has(key)) {
      errors.push(`Unsupported top-level field: ${key}.`);
    }
  }

  if (value.schemaVersion !== "ai-action-plan-v1") {
    errors.push("schemaVersion must be ai-action-plan-v1.");
  }
  if (!isString(value.kind) || !ACTION_KINDS.has(value.kind as AiActionKind)) {
    errors.push("kind must be a known AI action kind.");
  }
  if (!isNonEmptyString(value.title)) {
    errors.push("title must be a non-empty string.");
  }
  if (!isNonEmptyString(value.explanation)) {
    errors.push("explanation must be a non-empty string.");
  }
  if (!isString(value.confidence) || !CONFIDENCE_VALUES.has(value.confidence as AiConfidence)) {
    errors.push("confidence must be low, medium, or high.");
  }
  if (typeof value.requiresUserConfirmation !== "boolean") {
    errors.push("requiresUserConfirmation must be a boolean.");
  }

  if (typeof value.references !== "undefined") {
    if (!Array.isArray(value.references)) {
      errors.push("references must be an array when present.");
    } else {
      value.references.forEach((reference, index) => validateReference(reference, index, errors));
    }
  }

  if (typeof value.proposedInput !== "undefined" && !isRecord(value.proposedInput)) {
    errors.push("proposedInput must be an object when present.");
  }

  for (const unsafePath of findUnsafeFieldPaths(value)) {
    errors.push(`Unsafe field is not allowed: ${unsafePath}.`);
  }
  for (const manifestPath of findProviderManifestFieldPaths(value)) {
    errors.push(`Provider output must not define capability manifest fields: ${manifestPath}.`);
  }

  if (errors.length > 0) {
    return { valid: false, errors: unique(errors) };
  }

  return { valid: true, value: value as unknown as AiActionPlan, errors: [] };
}

export function findUnsafeFieldPaths(value: unknown): string[] {
  return findForbiddenProviderFieldPaths(value);
}

export function findProviderManifestFieldPaths(value: unknown): string[] {
  const found: string[] = [];
  visitProviderFields(value, "$", found);
  return found;
}

function validateReference(value: unknown, index: number, errors: string[]) {
  if (!isRecord(value)) {
    errors.push(`references[${index}] must be an object.`);
    return;
  }
  for (const key of Object.keys(value)) {
    if (!REFERENCE_FIELDS.has(key)) {
      errors.push(`Unsupported references[${index}] field: ${key}.`);
    }
  }
  if (!isString(value.kind) || !REFERENCE_KINDS.has(value.kind as AiActionReference["kind"])) {
    errors.push(`references[${index}].kind is invalid.`);
  }
  if (!isNonEmptyString(value.ref)) {
    errors.push(`references[${index}].ref must be a non-empty string.`);
  }
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function isNonEmptyString(value: unknown): value is string {
  return isString(value) && value.trim().length > 0;
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

function visitProviderFields(value: unknown, path: string, found: string[]) {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => visitProviderFields(entry, `${path}[${index}]`, found));
    return;
  }
  if (!isRecord(value)) {
    return;
  }
  for (const [key, entry] of Object.entries(value)) {
    const entryPath = `${path}.${key}`;
    if (PROVIDER_FORBIDDEN_MANIFEST_FIELDS.has(normalizeFieldName(key))) {
      found.push(entryPath);
    }
    visitProviderFields(entry, entryPath, found);
  }
}

function normalizeFieldName(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, "").toLowerCase();
}

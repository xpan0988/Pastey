import { CloudOpenAICompatibleProvider, type FetchLike } from "./cloudOpenAICompatibleProvider";
import { buildMockAiContextSnapshot, CLOUD_STRICT_AI_CONTEXT_POLICY, MOCK_AI_CONTEXT_POLICY } from "./contextSnapshot";
import { findUnsafeFieldPaths, isRecord } from "./actionPlanValidator";
import { scanProviderOutputRisk } from "./providerRiskScanner";
import type { AiGenerateResult, CloudOpenAICompatibleProviderConfig } from "./types";

/** Product operations. Provider output describes intent only; the Host owns
 * object resolution, destinations, and execution authority. */
export type AskBridgePrimitive = "Search" | "Transform" | "Transfer";
export type AskBridgeNaturalV1Status = "supported" | "unsupported_future";
export type AskBridgeSafeScope = "downloads" | "desktop" | "documents" | "pastey_shared";

export interface AskBridgeNaturalV1SearchStep {
  primitive: "Search";
  filenameHint: string;
  extensions: string[];
  safeScopes: AskBridgeSafeScope[];
}

export interface AskBridgeNaturalV1TransformStep {
  primitive: "Transform";
  intent: string;
}

export interface AskBridgeNaturalV1TransferStep {
  primitive: "Transfer";
  destination: "requesting_device" | "selected_device" | "user_selected_location" | "leave_on_producing_device";
  object: "selected_file" | "search_result" | "transform_result";
}

export type AskBridgeNaturalV1Step =
  | AskBridgeNaturalV1SearchStep
  | AskBridgeNaturalV1TransformStep
  | AskBridgeNaturalV1TransferStep;

export interface AskBridgeNaturalV1Plan {
  schemaVersion: "ask-bridge-natural-v1";
  title: string;
  status: AskBridgeNaturalV1Status;
  requiresUserConfirmation: true;
  steps: AskBridgeNaturalV1Step[];
  unsupportedReason?: string;
}

export type AskBridgeNaturalV1ValidationResult =
  | { valid: true; value: AskBridgeNaturalV1Plan; errors: [] }
  | { valid: false; errors: string[] };

export interface AskBridgeProviderHealthCheckResult {
  ok: boolean;
  providerId: string;
  model: string;
  validationStatus: "accepted" | "rejected" | "skipped";
  message: string;
  errors: string[];
}

const TOP_LEVEL_FIELDS = new Set([
  "schemaVersion",
  "title",
  "status",
  "requiresUserConfirmation",
  "steps",
  "unsupportedReason",
]);
const STEP_COMMON_FIELDS = new Set(["primitive"]);
const SEARCH_FIELDS = new Set(["primitive", "filenameHint", "extensions", "safeScopes"]);
const TRANSFORM_FIELDS = new Set(["primitive", "intent"]);
const TRANSFER_FIELDS = new Set(["primitive", "destination", "object"]);
const SAFE_SCOPES = new Set<AskBridgeSafeScope>(["downloads", "desktop", "documents", "pastey_shared"]);
const FORBIDDEN_NATURAL_FIELD_NAMES = new Set([
  "command",
  "cmd",
  "shell",
  "script",
  "code",
  "path",
  "absolutePath",
  "filePath",
  "cwd",
  "env",
  "environment",
  "args",
  "arguments",
  "argv",
  "stdin",
  "workingDirectory",
  "runtime",
  "interpreter",
  "compiler",
  "proxy",
  "network",
  "networkTarget",
  "url",
  "contents",
  "fileContents",
  "selectedPeers",
  "targetPeerRefs",
  "broadcast",
  "autoTransfer",
  "autoSend",
  "transferQueueId",
  "handoffId",
  "sourceRequestId",
  "candidateId",
  "candidateKind",
  "resultContract",
  "stdout",
  "stderr",
  "exitCode",
  "durationMs",
  "timedOut",
].map(normalizeFieldName));
const KNOWN_EXTENSIONS = new Set([
  "pdf",
  "txt",
  "md",
  "doc",
  "docx",
  "csv",
  "tsv",
  "xls",
  "xlsx",
  "ppt",
  "pptx",
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "zip",
  "json",
]);

export function validateAskBridgeNaturalV1Plan(value: unknown): AskBridgeNaturalV1ValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["natural-v1 plan must be an object."] };
  }
  for (const key of Object.keys(value)) {
    if (!TOP_LEVEL_FIELDS.has(key)) errors.push(`Unsupported natural-v1 field: ${key}.`);
  }
  for (const unsafePath of findUnsafeFieldPaths(value)) {
    errors.push(`Unsafe provider field is not allowed in natural-v1 plan: ${unsafePath}.`);
  }
  for (const unsafePath of findForbiddenNaturalFieldPaths(value)) {
    errors.push(`Forbidden natural-v1 field is not allowed: ${unsafePath}.`);
  }
  const riskScan = scanProviderOutputRisk(value);
  for (const finding of riskScan.findings) {
    if (finding.severity === "fail_closed") {
      errors.push(`Provider risk scanner rejected natural-v1 output at ${finding.path}: ${finding.reason}.`);
    }
  }
  if (value.schemaVersion !== "ask-bridge-natural-v1") errors.push("natural-v1 schemaVersion must be ask-bridge-natural-v1.");
  if (!isBoundedString(value.title, 1, 120)) errors.push("natural-v1 title must be a bounded string.");
  if (value.status !== "supported" && value.status !== "unsupported_future") {
    errors.push("natural-v1 status must be supported or unsupported_future.");
  }
  if (value.requiresUserConfirmation !== true) {
    errors.push("natural-v1 plans require user confirmation.");
  }
  if (!Array.isArray(value.steps) || value.steps.length === 0 || value.steps.length > 3) {
    errors.push("natural-v1 steps must contain one to three primitives.");
  } else {
    value.steps.forEach((step, index) => validateStep(step, index, errors));
    validatePrimitiveOrder(value.steps, value.status, errors);
  }
  if (typeof value.unsupportedReason !== "undefined" && !isBoundedString(value.unsupportedReason, 1, 240)) {
    errors.push("natural-v1 unsupportedReason must be a bounded string when present.");
  }
  if (value.status === "unsupported_future" && typeof value.unsupportedReason === "undefined") {
    errors.push("unsupported natural-v1 plans must include unsupportedReason.");
  }
  return errors.length === 0
    ? { valid: true, value: value as unknown as AskBridgeNaturalV1Plan, errors: [] }
    : { valid: false, errors: [...new Set(errors)] };
}

export function buildDeterministicAskBridgeNaturalV1Plan(userRequest: string): AskBridgeNaturalV1Plan {
  const search = buildSearchStep(userRequest);
  const wantsTransform = /\b(convert|transform|summari[sz]e|extract|resize|translate|compress|redact)\b/i.test(userRequest);
  const wantsTransfer = wantsTransform || /\b(send|return|get|fetch|bring|copy|download|back|to me|here)\b/i.test(userRequest);
  const transformIntent = wantsTransform ? extractTransformIntent(userRequest) : null;
  const steps: AskBridgeNaturalV1Step[] = [search];
  if (wantsTransform) {
    steps.push({ primitive: "Transform", intent: transformIntent ?? "unsupported transform" });
  }
  if (wantsTransfer) {
    steps.push({
      primitive: "Transfer",
      destination: "requesting_device",
      object: wantsTransform ? "transform_result" : "search_result",
    });
  }
  return {
    schemaVersion: "ask-bridge-natural-v1",
    title: wantsTransform
      ? "Search, Transform, Transfer"
      : wantsTransfer
        ? "Search and Transfer"
        : "Search",
    status: wantsTransform && transformIntent === null ? "unsupported_future" : "supported",
    requiresUserConfirmation: true,
    steps,
    unsupportedReason: wantsTransform && transformIntent === null ? "No statically registered Transform transition supports this intent and input type." : undefined,
  };
}

export function buildMockAskBridgeNaturalV1Plan(): AskBridgeNaturalV1Plan {
  return buildDeterministicAskBridgeNaturalV1Plan("Find the report pdf on the selected peer and transfer it to me.");
}

/** True only for the Bridge Plan shapes the sender UI can submit today. */
export function isSupportedBridgePlanSubmission(plan: AskBridgeNaturalV1Plan): boolean {
  if (plan.status !== "supported") return false;
  const primitives = plan.steps.map((step) => step.primitive).join(",");
  if (primitives === "Search" || primitives === "Search,Transform") return true;
  const transfer = plan.steps[plan.steps.length - 1];
  return (primitives === "Search,Transfer" || primitives === "Search,Transform,Transfer")
    && transfer?.primitive === "Transfer"
    && (transfer.destination === "requesting_device" || transfer.destination === "selected_device");
}

export async function generateMockAskBridgeNaturalV1Plan(userRequest: string): Promise<AiGenerateResult> {
  return {
    requestId: `ask-bridge-natural-mock-${Date.now()}`,
    providerId: "pastey-mock-provider",
    model: "pastey-safe-natural-v1",
    rawText: "Deterministic natural-v1 advisory generated locally. No model or network call occurred.",
    parsedPlan: buildDeterministicAskBridgeNaturalV1Plan(userRequest),
    usage: {
      inputTokens: 0,
      outputTokens: 0,
    },
  };
}

export async function checkAskBridgeNaturalV1ProviderHealth(
  config: CloudOpenAICompatibleProviderConfig,
  options: {
    apiKey?: string;
    fetchImpl?: FetchLike;
  } = {},
): Promise<AskBridgeProviderHealthCheckResult> {
  if (!config.enabled || !config.model.trim() || !config.baseUrl.trim() || !options.apiKey?.trim()) {
    return {
      ok: false,
      providerId: config.providerId,
      model: config.model,
      validationStatus: "skipped",
      message: "Cloud provider health check skipped because API configuration is incomplete.",
      errors: [],
    };
  }
  const provider = new CloudOpenAICompatibleProvider(config, options);
  const generated = await provider.generate({
    requestId: `ask-bridge-natural-health-${Date.now()}`,
    providerId: config.providerId,
    context: buildMockAiContextSnapshot(),
    contextPolicy: CLOUD_STRICT_AI_CONTEXT_POLICY,
    allowedActionKinds: [],
    outputSchema: "ask-bridge-natural-v1",
    userRequest: "Health check only: produce a Search plan for a file named report.pdf. Do not execute anything.",
  });
  if (generated.error) {
    return {
      ok: false,
      providerId: config.providerId,
      model: config.model,
      validationStatus: "rejected",
      message: `${generated.error.code}: ${generated.error.message}`,
      errors: [generated.error.message],
    };
  }
  const validation = validateAskBridgeNaturalV1Plan(generated.parsedPlan);
  return {
    ok: validation.valid,
    providerId: config.providerId,
    model: config.model,
    validationStatus: validation.valid ? "accepted" : "rejected",
    message: validation.valid
      ? "Provider returned a valid advisory-only natural-v1 plan. No room-control events or capabilities were executed."
      : "Provider health check failed host validation. No action was taken.",
    errors: validation.valid ? [] : validation.errors,
  };
}

export const NATURAL_V1_MOCK_POLICY = MOCK_AI_CONTEXT_POLICY;

function validateStep(value: unknown, index: number, errors: string[]) {
  if (!isRecord(value)) {
    errors.push(`natural-v1 steps[${index}] must be an object.`);
    return;
  }
  if (!("primitive" in value)) {
    errors.push(`natural-v1 steps[${index}] must include primitive.`);
    return;
  }
  if (value.primitive === "Search") {
    requireExactFields(value, SEARCH_FIELDS, `steps[${index}]`, errors);
    if (!isBoundedString(value.filenameHint, 1, 128) || !/[a-zA-Z0-9]/.test(value.filenameHint)) {
      errors.push(`natural-v1 steps[${index}].filenameHint must be bounded filename metadata.`);
    }
    if (!Array.isArray(value.extensions) || value.extensions.length > 10) {
      errors.push(`natural-v1 steps[${index}].extensions must be an array with at most 10 entries.`);
    } else {
      for (const extension of value.extensions) {
        if (typeof extension !== "string" || !/^[a-z0-9]{1,16}$/i.test(extension)) {
          errors.push(`natural-v1 steps[${index}].extensions must contain simple extension labels.`);
          break;
        }
      }
    }
    if (!Array.isArray(value.safeScopes) || value.safeScopes.length === 0) {
      errors.push(`natural-v1 steps[${index}].safeScopes must be non-empty.`);
    } else {
      const seen = new Set<string>();
      for (const scope of value.safeScopes) {
        if (typeof scope !== "string" || !SAFE_SCOPES.has(scope as AskBridgeSafeScope) || seen.has(scope)) {
          errors.push(`natural-v1 steps[${index}].safeScopes contains an unsupported scope.`);
          break;
        }
        seen.add(scope);
      }
    }
  } else if (value.primitive === "Transform") {
    requireExactFields(value, TRANSFORM_FIELDS, `steps[${index}]`, errors);
    if (!isBoundedString(value.intent, 1, 160)) errors.push(`natural-v1 steps[${index}].intent must be bounded.`);
  } else if (value.primitive === "Transfer") {
    requireExactFields(value, TRANSFER_FIELDS, `steps[${index}]`, errors);
    if (!["requesting_device", "selected_device", "user_selected_location", "leave_on_producing_device"].includes(String(value.destination))) {
      errors.push(`natural-v1 steps[${index}].destination is unsupported.`);
    }
    if (!["selected_file", "search_result", "transform_result"].includes(String(value.object))) {
      errors.push(`natural-v1 steps[${index}].object is unsupported.`);
    }
  } else {
    requireExactFields(value, STEP_COMMON_FIELDS, `steps[${index}]`, errors);
    errors.push(`natural-v1 steps[${index}].primitive must be Search, Transform, or Transfer.`);
  }
}

function validatePrimitiveOrder(
  steps: unknown[],
  status: unknown,
  errors: string[],
) {
  const primitives = steps.map((step) => isRecord(step) ? step.primitive : null);
  const key = primitives.join(" -> ");
  if (!["Search", "Search -> Transfer", "Search -> Transform", "Search -> Transform -> Transfer"].includes(key)) {
    errors.push("natural-v1 supports bounded Search, Transform, and Transfer combinations only.");
  }
  if (key === "Search -> Transform -> Transfer") {
    const transform = steps[1] as Record<string, unknown>;
    const transfer = steps[2] as Record<string, unknown>;
    if (transform.intent !== "extract readable text" || transfer.object !== "transform_result") {
      if (status !== "unsupported_future") errors.push("Unsupported Transform plans must be marked unsupported_future.");
    } else if (status !== "supported") {
      errors.push("extract readable text Transform plans must be marked supported.");
    }
  }
  if (status === "unsupported_future" && !primitives.includes("Transform")) {
    errors.push("natural-v1 unsupported_future is reserved for unsupported Transform plans.");
  }
}

function buildSearchStep(userRequest: string): AskBridgeNaturalV1SearchStep {
  const extensions = extractExtensions(userRequest);
  return {
    primitive: "Search",
    filenameHint: extractFilenameHint(userRequest, extensions),
    extensions,
    safeScopes: ["downloads", "desktop", "documents", "pastey_shared"],
  };
}

function extractExtensions(value: string): string[] {
  const found = new Set<string>();
  for (const match of value.matchAll(/(?:\.|\b)([a-z0-9]{1,16})\b/gi)) {
    const extension = match[1].toLowerCase();
    if (KNOWN_EXTENSIONS.has(extension)) found.add(extension);
  }
  return [...found].slice(0, 10);
}

function extractFilenameHint(value: string, extensions: readonly string[]): string {
  const withoutExtensions = extensions.reduce(
    (current, extension) => current.replace(new RegExp(`\\b${escapeRegExp(extension)}\\b`, "gi"), " "),
    value.replace(/\.[a-z0-9]{1,16}\b/gi, " "),
  );
  const words = withoutExtensions
    .toLowerCase()
    .replace(/[^a-z0-9 _-]+/g, " ")
    .split(/\s+/)
    .filter((word) => word.length > 1 && !STOP_WORDS.has(word));
  return (words.slice(0, 4).join(" ") || "file").slice(0, 128);
}

function extractTransformIntent(value: string): "extract readable text" | null {
  const textLike = /\.(txt|md|markdown|json|csv)\b/i.test(value) || /\b(text|markdown|json|csv)\b/i.test(value);
  const extraction = /\b(extract|normalize|readable text)\b/i.test(value);
  return textLike && extraction ? "extract readable text" : null;
}

const STOP_WORDS = new Set([
  "the",
  "and",
  "for",
  "from",
  "this",
  "that",
  "with",
  "find",
  "search",
  "send",
  "return",
  "get",
  "fetch",
  "bring",
  "copy",
  "download",
  "file",
  "files",
  "device",
  "peer",
  "selected",
  "other",
  "back",
  "here",
  "to",
  "me",
  "my",
  "on",
  "in",
]);

function findForbiddenNaturalFieldPaths(value: unknown): string[] {
  const found: string[] = [];
  visitNaturalFields(value, "$", found);
  return found;
}

function visitNaturalFields(value: unknown, path: string, found: string[]) {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => visitNaturalFields(entry, `${path}[${index}]`, found));
    return;
  }
  if (!isRecord(value)) return;
  for (const [key, entry] of Object.entries(value)) {
    const entryPath = `${path}.${key}`;
    if (FORBIDDEN_NATURAL_FIELD_NAMES.has(normalizeFieldName(key))) found.push(entryPath);
    visitNaturalFields(entry, entryPath, found);
  }
}

function requireExactFields(
  value: Record<string, unknown>,
  allowedFields: Set<string>,
  label: string,
  errors: string[],
) {
  for (const key of Object.keys(value)) {
    if (!allowedFields.has(key)) errors.push(`natural-v1 ${label} contains unsupported field: ${key}.`);
  }
}

function isBoundedString(value: unknown, min: number, max: number): value is string {
  return typeof value === "string" && value.trim().length >= min && value.length <= max;
}

function normalizeFieldName(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, "").toLowerCase();
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

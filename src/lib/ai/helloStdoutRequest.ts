import { isRecord } from "./actionPlanValidator";
import {
  findForbiddenProviderFieldPaths,
  HELLO_STDOUT_CAPABILITY,
  HELLO_STDOUT_EXPECTED_STDOUT,
  HELLO_STDOUT_RUNTIME_KIND
} from "./capabilityRegistry";
import {
  buildPendingAiActionCanonicalPayload,
  hashDeterministicString,
  hashPendingAiActionPayload,
  isPendingAiActionExpired,
  stableSerialize
} from "./pendingAction";
import type { PendingAiAction } from "./types";

export type HelloStdoutRuntimeKind = "rust_host_helper";

export interface HelloStdoutRequestConstraints {
  templateOnly: true;
  noRawShell: true;
  filesystem: "none";
  network: false;
  timeoutMs: number;
  maxStdoutBytes: number;
  maxStderrBytes: number;
}

export interface HelloStdoutRequest {
  schemaVersion: "pastey-runtime-hello-stdout-request-v1";
  requestId: string;
  nonce: string;
  createdAt: string;
  expiresAt: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: "runtime.hello_stdout";
  runtimeKind: HelloStdoutRuntimeKind;
  input: {
    expectedStdout: "hello peer";
  };
  constraints: HelloStdoutRequestConstraints;
  pendingPayloadHash: string;
  requestPayloadHash: string;
  transportStatus: "preview_only";
}

export type HelloStdoutRequestBuildResult =
  | { ok: true; request: HelloStdoutRequest }
  | { ok: false; errors: string[] };

export type HelloStdoutRequestValidationResult =
  | { valid: true; value: HelloStdoutRequest; errors: [] }
  | { valid: false; errors: string[] };

interface BuildHelloStdoutRequestOptions {
  now?: Date;
  ttlMs?: number;
  sourceDeviceRef?: string;
  nonce?: string;
  requestId?: string;
}

interface ValidateHelloStdoutRequestOptions {
  now?: Date;
}

const DEFAULT_REQUEST_TTL_MS = 2 * 60 * 1_000;
export {
  HELLO_STDOUT_CAPABILITY,
  HELLO_STDOUT_EXPECTED_STDOUT,
  HELLO_STDOUT_RUNTIME_KIND
};
export const DEFAULT_HELLO_STDOUT_CONSTRAINTS: HelloStdoutRequestConstraints = {
  templateOnly: true,
  noRawShell: true,
  filesystem: "none",
  network: false,
  timeoutMs: 1_000,
  maxStdoutBytes: 64,
  maxStderrBytes: 256
};
const REQUEST_FIELDS = [
  "schemaVersion",
  "requestId",
  "nonce",
  "createdAt",
  "expiresAt",
  "sourceDeviceRef",
  "targetPeerRef",
  "capability",
  "runtimeKind",
  "input",
  "constraints",
  "pendingPayloadHash",
  "requestPayloadHash",
  "transportStatus"
];
const INPUT_FIELDS = ["expectedStdout"];
const CONSTRAINT_FIELDS = [
  "templateOnly",
  "noRawShell",
  "filesystem",
  "network",
  "timeoutMs",
  "maxStdoutBytes",
  "maxStderrBytes"
];
let requestSequence = 0;

export function buildHelloStdoutRequestFromPendingAction(
  pending: PendingAiAction,
  options: BuildHelloStdoutRequestOptions = {}
): HelloStdoutRequestBuildResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_REQUEST_TTL_MS;

  if (pending.status !== "confirmed_local_only") {
    errors.push("Hello Stdout request preview requires a confirmed_local_only pending action.");
  }
  if (pending.policyResult.status !== "accepted" || !pending.policyResult.requiresUserConfirmation) {
    errors.push("Hello Stdout request preview requires the accepted confirmation-bound PolicyGate result.");
  }
  if (!Number.isFinite(now.getTime()) || !Number.isFinite(ttlMs) || ttlMs <= 0) {
    errors.push("Hello Stdout request preview requires a valid time and positive finite TTL.");
  }
  if (isPendingAiActionExpired(pending, now)) {
    errors.push("Hello Stdout request preview cannot be built from an expired pending action.");
  }
  if (pending.actionPlan.kind !== "request_peer_hello_stdout_demo") {
    errors.push("Hello Stdout request preview requires request_peer_hello_stdout_demo.");
  }

  let rebuiltPendingPayload;
  try {
    rebuiltPendingPayload = buildPendingAiActionCanonicalPayload(
      pending.actionPlan,
      pending.pendingId,
      pending.expiresAt
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

  if (errors.length > 0 || !rebuiltPendingPayload) {
    return { ok: false, errors: [...new Set(errors)] };
  }

  const createdAt = now.toISOString();
  const expiresAtMs = Math.min(now.getTime() + ttlMs, new Date(pending.expiresAt).getTime());
  const requestId = options.requestId ?? createPreviewIdentifier("hello-stdout-request", now);
  const nonce = options.nonce ?? createPreviewIdentifier("hello-stdout-nonce", now);
  const sourceDeviceRef = options.sourceDeviceRef ?? "local-device-preview";
  const requestWithoutHash: Omit<HelloStdoutRequest, "requestPayloadHash"> = {
    schemaVersion: "pastey-runtime-hello-stdout-request-v1",
    requestId,
    nonce,
    createdAt,
    expiresAt: new Date(expiresAtMs).toISOString(),
    sourceDeviceRef,
    targetPeerRef: rebuiltPendingPayload.targetPeerRef,
    capability: HELLO_STDOUT_CAPABILITY,
    runtimeKind: HELLO_STDOUT_RUNTIME_KIND,
    input: {
      expectedStdout: HELLO_STDOUT_EXPECTED_STDOUT
    },
    constraints: cloneConstraints(rebuiltPendingPayload.constraints),
    pendingPayloadHash: pending.payloadHash,
    transportStatus: "preview_only"
  };
  const request: HelloStdoutRequest = {
    ...requestWithoutHash,
    requestPayloadHash: hashHelloStdoutRequestPayload(requestWithoutHash)
  };
  const validation = validateHelloStdoutRequest(request, { now });

  return validation.valid
    ? { ok: true, request: validation.value }
    : { ok: false, errors: validation.errors };
}

export function canonicalizeHelloStdoutRequestForHash(
  request: Omit<HelloStdoutRequest, "requestPayloadHash">
): string {
  return stableSerialize(request);
}

export function hashHelloStdoutRequestPayload(
  request: Omit<HelloStdoutRequest, "requestPayloadHash">
): string {
  return hashDeterministicString(canonicalizeHelloStdoutRequestForHash(request));
}

export function validateHelloStdoutRequest(
  value: unknown,
  options: ValidateHelloStdoutRequestOptions = {}
): HelloStdoutRequestValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Hello Stdout request must be an object."] };
  }

  requireExactFields(value, REQUEST_FIELDS, "Hello Stdout request", errors);
  for (const path of findUnsafeFieldPaths(value)) {
    errors.push(`Unsafe field is not allowed in Hello Stdout request: ${path}.`);
  }
  if (value.schemaVersion !== "pastey-runtime-hello-stdout-request-v1") {
    errors.push("Hello Stdout request schemaVersion must be pastey-runtime-hello-stdout-request-v1.");
  }
  requireNonEmptyString(value.requestId, "requestId", errors);
  requireNonEmptyString(value.nonce, "nonce", errors);
  requireNonEmptyString(value.sourceDeviceRef, "sourceDeviceRef", errors);
  requireNonEmptyString(value.targetPeerRef, "targetPeerRef", errors);
  requireNonEmptyString(value.pendingPayloadHash, "pendingPayloadHash", errors);
  requireNonEmptyString(value.requestPayloadHash, "requestPayloadHash", errors);
  validateDates(value.createdAt, value.expiresAt, options.now ?? new Date(), errors);

  if (value.capability !== HELLO_STDOUT_CAPABILITY) {
    errors.push(`Hello Stdout request capability must be exactly ${HELLO_STDOUT_CAPABILITY}.`);
  }
  if (value.runtimeKind !== HELLO_STDOUT_RUNTIME_KIND) {
    errors.push(`Hello Stdout request runtimeKind must be exactly ${HELLO_STDOUT_RUNTIME_KIND}.`);
  }
  if (value.transportStatus !== "preview_only") {
    errors.push("Hello Stdout request transportStatus must be preview_only.");
  }
  validateInput(value.input, errors);
  validateConstraints(value.constraints, errors);

  if (errors.length === 0) {
    const { requestPayloadHash, ...requestWithoutHash } = value;
    const expectedHash = hashHelloStdoutRequestPayload(requestWithoutHash as Omit<HelloStdoutRequest, "requestPayloadHash">);
    if (requestPayloadHash !== expectedHash) {
      errors.push("Hello Stdout request payload hash does not match the canonical preview payload.");
    }
  }

  return errors.length === 0
    ? { valid: true, value: value as unknown as HelloStdoutRequest, errors: [] }
    : { valid: false, errors: [...new Set(errors)] };
}

function validateDates(createdAt: unknown, expiresAt: unknown, now: Date, errors: string[]) {
  const createdAtMs = typeof createdAt === "string" ? new Date(createdAt).getTime() : Number.NaN;
  const expiresAtMs = typeof expiresAt === "string" ? new Date(expiresAt).getTime() : Number.NaN;
  if (!Number.isFinite(createdAtMs)) {
    errors.push("Hello Stdout request requires a valid createdAt timestamp.");
  }
  if (!Number.isFinite(expiresAtMs)) {
    errors.push("Hello Stdout request requires a valid expiresAt timestamp.");
  } else if (expiresAtMs <= now.getTime()) {
    errors.push("Hello Stdout request is expired.");
  }
  if (Number.isFinite(createdAtMs) && Number.isFinite(expiresAtMs) && expiresAtMs <= createdAtMs) {
    errors.push("Hello Stdout request expiresAt must be after createdAt.");
  }
}

function validateInput(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("Hello Stdout request requires an input object.");
    return;
  }
  requireExactFields(value, INPUT_FIELDS, "Hello Stdout input", errors);
  if (value.expectedStdout !== HELLO_STDOUT_EXPECTED_STDOUT) {
    errors.push(`Hello Stdout request expectedStdout must be exactly ${HELLO_STDOUT_EXPECTED_STDOUT}.`);
  }
}

function validateConstraints(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("Hello Stdout request requires a constraints object.");
    return;
  }
  requireExactFields(value, CONSTRAINT_FIELDS, "Hello Stdout constraints", errors);
  if (value.templateOnly !== true) errors.push("Hello Stdout request requires templateOnly.");
  if (value.noRawShell !== true) errors.push("Hello Stdout request requires noRawShell.");
  if (value.filesystem !== "none") errors.push("Hello Stdout request filesystem must be none.");
  if (value.network !== false) errors.push("Hello Stdout request must disable network access.");
  if (!isPositiveFiniteNumber(value.timeoutMs)) errors.push("Hello Stdout request timeoutMs must be positive and finite.");
  if (!isPositiveFiniteNumber(value.maxStdoutBytes)) {
    errors.push("Hello Stdout request maxStdoutBytes must be positive and finite.");
  }
  if (!isPositiveFiniteNumber(value.maxStderrBytes)) {
    errors.push("Hello Stdout request maxStderrBytes must be positive and finite.");
  }
}

function cloneConstraints(value: Record<string, unknown>): HelloStdoutRequestConstraints {
  return {
    templateOnly: value.templateOnly as true,
    noRawShell: value.noRawShell as true,
    filesystem: value.filesystem as "none",
    network: value.network as false,
    timeoutMs: value.timeoutMs as number,
    maxStdoutBytes: value.maxStdoutBytes as number,
    maxStderrBytes: value.maxStderrBytes as number
  };
}

function requireExactFields(value: Record<string, unknown>, expectedFields: string[], label: string, errors: string[]) {
  const actual = Object.keys(value).sort();
  const expected = [...expectedFields].sort();
  if (actual.length !== expected.length || actual.some((field, index) => field !== expected[index])) {
    errors.push(`${label} contains missing or unsupported fields.`);
  }
}

function requireNonEmptyString(value: unknown, label: string, errors: string[]) {
  if (typeof value !== "string" || value.trim().length === 0) {
    errors.push(`Hello Stdout request requires ${label}.`);
  }
}

function findUnsafeFieldPaths(value: unknown): string[] {
  return findForbiddenProviderFieldPaths(value);
}

function isPositiveFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

function createPreviewIdentifier(prefix: string, now: Date): string {
  requestSequence += 1;
  const randomPart = globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${requestSequence}`;
  return `${prefix}-${randomPart}`;
}

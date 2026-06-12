import { isRecord } from "./actionPlanValidator";
import {
  buildPendingAiActionCanonicalPayload,
  hashDeterministicString,
  hashPendingAiActionPayload,
  isPendingAiActionExpired,
  stableSerialize
} from "./pendingAction";
import type { PendingAiAction } from "./types";

export type HelloPeerRuntime =
  | "python"
  | "node"
  | "sh"
  | "powershell"
  | "rust"
  | "unknown";

export interface HelloPeerRequestConstraints {
  templateOnly: true;
  noRawShell: true;
  filesystem: "none" | "temp-only";
  network: false;
  timeoutMs: number;
  maxStdoutBytes: number;
}

export interface HelloPeerRequest {
  schemaVersion: "pastey-capability-request/v1";
  requestId: string;
  nonce: string;
  createdAt: string;
  expiresAt: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
  capability: "runtime.execute_hello_template";
  runtimePreference: HelloPeerRuntime[];
  input: {
    message: "hello peer!";
  };
  constraints: HelloPeerRequestConstraints;
  pendingPayloadHash: string;
  requestPayloadHash: string;
  transportStatus: "preview_only";
}

export type HelloPeerRequestBuildResult =
  | { ok: true; request: HelloPeerRequest }
  | { ok: false; errors: string[] };

export type HelloPeerRequestValidationResult =
  | { valid: true; value: HelloPeerRequest; errors: [] }
  | { valid: false; errors: string[] };

interface BuildHelloPeerRequestOptions {
  now?: Date;
  ttlMs?: number;
  sourceDeviceRef?: string;
  nonce?: string;
  requestId?: string;
}

interface ValidateHelloPeerRequestOptions {
  now?: Date;
}

const DEFAULT_REQUEST_TTL_MS = 2 * 60 * 1_000;
const HELLO_CAPABILITY = "runtime.execute_hello_template";
const HELLO_MESSAGE = "hello peer!";
const DEFAULT_RUNTIME_PREFERENCE: HelloPeerRuntime[] = [
  "python",
  "node",
  "powershell",
  "sh",
  "rust",
  "unknown"
];
const KNOWN_RUNTIMES = new Set<HelloPeerRuntime>(DEFAULT_RUNTIME_PREFERENCE);
const REQUEST_FIELDS = [
  "schemaVersion",
  "requestId",
  "nonce",
  "createdAt",
  "expiresAt",
  "sourceDeviceRef",
  "targetPeerRef",
  "capability",
  "runtimePreference",
  "input",
  "constraints",
  "pendingPayloadHash",
  "requestPayloadHash",
  "transportStatus"
];
const CONSTRAINT_FIELDS = [
  "templateOnly",
  "noRawShell",
  "filesystem",
  "network",
  "timeoutMs",
  "maxStdoutBytes"
];
const UNSAFE_FIELDS = new Set([
  "command",
  "cmd",
  "shell",
  "script",
  "code",
  "path",
  "absolutePath",
  "filePath",
  "filesystemTree",
  "rawLogs",
  "secret",
  "token",
  "apiKey",
  "roomKey",
  "roomCode",
  "transportKey",
  "hiddenTransfer",
  "peerFilesystemSearch"
].map(normalizeFieldName));
let requestSequence = 0;

export function buildHelloPeerRequestFromPendingAction(
  pending: PendingAiAction,
  options: BuildHelloPeerRequestOptions = {}
): HelloPeerRequestBuildResult {
  const errors: string[] = [];
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_REQUEST_TTL_MS;

  if (pending.status !== "confirmed_local_only") {
    errors.push("Hello Peer request preview requires a confirmed_local_only pending action.");
  }
  if (pending.policyResult.status !== "accepted" || !pending.policyResult.requiresUserConfirmation) {
    errors.push("Hello Peer request preview requires the accepted confirmation-bound PolicyGate result.");
  }
  if (!Number.isFinite(now.getTime()) || !Number.isFinite(ttlMs) || ttlMs <= 0) {
    errors.push("Hello Peer request preview requires a valid time and positive finite TTL.");
  }
  if (isPendingAiActionExpired(pending, now)) {
    errors.push("Hello Peer request preview cannot be built from an expired pending action.");
  }
  if (pending.actionPlan.kind !== "request_peer_hello_demo") {
    errors.push("Hello Peer request preview requires request_peer_hello_demo.");
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
  const requestId = options.requestId ?? createPreviewIdentifier("hello-peer-request", now);
  const nonce = options.nonce ?? createPreviewIdentifier("hello-peer-nonce", now);
  const sourceDeviceRef = options.sourceDeviceRef ?? "local-device-preview";
  const requestWithoutHash: Omit<HelloPeerRequest, "requestPayloadHash"> = {
    schemaVersion: "pastey-capability-request/v1",
    requestId,
    nonce,
    createdAt,
    expiresAt: new Date(expiresAtMs).toISOString(),
    sourceDeviceRef,
    targetPeerRef: rebuiltPendingPayload.targetPeerRef,
    capability: HELLO_CAPABILITY,
    runtimePreference: [...DEFAULT_RUNTIME_PREFERENCE],
    input: {
      message: HELLO_MESSAGE
    },
    constraints: cloneConstraints(rebuiltPendingPayload.constraints),
    pendingPayloadHash: pending.payloadHash,
    transportStatus: "preview_only"
  };
  const request: HelloPeerRequest = {
    ...requestWithoutHash,
    requestPayloadHash: hashHelloPeerRequestPayload(requestWithoutHash)
  };
  const validation = validateHelloPeerRequest(request, { now });

  return validation.valid
    ? { ok: true, request: validation.value }
    : { ok: false, errors: validation.errors };
}

export function canonicalizeHelloPeerRequestForHash(
  request: Omit<HelloPeerRequest, "requestPayloadHash">
): string {
  return stableSerialize(request);
}

export function hashHelloPeerRequestPayload(
  request: Omit<HelloPeerRequest, "requestPayloadHash">
): string {
  return hashDeterministicString(canonicalizeHelloPeerRequestForHash(request));
}

export function validateHelloPeerRequest(
  value: unknown,
  options: ValidateHelloPeerRequestOptions = {}
): HelloPeerRequestValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Hello Peer request must be an object."] };
  }

  requireExactFields(value, REQUEST_FIELDS, "Hello Peer request", errors);
  for (const path of findUnsafeFieldPaths(value)) {
    errors.push(`Unsafe field is not allowed in Hello Peer request: ${path}.`);
  }
  if (value.schemaVersion !== "pastey-capability-request/v1") {
    errors.push("Hello Peer request schemaVersion must be pastey-capability-request/v1.");
  }
  requireNonEmptyString(value.requestId, "requestId", errors);
  requireNonEmptyString(value.nonce, "nonce", errors);
  requireNonEmptyString(value.sourceDeviceRef, "sourceDeviceRef", errors);
  requireNonEmptyString(value.targetPeerRef, "targetPeerRef", errors);
  requireNonEmptyString(value.pendingPayloadHash, "pendingPayloadHash", errors);
  requireNonEmptyString(value.requestPayloadHash, "requestPayloadHash", errors);
  validateDates(value.createdAt, value.expiresAt, options.now ?? new Date(), errors);

  if (value.capability !== HELLO_CAPABILITY) {
    errors.push(`Hello Peer request capability must be exactly ${HELLO_CAPABILITY}.`);
  }
  if (value.transportStatus !== "preview_only") {
    errors.push("Hello Peer request transportStatus must be preview_only.");
  }
  validateRuntimePreference(value.runtimePreference, errors);
  validateInput(value.input, errors);
  validateConstraints(value.constraints, errors);

  if (errors.length === 0) {
    const { requestPayloadHash, ...requestWithoutHash } = value;
    const expectedHash = hashHelloPeerRequestPayload(requestWithoutHash as Omit<HelloPeerRequest, "requestPayloadHash">);
    if (requestPayloadHash !== expectedHash) {
      errors.push("Hello Peer request payload hash does not match the canonical preview payload.");
    }
  }

  return errors.length === 0
    ? { valid: true, value: value as unknown as HelloPeerRequest, errors: [] }
    : { valid: false, errors: [...new Set(errors)] };
}

function validateDates(createdAt: unknown, expiresAt: unknown, now: Date, errors: string[]) {
  const createdAtMs = typeof createdAt === "string" ? new Date(createdAt).getTime() : Number.NaN;
  const expiresAtMs = typeof expiresAt === "string" ? new Date(expiresAt).getTime() : Number.NaN;
  if (!Number.isFinite(createdAtMs)) {
    errors.push("Hello Peer request requires a valid createdAt timestamp.");
  }
  if (!Number.isFinite(expiresAtMs)) {
    errors.push("Hello Peer request requires a valid expiresAt timestamp.");
  } else if (expiresAtMs <= now.getTime()) {
    errors.push("Hello Peer request is expired.");
  }
  if (Number.isFinite(createdAtMs) && Number.isFinite(expiresAtMs) && expiresAtMs <= createdAtMs) {
    errors.push("Hello Peer request expiresAt must be after createdAt.");
  }
}

function validateRuntimePreference(value: unknown, errors: string[]) {
  if (!Array.isArray(value) || value.length === 0) {
    errors.push("Hello Peer request requires at least one runtime preference.");
    return;
  }
  if (value.some((runtime) => typeof runtime !== "string" || !KNOWN_RUNTIMES.has(runtime as HelloPeerRuntime))) {
    errors.push("Hello Peer request contains an unknown runtime preference.");
  }
}

function validateInput(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("Hello Peer request requires an input object.");
    return;
  }
  requireExactFields(value, ["message"], "Hello Peer input", errors);
  if (value.message !== HELLO_MESSAGE) {
    errors.push(`Hello Peer request message must be exactly ${HELLO_MESSAGE}.`);
  }
}

function validateConstraints(value: unknown, errors: string[]) {
  if (!isRecord(value)) {
    errors.push("Hello Peer request requires a constraints object.");
    return;
  }
  requireExactFields(value, CONSTRAINT_FIELDS, "Hello Peer constraints", errors);
  if (value.templateOnly !== true) errors.push("Hello Peer request requires templateOnly.");
  if (value.noRawShell !== true) errors.push("Hello Peer request requires noRawShell.");
  if (value.filesystem !== "none" && value.filesystem !== "temp-only") {
    errors.push("Hello Peer request filesystem must be none or temp-only.");
  }
  if (value.network !== false) errors.push("Hello Peer request must disable network access.");
  if (!isPositiveFiniteNumber(value.timeoutMs)) errors.push("Hello Peer request timeoutMs must be positive and finite.");
  if (!isPositiveFiniteNumber(value.maxStdoutBytes)) {
    errors.push("Hello Peer request maxStdoutBytes must be positive and finite.");
  }
}

function cloneConstraints(value: Record<string, unknown>): HelloPeerRequestConstraints {
  return {
    templateOnly: value.templateOnly as true,
    noRawShell: value.noRawShell as true,
    filesystem: value.filesystem as "none" | "temp-only",
    network: value.network as false,
    timeoutMs: value.timeoutMs as number,
    maxStdoutBytes: value.maxStdoutBytes as number
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
    errors.push(`Hello Peer request requires ${label}.`);
  }
}

function findUnsafeFieldPaths(value: unknown, path = "$", found: string[] = []): string[] {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => findUnsafeFieldPaths(entry, `${path}[${index}]`, found));
  } else if (isRecord(value)) {
    for (const [key, entry] of Object.entries(value)) {
      const entryPath = `${path}.${key}`;
      if (UNSAFE_FIELDS.has(normalizeFieldName(key))) found.push(entryPath);
      findUnsafeFieldPaths(entry, entryPath, found);
    }
  }
  return found;
}

function normalizeFieldName(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, "").toLowerCase();
}

function isPositiveFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

function createPreviewIdentifier(prefix: string, now: Date): string {
  requestSequence += 1;
  const randomPart = globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${requestSequence}`;
  return `${prefix}-${randomPart}`;
}

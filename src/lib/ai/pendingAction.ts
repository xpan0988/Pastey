import { isRecord, validateAiActionPlan } from "./actionPlanValidator";
import type {
  AiActionPlan,
  AiPolicyResult,
  PendingAiAction,
  PendingAiActionCanonicalPayload
} from "./types";

const DEFAULT_PENDING_TTL_MS = 2 * 60 * 1_000;
const HELLO_CAPABILITY = "runtime.execute_hello_template";
const HELLO_MESSAGE = "hello peer!";
const HELLO_INPUT_FIELDS = ["targetPeerRef", "capability", "message", "constraints"];
const HELLO_CONSTRAINT_FIELDS = [
  "templateOnly",
  "noRawShell",
  "filesystem",
  "network",
  "timeoutMs",
  "maxStdoutBytes"
];
let pendingSequence = 0;

interface CreatePendingAiActionOptions {
  now?: Date;
  ttlMs?: number;
  pendingId?: string;
}

export function createPendingAiAction(
  plan: AiActionPlan,
  policyResult: AiPolicyResult,
  options: CreatePendingAiActionOptions = {}
): PendingAiAction {
  if (policyResult.status !== "accepted") {
    throw new Error("Cannot create a pending AI action from a rejected policy result.");
  }
  if (!plan.requiresUserConfirmation || !policyResult.requiresUserConfirmation) {
    throw new Error("Cannot create a pending AI action without required user confirmation.");
  }
  const validation = validateAiActionPlan(plan);
  if (!validation.valid) {
    throw new Error(`Cannot create a pending AI action from an invalid plan: ${validation.errors.join(" ")}`);
  }

  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? DEFAULT_PENDING_TTL_MS;
  if (!Number.isFinite(ttlMs) || ttlMs <= 0 || !Number.isFinite(now.getTime())) {
    throw new Error("Pending AI action requires a valid creation time and positive finite TTL.");
  }

  const pendingId = options.pendingId ?? createPendingId(now);
  if (pendingId.trim().length === 0) {
    throw new Error("Pending AI action requires a pending ID.");
  }

  const createdAt = now.toISOString();
  const expiresAt = new Date(now.getTime() + ttlMs).toISOString();
  const canonicalPayload = buildPendingAiActionCanonicalPayload(plan, pendingId, expiresAt);

  return {
    pendingId,
    actionPlan: cloneJson(plan),
    policyResult: cloneJson(policyResult),
    createdAt,
    expiresAt,
    canonicalPayload,
    payloadHash: hashPendingAiActionPayload(canonicalPayload),
    status: "pending"
  };
}

export function buildPendingAiActionCanonicalPayload(
  plan: AiActionPlan,
  pendingId: string,
  expiresAt: string
): PendingAiActionCanonicalPayload {
  if (plan.schemaVersion !== "ai-action-plan/v1" || plan.kind !== "request_peer_hello_demo") {
    throw new Error("Pending AI action supports only the validated Hello Peer advisory.");
  }
  if (!isRecord(plan.proposedInput) || !isRecord(plan.proposedInput.constraints)) {
    throw new Error("Pending AI action requires a complete Hello Peer proposedInput.");
  }
  requireExactFields(plan.proposedInput, HELLO_INPUT_FIELDS, "Hello Peer proposedInput");
  requireExactFields(plan.proposedInput.constraints, HELLO_CONSTRAINT_FIELDS, "Hello Peer constraints");

  const { targetPeerRef, capability, message, constraints } = plan.proposedInput;
  if (typeof targetPeerRef !== "string" || targetPeerRef.trim().length === 0) {
    throw new Error("Pending AI action requires a target peer reference.");
  }
  if (capability !== HELLO_CAPABILITY || message !== HELLO_MESSAGE) {
    throw new Error("Pending AI action requires the fixed Hello Peer capability and message.");
  }
  if (
    constraints.templateOnly !== true
    || constraints.noRawShell !== true
    || (constraints.filesystem !== "none" && constraints.filesystem !== "temp-only")
    || constraints.network !== false
    || !isPositiveFiniteNumber(constraints.timeoutMs)
    || !isPositiveFiniteNumber(constraints.maxStdoutBytes)
  ) {
    throw new Error("Pending AI action requires the bounded Hello Peer constraints.");
  }
  if (!Number.isFinite(new Date(expiresAt).getTime())) {
    throw new Error("Pending AI action requires a valid expiry.");
  }

  return {
    schemaVersion: plan.schemaVersion,
    kind: plan.kind,
    targetPeerRef,
    capability,
    message,
    constraints: cloneJson(constraints),
    references: [...(plan.references ?? [])]
      .map((reference) => cloneJson(reference))
      .sort((left, right) => `${left.kind}:${left.ref}`.localeCompare(`${right.kind}:${right.ref}`)),
    pendingId,
    expiresAt
  };
}

export function hashPendingAiActionPayload(payload: PendingAiActionCanonicalPayload): string {
  // This deterministic lightweight hash binds the local preview to displayed data.
  // It is not a transport security or cryptographic integrity primitive.
  return hashStableSerializedValue(payload);
}

export function hashStableSerializedValue(value: unknown): string {
  return hashDeterministicString(stableSerialize(value));
}

export function hashDeterministicString(serialized: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < serialized.length; index += 1) {
    hash ^= serialized.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return `fnv1a32:${(hash >>> 0).toString(16).padStart(8, "0")}`;
}

export function stableSerialize(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(stableSerialize).join(",")}]`;
  }
  if (isRecord(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableSerialize(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

export function isPendingAiActionExpired(pending: PendingAiAction, now = new Date()): boolean {
  const expiresAt = new Date(pending.expiresAt).getTime();
  return !Number.isFinite(expiresAt) || expiresAt <= now.getTime();
}

export function confirmPendingAiAction(pending: PendingAiAction, now = new Date()): PendingAiAction {
  if (pending.status !== "pending") {
    return pending;
  }
  if (isPendingAiActionExpired(pending, now)) {
    return { ...pending, status: "expired" };
  }
  return { ...pending, status: "confirmed_local_only" };
}

export function cancelPendingAiAction(pending: PendingAiAction): PendingAiAction {
  return pending.status === "pending" ? { ...pending, status: "cancelled" } : pending;
}

export function expirePendingAiAction(pending: PendingAiAction, now = new Date()): PendingAiAction {
  return pending.status === "pending" && isPendingAiActionExpired(pending, now)
    ? { ...pending, status: "expired" }
    : pending;
}

function createPendingId(now: Date): string {
  pendingSequence += 1;
  return `pending-ai-${now.getTime()}-${pendingSequence}`;
}

function requireExactFields(value: Record<string, unknown>, expectedFields: string[], label: string) {
  const actualFields = Object.keys(value).sort();
  const expected = [...expectedFields].sort();
  if (actualFields.length !== expected.length || actualFields.some((field, index) => field !== expected[index])) {
    throw new Error(`${label} contains missing or unsupported fields.`);
  }
}

function isPositiveFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

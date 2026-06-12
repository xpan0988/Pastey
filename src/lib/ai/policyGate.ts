import { findUnsafeFieldPaths, isRecord } from "./actionPlanValidator";
import type { AiActionPlan, AiContextSnapshot, AiPolicyResult } from "./types";

const HELLO_CAPABILITY = "runtime.execute_hello_template";
const HELLO_MESSAGE = "hello peer!";
const HELLO_INPUT_FIELDS = new Set(["targetPeerRef", "capability", "message", "constraints"]);
const HELLO_CONSTRAINT_FIELDS = new Set([
  "templateOnly",
  "noRawShell",
  "filesystem",
  "network",
  "timeoutMs",
  "maxStdoutBytes"
]);
const POLICY_FORBIDDEN_FIELDS = new Set([
  "hiddenTransfer",
  "automaticTransfer",
  "fileRead",
  "peerFileRead",
  "peerFilesystemSearch",
  "filesystemSearch",
  "schedulerMutation",
  "microFlowGroupMutation",
  "transferWindowMutation",
  "runtimeWindowMutation"
].map(normalizeFieldName));

export function evaluateAiPolicy(plan: AiActionPlan, context: AiContextSnapshot): AiPolicyResult {
  const reasons: string[] = [];
  const warnings = ["Advisory only. The AI Slot preview cannot dispatch or execute this plan."];

  if (!context.allowedActions.includes(plan.kind)) {
    reasons.push(`Action kind ${plan.kind} is not allowed by the current context.`);
  }
  if (plan.kind !== "request_peer_hello_demo") {
    reasons.push(`Action kind ${plan.kind} is not enabled by the AI Slot v0 PolicyGate.`);
  }
  if (!plan.requiresUserConfirmation) {
    reasons.push("Hello Peer requires explicit local user confirmation.");
  }
  if (!context.room?.hasActiveRoom || !context.room.trustedRoom) {
    reasons.push("Hello Peer requires a current trusted room.");
  }

  for (const unsafePath of findUnsafeFieldPaths(plan)) {
    reasons.push(`Unsafe shell, code, credential, or path-like field: ${unsafePath}.`);
  }
  for (const forbiddenPath of findPolicyForbiddenPaths(plan.proposedInput)) {
    reasons.push(`Forbidden execution or mutation indicator: ${forbiddenPath}.`);
  }

  const input = plan.proposedInput;
  if (!isRecord(input)) {
    reasons.push("Hello Peer requires a proposedInput object.");
    return policyResult(plan, reasons, warnings);
  }
  rejectUnsupportedFields(input, HELLO_INPUT_FIELDS, "proposedInput", reasons);

  const targetPeerRef = input.targetPeerRef;
  if (typeof targetPeerRef !== "string" || targetPeerRef.trim().length === 0) {
    reasons.push("Hello Peer requires a targetPeerRef.");
  }

  const peer = typeof targetPeerRef === "string"
    ? context.peers?.find((candidate) => candidate.peerRef === targetPeerRef)
    : undefined;
  if (!peer || !peer.visible || !peer.trusted) {
    reasons.push("Target peer must be current, visible, and trusted.");
  } else if (!peer.capabilities?.includes(HELLO_CAPABILITY)) {
    reasons.push(`Target peer does not advertise ${HELLO_CAPABILITY}.`);
  }

  if (input.capability !== HELLO_CAPABILITY) {
    reasons.push(`Hello Peer capability must be exactly ${HELLO_CAPABILITY}.`);
  }
  if (input.message !== HELLO_MESSAGE) {
    reasons.push(`Hello Peer message must be exactly ${HELLO_MESSAGE}.`);
  }

  validateConstraints(input.constraints, reasons);
  return policyResult(plan, reasons, warnings);
}

function validateConstraints(value: unknown, reasons: string[]) {
  if (!isRecord(value)) {
    reasons.push("Hello Peer requires a constraints object.");
    return;
  }
  rejectUnsupportedFields(value, HELLO_CONSTRAINT_FIELDS, "constraints", reasons);
  if (value.templateOnly !== true) {
    reasons.push("Hello Peer constraints must require templateOnly.");
  }
  if (value.noRawShell !== true) {
    reasons.push("Hello Peer constraints must require noRawShell.");
  }
  if (value.filesystem !== "none" && value.filesystem !== "temp-only") {
    reasons.push("Hello Peer filesystem constraint must be none or temp-only.");
  }
  if (value.network !== false) {
    reasons.push("Hello Peer constraints must disable network access.");
  }
  if (!isPositiveFiniteNumber(value.timeoutMs)) {
    reasons.push("Hello Peer timeoutMs must be a positive finite number.");
  }
  if (!isPositiveFiniteNumber(value.maxStdoutBytes)) {
    reasons.push("Hello Peer maxStdoutBytes must be a positive finite number.");
  }
}

function policyResult(plan: AiActionPlan, reasons: string[], warnings: string[]): AiPolicyResult {
  return {
    status: reasons.length === 0 ? "accepted" : "rejected",
    requiresUserConfirmation: plan.requiresUserConfirmation,
    reasons: [...new Set(reasons)],
    warnings
  };
}

function rejectUnsupportedFields(
  value: Record<string, unknown>,
  allowedFields: Set<string>,
  label: string,
  reasons: string[]
) {
  for (const key of Object.keys(value)) {
    if (!allowedFields.has(key)) {
      reasons.push(`Unsupported ${label} field: ${key}.`);
    }
  }
}

function findPolicyForbiddenPaths(value: unknown): string[] {
  const paths: string[] = [];
  visitValue(value, "$.proposedInput", paths);
  return paths;
}

function visitValue(value: unknown, path: string, paths: string[]) {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => visitValue(entry, `${path}[${index}]`, paths));
    return;
  }
  if (!isRecord(value)) {
    return;
  }
  for (const [key, entry] of Object.entries(value)) {
    const entryPath = `${path}.${key}`;
    if (POLICY_FORBIDDEN_FIELDS.has(normalizeFieldName(key))) {
      paths.push(entryPath);
    }
    visitValue(entry, entryPath, paths);
  }
}

function normalizeFieldName(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, "").toLowerCase();
}

function isPositiveFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

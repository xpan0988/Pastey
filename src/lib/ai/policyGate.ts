import { findUnsafeFieldPaths, isRecord } from "./actionPlanValidator";
import {
  getAgentBridgeCapabilityContractByActionKind,
  normalizeCapabilityFieldName,
  type AgentBridgeCapabilityContract
} from "./capabilityRegistry";
import { validateCandidatePayloadAdvisoryInput } from "./candidatePayloadAdvisory";
import { validateArtifactTransformRequestInput } from "./artifactTransformRequest";
import { validateFileCandidateAdvisoryInput } from "./fileCandidateAdvisory";
import type { AiActionPlan, AiContextSnapshot, AiPolicyResult } from "./types";

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
].map(normalizeCapabilityFieldName));
const FIXED_MESSAGE_INPUT_FIELDS = new Set(["targetPeerRef", "capability", "message", "constraints"]);

export function evaluateAiPolicy(plan: AiActionPlan, context: AiContextSnapshot): AiPolicyResult {
  const reasons: string[] = [];
  const warnings = ["Advisory only. The AI Slot preview cannot dispatch or execute this plan."];

  if (!context.allowedActions.includes(plan.kind)) {
    reasons.push(`Action kind ${plan.kind} is not allowed by the current context.`);
  }
  const contract = getAgentBridgeCapabilityContractByActionKind(plan.kind);
  if (!contract) {
    reasons.push(`Action kind ${plan.kind} is not enabled by the AI Slot v0 PolicyGate.`);
  }
  if (!plan.requiresUserConfirmation) {
    reasons.push("Agent Bridge capability requests require explicit local user confirmation.");
  }
  if (!context.room?.hasActiveRoom || !context.room.trustedRoom) {
    reasons.push("Agent Bridge capability requests require a current trusted room.");
  }

  for (const unsafePath of findUnsafeFieldPaths(plan)) {
    reasons.push(`Unsafe shell, code, credential, or path-like field: ${unsafePath}.`);
  }
  for (const forbiddenPath of findPolicyForbiddenPaths(plan.proposedInput)) {
    reasons.push(`Forbidden execution or mutation indicator: ${forbiddenPath}.`);
  }

  const input = plan.proposedInput;
  if (!isRecord(input)) {
    reasons.push("Agent Bridge capability requests require a proposedInput object.");
    return policyResult(plan, reasons, warnings);
  }
  if (contract?.providerInputShape === "file_candidate_advisory") {
    validateFileCandidateAdvisoryPolicyInput(input, reasons);
  } else if (contract?.providerInputShape === "candidate_payload_request") {
    validateCandidatePayloadAdvisoryPolicyInput(input, reasons);
  } else if (contract?.providerInputShape === "artifact_transform_request") {
    const validation = validateArtifactTransformRequestInput(input);
    if (!validation.valid) reasons.push(...validation.errors);
  } else {
    rejectUnsupportedFields(input, FIXED_MESSAGE_INPUT_FIELDS, "proposedInput", reasons);
  }

  const targetPeerRef = input.targetPeerRef;
  if (typeof targetPeerRef !== "string" || targetPeerRef.trim().length === 0) {
    reasons.push("Agent Bridge capability requests require a targetPeerRef.");
  }

  const peer = typeof targetPeerRef === "string"
    ? context.peers?.find((candidate) => candidate.peerRef === targetPeerRef)
    : undefined;
  if (!peer || !peer.visible || !peer.trusted) {
    reasons.push("Target peer must be current, visible, and trusted.");
  } else if (contract?.requiresPeerCapabilityAdvertisement && !peer.capabilities?.includes(contract.capability)) {
    reasons.push(`Target peer does not advertise ${contract.capability}.`);
  }

  if (contract && input.capability !== contract.capability) {
    reasons.push(`${contract.ui.label} capability must be exactly ${contract.capability}.`);
  }
  if (contract?.providerInputShape === "fixed_message") {
    const providerInputField = contract.providerInputField ?? "message";
    if (input[providerInputField] !== contract.providerInputValue) {
      reasons.push(`${contract.ui.label} ${providerInputField} must be exactly ${contract.providerInputValue}.`);
    }
  }

  if (contract?.providerInputShape === "fixed_message") {
    validateConstraints(input.constraints, contract, reasons);
  }
  return policyResult(plan, reasons, warnings);
}

function validateFileCandidateAdvisoryPolicyInput(input: Record<string, unknown>, reasons: string[]) {
  const validation = validateFileCandidateAdvisoryInput(input);
  if (!validation.valid) {
    reasons.push(...validation.errors);
  }
}

function validateCandidatePayloadAdvisoryPolicyInput(input: Record<string, unknown>, reasons: string[]) {
  const validation = validateCandidatePayloadAdvisoryInput(input);
  if (!validation.valid) {
    reasons.push(...validation.errors);
  }
}

function validateConstraints(
  value: unknown,
  contract: AgentBridgeCapabilityContract,
  reasons: string[]
) {
  if (!isRecord(value)) {
    reasons.push(`${contract.ui.label} requires a constraints object.`);
    return;
  }
  const allowedFields = new Set(contract.constraintFields);
  rejectUnsupportedFields(value, allowedFields, "constraints", reasons);
  if (value.templateOnly !== true) {
    reasons.push(`${contract.ui.label} constraints must require templateOnly.`);
  }
  if (value.noRawShell !== true) {
    reasons.push(`${contract.ui.label} constraints must require noRawShell.`);
  }
  if (value.filesystem !== "none" && (!contract.allowTempOnlyFilesystem || value.filesystem !== "temp-only")) {
    reasons.push(`${contract.ui.label} filesystem constraint must be ${contract.allowTempOnlyFilesystem ? "none or temp-only" : "none"}.`);
  }
  if (value.network !== false) {
    reasons.push(`${contract.ui.label} constraints must disable network access.`);
  }
  if (!isPositiveFiniteNumber(value.timeoutMs)) {
    reasons.push(`${contract.ui.label} timeoutMs must be a positive finite number.`);
  }
  if (!isPositiveFiniteNumber(value.maxStdoutBytes)) {
    reasons.push(`${contract.ui.label} maxStdoutBytes must be a positive finite number.`);
  }
  if (allowedFields.has("maxStderrBytes") && !isPositiveFiniteNumber(value.maxStderrBytes)) {
    reasons.push(`${contract.ui.label} maxStderrBytes must be a positive finite number.`);
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
    if (POLICY_FORBIDDEN_FIELDS.has(normalizeCapabilityFieldName(key))) {
      paths.push(entryPath);
    }
    visitValue(entry, entryPath, paths);
  }
}

function isPositiveFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

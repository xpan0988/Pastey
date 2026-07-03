import {
  buildCandidatePayloadRequestFromPendingAction,
  buildFileCandidateRequestFromPendingAction,
  confirmPendingAiAction,
  createPendingAiAction,
  evaluateAiPolicy,
  validateAiActionPlan,
  validateCandidatePayloadExecutionResult,
  validateFileCandidateExecutionResult,
  type AiActionPlan,
  type AiContextSnapshot,
  type CandidatePayloadExecutionResult,
  type CandidatePayloadRequest,
  type FileCandidateExecutionResult,
  type FileCandidateMetadata,
  type FileCandidateRequest,
  type PendingAiAction,
} from "../ai";
import {
  assertExactCapability,
  rejectForbiddenPublicFields,
} from "./capabilityTemplateHelpers";
import { requireCapabilityManifest } from "./capabilityManifest";

export type CandidatePayloadWorkflowState =
  | "idle"
  | "search_preview_ready"
  | "search_pending_receiver_consent"
  | "search_completed_candidates_ready"
  | "candidate_selection_required"
  | "payload_preview_ready"
  | "payload_pending_receiver_consent"
  | "handoff_queued"
  | "failed";

export type CandidatePayloadWorkflowEvent =
  | "ai_search_advisory_validated"
  | "local_search_confirmed"
  | "receiver_search_allowed"
  | "candidate_results_received"
  | "candidate_selected_by_user"
  | "payload_request_preview_created"
  | "receiver_payload_allowed"
  | "candidate_payload_handoff_queued"
  | "workflow_failed";

export interface CandidatePayloadWorkflowSearchSummary {
  readonly capability: "filesystem.find_file_candidates";
  readonly requestId?: string;
  readonly targetPeerRef: string;
  readonly filenameHint: string;
  readonly searchMode: "filename_metadata_only";
}

export interface CandidatePayloadWorkflowCandidate {
  readonly sourceRequestId: string;
  readonly candidateId: string;
  readonly candidateKind: "filesystem_file";
  readonly candidateDisplayName: string;
  readonly redactedLocation: string;
  readonly extension: string;
  readonly mimeFamily: FileCandidateMetadata["mimeFamily"];
  readonly sizeBytes: number;
  readonly modifiedAt: string;
  readonly matchReason: FileCandidateMetadata["matchReason"];
  readonly confidence: FileCandidateMetadata["confidence"];
}

export interface CandidatePayloadWorkflowPayloadSummary {
  readonly capability: "transfer.request_candidate_payload";
  readonly requestId: string;
  readonly targetPeerRef: string;
  readonly sourceCapability: "filesystem.find_file_candidates";
  readonly sourceRequestId: string;
  readonly candidateId: string;
  readonly candidateKind: "filesystem_file";
  readonly candidateDisplayName: string;
}

export interface CandidatePayloadWorkflowHandoffSummary {
  readonly status: "handoff_queued";
  readonly transferredBytes: 0;
  readonly handoffQueued: true;
  readonly transferStatus: "queued";
}

export interface CandidatePayloadWorkflowSnapshot {
  readonly state: CandidatePayloadWorkflowState;
  readonly events: readonly CandidatePayloadWorkflowEvent[];
  readonly search?: CandidatePayloadWorkflowSearchSummary;
  readonly candidates?: readonly CandidatePayloadWorkflowCandidate[];
  readonly selectedCandidate?: CandidatePayloadWorkflowCandidate;
  readonly payload?: CandidatePayloadWorkflowPayloadSummary;
  readonly handoff?: CandidatePayloadWorkflowHandoffSummary;
  readonly errors?: readonly string[];
}

export interface CandidatePayloadWorkflow {
  readonly snapshot: CandidatePayloadWorkflowSnapshot;
  readonly searchPendingAction?: PendingAiAction;
  readonly searchRequest?: FileCandidateRequest;
  readonly payloadPendingAction?: PendingAiAction;
  readonly payloadRequest?: CandidatePayloadRequest;
}

export type CandidatePayloadWorkflowResult =
  | { ok: true; workflow: CandidatePayloadWorkflow }
  | { ok: false; workflow: CandidatePayloadWorkflow; errors: string[] };

export type CandidatePayloadWorkflowRequestResult<TRequest> =
  | { ok: true; workflow: CandidatePayloadWorkflow; request: TRequest }
  | { ok: false; workflow: CandidatePayloadWorkflow; errors: string[] };

const SEARCH_MANIFEST = requireCapabilityManifest("filesystem.find_file_candidates");
const PAYLOAD_MANIFEST = requireCapabilityManifest("transfer.request_candidate_payload");

export function createCandidatePayloadWorkflow(): CandidatePayloadWorkflow {
  return {
    snapshot: {
      state: "idle",
      events: [],
    },
  };
}

export function startCandidatePayloadWorkflowFromSearchAdvisory(
  workflow: CandidatePayloadWorkflow,
  plan: AiActionPlan,
  context: AiContextSnapshot,
  options: { now?: Date; ttlMs?: number; pendingId?: string } = {},
): CandidatePayloadWorkflowResult {
  if (workflow.snapshot.state !== "idle") {
    return failWorkflow(workflow, "Candidate payload workflow search advisory can start only from idle.");
  }
  if (plan.kind !== SEARCH_MANIFEST.providerActionKind) {
    return failWorkflow(workflow, "Candidate payload workflow must start with filesystem.find_file_candidates.");
  }
  const validation = validateAiActionPlan(plan);
  if (!validation.valid) {
    return failWorkflow(workflow, validation.errors);
  }
  const policy = evaluateAiPolicy(validation.value, context);
  if (policy.status !== "accepted") {
    return failWorkflow(workflow, policy.reasons);
  }
  const pending = createPendingAiAction(validation.value, policy, options);
  const proposedInput = validation.value.proposedInput;
  const search = {
    capability: "filesystem.find_file_candidates" as const,
    targetPeerRef: String(proposedInput?.targetPeerRef ?? ""),
    filenameHint: String((proposedInput?.query as { filenameHint?: unknown } | undefined)?.filenameHint ?? ""),
    searchMode: "filename_metadata_only" as const,
  };
  return okWorkflow({
    ...workflow,
    searchPendingAction: pending,
    snapshot: withEvent({
      ...workflow.snapshot,
      state: "search_preview_ready",
      search,
      errors: undefined,
    }, "ai_search_advisory_validated"),
  });
}

export function confirmCandidatePayloadWorkflowSearch(
  workflow: CandidatePayloadWorkflow,
  options: { now?: Date; ttlMs?: number; sourceDeviceRef?: string; nonce?: string; requestId?: string } = {},
): CandidatePayloadWorkflowRequestResult<FileCandidateRequest> {
  if (workflow.snapshot.state !== "search_preview_ready" || !workflow.searchPendingAction) {
    return failWorkflowWithRequest(workflow, "Candidate payload workflow search must be preview-ready before local confirmation.");
  }
  const confirmed = confirmPendingAiAction(workflow.searchPendingAction, options.now);
  const built = buildFileCandidateRequestFromPendingAction(confirmed, options);
  if (!built.ok) {
    return failWorkflowWithRequest(workflow, built.errors);
  }
  const next = {
    ...workflow,
    searchPendingAction: confirmed,
    searchRequest: built.request,
    snapshot: withEvent({
      ...workflow.snapshot,
      state: "search_pending_receiver_consent",
      search: {
        capability: "filesystem.find_file_candidates" as const,
        requestId: built.request.requestId,
        targetPeerRef: built.request.targetPeerRef,
        filenameHint: built.request.input.query.filenameHint,
        searchMode: built.request.input.query.searchMode,
      },
      errors: undefined,
    }, "local_search_confirmed"),
  };
  return { ok: true, workflow: next, request: built.request };
}

export function markCandidatePayloadWorkflowSearchAllowed(
  workflow: CandidatePayloadWorkflow,
): CandidatePayloadWorkflowResult {
  if (workflow.snapshot.state !== "search_pending_receiver_consent") {
    return failWorkflow(workflow, "Candidate payload workflow search must be pending receiver consent before Allow once.");
  }
  return okWorkflow({
    ...workflow,
    snapshot: withEvent(workflow.snapshot, "receiver_search_allowed"),
  });
}

export function receiveCandidatePayloadWorkflowSearchResult(
  workflow: CandidatePayloadWorkflow,
  result: FileCandidateExecutionResult,
): CandidatePayloadWorkflowResult {
  if (
    workflow.snapshot.state !== "search_pending_receiver_consent" &&
    workflow.snapshot.state !== "search_completed_candidates_ready"
  ) {
    return failWorkflow(workflow, "Candidate payload workflow cannot receive candidates before search consent.");
  }
  rejectForbiddenPublicFields(result);
  const validation = validateFileCandidateExecutionResult(result);
  if (!validation.valid) {
    return failWorkflow(workflow, validation.errors);
  }
  if (!workflow.searchRequest || result.requestId !== workflow.searchRequest.requestId) {
    return failWorkflow(workflow, "Candidate payload workflow search result must match the active search request.");
  }
  if (result.status !== "completed") {
    return failWorkflow(workflow, `Candidate payload workflow search did not complete: ${result.status}.`);
  }
  const candidates = result.candidates.map((candidate) => safeWorkflowCandidate(result.requestId, candidate));
  const completed = {
    ...workflow,
    snapshot: withEvent({
      ...workflow.snapshot,
      state: "search_completed_candidates_ready",
      candidates,
      errors: undefined,
    }, "candidate_results_received"),
  };
  return okWorkflow({
    ...completed,
    snapshot: {
      ...completed.snapshot,
      state: "candidate_selection_required",
    },
  });
}

export function buildCandidatePayloadWorkflowPayloadPreview(
  workflow: CandidatePayloadWorkflow,
  selection: { candidateId: string; selectedByUser: boolean },
  context: AiContextSnapshot,
  options: { now?: Date; ttlMs?: number; sourceDeviceRef?: string; nonce?: string; requestId?: string; pendingId?: string } = {},
): CandidatePayloadWorkflowRequestResult<CandidatePayloadRequest> {
  if (workflow.snapshot.state !== "candidate_selection_required" || !workflow.searchRequest) {
    return failWorkflowWithRequest(workflow, "Candidate payload workflow requires completed search results before payload preview.");
  }
  if (!selection.selectedByUser) {
    return failWorkflowWithRequest(workflow, "Candidate payload workflow requires explicit user candidate selection.");
  }
  const selected = workflow.snapshot.candidates?.find((candidate) => candidate.candidateId === selection.candidateId);
  if (!selected) {
    return failWorkflowWithRequest(workflow, "Candidate payload workflow selected candidate must come from the active search result.");
  }
  if (selected.sourceRequestId !== workflow.searchRequest.requestId) {
    return failWorkflowWithRequest(workflow, "Candidate payload workflow selected candidate must bind the active source request.");
  }
  assertExactCapability({ expected: SEARCH_MANIFEST.capability, actual: workflow.searchRequest.capability });
  const plan = buildPayloadPlanFromSelectedCandidate(workflow.searchRequest.targetPeerRef, selected);
  assertExactCapability({
    expected: PAYLOAD_MANIFEST.capability,
    actual: String(plan.proposedInput?.capability ?? ""),
  });
  const validation = validateAiActionPlan(plan);
  if (!validation.valid) {
    return failWorkflowWithRequest(workflow, validation.errors);
  }
  const policy = evaluateAiPolicy(validation.value, context);
  if (policy.status !== "accepted") {
    return failWorkflowWithRequest(workflow, policy.reasons);
  }
  const pending = createPendingAiAction(validation.value, policy, {
    now: options.now,
    ttlMs: options.ttlMs,
    pendingId: options.pendingId,
  });
  const confirmed = confirmPendingAiAction(pending, options.now);
  const built = buildCandidatePayloadRequestFromPendingAction(confirmed, options);
  if (!built.ok) {
    return failWorkflowWithRequest(workflow, built.errors);
  }
  const payload = {
    capability: "transfer.request_candidate_payload" as const,
    requestId: built.request.requestId,
    targetPeerRef: built.request.targetPeerRef,
    sourceCapability: built.request.input.sourceCapability,
    sourceRequestId: built.request.input.sourceRequestId,
    candidateId: built.request.input.candidateId,
    candidateKind: built.request.input.candidateKind,
    candidateDisplayName: built.request.input.candidateDisplayName,
  };
  const next = {
    ...workflow,
    payloadPendingAction: confirmed,
    payloadRequest: built.request,
    snapshot: withEvent(withEvent({
      ...workflow.snapshot,
      state: "payload_preview_ready",
      selectedCandidate: selected,
      payload,
      errors: undefined,
    }, "candidate_selected_by_user"), "payload_request_preview_created"),
  };
  return { ok: true, workflow: next, request: built.request };
}

export function markCandidatePayloadWorkflowPayloadPendingConsent(
  workflow: CandidatePayloadWorkflow,
): CandidatePayloadWorkflowResult {
  if (workflow.snapshot.state !== "payload_preview_ready") {
    return failWorkflow(workflow, "Candidate payload workflow payload preview must be ready before receiver consent.");
  }
  return okWorkflow({
    ...workflow,
    snapshot: {
      ...workflow.snapshot,
      state: "payload_pending_receiver_consent",
    },
  });
}

export function markCandidatePayloadWorkflowPayloadAllowed(
  workflow: CandidatePayloadWorkflow,
): CandidatePayloadWorkflowResult {
  if (workflow.snapshot.state !== "payload_pending_receiver_consent") {
    return failWorkflow(workflow, "Candidate payload workflow payload request must be pending receiver consent before Allow once.");
  }
  return okWorkflow({
    ...workflow,
    snapshot: withEvent(workflow.snapshot, "receiver_payload_allowed"),
  });
}

export function receiveCandidatePayloadWorkflowHandoffResult(
  workflow: CandidatePayloadWorkflow,
  result: CandidatePayloadExecutionResult,
): CandidatePayloadWorkflowResult {
  if (
    workflow.snapshot.state !== "payload_pending_receiver_consent" &&
    workflow.snapshot.state !== "payload_preview_ready"
  ) {
    return failWorkflow(workflow, "Candidate payload workflow cannot receive handoff result before payload request.");
  }
  rejectForbiddenPublicFields(result);
  const validation = validateCandidatePayloadExecutionResult(result);
  if (!validation.valid) {
    return failWorkflow(workflow, validation.errors);
  }
  if (!workflow.payloadRequest || result.requestId !== workflow.payloadRequest.requestId) {
    return failWorkflow(workflow, "Candidate payload workflow handoff result must match the active payload request.");
  }
  if (result.status !== "handoff_queued") {
    return failWorkflow(workflow, `Candidate payload workflow handoff did not queue: ${result.status}.`);
  }
  return okWorkflow({
    ...workflow,
    snapshot: withEvent({
      ...workflow.snapshot,
      state: "handoff_queued",
      handoff: {
        status: "handoff_queued",
        transferredBytes: 0,
        handoffQueued: true,
        transferStatus: "queued",
      },
      errors: undefined,
    }, "candidate_payload_handoff_queued"),
  });
}

function buildPayloadPlanFromSelectedCandidate(
  targetPeerRef: string,
  selected: CandidatePayloadWorkflowCandidate,
): AiActionPlan {
  return {
    schemaVersion: "ai-action-plan-v1",
    kind: PAYLOAD_MANIFEST.providerActionKind,
    title: "Request selected candidate payload",
    explanation: "Request selected candidate payload from peer. Receiver must Allow once before handoff.",
    confidence: "medium",
    requiresUserConfirmation: true,
    references: [
      { kind: "peer", ref: targetPeerRef },
      { kind: "transfer", ref: selected.sourceRequestId },
    ],
    proposedInput: {
      capability: PAYLOAD_MANIFEST.capability,
      targetPeerRef,
      sourceCapability: SEARCH_MANIFEST.capability,
      sourceRequestId: selected.sourceRequestId,
      candidateId: selected.candidateId,
      candidateDisplayName: selected.candidateDisplayName,
      candidateKind: selected.candidateKind,
      redactedLocation: selected.redactedLocation,
      sizeBytes: selected.sizeBytes,
      modifiedAt: selected.modifiedAt,
      mimeFamily: selected.mimeFamily,
      extension: selected.extension,
    },
  };
}

function safeWorkflowCandidate(
  sourceRequestId: string,
  candidate: FileCandidateMetadata,
): CandidatePayloadWorkflowCandidate {
  const value = {
    sourceRequestId,
    candidateId: candidate.candidateId,
    candidateKind: "filesystem_file" as const,
    candidateDisplayName: candidate.displayName,
    redactedLocation: candidate.redactedLocation,
    extension: candidate.extension,
    mimeFamily: candidate.mimeFamily,
    sizeBytes: candidate.sizeBytes,
    modifiedAt: candidate.modifiedAt,
    matchReason: candidate.matchReason,
    confidence: candidate.confidence,
  };
  rejectForbiddenPublicFields(value);
  return value;
}

function okWorkflow(workflow: CandidatePayloadWorkflow): CandidatePayloadWorkflowResult {
  rejectForbiddenPublicFields(workflow.snapshot);
  return { ok: true, workflow };
}

function failWorkflow(
  workflow: CandidatePayloadWorkflow,
  errors: string | readonly string[],
): CandidatePayloadWorkflowResult {
  const next = failedWorkflow(workflow, errors);
  return { ok: false, workflow: next, errors: [...(next.snapshot.errors ?? [])] };
}

function failWorkflowWithRequest<TRequest>(
  workflow: CandidatePayloadWorkflow,
  errors: string | readonly string[],
): CandidatePayloadWorkflowRequestResult<TRequest> {
  const next = failedWorkflow(workflow, errors);
  return { ok: false, workflow: next, errors: [...(next.snapshot.errors ?? [])] };
}

function failedWorkflow(
  workflow: CandidatePayloadWorkflow,
  errors: string | readonly string[],
): CandidatePayloadWorkflow {
  const normalizedErrors = typeof errors === "string" ? [errors] : [...errors];
  return {
    ...workflow,
    snapshot: withEvent({
      ...workflow.snapshot,
      state: "failed",
      errors: normalizedErrors,
    }, "workflow_failed"),
  };
}

function withEvent(
  snapshot: CandidatePayloadWorkflowSnapshot,
  event: CandidatePayloadWorkflowEvent,
): CandidatePayloadWorkflowSnapshot {
  return {
    ...snapshot,
    events: [...snapshot.events, event],
  };
}

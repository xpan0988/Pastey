import {
  buildCapabilityExecuteRequestControlEvent,
  buildCapabilityExecutionResultControlEvent,
  validateRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityExecutionResultRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type RoomControlEventBuildResult,
} from "./roomControlEvent";
import {
  validatePeerConsentRecord,
  type PeerConsentRecord,
} from "./peerConsent";
import type { PeerConsentConsumptionState } from "./helloPeerExecution";
import {
  CANDIDATE_PAYLOAD_CAPABILITY,
  CANDIDATE_PAYLOAD_EXECUTOR_KIND,
  FILE_CANDIDATES_CAPABILITY,
} from "../ai/capabilityRegistry";
import {
  CANDIDATE_PAYLOAD_EXECUTION_REQUEST_SCHEMA,
  CANDIDATE_PAYLOAD_RESULT_SCHEMA,
  buildCandidatePayloadResultCandidate,
  type CandidatePayloadExecutionRequest,
  type CandidatePayloadExecutionResult,
  type CandidatePayloadResolution,
} from "../ai/candidatePayloadRequest";

export type CandidatePayloadExecutionBuildResult =
  | { ok: true; request: CandidatePayloadExecutionRequest; event: CapabilityExecuteRequestRoomControlEvent }
  | { ok: false; errors: string[] };

export type CandidatePayloadExecutionPolicyResult = {
  state: PeerConsentConsumptionState;
  result: CandidatePayloadExecutionResult;
  resultEvent: CapabilityExecutionResultRoomControlEvent;
  executed: boolean;
};

export type CandidatePayloadResolver = (
  request: CandidatePayloadExecutionRequest,
) => Promise<CandidatePayloadLocalResolution>;

export interface CandidatePayloadLocalResolution extends CandidatePayloadResolution {
  receiverLocalSource?: string;
}

export type CandidatePayloadHandoffResult =
  | { queued: true }
  | { queued: false; errorCode?: "handoff_failed" | "unsupported_route" | "policy_rejected" };

export type CandidatePayloadHandoff = (
  request: CandidatePayloadExecutionRequest,
  resolution: CandidatePayloadLocalResolution,
) => Promise<CandidatePayloadHandoffResult>;

let executionSequence = 0;

export function buildCandidatePayloadExecutionRequest(
  preview: CapabilityPreviewRoomControlEvent,
  acknowledgement: CapabilityPreviewAckRoomControlEvent,
  options: { now?: Date; ttlMs?: number; executionId?: string; eventId?: string } = {},
): CandidatePayloadExecutionBuildResult {
  const now = options.now ?? new Date();
  const errors: string[] = [];
  const previewValidation = validateRoomControlEvent(preview, { now });
  const ackValidation = validateRoomControlEvent(acknowledgement, { now });
  const candidatePayloadRequest = preview.payload.request.capability === CANDIDATE_PAYLOAD_CAPABILITY
    ? preview.payload.request
    : null;
  if (!previewValidation.valid || previewValidation.value.kind !== "capability_preview") {
    errors.push("Candidate payload execution request requires a valid source capability preview.");
  }
  if (!ackValidation.valid || ackValidation.value.kind !== "capability_preview_ack") {
    errors.push("Candidate payload execution request requires a valid capability preview acknowledgement.");
  }
  const consent = acknowledgement.payload.consent;
  if (!consent) {
    errors.push("Candidate payload execution request requires an exact allow-once consent grant.");
  } else if (
    acknowledgement.roomRef !== preview.roomRef ||
    acknowledgement.sourceDeviceRef !== preview.targetPeerRef ||
    acknowledgement.targetPeerRef !== preview.sourceDeviceRef ||
    consent.sourcePreviewEventId !== preview.eventId ||
    consent.envelopeId !== preview.payload.envelopeId ||
    consent.requestId !== preview.payload.request.requestId ||
    consent.requestPayloadHash !== preview.payload.request.requestPayloadHash ||
    consent.capability !== CANDIDATE_PAYLOAD_CAPABILITY ||
    !candidatePayloadRequest ||
    !("sourceCapability" in consent) ||
    consent.sourceCapability !== FILE_CANDIDATES_CAPABILITY ||
    consent.sourceRequestId !== candidatePayloadRequest.input.sourceRequestId ||
    consent.candidateId !== candidatePayloadRequest.input.candidateId ||
    consent.candidateKind !== candidatePayloadRequest.input.candidateKind ||
    consent.candidateDisplayName !== candidatePayloadRequest.input.candidateDisplayName
  ) {
    errors.push("Candidate payload execution request consent grant does not match the exact preview/ack chain.");
  }
  if (!candidatePayloadRequest) {
    errors.push("Candidate payload execution request requires a candidate payload preview.");
  }
  if (consent && Date.parse(consent.expiresAt) <= now.getTime()) {
    errors.push("Candidate payload execution request consent grant is expired.");
  }
  if (errors.length > 0 || !consent || !candidatePayloadRequest) {
    return { ok: false, errors: unique(errors) };
  }
  const ttlMs = options.ttlMs ?? 60_000;
  const request: CandidatePayloadExecutionRequest = {
    schemaVersion: CANDIDATE_PAYLOAD_EXECUTION_REQUEST_SCHEMA,
    executionId: options.executionId ?? createExecutionId(now),
    consentId: consent.consentId,
    sourcePreviewEventId: consent.sourcePreviewEventId,
    envelopeId: consent.envelopeId,
    requestId: consent.requestId,
    requestPayloadHash: consent.requestPayloadHash,
    roomRef: preview.roomRef,
    sourceDeviceRef: preview.sourceDeviceRef,
    targetPeerRef: preview.targetPeerRef,
    capability: CANDIDATE_PAYLOAD_CAPABILITY,
    executorKind: CANDIDATE_PAYLOAD_EXECUTOR_KIND,
    sourceCapability: FILE_CANDIDATES_CAPABILITY,
    sourceRequestId: candidatePayloadRequest.input.sourceRequestId,
    candidateId: candidatePayloadRequest.input.candidateId,
    candidateKind: candidatePayloadRequest.input.candidateKind,
    candidateDisplayName: candidatePayloadRequest.input.candidateDisplayName,
    createdAt: now.toISOString(),
    expiresAt: new Date(Math.min(now.getTime() + ttlMs, Date.parse(consent.expiresAt))).toISOString(),
  };
  const event = buildCapabilityExecuteRequestControlEvent(request, {
    now,
    eventId: options.eventId,
  });
  if (!event.ok || event.event.kind !== "capability_execute_request") {
    return { ok: false, errors: event.ok ? ["Execution request builder produced the wrong event kind."] : event.errors };
  }
  return { ok: true, request, event: event.event };
}

export async function executeInboundCandidatePayloadRequest(
  event: CapabilityExecuteRequestRoomControlEvent,
  consent: PeerConsentRecord | undefined,
  state: PeerConsentConsumptionState,
  resolver: CandidatePayloadResolver,
  handoff: CandidatePayloadHandoff,
  context: {
    roomRef: string;
    sourceDeviceRef: string;
    targetPeerRef: string;
    now?: Date;
    resultEventId?: string;
  },
): Promise<CandidatePayloadExecutionPolicyResult> {
  const now = context.now ?? new Date();
  const request = event.payload as CandidatePayloadExecutionRequest;
  const validation = validateRoomControlEvent(event, {
    now,
    expectedRoomRef: context.roomRef,
    expectedSourceDeviceRef: context.sourceDeviceRef,
    expectedTargetPeerRef: context.targetPeerRef,
  });
  let status: CandidatePayloadExecutionResult["status"] = "rejected";
  let errorCode: NonNullable<CandidatePayloadExecutionResult["errorCode"]> = "policy_rejected";

  if (!validation.valid || validation.value.kind !== "capability_execute_request" || request.capability !== CANDIDATE_PAYLOAD_CAPABILITY) {
    errorCode = "malformed_request";
  } else if (
    state.consumedExecutionIds.includes(request.executionId) ||
    state.consumedConsentIds.includes(request.consentId) ||
    state.consumedRequestIds.includes(request.requestId)
  ) {
    status = "already_consumed";
    errorCode = "already_consumed";
  } else if (!consent) {
    errorCode = "missing_consent";
  } else {
    const consentValidation = validatePeerConsentRecord(consent, { now });
    if (!consentValidation.valid) {
      status = Date.parse(consent.binding.expiresAt) <= now.getTime() ? "expired" : "rejected";
      errorCode = status === "expired" ? "consent_expired" : "invalid_consent";
    } else if (consent.status !== "allowed_once" || consent.decision !== "allow_once") {
      errorCode = "consent_not_allowed_once";
    } else if (!requestMatchesConsent(request, consent)) {
      errorCode = "consent_binding_mismatch";
    } else {
      const consumed = consume(state, request);
      try {
        const candidateResolution = await resolver(request);
        if (candidateResolution.resolved && candidateResolution.reason === "resolved") {
          const handoffResult = await handoff(request, candidateResolution);
          return buildPolicyResult(
            event,
            consumed,
            resolvedResult(request, candidateResolution, now, handoffResult),
            true,
            context.resultEventId,
            now,
          );
        }
        return buildPolicyResult(
          event,
          consumed,
          resolvedResult(request, candidateResolution, now),
          true,
          context.resultEventId,
          now,
        );
      } catch {
        return buildPolicyResult(
          event,
          consumed,
          failureResult(request, "failed", "policy_rejected", now),
          true,
          context.resultEventId,
          now,
        );
      }
    }
  }
  return buildPolicyResult(event, state, failureResult(request, status, errorCode, now), false, context.resultEventId, now);
}

function requestMatchesConsent(request: CandidatePayloadExecutionRequest, consent: PeerConsentRecord): boolean {
  const binding = consent.binding;
  return binding.capability === CANDIDATE_PAYLOAD_CAPABILITY
    && request.consentId === binding.consentId
    && request.sourcePreviewEventId === binding.sourceEventId
    && request.envelopeId === binding.envelopeId
    && request.requestId === binding.requestId
    && request.requestPayloadHash === binding.requestPayloadHash
    && request.roomRef === binding.roomRef
    && request.sourceDeviceRef === binding.sourceDeviceRef
    && request.targetPeerRef === binding.targetPeerRef
    && request.capability === binding.capability
    && request.sourceCapability === binding.sourceCapability
    && request.sourceRequestId === binding.sourceRequestId
    && request.candidateId === binding.candidateId
    && request.candidateKind === binding.candidateKind
    && request.candidateDisplayName === binding.candidateDisplayName
    && Date.parse(request.expiresAt) <= Date.parse(binding.expiresAt);
}

function consume(
  state: PeerConsentConsumptionState,
  request: CandidatePayloadExecutionRequest,
): PeerConsentConsumptionState {
  return {
    consumedConsentIds: [...state.consumedConsentIds, request.consentId],
    consumedRequestIds: [...state.consumedRequestIds, request.requestId],
    consumedExecutionIds: [...state.consumedExecutionIds, request.executionId],
  };
}

function resolvedResult(
  request: CandidatePayloadExecutionRequest,
  candidateResolution: CandidatePayloadResolution,
  now: Date,
  handoffResult?: CandidatePayloadHandoffResult,
): CandidatePayloadExecutionResult {
  const {
    receiverLocalSource: _receiverLocalSource,
    ...publicCandidateResolution
  } = candidateResolution as CandidatePayloadLocalResolution;
  const status: CandidatePayloadExecutionResult["status"] = candidateResolution.resolved && candidateResolution.reason === "resolved"
    ? handoffResult?.queued
      ? "handoff_queued"
      : handoffResult
        ? "handoff_failed"
        : "candidate_resolved_handoff_not_implemented"
    : candidateResolution.reason === "not_found"
      ? "candidate_not_found"
      : candidateResolution.reason === "expired"
        ? "candidate_expired"
        : candidateResolution.reason === "changed"
          ? "candidate_changed"
          : "rejected";
  return {
    schemaVersion: CANDIDATE_PAYLOAD_RESULT_SCHEMA,
    capability: CANDIDATE_PAYLOAD_CAPABILITY,
    executionId: request.executionId,
    requestId: request.requestId,
    consentId: request.consentId,
    status,
    candidate: buildCandidatePayloadResultCandidate({
      ...request,
      ...(candidateResolution.displayName ? { candidateDisplayName: candidateResolution.displayName } : {}),
      ...(candidateResolution.sizeBytes !== undefined ? { sizeBytes: candidateResolution.sizeBytes } : {}),
      ...(candidateResolution.mimeFamily !== undefined ? { mimeFamily: candidateResolution.mimeFamily } : {}),
      ...(candidateResolution.extension !== undefined ? { extension: candidateResolution.extension } : {}),
    }),
    candidateResolution: publicCandidateResolution,
    transferredBytes: 0,
    handoffQueued: status === "handoff_queued",
    ...(status === "handoff_queued" ? { transferStatus: "queued" as const } : {}),
    errorCode: status === "rejected"
      ? "consent_binding_mismatch"
      : status === "handoff_failed"
        ? handoffResult && !handoffResult.queued ? handoffResult.errorCode ?? "handoff_failed" : "handoff_failed"
        : null,
    createdAt: now.toISOString(),
  };
}

function failureResult(
  request: CandidatePayloadExecutionRequest,
  status: Exclude<CandidatePayloadExecutionResult["status"], "handoff_not_implemented" | "handoff_queued">,
  errorCode: NonNullable<CandidatePayloadExecutionResult["errorCode"]>,
  now: Date,
): CandidatePayloadExecutionResult {
  return {
    schemaVersion: CANDIDATE_PAYLOAD_RESULT_SCHEMA,
    capability: CANDIDATE_PAYLOAD_CAPABILITY,
    executionId: request.executionId,
    requestId: request.requestId,
    consentId: request.consentId,
    status,
    candidate: {
      candidateId: request.candidateId ?? "unknown-candidate",
      candidateKind: request.candidateKind ?? "filesystem_file",
      candidateDisplayName: request.candidateDisplayName ?? "unknown",
    },
    transferredBytes: 0,
    handoffQueued: false,
    errorCode,
    createdAt: now.toISOString(),
  };
}

function buildPolicyResult(
  requestEvent: CapabilityExecuteRequestRoomControlEvent,
  state: PeerConsentConsumptionState,
  result: CandidatePayloadExecutionResult,
  executed: boolean,
  eventId: string | undefined,
  now: Date,
): CandidatePayloadExecutionPolicyResult {
  const built: RoomControlEventBuildResult = buildCapabilityExecutionResultControlEvent(
    result,
    requestEvent,
    { now, eventId },
  );
  if (!built.ok || built.event.kind !== "capability_execution_result") {
    throw new Error("Candidate payload execution result construction failed.");
  }
  return { state, result, resultEvent: built.event, executed };
}

function createExecutionId(now: Date): string {
  executionSequence += 1;
  return `candidate-payload-execution-${globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${executionSequence}`}`;
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

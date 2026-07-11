import {
  buildCapabilityExecuteRequestControlEvent,
  buildCapabilityExecutionResultControlEvent,
  validateRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityExecutionResultRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
} from "./roomControlEvent";
import { validatePeerConsentRecord, type PeerConsentRecord } from "./peerConsent";
import type { PeerConsentConsumptionState } from "./helloPeerExecution";
import {
  ARTIFACT_TRANSFORM_CAPABILITY,
  ARTIFACT_TRANSFORM_EXECUTION_REQUEST_SCHEMA,
  ARTIFACT_TRANSFORM_RESULT_SCHEMA,
  validateArtifactTransformExecutionResult,
  type ArtifactTransformExecutionRequest,
  type ArtifactTransformExecutionResult,
} from "../ai/artifactTransformRequest";

export type TransformExecutor = (request: ArtifactTransformExecutionRequest) => Promise<ArtifactTransformExecutionResult>;
export type TransformCandidateClaim = (request: ArtifactTransformExecutionRequest) => Promise<"claimed" | "candidate_not_found" | "candidate_expired" | "candidate_changed" | "candidate_claimed">;

export interface TransformExecutionPolicyResult {
  state: PeerConsentConsumptionState;
  result: ArtifactTransformExecutionResult;
  resultEvent: CapabilityExecutionResultRoomControlEvent;
  executed: boolean;
}

let sequence = 0;

/** Production executor: it deliberately performs no process, runtime, or sandbox fallback. */
export const unavailableTransformExecutor: TransformExecutor = async (request) => failure(request, "rejected", "sandbox_unavailable");

export function buildArtifactTransformExecutionRequest(
  preview: CapabilityPreviewRoomControlEvent,
  acknowledgement: CapabilityPreviewAckRoomControlEvent,
  options: { now?: Date; ttlMs?: number; executionId?: string; eventId?: string } = {},
): { ok: true; request: ArtifactTransformExecutionRequest; event: CapabilityExecuteRequestRoomControlEvent } | { ok: false; errors: string[] } {
  const now = options.now ?? new Date();
  const request = preview.payload.request;
  const consent = acknowledgement.payload.consent;
  const transformConsent = consent?.capability === ARTIFACT_TRANSFORM_CAPABILITY ? consent : null;
  const errors: string[] = [];
  if (request.capability !== ARTIFACT_TRANSFORM_CAPABILITY) errors.push("Artifact Transform requires an Artifact Transform preview.");
  if (!transformConsent) errors.push("Artifact Transform requires exact allow-once Transform consent.");
  if (!transformConsent || request.capability !== ARTIFACT_TRANSFORM_CAPABILITY) return { ok: false, errors };
  if (
    acknowledgement.roomRef !== preview.roomRef || acknowledgement.sourceDeviceRef !== preview.targetPeerRef || acknowledgement.targetPeerRef !== preview.sourceDeviceRef ||
    transformConsent.sourcePreviewEventId !== preview.eventId || transformConsent.envelopeId !== preview.payload.envelopeId || transformConsent.requestId !== request.requestId ||
    transformConsent.requestPayloadHash !== request.requestPayloadHash || transformConsent.sourceCapability !== request.sourceCapability ||
    transformConsent.sourceRequestId !== request.sourceRequestId || transformConsent.candidateId !== request.candidateId ||
    transformConsent.candidateKind !== request.candidateKind || transformConsent.resultContract !== request.resultContract || Date.parse(transformConsent.expiresAt) <= now.getTime()
  ) errors.push("Artifact Transform consent grant does not match the exact preview chain.");
  if (errors.length) return { ok: false, errors: [...new Set(errors)] };
  const execution: ArtifactTransformExecutionRequest = {
    schemaVersion: ARTIFACT_TRANSFORM_EXECUTION_REQUEST_SCHEMA,
    executionId: options.executionId ?? id("artifact-transform-execution", now),
    consentId: transformConsent.consentId,
    sourcePreviewEventId: transformConsent.sourcePreviewEventId,
    envelopeId: transformConsent.envelopeId,
    requestId: transformConsent.requestId,
    requestPayloadHash: transformConsent.requestPayloadHash,
    roomRef: preview.roomRef,
    sourceDeviceRef: preview.sourceDeviceRef,
    targetPeerRef: preview.targetPeerRef,
    capability: ARTIFACT_TRANSFORM_CAPABILITY,
    sourceCapability: request.sourceCapability,
    sourceRequestId: request.sourceRequestId,
    candidateId: request.candidateId,
    candidateKind: request.candidateKind,
    resultContract: request.resultContract,
    createdAt: now.toISOString(),
    expiresAt: new Date(Math.min(now.getTime() + (options.ttlMs ?? 60_000), Date.parse(transformConsent.expiresAt))).toISOString(),
  };
  const event = buildCapabilityExecuteRequestControlEvent(execution, { now, eventId: options.eventId });
  return event.ok && event.event.kind === "capability_execute_request"
    ? { ok: true, request: execution, event: event.event }
    : { ok: false, errors: event.ok ? ["Artifact Transform execution event is invalid."] : event.errors };
}

export async function executeInboundArtifactTransformRequest(
  event: CapabilityExecuteRequestRoomControlEvent,
  consent: PeerConsentRecord | undefined,
  state: PeerConsentConsumptionState,
  claim: TransformCandidateClaim,
  executor: TransformExecutor,
  context: { roomRef: string; sourceDeviceRef: string; targetPeerRef: string; now?: Date; resultEventId?: string },
): Promise<TransformExecutionPolicyResult> {
  const now = context.now ?? new Date();
  const request = event.payload as ArtifactTransformExecutionRequest;
  const valid = validateRoomControlEvent(event, { now, expectedRoomRef: context.roomRef, expectedSourceDeviceRef: context.sourceDeviceRef, expectedTargetPeerRef: context.targetPeerRef });
  if (!valid.valid || request.capability !== ARTIFACT_TRANSFORM_CAPABILITY) return result(event, state, failure(request, "rejected", "malformed_request"), false, context.resultEventId, now);
  if (state.consumedExecutionIds.includes(request.executionId) || state.consumedConsentIds.includes(request.consentId) || state.consumedRequestIds.includes(request.requestId)) return result(event, state, failure(request, "already_consumed", "already_consumed"), false, context.resultEventId, now);
  if (!consent) return result(event, state, failure(request, "rejected", "missing_consent"), false, context.resultEventId, now);
  const consentValidation = validatePeerConsentRecord(consent, { now });
  if (!consentValidation.valid) return result(event, state, failure(request, Date.parse(consent.binding.expiresAt) <= now.getTime() ? "expired" : "rejected", Date.parse(consent.binding.expiresAt) <= now.getTime() ? "consent_expired" : "invalid_consent"), false, context.resultEventId, now);
  if (consent.decision !== "allow_once" || consent.status !== "allowed_once") return result(event, state, failure(request, "rejected", "consent_not_allowed_once"), false, context.resultEventId, now);
  if (!matches(request, consent)) return result(event, state, failure(request, "rejected", "consent_binding_mismatch"), false, context.resultEventId, now);
  const consumed = consume(state, request);
  const claimResult = await claim(request);
  if (claimResult !== "claimed") return result(event, consumed, failure(request, claimResult === "candidate_expired" ? "expired" : claimResult === "candidate_claimed" ? "already_consumed" : "rejected", claimResult), true, context.resultEventId, now);
  try {
    const transformed = await executor(request);
    const validated = validateArtifactTransformExecutionResult(transformed);
    if (!validated.valid || transformed.executionId !== request.executionId || transformed.requestId !== request.requestId || transformed.consentId !== request.consentId) return result(event, consumed, failure(request, "failed", "invalid_executor_result"), true, context.resultEventId, now);
    return result(event, consumed, validated.value, true, context.resultEventId, now);
  } catch {
    return result(event, consumed, failure(request, "failed", "executor_failed"), true, context.resultEventId, now);
  }
}

function matches(request: ArtifactTransformExecutionRequest, consent: PeerConsentRecord): boolean {
  const binding = consent.binding;
  return binding.capability === ARTIFACT_TRANSFORM_CAPABILITY && request.consentId === binding.consentId && request.sourcePreviewEventId === binding.sourceEventId && request.envelopeId === binding.envelopeId && request.requestId === binding.requestId && request.requestPayloadHash === binding.requestPayloadHash && request.roomRef === binding.roomRef && request.sourceDeviceRef === binding.sourceDeviceRef && request.targetPeerRef === binding.targetPeerRef && request.sourceCapability === binding.sourceCapability && request.sourceRequestId === binding.sourceRequestId && request.candidateId === binding.candidateId && request.candidateKind === binding.candidateKind && request.resultContract === binding.resultContract && Date.parse(request.expiresAt) <= Date.parse(binding.expiresAt);
}
function consume(state: PeerConsentConsumptionState, request: ArtifactTransformExecutionRequest): PeerConsentConsumptionState { return { consumedConsentIds: [...state.consumedConsentIds, request.consentId], consumedRequestIds: [...state.consumedRequestIds, request.requestId], consumedExecutionIds: [...state.consumedExecutionIds, request.executionId] }; }
function failure(request: Pick<ArtifactTransformExecutionRequest, "executionId" | "requestId" | "consentId">, status: Exclude<ArtifactTransformExecutionResult["status"], "completed">, errorCode: ArtifactTransformExecutionResult["errorCode"]): ArtifactTransformExecutionResult { return { schemaVersion: ARTIFACT_TRANSFORM_RESULT_SCHEMA, capability: ARTIFACT_TRANSFORM_CAPABILITY, executionId: request.executionId, requestId: request.requestId, consentId: request.consentId, status, errorCode, createdAt: new Date().toISOString() }; }
function result(event: CapabilityExecuteRequestRoomControlEvent, state: PeerConsentConsumptionState, value: ArtifactTransformExecutionResult, executed: boolean, eventId: string | undefined, now: Date): TransformExecutionPolicyResult { const built = buildCapabilityExecutionResultControlEvent(value, event, { now, eventId }); if (!built.ok || built.event.kind !== "capability_execution_result") throw new Error(built.ok ? "Artifact Transform result event is invalid." : built.errors.join(" ")); return { state, result: value, resultEvent: built.event, executed }; }
function id(prefix: string, now: Date) { sequence += 1; return `${prefix}-${globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${sequence}`}`; }

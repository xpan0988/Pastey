import {
  buildCapabilityExecuteRequestControlEvent,
  buildCapabilityExecutionResultControlEvent,
  validateRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityExecutionResultRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
} from "./roomControlEvent";
import { invoke } from "@tauri-apps/api/core";
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

export type TransformLifecyclePhase = "prepared" | "claimed" | "revalidated" | "executor_started" | "completed";
export type TransformLeaseStatus = "leased" | "already_leased" | "candidate_not_found" | "candidate_expired" | "candidate_changed" | "candidate_claimed";

/**
 * This boundary deliberately contains no receiver-local handle. The host owns the lease,
 * path, digest, identity, and any future staging area; TypeScript receives status only.
 */
export interface TransformLeaseHost {
  acquire(request: ArtifactTransformExecutionRequest): Promise<TransformLeaseStatus>;
  revalidate(request: ArtifactTransformExecutionRequest): Promise<"revalidated" | "candidate_not_found" | "candidate_expired" | "candidate_changed" | "candidate_claimed">;
  release(request: ArtifactTransformExecutionRequest): Promise<void>;
}

export interface TransformExecutor {
  prepare(request: ArtifactTransformExecutionRequest): Promise<{ status: "ready" } | { status: "unavailable"; errorCode: "sandbox_unavailable" }>;
  start(request: ArtifactTransformExecutionRequest): Promise<{ status: "started"; result: ArtifactTransformExecutionResult } | { status: "rejected"; errorCode: "executor_failed" | "timed_out" }>;
}

/**
 * Isolated test seam only.  Production never receives raw completed output: Rust
 * finalizes and transports it before returning bounded delivery metadata.
 */
type TransformResultBoundary = (request: ArtifactTransformExecutionRequest, result: ArtifactTransformExecutionResult) => Promise<
  | { ok: true; result: ArtifactTransformExecutionResult }
  | { ok: false }
  | { ok: true; deliveredByRust: true; terminalCategory: "completed" | "failed" | "timed_out" | "rejected" }
>;

export interface TransformExecutionPolicyResult {
  state: PeerConsentConsumptionState;
  /** Present only for local validation failures or injected test seams. */
  result?: ArtifactTransformExecutionResult;
  /** Never constructed for a production Rust-finalized Transform result. */
  resultEvent?: CapabilityExecutionResultRoomControlEvent;
  /** Rust journal terminal category when Rust owns finalization and transport. */
  terminalCategory?: "completed" | "failed" | "timed_out" | "rejected" | "execution_state_unknown";
  executed: boolean;
  lifecycle: readonly TransformLifecyclePhase[];
}

let sequence = 0;

/** Production executor: it deliberately performs no process, runtime, sandbox, or fallback work. */
export const unavailableTransformExecutor: TransformExecutor = {
  async prepare() { return { status: "unavailable", errorCode: "sandbox_unavailable" }; },
  async start() { return { status: "rejected", errorCode: "executor_failed" }; },
};

/** This exists only to make an accidental lease call on the unavailable production path fail closed. */
export const unavailableTransformLeaseHost: TransformLeaseHost = {
  async acquire() { return "candidate_not_found"; },
  async revalidate() { return "candidate_not_found"; },
  async release() {},
};

const rustBackedTransformResultBoundary: TransformResultBoundary = async (request, result) => {
  const outcome = await invoke<{ terminalCategory: "completed" | "failed" | "timed_out" | "rejected"; sent: boolean }>("finalize_and_send_transform_result", { request, rawResult: {
    status: result.status === "completed" ? "completed" : result.status === "failed" ? "failed" : result.status === "timed_out" ? "timed_out" : "rejected",
    ...(result.result ? { result: result.result } : {}),
    ...(result.errorCode === "executor_failed" || result.errorCode === "invalid_executor_result" || result.errorCode === "policy_rejected" || result.errorCode === "timed_out" ? { errorCode: result.errorCode } : {}),
  }});
  return { ok: true, deliveredByRust: true, terminalCategory: outcome.terminalCategory };
};

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
  leaseHost: TransformLeaseHost,
  executor: TransformExecutor,
  context: { roomRef: string; sourceDeviceRef: string; targetPeerRef: string; now?: Date; resultEventId?: string; resultBoundary?: TransformResultBoundary },
): Promise<TransformExecutionPolicyResult> {
  const now = context.now ?? new Date();
  const request = event.payload as ArtifactTransformExecutionRequest;
  const prepared: readonly TransformLifecyclePhase[] = ["prepared"];
  const valid = validateRoomControlEvent(event, { now, expectedRoomRef: context.roomRef, expectedSourceDeviceRef: context.sourceDeviceRef, expectedTargetPeerRef: context.targetPeerRef });
  if (!valid.valid || request.capability !== ARTIFACT_TRANSFORM_CAPABILITY) return result(event, state, failure(request, "rejected", "malformed_request", now), false, prepared, context.resultEventId, now);
  if (state.consumedExecutionIds.includes(request.executionId) || state.consumedConsentIds.includes(request.consentId) || state.consumedRequestIds.includes(request.requestId)) return result(event, state, failure(request, "already_consumed", "already_consumed", now), false, prepared, context.resultEventId, now);
  if (!consent) return result(event, state, failure(request, "rejected", "missing_consent", now), false, prepared, context.resultEventId, now);
  const consentValidation = validatePeerConsentRecord(consent, { now });
  if (!consentValidation.valid) return result(event, state, failure(request, Date.parse(consent.binding.expiresAt) <= now.getTime() ? "expired" : "rejected", Date.parse(consent.binding.expiresAt) <= now.getTime() ? "consent_expired" : "invalid_consent", now), false, prepared, context.resultEventId, now);
  if (consent.decision !== "allow_once" || consent.status !== "allowed_once") return result(event, state, failure(request, "rejected", "consent_not_allowed_once", now), false, prepared, context.resultEventId, now);
  if (!matches(request, consent)) return result(event, state, failure(request, "rejected", "consent_binding_mismatch", now), false, prepared, context.resultEventId, now);

  // Availability is checked before consuming consent or acquiring a receiver-local lease.
  const preparation = await executor.prepare(request);
  if (preparation.status !== "ready") return result(event, state, failure(request, "rejected", preparation.errorCode, now), false, prepared, context.resultEventId, now);

  const acquired = await leaseHost.acquire(request);
  if (acquired !== "leased" && acquired !== "already_leased") return result(event, state, failure(request, acquired === "candidate_expired" ? "expired" : acquired === "candidate_claimed" ? "already_consumed" : "rejected", acquired, now), false, prepared, context.resultEventId, now);
  const claimed: readonly TransformLifecyclePhase[] = [...prepared, "claimed"];
  const revalidated = await leaseHost.revalidate(request);
  if (revalidated !== "revalidated") {
    await leaseHost.release(request);
    return result(event, state, failure(request, revalidated === "candidate_expired" ? "expired" : revalidated === "candidate_claimed" ? "already_consumed" : "rejected", revalidated, now), false, claimed, context.resultEventId, now);
  }
  const ready: readonly TransformLifecyclePhase[] = [...claimed, "revalidated"];
  try {
    const started = await executor.start(request);
    if (started.status !== "started") {
      await leaseHost.release(request);
      return result(event, state, failure(request, "rejected", started.errorCode, now), false, ready, context.resultEventId, now);
    }
    const consumed = consume(state, request);
    const executionStarted: readonly TransformLifecyclePhase[] = [...ready, "executor_started"];
    const boundary = await (context.resultBoundary ?? rustBackedTransformResultBoundary)(request, started.result);
    if ("deliveredByRust" in boundary) {
      await leaseHost.release(request);
      return {
        state: consumed,
        executed: true,
        lifecycle: boundary.terminalCategory === "completed" ? [...executionStarted, "completed"] : executionStarted,
        terminalCategory: boundary.terminalCategory,
      };
    }
    const transformed = boundary.ok ? boundary.result : failure(request, "failed", "invalid_executor_result", now);
    const validated = validateArtifactTransformExecutionResult(transformed);
    if (!boundary.ok || !validated.valid || transformed.executionId !== request.executionId || transformed.requestId !== request.requestId || transformed.consentId !== request.consentId) return result(event, consumed, failure(request, "failed", "invalid_executor_result", now), true, executionStarted, context.resultEventId, now);
    await leaseHost.release(request);
    return result(event, consumed, validated.value, true, transformed.status === "completed" ? [...executionStarted, "completed"] : executionStarted, context.resultEventId, now);
  } catch {
    await leaseHost.release(request);
    return result(event, consume(state, request), failure(request, "failed", "executor_failed", now), true, [...ready, "executor_started"], context.resultEventId, now);
  }
}

function matches(request: ArtifactTransformExecutionRequest, consent: PeerConsentRecord): boolean {
  const binding = consent.binding;
  return binding.capability === ARTIFACT_TRANSFORM_CAPABILITY && request.consentId === binding.consentId && request.sourcePreviewEventId === binding.sourceEventId && request.envelopeId === binding.envelopeId && request.requestId === binding.requestId && request.requestPayloadHash === binding.requestPayloadHash && request.roomRef === binding.roomRef && request.sourceDeviceRef === binding.sourceDeviceRef && request.targetPeerRef === binding.targetPeerRef && request.sourceCapability === binding.sourceCapability && request.sourceRequestId === binding.sourceRequestId && request.candidateId === binding.candidateId && request.candidateKind === binding.candidateKind && request.resultContract === binding.resultContract && Date.parse(request.expiresAt) <= Date.parse(binding.expiresAt);
}
function consume(state: PeerConsentConsumptionState, request: ArtifactTransformExecutionRequest): PeerConsentConsumptionState { return { consumedConsentIds: [...state.consumedConsentIds, request.consentId], consumedRequestIds: [...state.consumedRequestIds, request.requestId], consumedExecutionIds: [...state.consumedExecutionIds, request.executionId] }; }
function failure(request: Pick<ArtifactTransformExecutionRequest, "executionId" | "requestId" | "consentId">, status: Exclude<ArtifactTransformExecutionResult["status"], "completed">, errorCode: ArtifactTransformExecutionResult["errorCode"], now: Date): ArtifactTransformExecutionResult { return { schemaVersion: ARTIFACT_TRANSFORM_RESULT_SCHEMA, capability: ARTIFACT_TRANSFORM_CAPABILITY, executionId: request.executionId, requestId: request.requestId, consentId: request.consentId, status, errorCode, createdAt: now.toISOString() }; }
/** Test seams may construct an in-memory event. Production Transform success is sent only by Rust. */
function result(event: CapabilityExecuteRequestRoomControlEvent, state: PeerConsentConsumptionState, value: ArtifactTransformExecutionResult, executed: boolean, lifecycle: readonly TransformLifecyclePhase[], eventId: string | undefined, now: Date): TransformExecutionPolicyResult { const built = buildCapabilityExecutionResultControlEvent(value, event, { now, eventId }); if (!built.ok || built.event.kind !== "capability_execution_result") throw new Error(built.ok ? "Artifact Transform result event is invalid." : built.errors.join(" ")); return { state, result: value, resultEvent: built.event, executed, lifecycle }; }
function id(prefix: string, now: Date) { sequence += 1; return `${prefix}-${globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${sequence}`}`; }

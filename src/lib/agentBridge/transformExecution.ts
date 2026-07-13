import {
  buildCapabilityExecuteRequestControlEvent,
  validateRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
} from "./roomControlEvent";
import { invoke } from "@tauri-apps/api/core";
import { validatePeerConsentRecord, type PeerConsentRecord } from "./peerConsent";
import type { PeerConsentConsumptionState } from "./helloPeerExecution";
import {
  ARTIFACT_TRANSFORM_CAPABILITY,
  ARTIFACT_TRANSFORM_EXECUTION_REQUEST_SCHEMA,
  type ArtifactTransformExecutionRequest,
} from "../ai/artifactTransformRequest";

/** Pastey lifecycle facts only; executor details remain receiver-host-private. */
export type TransformLifecyclePhase = "prepared" | "claimed" | "revalidated" | "executor_started" | "completed";

/** Bounded Rust-approved metadata. It deliberately contains no output payload. */
export interface TransformHostOutcome {
  executed: boolean;
  lifecycle: readonly TransformLifecyclePhase[];
  terminalCategory?: "completed" | "failed" | "timed_out" | "rejected" | "execution_state_unknown";
  errorCode?: string;
  deliveryStatus: "sent" | "replay" | "not_sent";
}

/** TypeScript can ask the receiver host for an outcome; it cannot start or feed an executor. */
export interface TransformReceiverHost {
  execute(request: ArtifactTransformExecutionRequest): Promise<TransformHostOutcome>;
}

export interface TransformExecutionPolicyResult extends TransformHostOutcome {
  state: PeerConsentConsumptionState;
}

let sequence = 0;

/** Production host: Rust validates first, then its unavailable adapter fails before mutation. */
export const rustTransformReceiverHost: TransformReceiverHost = {
  async execute(request) {
    return invoke<TransformHostOutcome>("execute_transform_with_receiver_host", { request });
  },
};

/** Test-only bounded stub; it contains no raw executor result. */
export const unavailableTransformReceiverHost: TransformReceiverHost = {
  async execute() {
    return { executed: false, lifecycle: ["prepared"], errorCode: "sandbox_unavailable", deliveryStatus: "not_sent" };
  },
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
  host: TransformReceiverHost,
  context: { roomRef: string; sourceDeviceRef: string; targetPeerRef: string; now?: Date },
): Promise<TransformExecutionPolicyResult> {
  const now = context.now ?? new Date();
  const request = event.payload as ArtifactTransformExecutionRequest;
  const prepared: readonly TransformLifecyclePhase[] = ["prepared"];
  const valid = validateRoomControlEvent(event, { now, expectedRoomRef: context.roomRef, expectedSourceDeviceRef: context.sourceDeviceRef, expectedTargetPeerRef: context.targetPeerRef });
  if (!valid.valid || request.capability !== ARTIFACT_TRANSFORM_CAPABILITY) return rejected(state, "malformed_request", prepared);
  if (state.consumedExecutionIds.includes(request.executionId) || state.consumedConsentIds.includes(request.consentId) || state.consumedRequestIds.includes(request.requestId)) return rejected(state, "already_consumed", prepared);
  if (!consent) return rejected(state, "missing_consent", prepared);
  const consentValidation = validatePeerConsentRecord(consent, { now });
  if (!consentValidation.valid) return rejected(state, Date.parse(consent.binding.expiresAt) <= now.getTime() ? "consent_expired" : "invalid_consent", prepared);
  if (consent.decision !== "allow_once" || consent.status !== "allowed_once") return rejected(state, "consent_not_allowed_once", prepared);
  if (!matches(request, consent)) return rejected(state, "consent_binding_mismatch", prepared);

  const outcome = await host.execute(request);
  return { ...outcome, state: outcome.executed ? consume(state, request) : state };
}

function rejected(state: PeerConsentConsumptionState, errorCode: string, lifecycle: readonly TransformLifecyclePhase[]): TransformExecutionPolicyResult {
  return { state, executed: false, lifecycle, errorCode, deliveryStatus: "not_sent" };
}

function matches(request: ArtifactTransformExecutionRequest, consent: PeerConsentRecord): boolean {
  const binding = consent.binding;
  return binding.capability === ARTIFACT_TRANSFORM_CAPABILITY && request.consentId === binding.consentId && request.sourcePreviewEventId === binding.sourceEventId && request.envelopeId === binding.envelopeId && request.requestId === binding.requestId && request.requestPayloadHash === binding.requestPayloadHash && request.roomRef === binding.roomRef && request.sourceDeviceRef === binding.sourceDeviceRef && request.targetPeerRef === binding.targetPeerRef && request.sourceCapability === binding.sourceCapability && request.sourceRequestId === binding.sourceRequestId && request.candidateId === binding.candidateId && request.candidateKind === binding.candidateKind && request.resultContract === binding.resultContract && Date.parse(request.expiresAt) <= Date.parse(binding.expiresAt);
}

function consume(state: PeerConsentConsumptionState, request: ArtifactTransformExecutionRequest): PeerConsentConsumptionState {
  return { consumedConsentIds: [...state.consumedConsentIds, request.consentId], consumedRequestIds: [...state.consumedRequestIds, request.requestId], consumedExecutionIds: [...state.consumedExecutionIds, request.executionId] };
}

function id(prefix: string, now: Date) {
  sequence += 1;
  return `${prefix}-${globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${sequence}`}`;
}

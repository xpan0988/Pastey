import {
  buildCapabilityExecuteRequestControlEvent,
  buildCapabilityExecutionResultControlEvent,
  validateRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityExecutionResultRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type HelloPeerExecutionRequest,
  type HelloPeerExecutionResult,
  type RoomControlEventBuildResult,
} from "./roomControlEvent";
import {
  validatePeerConsentRecord,
  type PeerConsentRecord,
} from "./peerConsent";
import {
  HELLO_TEMPLATE_CAPABILITY,
  HELLO_TEMPLATE_MESSAGE
} from "../ai/capabilityRegistry";

export interface PeerConsentConsumptionState {
  consumedConsentIds: string[];
  consumedRequestIds: string[];
  consumedExecutionIds: string[];
}

export type HelloPeerExecutionBuildResult =
  | { ok: true; request: HelloPeerExecutionRequest; event: CapabilityExecuteRequestRoomControlEvent }
  | { ok: false; errors: string[] };

export type HelloPeerExecutionPolicyResult = {
  state: PeerConsentConsumptionState;
  result: HelloPeerExecutionResult;
  resultEvent: CapabilityExecutionResultRoomControlEvent;
  executed: boolean;
};

const EXECUTION_TIMEOUT_MS = 1_000;
const MAX_OUTPUT_BYTES = 64;
let executionSequence = 0;

export function createPeerConsentConsumptionState(): PeerConsentConsumptionState {
  return {
    consumedConsentIds: [],
    consumedRequestIds: [],
    consumedExecutionIds: [],
  };
}

export function preservePeerConsentConsumptionState(
  state: PeerConsentConsumptionState,
  previousSessionIdentity: string | null,
  nextSessionIdentity: string | null,
): PeerConsentConsumptionState {
  return previousSessionIdentity === nextSessionIdentity
    ? state
    : createPeerConsentConsumptionState();
}

export function executeHelloPeerTemplate(): "hello peer!" {
  return HELLO_TEMPLATE_MESSAGE;
}

export function buildHelloPeerExecutionRequest(
  preview: CapabilityPreviewRoomControlEvent,
  acknowledgement: CapabilityPreviewAckRoomControlEvent,
  options: { now?: Date; ttlMs?: number; executionId?: string; eventId?: string } = {},
): HelloPeerExecutionBuildResult {
  const now = options.now ?? new Date();
  const errors: string[] = [];
  const previewValidation = validateRoomControlEvent(preview, { now });
  const ackValidation = validateRoomControlEvent(acknowledgement, { now });
  if (!previewValidation.valid || previewValidation.value.kind !== "capability_preview") {
    errors.push("Execution request requires a valid source capability preview.");
  }
  if (!ackValidation.valid || ackValidation.value.kind !== "capability_preview_ack") {
    errors.push("Execution request requires a valid capability preview acknowledgement.");
  }
  const consent = acknowledgement.payload.consent;
  if (!consent) {
    errors.push("Execution request requires an exact allow-once consent grant.");
  } else if (
    acknowledgement.roomRef !== preview.roomRef ||
    acknowledgement.sourceDeviceRef !== preview.targetPeerRef ||
    acknowledgement.targetPeerRef !== preview.sourceDeviceRef ||
    consent.sourcePreviewEventId !== preview.eventId ||
    consent.envelopeId !== preview.payload.envelopeId ||
    consent.requestId !== preview.payload.request.requestId ||
    consent.requestPayloadHash !== preview.payload.request.requestPayloadHash ||
    consent.capability !== HELLO_TEMPLATE_CAPABILITY ||
    consent.exactMessage !== HELLO_TEMPLATE_MESSAGE
  ) {
    errors.push("Execution request consent grant does not match the exact preview/ack chain.");
  }
  if (consent && Date.parse(consent.expiresAt) <= now.getTime()) {
    errors.push("Execution request consent grant is expired.");
  }
  if (errors.length > 0 || !consent) {
    return { ok: false, errors: unique(errors) };
  }
  const ttlMs = options.ttlMs ?? 60_000;
  const request: HelloPeerExecutionRequest = {
    schemaVersion: "pastey-hello-peer-execution-request-v1",
    executionId: options.executionId ?? createExecutionId(now),
    consentId: consent.consentId,
    sourcePreviewEventId: consent.sourcePreviewEventId,
    envelopeId: consent.envelopeId,
    requestId: consent.requestId,
    requestPayloadHash: consent.requestPayloadHash,
    roomRef: preview.roomRef,
    sourceDeviceRef: preview.sourceDeviceRef,
    targetPeerRef: preview.targetPeerRef,
    capability: HELLO_TEMPLATE_CAPABILITY,
    exactMessage: HELLO_TEMPLATE_MESSAGE,
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

export function executeInboundHelloPeerRequest(
  event: CapabilityExecuteRequestRoomControlEvent,
  consent: PeerConsentRecord | undefined,
  state: PeerConsentConsumptionState,
  context: {
    roomRef: string;
    sourceDeviceRef: string;
    targetPeerRef: string;
    now?: Date;
    nowMs?: () => number;
    resultEventId?: string;
  },
): HelloPeerExecutionPolicyResult {
  const now = context.now ?? new Date();
  const request = event.payload as HelloPeerExecutionRequest;
  const validation = validateRoomControlEvent(event, {
    now,
    expectedRoomRef: context.roomRef,
    expectedSourceDeviceRef: context.sourceDeviceRef,
    expectedTargetPeerRef: context.targetPeerRef,
  });
  let status: HelloPeerExecutionResult["status"] = "rejected";
  let errorCode = "policy_rejected";
  let executed = false;

  if (!validation.valid || validation.value.kind !== "capability_execute_request" || request.capability !== HELLO_TEMPLATE_CAPABILITY) {
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
      const start = context.nowMs?.() ?? Date.now();
      const output = executeHelloPeerTemplate();
      const elapsed = (context.nowMs?.() ?? Date.now()) - start;
      executed = true;
      if (elapsed > EXECUTION_TIMEOUT_MS) {
        status = "failed";
        errorCode = "execution_timeout";
      } else if (
        output !== HELLO_TEMPLATE_MESSAGE ||
        new TextEncoder().encode(output).byteLength > MAX_OUTPUT_BYTES
      ) {
        status = "failed";
        errorCode = "invalid_bounded_output";
      } else {
        status = "succeeded";
        const result = successResult(request, now);
        return buildPolicyResult(event, consumed, result, true, context.resultEventId, now);
      }
      return buildPolicyResult(event, consumed, failureResult(request, status, errorCode, now), executed, context.resultEventId, now);
    }
  }
  return buildPolicyResult(event, state, failureResult(request, status, errorCode, now), false, context.resultEventId, now);
}

export function matchExecutionResultToRequest(
  result: CapabilityExecutionResultRoomControlEvent,
  request: CapabilityExecuteRequestRoomControlEvent,
  now = new Date(),
): boolean {
  const validation = validateRoomControlEvent(result, { now });
  return validation.valid
    && validation.value.kind === "capability_execution_result"
    && result.roomRef === request.roomRef
    && result.sourceDeviceRef === request.targetPeerRef
    && result.targetPeerRef === request.sourceDeviceRef
    && result.payload.executionId === request.payload.executionId
    && result.payload.requestId === request.payload.requestId
    && result.payload.consentId === request.payload.consentId;
}

function requestMatchesConsent(request: HelloPeerExecutionRequest, consent: PeerConsentRecord): boolean {
  const binding = consent.binding;
  return request.consentId === binding.consentId
    && request.sourcePreviewEventId === binding.sourceEventId
    && request.envelopeId === binding.envelopeId
    && request.requestId === binding.requestId
    && request.requestPayloadHash === binding.requestPayloadHash
    && request.roomRef === binding.roomRef
    && request.sourceDeviceRef === binding.sourceDeviceRef
    && request.targetPeerRef === binding.targetPeerRef
    && request.capability === binding.capability
    && request.exactMessage === binding.exactMessage
    && Date.parse(request.expiresAt) <= Date.parse(binding.expiresAt);
}

function consume(
  state: PeerConsentConsumptionState,
  request: HelloPeerExecutionRequest,
): PeerConsentConsumptionState {
  return {
    consumedConsentIds: [...state.consumedConsentIds, request.consentId],
    consumedRequestIds: [...state.consumedRequestIds, request.requestId],
    consumedExecutionIds: [...state.consumedExecutionIds, request.executionId],
  };
}

function successResult(request: HelloPeerExecutionRequest, now: Date): HelloPeerExecutionResult {
  return {
    schemaVersion: "pastey-hello-peer-execution-result-v1",
    executionId: request.executionId,
    requestId: request.requestId,
    consentId: request.consentId,
    status: "succeeded",
    output: HELLO_TEMPLATE_MESSAGE,
    createdAt: now.toISOString(),
  };
}

function failureResult(
  request: HelloPeerExecutionRequest,
  status: Exclude<HelloPeerExecutionResult["status"], "succeeded">,
  errorCode: string,
  now: Date,
): HelloPeerExecutionResult {
  return {
    schemaVersion: "pastey-hello-peer-execution-result-v1",
    executionId: request.executionId,
    requestId: request.requestId,
    consentId: request.consentId,
    status,
    errorCode,
    createdAt: now.toISOString(),
  };
}

function buildPolicyResult(
  requestEvent: CapabilityExecuteRequestRoomControlEvent,
  state: PeerConsentConsumptionState,
  result: HelloPeerExecutionResult,
  executed: boolean,
  eventId: string | undefined,
  now: Date,
): HelloPeerExecutionPolicyResult {
  const built: RoomControlEventBuildResult = buildCapabilityExecutionResultControlEvent(
    result,
    requestEvent,
    { now, eventId },
  );
  if (!built.ok || built.event.kind !== "capability_execution_result") {
    throw new Error("Bounded execution result construction failed.");
  }
  return { state, result, resultEvent: built.event, executed };
}

function createExecutionId(now: Date): string {
  executionSequence += 1;
  return `hello-peer-execution-${globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${executionSequence}`}`;
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

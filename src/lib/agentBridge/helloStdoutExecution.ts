import {
  buildCapabilityExecuteRequestControlEvent,
  buildCapabilityExecutionResultControlEvent,
  validateRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityExecutionResultRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type HelloStdoutExecutionRequest,
  type HelloStdoutExecutionResult,
  type RoomControlEventBuildResult,
} from "./roomControlEvent";
import {
  validatePeerConsentRecord,
  type PeerConsentRecord,
} from "./peerConsent";
import type { PeerConsentConsumptionState } from "./helloPeerExecution";
import {
  HELLO_STDOUT_CAPABILITY,
  HELLO_STDOUT_EXPECTED_STDOUT,
  HELLO_STDOUT_RUNTIME_KIND
} from "../ai/capabilityRegistry";

export type HelloStdoutExecutionBuildResult =
  | { ok: true; request: HelloStdoutExecutionRequest; event: CapabilityExecuteRequestRoomControlEvent }
  | { ok: false; errors: string[] };

export type HelloStdoutHostExecutor = (
  request: HelloStdoutExecutionRequest,
) => Promise<HelloStdoutExecutionResult>;

export type HelloStdoutExecutionPolicyResult = {
  state: PeerConsentConsumptionState;
  result: HelloStdoutExecutionResult;
  resultEvent: CapabilityExecutionResultRoomControlEvent;
  executed: boolean;
};

let executionSequence = 0;

export function buildHelloStdoutExecutionRequest(
  preview: CapabilityPreviewRoomControlEvent,
  acknowledgement: CapabilityPreviewAckRoomControlEvent,
  options: { now?: Date; ttlMs?: number; executionId?: string; eventId?: string } = {},
): HelloStdoutExecutionBuildResult {
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
    consent.capability !== HELLO_STDOUT_CAPABILITY ||
    !("expectedStdout" in consent) ||
    consent.expectedStdout !== HELLO_STDOUT_EXPECTED_STDOUT
  ) {
    errors.push("Execution request consent grant does not match the exact preview/ack chain.");
  }
  if (preview.payload.request.capability !== HELLO_STDOUT_CAPABILITY) {
    errors.push("Hello Stdout execution request requires a Hello Stdout preview.");
  }
  if (consent && Date.parse(consent.expiresAt) <= now.getTime()) {
    errors.push("Execution request consent grant is expired.");
  }
  if (errors.length > 0 || !consent) {
    return { ok: false, errors: unique(errors) };
  }
  const ttlMs = options.ttlMs ?? 60_000;
  const request: HelloStdoutExecutionRequest = {
    schemaVersion: "pastey-runtime-hello-stdout-execution-request/v1",
    executionId: options.executionId ?? createExecutionId(now),
    consentId: consent.consentId,
    sourcePreviewEventId: consent.sourcePreviewEventId,
    envelopeId: consent.envelopeId,
    requestId: consent.requestId,
    requestPayloadHash: consent.requestPayloadHash,
    roomRef: preview.roomRef,
    sourceDeviceRef: preview.sourceDeviceRef,
    targetPeerRef: preview.targetPeerRef,
    capability: HELLO_STDOUT_CAPABILITY,
    expectedStdout: HELLO_STDOUT_EXPECTED_STDOUT,
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

export async function executeInboundHelloStdoutRequest(
  event: CapabilityExecuteRequestRoomControlEvent,
  consent: PeerConsentRecord | undefined,
  state: PeerConsentConsumptionState,
  executor: HelloStdoutHostExecutor,
  context: {
    roomRef: string;
    sourceDeviceRef: string;
    targetPeerRef: string;
    now?: Date;
    resultEventId?: string;
  },
): Promise<HelloStdoutExecutionPolicyResult> {
  const now = context.now ?? new Date();
  const request = event.payload as HelloStdoutExecutionRequest;
  const validation = validateRoomControlEvent(event, {
    now,
    expectedRoomRef: context.roomRef,
    expectedSourceDeviceRef: context.sourceDeviceRef,
    expectedTargetPeerRef: context.targetPeerRef,
  });
  let status: HelloStdoutExecutionResult["status"] = "rejected";
  let errorCode = "policy_rejected";
  let executed = false;

  if (!validation.valid || validation.value.kind !== "capability_execute_request" || request.capability !== HELLO_STDOUT_CAPABILITY) {
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
        executed = true;
        const result = await executor(request);
        return buildPolicyResult(event, consumed, result, true, context.resultEventId, now);
      } catch {
        return buildPolicyResult(
          event,
          consumed,
          failureResult(request, "failed", "runtime_unavailable", now),
          true,
          context.resultEventId,
          now,
        );
      }
    }
  }
  return buildPolicyResult(event, state, failureResult(request, status, errorCode, now), executed, context.resultEventId, now);
}

function requestMatchesConsent(request: HelloStdoutExecutionRequest, consent: PeerConsentRecord): boolean {
  const binding = consent.binding;
  return binding.capability === HELLO_STDOUT_CAPABILITY
    && request.consentId === binding.consentId
    && request.sourcePreviewEventId === binding.sourceEventId
    && request.envelopeId === binding.envelopeId
    && request.requestId === binding.requestId
    && request.requestPayloadHash === binding.requestPayloadHash
    && request.roomRef === binding.roomRef
    && request.sourceDeviceRef === binding.sourceDeviceRef
    && request.targetPeerRef === binding.targetPeerRef
    && request.capability === binding.capability
    && request.expectedStdout === binding.expectedStdout
    && Date.parse(request.expiresAt) <= Date.parse(binding.expiresAt);
}

function consume(
  state: PeerConsentConsumptionState,
  request: HelloStdoutExecutionRequest,
): PeerConsentConsumptionState {
  return {
    consumedConsentIds: [...state.consumedConsentIds, request.consentId],
    consumedRequestIds: [...state.consumedRequestIds, request.requestId],
    consumedExecutionIds: [...state.consumedExecutionIds, request.executionId],
  };
}

function failureResult(
  request: HelloStdoutExecutionRequest,
  status: Exclude<HelloStdoutExecutionResult["status"], "succeeded">,
  errorCode: string,
  now: Date,
): HelloStdoutExecutionResult {
  return {
    schemaVersion: "pastey-runtime-hello-stdout-execution-result/v1",
    executionId: request.executionId,
    requestId: request.requestId,
    consentId: request.consentId,
    capability: HELLO_STDOUT_CAPABILITY,
    runtimeKind: HELLO_STDOUT_RUNTIME_KIND,
    status,
    stdout: "",
    stderr: "",
    exitCode: 1,
    durationMs: 0,
    timedOut: false,
    stdoutTruncated: false,
    stderrTruncated: false,
    errorCode,
    createdAt: now.toISOString(),
  };
}

function buildPolicyResult(
  requestEvent: CapabilityExecuteRequestRoomControlEvent,
  state: PeerConsentConsumptionState,
  result: HelloStdoutExecutionResult,
  executed: boolean,
  eventId: string | undefined,
  now: Date,
): HelloStdoutExecutionPolicyResult {
  const built: RoomControlEventBuildResult = buildCapabilityExecutionResultControlEvent(
    result,
    requestEvent,
    { now, eventId },
  );
  if (!built.ok || built.event.kind !== "capability_execution_result") {
    throw new Error("Bounded Hello Stdout execution result construction failed.");
  }
  return { state, result, resultEvent: built.event, executed };
}

function createExecutionId(now: Date): string {
  executionSequence += 1;
  return `hello-stdout-execution-${globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${executionSequence}`}`;
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

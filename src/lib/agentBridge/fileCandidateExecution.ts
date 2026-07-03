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
import { requireCapabilityManifest } from "./capabilityManifest";
import {
  assertConsentNotExpired,
  assertExactCapability,
  bindRequestHash,
  rejectForbiddenPublicFields,
} from "./capabilityTemplateHelpers";
import {
  FILE_CANDIDATES_EXECUTION_REQUEST_SCHEMA,
  FILE_CANDIDATES_RESULT_SCHEMA,
  type FileCandidateExecutionRequest,
  type FileCandidateExecutionResult,
} from "../ai/fileCandidateRequest";

const FILE_CANDIDATES_CAPABILITY_MANIFEST = requireCapabilityManifest("filesystem.find_file_candidates");
const FILE_CANDIDATES_CAPABILITY: "filesystem.find_file_candidates" =
  FILE_CANDIDATES_CAPABILITY_MANIFEST.capability as "filesystem.find_file_candidates";
const FILE_CANDIDATES_EXECUTOR_KIND: "filesystem_find_candidates_host" =
  FILE_CANDIDATES_CAPABILITY_MANIFEST.executorKind as "filesystem_find_candidates_host";

export type FileCandidateExecutionBuildResult =
  | { ok: true; request: FileCandidateExecutionRequest; event: CapabilityExecuteRequestRoomControlEvent }
  | { ok: false; errors: string[] };

export type FileCandidateHostExecutor = (
  request: FileCandidateExecutionRequest,
) => Promise<FileCandidateExecutionResult>;

export type FileCandidateExecutionPolicyResult = {
  state: PeerConsentConsumptionState;
  result: FileCandidateExecutionResult;
  resultEvent: CapabilityExecutionResultRoomControlEvent;
  executed: boolean;
};

let executionSequence = 0;

export function buildFileCandidateExecutionRequest(
  preview: CapabilityPreviewRoomControlEvent,
  acknowledgement: CapabilityPreviewAckRoomControlEvent,
  options: { now?: Date; ttlMs?: number; executionId?: string; eventId?: string } = {},
): FileCandidateExecutionBuildResult {
  const now = options.now ?? new Date();
  const errors: string[] = [];
  const previewValidation = validateRoomControlEvent(preview, { now });
  const ackValidation = validateRoomControlEvent(acknowledgement, { now });
  const fileCandidateRequest = preview.payload.request.capability === FILE_CANDIDATES_CAPABILITY
    ? preview.payload.request
    : null;
  if (!previewValidation.valid || previewValidation.value.kind !== "capability_preview") {
    errors.push("File candidate execution request requires a valid source capability preview.");
  }
  if (!ackValidation.valid || ackValidation.value.kind !== "capability_preview_ack") {
    errors.push("File candidate execution request requires a valid capability preview acknowledgement.");
  }
  const consent = acknowledgement.payload.consent;
  if (!consent) {
    errors.push("File candidate execution request requires an exact allow-once consent grant.");
  } else if (
    acknowledgement.roomRef !== preview.roomRef ||
    acknowledgement.sourceDeviceRef !== preview.targetPeerRef ||
    acknowledgement.targetPeerRef !== preview.sourceDeviceRef ||
    consent.sourcePreviewEventId !== preview.eventId ||
    consent.envelopeId !== preview.payload.envelopeId ||
    consent.requestId !== preview.payload.request.requestId ||
    !matchesHash(preview.payload.request.requestPayloadHash, consent.requestPayloadHash) ||
    !matchesCapability(FILE_CANDIDATES_CAPABILITY, consent.capability) ||
    !fileCandidateRequest ||
    !("filenameHint" in consent) ||
    consent.filenameHint !== fileCandidateRequest.input.query.filenameHint ||
    consent.searchMode !== fileCandidateRequest.input.query.searchMode
  ) {
    errors.push("File candidate execution request consent grant does not match the exact preview/ack chain.");
  }
  if (!fileCandidateRequest) {
    errors.push("File candidate execution request requires a file candidate preview.");
  }
  if (consent && isExpired(consent.expiresAt, now)) {
    errors.push("File candidate execution request consent grant is expired.");
  }
  if (errors.length > 0 || !consent || !fileCandidateRequest) {
    return { ok: false, errors: unique(errors) };
  }
  const ttlMs = options.ttlMs ?? 60_000;
  const request: FileCandidateExecutionRequest = {
    schemaVersion: FILE_CANDIDATES_EXECUTION_REQUEST_SCHEMA,
    executionId: options.executionId ?? createExecutionId(now),
    consentId: consent.consentId,
    sourcePreviewEventId: consent.sourcePreviewEventId,
    envelopeId: consent.envelopeId,
    requestId: consent.requestId,
    requestPayloadHash: consent.requestPayloadHash,
    roomRef: preview.roomRef,
    sourceDeviceRef: preview.sourceDeviceRef,
    targetPeerRef: preview.targetPeerRef,
    capability: FILE_CANDIDATES_CAPABILITY,
    executorKind: FILE_CANDIDATES_EXECUTOR_KIND,
    input: fileCandidateRequest.input,
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

export async function executeInboundFileCandidateRequest(
  event: CapabilityExecuteRequestRoomControlEvent,
  consent: PeerConsentRecord | undefined,
  state: PeerConsentConsumptionState,
  executor: FileCandidateHostExecutor,
  context: {
    roomRef: string;
    sourceDeviceRef: string;
    targetPeerRef: string;
    now?: Date;
    resultEventId?: string;
  },
): Promise<FileCandidateExecutionPolicyResult> {
  const now = context.now ?? new Date();
  const request = event.payload as FileCandidateExecutionRequest;
  const validation = validateRoomControlEvent(event, {
    now,
    expectedRoomRef: context.roomRef,
    expectedSourceDeviceRef: context.sourceDeviceRef,
    expectedTargetPeerRef: context.targetPeerRef,
  });
  let status: FileCandidateExecutionResult["status"] = "rejected";
  let errorCode: NonNullable<FileCandidateExecutionResult["errorCode"]> = "policy_rejected";
  let executed = false;

  if (!validation.valid || validation.value.kind !== "capability_execute_request" || request.capability !== FILE_CANDIDATES_CAPABILITY) {
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
        rejectForbiddenPublicFields(result);
        return buildPolicyResult(event, consumed, result, true, context.resultEventId, now);
      } catch {
        return buildPolicyResult(
          event,
          consumed,
          failureResult(request, "failed", "executor_unavailable", now),
          true,
          context.resultEventId,
          now,
        );
      }
    }
  }
  return buildPolicyResult(event, state, failureResult(request, status, errorCode, now), executed, context.resultEventId, now);
}

function requestMatchesConsent(request: FileCandidateExecutionRequest, consent: PeerConsentRecord): boolean {
  const binding = consent.binding;
  return matchesCapability(FILE_CANDIDATES_CAPABILITY, binding.capability)
    && request.consentId === binding.consentId
    && request.sourcePreviewEventId === binding.sourceEventId
    && request.envelopeId === binding.envelopeId
    && request.requestId === binding.requestId
    && matchesHash(binding.requestPayloadHash, request.requestPayloadHash)
    && request.roomRef === binding.roomRef
    && request.sourceDeviceRef === binding.sourceDeviceRef
    && request.targetPeerRef === binding.targetPeerRef
    && matchesCapability(binding.capability, request.capability)
    && "filenameHint" in binding
    && "searchMode" in binding
    && request.input.query.filenameHint === binding.filenameHint
    && request.input.query.searchMode === binding.searchMode
    && Date.parse(request.expiresAt) <= Date.parse(binding.expiresAt);
}

function consume(
  state: PeerConsentConsumptionState,
  request: FileCandidateExecutionRequest,
): PeerConsentConsumptionState {
  return {
    consumedConsentIds: [...state.consumedConsentIds, request.consentId],
    consumedRequestIds: [...state.consumedRequestIds, request.requestId],
    consumedExecutionIds: [...state.consumedExecutionIds, request.executionId],
  };
}

function failureResult(
  request: FileCandidateExecutionRequest,
  status: Exclude<FileCandidateExecutionResult["status"], "completed">,
  errorCode: NonNullable<FileCandidateExecutionResult["errorCode"]>,
  now: Date,
): FileCandidateExecutionResult {
  const query = request.input?.query ?? {
    filenameHint: "unknown",
    extensions: [],
    searchMode: "filename_metadata_only" as const,
  };
  return {
    schemaVersion: FILE_CANDIDATES_RESULT_SCHEMA,
    capability: FILE_CANDIDATES_CAPABILITY,
    executionId: request.executionId,
    requestId: request.requestId,
    consentId: request.consentId,
    status,
    queryEcho: {
      filenameHint: query.filenameHint,
      extensions: [...query.extensions],
      searchMode: query.searchMode,
    },
    candidates: [],
    omitted: {
      tooManyMatches: false,
      hiddenFilesSkipped: false,
      symlinksSkipped: false,
      scopesSkipped: [],
    },
    durationMs: 0,
    truncated: false,
    errorCode,
    createdAt: now.toISOString(),
  };
}

function buildPolicyResult(
  requestEvent: CapabilityExecuteRequestRoomControlEvent,
  state: PeerConsentConsumptionState,
  result: FileCandidateExecutionResult,
  executed: boolean,
  eventId: string | undefined,
  now: Date,
): FileCandidateExecutionPolicyResult {
  const built: RoomControlEventBuildResult = buildCapabilityExecutionResultControlEvent(
    result,
    requestEvent,
    { now, eventId },
  );
  if (!built.ok || built.event.kind !== "capability_execution_result") {
    throw new Error("Bounded file candidate execution result construction failed.");
  }
  return { state, result, resultEvent: built.event, executed };
}

function createExecutionId(now: Date): string {
  executionSequence += 1;
  return `file-candidates-execution-${globalThis.crypto?.randomUUID?.() ?? `${now.getTime()}-${executionSequence}`}`;
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

function matchesCapability(expected: string, actual: string): boolean {
  try {
    assertExactCapability({ expected, actual });
    return true;
  } catch {
    return false;
  }
}

function matchesHash(expected: string, actual: string): boolean {
  try {
    bindRequestHash({ expected, actual });
    return true;
  } catch {
    return false;
  }
}

function isExpired(expiresAt: string, now: Date): boolean {
  try {
    assertConsentNotExpired(expiresAt, now.getTime());
    return false;
  } catch {
    return true;
  }
}

import assert from "node:assert/strict";
import test from "node:test";

import {
  buildCandidatePayloadRequestFromPendingAction,
  buildCapabilityRequestPreviewEnvelope,
  buildFileCandidateRequestFromPendingAction,
  buildHelloPeerRequestFromPendingAction,
  buildMockAiContextSnapshot,
  buildMockCandidatePayloadPlan,
  buildMockFileCandidatePlan,
  buildMockHelloPeerPlan,
  confirmPendingAiAction,
  createPendingAiAction,
  evaluateAiPolicy,
  validateAiActionPlan,
  validateCandidatePayloadExecutionResult,
  validateCandidatePayloadRequest,
  type AiActionPlan,
  type CandidatePayloadExecutionRequest,
  type CandidatePayloadExecutionResult,
} from "../src/lib/ai";
import {
  allowPeerCapabilityOnce,
  buildCandidatePayloadExecutionRequest,
  buildFileCandidateExecutionRequest,
  buildHelloPeerExecutionRequest,
  buildPeerConsentStatusEvent,
  buildSessionBoundCapabilityPreviewControlEvent,
  createPeerConsentConsumptionState,
  createPeerConsentSessionState,
  evaluatePeerCapabilityPreview,
  executeInboundCandidatePayloadRequest,
  matchExecutionResultToRequest,
  validateRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type CandidatePayloadLocalResolution,
  type PeerConsentRecord,
} from "../src/lib/agentBridge";

const ROOM = "room";
const SOURCE = "room-session:source";
const TARGET = "room-session:target";
const NOW = new Date("2026-06-29T00:00:00.000Z");
const ALLOWED_AT = new Date("2026-06-29T00:00:10.000Z");
const EXECUTE_AT = new Date("2026-06-29T00:00:20.000Z");

async function resolvedCandidatePayload(
  request: CandidatePayloadExecutionRequest,
): Promise<CandidatePayloadLocalResolution> {
  return {
    sourceCapability: "filesystem.find_file_candidates",
    sourceRequestId: request.sourceRequestId,
    candidateId: request.candidateId,
    candidateKind: request.candidateKind,
    resolved: true,
    reason: "resolved",
    displayName: request.candidateDisplayName,
    sizeBytes: 21,
    modifiedAt: EXECUTE_AT.toISOString(),
    mimeFamily: "document",
    extension: "pdf",
    receiverLocalSource: "/receiver/local/assignment.pdf",
  };
}

async function queueCandidatePayloadHandoff() {
  return { queued: true as const };
}

async function failIfHandoffRuns() {
  throw new Error("handoff must not run");
}

function unresolvedCandidatePayload(
  request: CandidatePayloadExecutionRequest,
  reason: "not_found" | "expired" | "changed",
): CandidatePayloadLocalResolution {
  return {
    sourceCapability: "filesystem.find_file_candidates",
    sourceRequestId: request.sourceRequestId,
    candidateId: request.candidateId,
    candidateKind: request.candidateKind,
    resolved: false,
    reason,
    displayName: request.candidateDisplayName,
  };
}

function confirmedPending(plan: AiActionPlan, pendingId: string) {
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  assert.equal(policy.status, "accepted", policy.status === "rejected" ? policy.reasons.join(" ") : undefined);
  return confirmPendingAiAction(
    createPendingAiAction(plan, policy, {
      now: new Date("2026-06-28T23:59:00.000Z"),
      ttlMs: 300_000,
      pendingId,
    }),
    new Date("2026-06-28T23:59:30.000Z"),
  );
}

function candidatePayloadChain(): {
  preview: CapabilityPreviewRoomControlEvent;
  consent: PeerConsentRecord;
  ack: CapabilityPreviewAckRoomControlEvent;
  request: CapabilityExecuteRequestRoomControlEvent;
} {
  const previewRequest = buildCandidatePayloadRequestFromPendingAction(
    confirmedPending(buildMockCandidatePayloadPlan(), "candidate-payload-pending"),
    {
      now: NOW,
      ttlMs: 120_000,
      requestId: "candidate-payload-request",
      nonce: "candidate-payload-nonce",
      sourceDeviceRef: SOURCE,
    },
  );
  assert.equal(previewRequest.ok, true, previewRequest.ok ? undefined : previewRequest.errors.join(" "));
  if (!previewRequest.ok) throw new Error("Expected candidate payload preview request.");

  const envelope = buildCapabilityRequestPreviewEnvelope(previewRequest.request, {
    roomRef: ROOM,
    now: NOW,
    ttlMs: 120_000,
    envelopeId: "candidate-payload-envelope",
  });
  assert.equal(envelope.ok, true, envelope.ok ? undefined : envelope.errors.join(" "));
  if (!envelope.ok) throw new Error("Expected candidate payload envelope.");

  const preview = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, {
    roomId: ROOM,
    localSessionRef: SOURCE,
    peerSessionRef: TARGET,
    peerConnected: true,
  }, { now: NOW });
  assert.equal(preview.ok, true, preview.ok ? undefined : preview.errors.join(" "));
  if (!preview.ok || preview.event.kind !== "capability_preview") throw new Error("Expected preview.");

  const review = evaluatePeerCapabilityPreview(preview.event, {
    roomRef: ROOM,
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    session: createPeerConsentSessionState(),
    now: ALLOWED_AT,
    consentId: "candidate-payload-consent",
  });
  assert.equal(review.status, "reviewable", review.status === "rejected" ? review.errors.join(" ") : undefined);
  if (review.status !== "reviewable") throw new Error("Expected reviewable preview.");

  const allowed = allowPeerCapabilityOnce(review.binding, createPeerConsentSessionState(), {
    now: ALLOWED_AT,
  });
  assert.equal(allowed.ok, true, allowed.ok ? undefined : allowed.errors.join(" "));
  if (!allowed.ok) throw new Error("Expected allow once.");

  const ack = buildPeerConsentStatusEvent(preview.event, allowed.record, {
    now: ALLOWED_AT,
    eventId: "candidate-payload-ack",
  });
  assert.equal(ack.ok, true, ack.ok ? undefined : ack.errors.join(" "));
  if (!ack.ok || ack.event.kind !== "capability_preview_ack") throw new Error("Expected ack.");

  const execution = buildCandidatePayloadExecutionRequest(preview.event, ack.event, {
    now: EXECUTE_AT,
    executionId: "candidate-payload-execution",
    eventId: "candidate-payload-execution-event",
  });
  assert.equal(execution.ok, true, execution.ok ? undefined : execution.errors.join(" "));
  if (!execution.ok) throw new Error("Expected execution request.");

  return {
    preview: preview.event,
    consent: allowed.record,
    ack: ack.event,
    request: execution.event,
  };
}

function fileCandidateConsent(): PeerConsentRecord {
  const previewRequest = buildFileCandidateRequestFromPendingAction(
    confirmedPending(buildMockFileCandidatePlan(), "file-candidate-pending-for-payload"),
    {
      now: NOW,
      ttlMs: 120_000,
      requestId: "file-candidate-request",
      nonce: "file-candidate-nonce",
      sourceDeviceRef: SOURCE,
    },
  );
  assert.equal(previewRequest.ok, true);
  if (!previewRequest.ok) throw new Error("Expected file candidate request.");
  const envelope = buildCapabilityRequestPreviewEnvelope(previewRequest.request, {
    roomRef: ROOM,
    now: NOW,
    ttlMs: 120_000,
    envelopeId: "file-candidate-envelope",
  });
  assert.equal(envelope.ok, true);
  if (!envelope.ok) throw new Error("Expected file candidate envelope.");
  const preview = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, {
    roomId: ROOM,
    localSessionRef: SOURCE,
    peerSessionRef: TARGET,
    peerConnected: true,
  }, { now: NOW });
  assert.equal(preview.ok, true);
  if (!preview.ok || preview.event.kind !== "capability_preview") throw new Error("Expected file candidate preview.");
  const review = evaluatePeerCapabilityPreview(preview.event, {
    roomRef: ROOM,
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    session: createPeerConsentSessionState(),
    now: ALLOWED_AT,
    consentId: "file-candidate-consent",
  });
  assert.equal(review.status, "reviewable");
  if (review.status !== "reviewable") throw new Error("Expected file candidate review.");
  const allowed = allowPeerCapabilityOnce(review.binding, createPeerConsentSessionState(), { now: ALLOWED_AT });
  assert.equal(allowed.ok, true);
  if (!allowed.ok) throw new Error("Expected file candidate allow once.");
  return allowed.record;
}

function helloConsent(): PeerConsentRecord {
  const previewRequest = buildHelloPeerRequestFromPendingAction(
    confirmedPending(buildMockHelloPeerPlan(), "hello-pending-for-payload"),
    {
      now: NOW,
      ttlMs: 120_000,
      requestId: "hello-request",
      nonce: "hello-nonce",
      sourceDeviceRef: SOURCE,
      targetPeerRef: TARGET,
    },
  );
  assert.equal(previewRequest.ok, true);
  if (!previewRequest.ok) throw new Error("Expected Hello request.");
  const envelope = buildCapabilityRequestPreviewEnvelope(previewRequest.request, {
    roomRef: ROOM,
    now: NOW,
    ttlMs: 120_000,
    envelopeId: "hello-envelope",
  });
  assert.equal(envelope.ok, true);
  if (!envelope.ok) throw new Error("Expected Hello envelope.");
  const preview = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, {
    roomId: ROOM,
    localSessionRef: SOURCE,
    peerSessionRef: TARGET,
    peerConnected: true,
  }, { now: NOW });
  assert.equal(preview.ok, true);
  if (!preview.ok || preview.event.kind !== "capability_preview") throw new Error("Expected Hello preview.");
  const review = evaluatePeerCapabilityPreview(preview.event, {
    roomRef: ROOM,
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    session: createPeerConsentSessionState(),
    now: ALLOWED_AT,
    consentId: "hello-consent",
  });
  assert.equal(review.status, "reviewable");
  if (review.status !== "reviewable") throw new Error("Expected Hello review.");
  const allowed = allowPeerCapabilityOnce(review.binding, createPeerConsentSessionState(), { now: ALLOWED_AT });
  assert.equal(allowed.ok, true);
  if (!allowed.ok) throw new Error("Expected Hello allow once.");
  return allowed.record;
}

test("safe advisory plan builds candidate payload request preview", () => {
  const plan = buildMockCandidatePayloadPlan();
  const validation = validateAiActionPlan(plan);
  assert.equal(validation.valid, true);
  if (!validation.valid) return;
  const policy = evaluateAiPolicy(validation.value, buildMockAiContextSnapshot());
  assert.equal(policy.status, "accepted");
  const request = buildCandidatePayloadRequestFromPendingAction(
    confirmedPending(validation.value, "candidate-payload-pending-safe"),
    {
      now: NOW,
      requestId: "candidate-payload-request-safe",
      nonce: "candidate-payload-nonce-safe",
      sourceDeviceRef: SOURCE,
    },
  );
  assert.equal(request.ok, true, request.ok ? undefined : request.errors.join(" "));
  if (!request.ok) return;
  assert.equal(request.request.capability, "transfer.request_candidate_payload");
  assert.equal(request.request.executorKind, "transfer_candidate_payload_host");
  assert.equal(request.request.input.sourceCapability, "filesystem.find_file_candidates");
  assert.equal(request.request.input.candidateId, "file-candidate-request-opaque-1");
  assert.equal(validateCandidatePayloadRequest(request.request, { now: NOW }).valid, true);
});

test("preview envelope validates and receiver Allow once creates capability-specific grant", () => {
  const chain = candidatePayloadChain();
  assert.equal(validateRoomControlEvent(chain.preview, {
    now: NOW,
    expectedRoomRef: ROOM,
    expectedSourceDeviceRef: SOURCE,
    expectedTargetPeerRef: TARGET,
  }).valid, true);
  assert.equal(chain.consent.binding.capability, "transfer.request_candidate_payload");
  assert.equal(chain.consent.binding.requestId, chain.preview.payload.request.requestId);
  assert.equal(chain.ack.payload.consent?.schemaVersion, "transfer-request-candidate-payload-consent-grant-v1");
  assert.equal(chain.ack.payload.consent?.capability, "transfer.request_candidate_payload");
});

test("execution request binds exact request, session, peer, capability, and hash", () => {
  const chain = candidatePayloadChain();
  assert.equal(chain.request.payload.capability, "transfer.request_candidate_payload");
  assert.equal(chain.request.payload.executorKind, "transfer_candidate_payload_host");
  assert.equal(chain.request.payload.requestId, chain.preview.payload.request.requestId);
  assert.equal(chain.request.payload.requestPayloadHash, chain.preview.payload.request.requestPayloadHash);
  assert.equal(chain.request.payload.roomRef, ROOM);
  assert.equal(chain.request.payload.sourceDeviceRef, SOURCE);
  assert.equal(chain.request.payload.targetPeerRef, TARGET);
  assert.equal(chain.request.payload.candidateId, "file-candidate-request-opaque-1");
  assert.equal(validateRoomControlEvent(chain.request, { now: EXECUTE_AT }).valid, true);
});

test("execution validates consent, resolves candidate metadata, queues handoff, and transfers no bytes yet", async () => {
  const chain = candidatePayloadChain();
  const executed = await executeInboundCandidatePayloadRequest(
    chain.request,
    chain.consent,
    createPeerConsentConsumptionState(),
    resolvedCandidatePayload,
    queueCandidatePayloadHandoff,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(executed.executed, true);
  assert.deepEqual({
    status: executed.result.status,
    transferredBytes: executed.result.transferredBytes,
    handoffQueued: executed.result.handoffQueued,
    transferStatus: executed.result.transferStatus,
  }, {
    status: "handoff_queued",
    transferredBytes: 0,
    handoffQueued: true,
    transferStatus: "queued",
  });
  assert.equal(executed.result.candidateResolution?.resolved, true);
  assert.equal(executed.result.candidateResolution?.reason, "resolved");
  assert.equal(executed.result.transferredBytes, 0);
  assert.equal(executed.result.handoffQueued, true);
  assert.equal(executed.result.transferStatus, "queued");
  assert.equal(executed.result.errorCode, null);
  assert.equal(validateCandidatePayloadExecutionResult(executed.result).valid, true);
  assert.equal(validateRoomControlEvent(executed.resultEvent, { now: EXECUTE_AT }).valid, true);
  assert.equal(matchExecutionResultToRequest(executed.resultEvent, chain.request, EXECUTE_AT), true);
});

test("one-time consent rejects replay", async () => {
  const chain = candidatePayloadChain();
  const first = await executeInboundCandidatePayloadRequest(
    chain.request,
    chain.consent,
    createPeerConsentConsumptionState(),
    resolvedCandidatePayload,
    queueCandidatePayloadHandoff,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  const replay = await executeInboundCandidatePayloadRequest(
    chain.request,
    chain.consent,
    first.state,
    async () => {
      throw new Error("resolver must not run on replay");
    },
    failIfHandoffRuns,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(replay.executed, false);
  assert.equal(replay.result.status, "already_consumed");
  assert.equal(replay.result.errorCode, "already_consumed");
});

test("expired consent, wrong request hash, wrong capability, and wrong source binding fail closed", async () => {
  const chain = candidatePayloadChain();
  const expiredConsent = {
    ...chain.consent,
    binding: {
      ...chain.consent.binding,
      expiresAt: new Date(EXECUTE_AT.getTime() - 1).toISOString(),
    },
  } as PeerConsentRecord;
  const expired = await executeInboundCandidatePayloadRequest(
    chain.request,
    expiredConsent,
    createPeerConsentConsumptionState(),
    async () => {
      throw new Error("resolver must not run with expired consent");
    },
    failIfHandoffRuns,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(expired.executed, false);
  assert.equal(expired.result.status, "expired");
  assert.equal(expired.result.errorCode, "consent_expired");

  const wrongHashRequest = structuredClone(chain.request) as CapabilityExecuteRequestRoomControlEvent;
  wrongHashRequest.payload.requestPayloadHash = "wrong-request-hash";
  const wrongHash = await executeInboundCandidatePayloadRequest(
    wrongHashRequest,
    chain.consent,
    createPeerConsentConsumptionState(),
    async () => {
      throw new Error("resolver must not run with a wrong request hash");
    },
    failIfHandoffRuns,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(wrongHash.executed, false);
  assert.equal(wrongHash.result.errorCode, "consent_binding_mismatch");

  const wrongCapabilityRequest = structuredClone(chain.request) as CapabilityExecuteRequestRoomControlEvent;
  wrongCapabilityRequest.payload.capability = "runtime.hello_stdout" as CandidatePayloadExecutionRequest["capability"];
  const wrongCapability = await executeInboundCandidatePayloadRequest(
    wrongCapabilityRequest,
    chain.consent,
    createPeerConsentConsumptionState(),
    async () => {
      throw new Error("resolver must not run with a wrong capability");
    },
    failIfHandoffRuns,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(wrongCapability.executed, false);
  assert.equal(wrongCapability.result.errorCode, "malformed_request");

  const wrongSourceBindingRequest = structuredClone(chain.request) as CapabilityExecuteRequestRoomControlEvent;
  wrongSourceBindingRequest.payload.sourceRequestId = "different-discovery-request";
  const wrongSourceBinding = await executeInboundCandidatePayloadRequest(
    wrongSourceBindingRequest,
    chain.consent,
    createPeerConsentConsumptionState(),
    async () => {
      throw new Error("resolver must not run with a wrong source discovery binding");
    },
    failIfHandoffRuns,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(wrongSourceBinding.executed, false);
  assert.equal(wrongSourceBinding.result.errorCode, "consent_binding_mismatch");
});

test("file-candidate consent cannot authorize candidate payload request", async () => {
  const chain = candidatePayloadChain();
  const rejected = await executeInboundCandidatePayloadRequest(
    chain.request,
    fileCandidateConsent(),
    createPeerConsentConsumptionState(),
    async () => {
      throw new Error("resolver must not run with discovery consent");
    },
    failIfHandoffRuns,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(rejected.executed, false);
  assert.equal(rejected.result.errorCode, "consent_binding_mismatch");
});

test("candidate payload consent cannot authorize search, Hello, or future transfer", async () => {
  const chain = candidatePayloadChain();
  assert.equal(buildFileCandidateExecutionRequest(chain.preview, chain.ack).ok, false);
  assert.equal(buildHelloPeerExecutionRequest(chain.preview, chain.ack).ok, false);
  const rejected = await executeInboundCandidatePayloadRequest(
    chain.request,
    helloConsent(),
    createPeerConsentConsumptionState(),
    async () => {
      throw new Error("resolver must not run with Hello consent");
    },
    failIfHandoffRuns,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(rejected.executed, false);
  assert.equal(rejected.result.errorCode, "consent_binding_mismatch");
  assert.equal("transferQueueId" in chain.consent.binding, false);
  assert.equal("handoffId" in chain.consent.binding, false);
});

test("missing, expired, changed, or deleted candidates do not enqueue handoff", async () => {
  for (const scenario of [
    { label: "missing", reason: "not_found" as const, status: "candidate_not_found" },
    { label: "expired", reason: "expired" as const, status: "candidate_expired" },
    { label: "changed", reason: "changed" as const, status: "candidate_changed" },
    { label: "deleted", reason: "not_found" as const, status: "candidate_not_found" },
  ]) {
    const chain = candidatePayloadChain();
    let handoffCalls = 0;
    const executed = await executeInboundCandidatePayloadRequest(
      chain.request,
      chain.consent,
      createPeerConsentConsumptionState(),
      async (request) => unresolvedCandidatePayload(request, scenario.reason),
      async () => {
        handoffCalls += 1;
        return { queued: true as const };
      },
      { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
    );
    assert.equal(executed.executed, true, scenario.label);
    assert.equal(executed.result.status, scenario.status, scenario.label);
    assert.equal(executed.result.handoffQueued, false, scenario.label);
    assert.equal(executed.result.transferredBytes, 0, scenario.label);
    assert.equal(handoffCalls, 0, scenario.label);
  }
});

test("selected-peers, broadcast, and unsafe provider fields reject", () => {
  for (const unsafe of [
    { selectedPeers: ["peer-a", "peer-b"] },
    { targetPeerRefs: ["peer-a", "peer-b"] },
    { broadcast: true },
    { absolutePath: "/Users/example/secret.pdf" },
    { filePath: "/Users/example/secret.pdf" },
    { contents: "secret" },
    { transferQueueId: "queue-1" },
    { handoffId: "handoff-1" },
    { autoSend: true },
  ]) {
    const plan = structuredClone(buildMockCandidatePayloadPlan()) as AiActionPlan;
    plan.proposedInput = { ...plan.proposedInput, ...unsafe };
    const validation = validateAiActionPlan(plan);
    if (validation.valid) {
      const policy = evaluateAiPolicy(validation.value, buildMockAiContextSnapshot());
      assert.equal(policy.status, "rejected", `expected PolicyGate rejection for ${Object.keys(unsafe)[0]}`);
    } else {
      assert.equal(validation.valid, false, `expected schema rejection for ${Object.keys(unsafe)[0]}`);
    }
  }
});

test("path-like candidate IDs reject", () => {
  for (const candidateId of ["/tmp/secret.pdf", "C:\\Users\\example\\secret.pdf", "folder/secret.pdf"]) {
    const plan = structuredClone(buildMockCandidatePayloadPlan()) as AiActionPlan;
    plan.proposedInput = { ...plan.proposedInput, candidateId };
    const validation = validateAiActionPlan(plan);
    assert.equal(validation.valid, true, `generic action-plan schema should stay capability-agnostic for ${candidateId}`);
    if (!validation.valid) continue;
    const policy = evaluateAiPolicy(validation.value, buildMockAiContextSnapshot());
    assert.equal(policy.status, "rejected", `expected path-like PolicyGate rejection for ${candidateId}`);
  }
});

test("result rejects absolute paths, contents, transfer queue ID, and handoff fields", () => {
  const chain = candidatePayloadChain();
  const result: CandidatePayloadExecutionResult = {
    schemaVersion: "transfer-request-candidate-payload-result-v1",
    capability: "transfer.request_candidate_payload",
    executionId: chain.request.payload.executionId,
    requestId: chain.request.payload.requestId,
    consentId: chain.request.payload.consentId,
    status: "handoff_not_implemented",
    candidate: {
      candidateId: chain.request.payload.candidateId,
      candidateKind: "filesystem_file",
      candidateDisplayName: chain.request.payload.candidateDisplayName,
      sizeBytes: 21,
      mimeFamily: "document",
      extension: "pdf",
    },
    transferredBytes: 0,
    handoffQueued: false,
    errorCode: null,
    createdAt: EXECUTE_AT.toISOString(),
  };
  assert.equal(validateCandidatePayloadExecutionResult(result).valid, true);
  assert.equal(validateCandidatePayloadExecutionResult({
    ...result,
    status: "handoff_queued",
    handoffQueued: true,
    transferStatus: "queued",
  }).valid, true);
  assert.equal(validateCandidatePayloadExecutionResult({
    ...result,
    status: "candidate_resolved_handoff_not_implemented",
    candidateResolution: {
      sourceCapability: "filesystem.find_file_candidates",
      sourceRequestId: chain.request.payload.sourceRequestId,
      candidateId: chain.request.payload.candidateId,
      candidateKind: "filesystem_file",
      resolved: true,
      reason: "resolved",
      displayName: chain.request.payload.candidateDisplayName,
      sizeBytes: 21,
      modifiedAt: EXECUTE_AT.toISOString(),
      mimeFamily: "document",
      extension: "pdf",
    },
  }).valid, true);
  assert.equal(validateCandidatePayloadExecutionResult({
    ...result,
    candidate: { ...result.candidate, candidateId: "/tmp/secret.pdf" },
  }).valid, false);
  assert.equal(validateCandidatePayloadExecutionResult({
    ...result,
    contents: "secret",
  }).valid, false);
  assert.equal(validateCandidatePayloadExecutionResult({
    ...result,
    transferQueueId: "queue-1",
  }).valid, false);
  assert.equal(validateCandidatePayloadExecutionResult({
    ...result,
    handoffId: "handoff-1",
  }).valid, false);
  assert.equal(validateCandidatePayloadExecutionResult({
    ...result,
    handoffQueued: true,
  }).valid, false);
  assert.equal(validateCandidatePayloadExecutionResult({
    ...result,
    transferStatus: "queued",
  }).valid, false);
  assert.equal(validateCandidatePayloadExecutionResult({
    ...result,
    transferredBytes: 1,
  }).valid, false);
});

test("no auto-send exists and queued handoff still reports no transferred bytes or queue identifiers", async () => {
  const chain = candidatePayloadChain();
  const payload = chain.request.payload as Record<string, unknown>;
  for (const field of [
    "absolutePath",
    "filePath",
    "localPath",
    "realPath",
    "path",
    "contents",
    "transferQueueId",
    "transferQueueItemId",
    "handoffId",
    "sendFile",
    "autoSend",
  ]) {
    assert.equal(field in payload, false);
  }
  const executed = await executeInboundCandidatePayloadRequest(
    chain.request,
    chain.consent,
    createPeerConsentConsumptionState(),
    resolvedCandidatePayload,
    queueCandidatePayloadHandoff,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(executed.result.status, "handoff_queued");
  assert.equal(executed.result.transferredBytes, 0);
  assert.equal(executed.result.handoffQueued, true);
  assert.equal("transferQueueId" in executed.result, false);
  assert.equal("handoffId" in executed.result, false);
});

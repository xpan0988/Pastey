import assert from "node:assert/strict";
import test from "node:test";

import {
  buildCapabilityRequestPreviewEnvelope,
  buildFileCandidateRequestFromPendingAction,
  buildHelloPeerRequestFromPendingAction,
  buildMockAiContextSnapshot,
  buildMockFileCandidatePlan,
  buildMockHelloPeerPlan,
  confirmPendingAiAction,
  createPendingAiAction,
  evaluateAiPolicy,
  validateAiActionPlan,
  validateFileCandidateExecutionResult,
  type AiActionPlan,
  type FileCandidateExecutionRequest,
  type FileCandidateExecutionResult,
} from "../src/lib/ai";
import {
  allowPeerCapabilityOnce,
  buildFileCandidateExecutionRequest,
  buildHelloPeerExecutionRequest,
  buildPeerConsentStatusEvent,
  buildSessionBoundCapabilityPreviewControlEvent,
  createPeerConsentConsumptionState,
  createPeerConsentSessionState,
  evaluatePeerCapabilityPreview,
  executeInboundFileCandidateRequest,
  executeInboundHelloPeerRequest,
  matchExecutionResultToRequest,
  validateRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type PeerConsentRecord,
} from "../src/lib/agentBridge";

const ROOM = "room";
const SOURCE = "room-session:source";
const TARGET = "room-session:target";
const NOW = new Date("2026-06-29T00:00:00.000Z");
const ALLOWED_AT = new Date("2026-06-29T00:00:10.000Z");
const EXECUTE_AT = new Date("2026-06-29T00:00:20.000Z");

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

function fileCandidateChain(): {
  preview: CapabilityPreviewRoomControlEvent;
  consent: PeerConsentRecord;
  ack: CapabilityPreviewAckRoomControlEvent;
  request: CapabilityExecuteRequestRoomControlEvent;
} {
  const previewRequest = buildFileCandidateRequestFromPendingAction(
    confirmedPending(buildMockFileCandidatePlan(), "file-candidate-pending"),
    {
      now: NOW,
      ttlMs: 120_000,
      requestId: "file-candidate-request",
      nonce: "file-candidate-nonce",
      sourceDeviceRef: SOURCE,
    },
  );
  assert.equal(previewRequest.ok, true, previewRequest.ok ? undefined : previewRequest.errors.join(" "));
  if (!previewRequest.ok) throw new Error("Expected file-candidate preview request.");

  const envelope = buildCapabilityRequestPreviewEnvelope(previewRequest.request, {
    roomRef: ROOM,
    now: NOW,
    ttlMs: 120_000,
    envelopeId: "file-candidate-envelope",
  });
  assert.equal(envelope.ok, true, envelope.ok ? undefined : envelope.errors.join(" "));
  if (!envelope.ok) throw new Error("Expected file-candidate envelope.");

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
    consentId: "file-candidate-consent",
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
    eventId: "file-candidate-ack",
  });
  assert.equal(ack.ok, true, ack.ok ? undefined : ack.errors.join(" "));
  if (!ack.ok || ack.event.kind !== "capability_preview_ack") throw new Error("Expected ack.");

  const execution = buildFileCandidateExecutionRequest(preview.event, ack.event, {
    now: EXECUTE_AT,
    executionId: "file-candidate-execution",
    eventId: "file-candidate-execution-event",
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

function helloConsent(): PeerConsentRecord {
  const previewRequest = buildHelloPeerRequestFromPendingAction(
    confirmedPending(buildMockHelloPeerPlan(), "hello-pending"),
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
  const allowed = allowPeerCapabilityOnce(review.binding, createPeerConsentSessionState(), {
    now: ALLOWED_AT,
  });
  assert.equal(allowed.ok, true);
  if (!allowed.ok) throw new Error("Expected Hello allow once.");
  return allowed.record;
}

function hostResult(request: FileCandidateExecutionRequest): FileCandidateExecutionResult {
  return {
    schemaVersion: "filesystem-find-file-candidates-result-v1",
    capability: "filesystem.find_file_candidates",
    executionId: request.executionId,
    requestId: request.requestId,
    consentId: request.consentId,
    status: "completed",
    queryEcho: {
      filenameHint: request.input.query.filenameHint,
      extensions: [...request.input.query.extensions],
      searchMode: request.input.query.searchMode,
    },
    candidates: [{
      candidateId: "file-candidate-request-opaque-1",
      displayName: "exact-target.pdf",
      redactedLocation: "Pastey Shared/exact-target.pdf",
      extension: "pdf",
      mimeFamily: "document",
      sizeBytes: 21,
      modifiedAt: EXECUTE_AT.toISOString(),
      matchReason: "filename_exact_match",
      confidence: "high",
    }],
    omitted: {
      tooManyMatches: false,
      hiddenFilesSkipped: true,
      symlinksSkipped: true,
      scopesSkipped: [],
    },
    durationMs: 7,
    truncated: false,
    errorCode: null,
    createdAt: EXECUTE_AT.toISOString(),
  };
}

test("safe file-candidate advisory builds preview, consent, execution request, and result event", async () => {
  const chain = fileCandidateChain();

  assert.equal(chain.preview.payload.request.capability, "filesystem.find_file_candidates");
  assert.equal(chain.preview.payload.request.input.query.searchMode, "filename_metadata_only");
  assert.equal(validateRoomControlEvent(chain.preview, {
    now: NOW,
    expectedRoomRef: ROOM,
    expectedSourceDeviceRef: SOURCE,
    expectedTargetPeerRef: TARGET,
  }).valid, true);
  assert.equal(chain.consent.binding.capability, "filesystem.find_file_candidates");
  assert.equal(chain.consent.binding.requestId, chain.preview.payload.request.requestId);
  assert.equal(chain.request.payload.capability, "filesystem.find_file_candidates");
  assert.equal(chain.request.payload.executorKind, "filesystem_find_candidates_host");
  for (const forbidden of ["filePath", "absolutePath", "transferQueueItemId", "sendFile", "autoTransfer", "command", "script"]) {
    assert.equal(forbidden in chain.request.payload, false);
  }

  const executed = await executeInboundFileCandidateRequest(
    chain.request,
    chain.consent,
    createPeerConsentConsumptionState(),
    async (request) => hostResult(request),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(executed.executed, true);
  assert.equal(executed.result.status, "completed");
  assert.equal(executed.result.candidates.length, 1);
  assert.equal(validateFileCandidateExecutionResult(executed.result).valid, true);
  assert.equal(validateRoomControlEvent(executed.resultEvent, { now: EXECUTE_AT }).valid, true);
  assert.equal(matchExecutionResultToRequest(executed.resultEvent, chain.request, EXECUTE_AT), true);
  assert.equal("transferQueueItemId" in executed.result, false);
  assert.equal("sendFile" in executed.result, false);
});

test("file-candidate consent is consumed once and cannot replay", async () => {
  const chain = fileCandidateChain();
  const first = await executeInboundFileCandidateRequest(
    chain.request,
    chain.consent,
    createPeerConsentConsumptionState(),
    async (request) => hostResult(request),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(first.executed, true);

  const replay = await executeInboundFileCandidateRequest(
    chain.request,
    chain.consent,
    first.state,
    async () => {
      throw new Error("executor must not run on replay");
    },
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(replay.executed, false);
  assert.equal(replay.result.status, "already_consumed");
  assert.equal(replay.result.errorCode, "already_consumed");
});

test("Hello consent cannot authorize file-candidate search and file consent cannot authorize Hello", async () => {
  const file = fileCandidateChain();
  const rejected = await executeInboundFileCandidateRequest(
    file.request,
    helloConsent(),
    createPeerConsentConsumptionState(),
    async () => {
      throw new Error("executor must not run with Hello consent");
    },
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(rejected.executed, false);
  assert.equal(rejected.result.errorCode, "consent_binding_mismatch");

  assert.equal(buildHelloPeerExecutionRequest(file.preview, file.ack).ok, false);
  assert.notEqual(file.consent.binding.capability, "transfer.request_candidate_payload");
});

test("unsafe provider fields and selected-peers or broadcast intent reject before preview", () => {
  for (const unsafe of [
    { command: "find /" },
    { script: "walk files" },
    { cwd: "/Users/example" },
    { env: { HOME: "/Users/example" } },
    { networkTarget: "https://example.com" },
    { stdout: "candidate" },
    { stderr: "" },
    { exitCode: 0 },
    { selectedPeers: ["peer-a", "peer-b"] },
    { broadcast: true },
  ]) {
    const plan = structuredClone(buildMockFileCandidatePlan()) as AiActionPlan;
    plan.proposedInput = { ...plan.proposedInput, ...unsafe };
    const validation = validateAiActionPlan(plan);
    assert.equal(validation.valid, false, `expected rejection for ${Object.keys(unsafe)[0]}`);
  }
});

test("file-candidate result schema rejects absolute or path-like candidate data", () => {
  const chain = fileCandidateChain();
  const result = hostResult(chain.request.payload as FileCandidateExecutionRequest);
  assert.equal(validateFileCandidateExecutionResult({
    ...result,
    candidates: [{
      ...result.candidates[0],
      candidateId: "/tmp/secret.pdf",
    }],
  }).valid, false);
  assert.equal(validateFileCandidateExecutionResult({
    ...result,
    candidates: [{
      ...result.candidates[0],
      redactedLocation: "/Users/example/secret.pdf",
    }],
  }).valid, false);
});

test("file-candidate execution does not expose transfer handoff fields", () => {
  const chain = fileCandidateChain();
  const payload = chain.request.payload as Record<string, unknown>;
  for (const field of [
    "candidatePath",
    "absolutePath",
    "filePath",
    "transferQueueItemId",
    "sendFile",
    "prepareTransfer",
    "autoSend",
  ]) {
    assert.equal(field in payload, false);
  }
});

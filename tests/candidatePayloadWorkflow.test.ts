import assert from "node:assert/strict";
import test from "node:test";

import {
  buildCapabilityRequestPreviewEnvelope,
  buildMockAiContextSnapshot,
  buildMockCandidatePayloadPlan,
  buildMockFileCandidatePlan,
  validateAiActionPlan,
  type AiActionPlan,
  type CandidatePayloadExecutionRequest,
  type FileCandidateExecutionRequest,
  type FileCandidateExecutionResult,
} from "../src/lib/ai";
import {
  allowPeerCapabilityOnce,
  buildCandidatePayloadExecutionRequest,
  buildFileCandidateExecutionRequest,
  buildPeerConsentStatusEvent,
  buildSessionBoundCapabilityPreviewControlEvent,
  createCandidatePayloadWorkflow,
  createPeerConsentConsumptionState,
  createPeerConsentSessionState,
  denyPeerCapability,
  evaluatePeerCapabilityPreview,
  executeInboundCandidatePayloadRequest,
  executeInboundFileCandidateRequest,
  markCandidatePayloadWorkflowPayloadAllowed,
  markCandidatePayloadWorkflowPayloadPendingConsent,
  markCandidatePayloadWorkflowSearchAllowed,
  receiveCandidatePayloadWorkflowHandoffResult,
  receiveCandidatePayloadWorkflowSearchResult,
  startCandidatePayloadWorkflowFromSearchAdvisory,
  confirmCandidatePayloadWorkflowSearch,
  buildCandidatePayloadWorkflowPayloadPreview,
  type CandidatePayloadLocalResolution,
  type CandidatePayloadWorkflow,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type PeerConsentRecord,
} from "../src/lib/agentBridge";

const ROOM = "room";
const SOURCE = "room-session:source";
const TARGET = "room-session:target";
const NOW = new Date("2026-07-03T00:00:00.000Z");
const ALLOWED_AT = new Date("2026-07-03T00:00:10.000Z");
const EXECUTE_AT = new Date("2026-07-03T00:00:20.000Z");

function assignmentSearchPlan(): AiActionPlan {
  const plan = structuredClone(buildMockFileCandidatePlan()) as AiActionPlan;
  plan.proposedInput = {
    ...plan.proposedInput,
    targetPeerRef: "mock-peer-1",
    query: {
      rawUserRequest: "find the assignment pdf on my other device and send it here",
      filenameHint: "assignment",
      extensions: ["pdf"],
      searchMode: "filename_metadata_only",
    },
  };
  return plan;
}

function startSearchWorkflow(): CandidatePayloadWorkflow {
  const started = startCandidatePayloadWorkflowFromSearchAdvisory(
    createCandidatePayloadWorkflow(),
    assignmentSearchPlan(),
    buildMockAiContextSnapshot(),
    { now: NOW, pendingId: "workflow-search-pending" },
  );
  assert.equal(started.ok, true, started.ok ? undefined : started.errors.join(" "));
  return started.workflow;
}

function confirmSearchWorkflow(workflow = startSearchWorkflow()) {
  const confirmed = confirmCandidatePayloadWorkflowSearch(workflow, {
    now: NOW,
    requestId: "workflow-search-request",
    nonce: "workflow-search-nonce",
    sourceDeviceRef: SOURCE,
  });
  assert.equal(confirmed.ok, true, confirmed.ok ? undefined : confirmed.errors.join(" "));
  return confirmed;
}

function buildPreview(
  request: Parameters<typeof buildCapabilityRequestPreviewEnvelope>[0],
  envelopeId: string,
): CapabilityPreviewRoomControlEvent {
  const envelope = buildCapabilityRequestPreviewEnvelope(request, {
    roomRef: ROOM,
    now: NOW,
    ttlMs: 120_000,
    envelopeId,
  });
  assert.equal(envelope.ok, true, envelope.ok ? undefined : envelope.errors.join(" "));
  if (!envelope.ok) throw new Error("Expected envelope.");
  const preview = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, {
    roomId: ROOM,
    localSessionRef: SOURCE,
    peerSessionRef: TARGET,
    peerConnected: true,
  }, { now: NOW });
  assert.equal(preview.ok, true, preview.ok ? undefined : preview.errors.join(" "));
  if (!preview.ok || preview.event.kind !== "capability_preview") throw new Error("Expected preview.");
  return preview.event;
}

function allowPreview(
  preview: CapabilityPreviewRoomControlEvent,
  consentId: string,
): { consent: PeerConsentRecord; ack: CapabilityPreviewAckRoomControlEvent } {
  const review = evaluatePeerCapabilityPreview(preview, {
    roomRef: ROOM,
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    session: createPeerConsentSessionState(),
    now: ALLOWED_AT,
    consentId,
  });
  assert.equal(review.status, "reviewable", review.status === "rejected" ? review.errors.join(" ") : undefined);
  if (review.status !== "reviewable") throw new Error("Expected reviewable preview.");
  const allowed = allowPeerCapabilityOnce(review.binding, createPeerConsentSessionState(), { now: ALLOWED_AT });
  assert.equal(allowed.ok, true, allowed.ok ? undefined : allowed.errors.join(" "));
  if (!allowed.ok) throw new Error("Expected Allow once.");
  const ack = buildPeerConsentStatusEvent(preview, allowed.record, {
    now: ALLOWED_AT,
    eventId: `${consentId}-ack`,
  });
  assert.equal(ack.ok, true, ack.ok ? undefined : ack.errors.join(" "));
  if (!ack.ok || ack.event.kind !== "capability_preview_ack") throw new Error("Expected ack.");
  return { consent: allowed.record, ack: ack.event };
}

function searchHostResult(request: FileCandidateExecutionRequest): FileCandidateExecutionResult {
  return {
    schemaVersion: "filesystem-find-file-candidates-result-v1",
    capability: "filesystem.find_file_candidates",
    executionId: request.executionId,
    requestId: request.requestId,
    consentId: request.consentId,
    status: "completed",
    queryEcho: {
      filenameHint: request.input.query.filenameHint,
      extensions: request.input.query.extensions,
      searchMode: request.input.query.searchMode,
    },
    candidates: [{
      candidateId: `${request.requestId}-opaque-1`,
      displayName: "assignment.pdf",
      redactedLocation: "Pastey Shared/assignment.pdf",
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
    durationMs: 12,
    truncated: false,
    errorCode: null,
    createdAt: EXECUTE_AT.toISOString(),
  };
}

async function workflowWithCandidates() {
  let workflow = confirmSearchWorkflow().workflow;
  const preview = buildPreview(workflow.searchRequest!, "workflow-search-envelope");
  const { consent, ack } = allowPreview(preview, "workflow-search-consent");
  const allowed = markCandidatePayloadWorkflowSearchAllowed(workflow);
  assert.equal(allowed.ok, true);
  workflow = allowed.workflow;
  const execution = buildFileCandidateExecutionRequest(preview, ack, {
    now: EXECUTE_AT,
    executionId: "workflow-search-execution",
    eventId: "workflow-search-execution-event",
  });
  assert.equal(execution.ok, true, execution.ok ? undefined : execution.errors.join(" "));
  if (!execution.ok) throw new Error("Expected search execution request.");
  const executed = await executeInboundFileCandidateRequest(
    execution.event,
    consent,
    createPeerConsentConsumptionState(),
    async (request) => searchHostResult(request),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  const received = receiveCandidatePayloadWorkflowSearchResult(workflow, executed.result);
  assert.equal(received.ok, true, received.ok ? undefined : received.errors.join(" "));
  return {
    workflow: received.workflow,
    searchConsent: consent,
    searchAck: ack,
    searchExecution: execution.event,
  };
}

async function workflowWithPayloadPreview() {
  const withCandidates = await workflowWithCandidates();
  const selected = buildCandidatePayloadWorkflowPayloadPreview(
    withCandidates.workflow,
    { candidateId: "workflow-search-request-opaque-1", selectedByUser: true },
    buildMockAiContextSnapshot(),
    {
      now: EXECUTE_AT,
      requestId: "workflow-payload-request",
      nonce: "workflow-payload-nonce",
      sourceDeviceRef: SOURCE,
      pendingId: "workflow-payload-pending",
    },
  );
  assert.equal(selected.ok, true, selected.ok ? undefined : selected.errors.join(" "));
  return selected.workflow;
}

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

test("natural-language advisory starts only the search capability", () => {
  const started = startCandidatePayloadWorkflowFromSearchAdvisory(
    createCandidatePayloadWorkflow(),
    assignmentSearchPlan(),
    buildMockAiContextSnapshot(),
    { now: NOW, pendingId: "workflow-search-pending-safe" },
  );
  assert.equal(started.ok, true, started.ok ? undefined : started.errors.join(" "));
  assert.equal(started.workflow.snapshot.state, "search_preview_ready");
  assert.equal(started.workflow.snapshot.search?.capability, "filesystem.find_file_candidates");
  assert.equal(started.workflow.snapshot.search?.filenameHint, "assignment");

  const directPayload = startCandidatePayloadWorkflowFromSearchAdvisory(
    createCandidatePayloadWorkflow(),
    buildMockCandidatePayloadPlan(),
    buildMockAiContextSnapshot(),
    { now: NOW },
  );
  assert.equal(directPayload.ok, false);
  assert.match(directPayload.errors.join(" "), /filesystem\.find_file_candidates/);
});

test("workflow cannot jump to payload request or let AI auto-select candidate", async () => {
  const fromIdle = buildCandidatePayloadWorkflowPayloadPreview(
    createCandidatePayloadWorkflow(),
    { candidateId: "candidate", selectedByUser: true },
    buildMockAiContextSnapshot(),
  );
  assert.equal(fromIdle.ok, false);

  const { workflow } = await workflowWithCandidates();
  const autoSelected = buildCandidatePayloadWorkflowPayloadPreview(
    workflow,
    { candidateId: "workflow-search-request-opaque-1", selectedByUser: false },
    buildMockAiContextSnapshot(),
  );
  assert.equal(autoSelected.ok, false);
  assert.match(autoSelected.errors.join(" "), /explicit user candidate selection/);
});

test("unsafe provider fields and path-like candidate metadata reject", async () => {
  const unsafePlan = assignmentSearchPlan();
  unsafePlan.proposedInput = {
    ...unsafePlan.proposedInput,
    absolutePath: "/Users/example/assignment.pdf",
  };
  const unsafeValidation = validateAiActionPlan(unsafePlan);
  assert.equal(unsafeValidation.valid, false);
  const unsafeStarted = startCandidatePayloadWorkflowFromSearchAdvisory(
    createCandidatePayloadWorkflow(),
    unsafePlan,
    buildMockAiContextSnapshot(),
  );
  assert.equal(unsafeStarted.ok, false);

  const confirmed = confirmSearchWorkflow();
  const result = searchHostResult({
    schemaVersion: "filesystem-find-file-candidates-execution-request-v1",
    executionId: "execution",
    consentId: "consent",
    sourcePreviewEventId: "preview",
    envelopeId: "envelope",
    requestId: confirmed.request.requestId,
    requestPayloadHash: confirmed.request.requestPayloadHash,
    roomRef: ROOM,
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    capability: "filesystem.find_file_candidates",
    executorKind: "filesystem_find_candidates_host",
    input: confirmed.request.input,
    createdAt: EXECUTE_AT.toISOString(),
    expiresAt: new Date(EXECUTE_AT.getTime() + 60_000).toISOString(),
  });
  result.candidates[0] = {
    ...result.candidates[0],
    redactedLocation: "/Users/example/assignment.pdf",
  };
  const received = receiveCandidatePayloadWorkflowSearchResult(confirmed.workflow, result);
  assert.equal(received.ok, false);
});

test("successful deterministic workflow reaches handoff_queued without exposing paths or contents", async () => {
  let workflow = await workflowWithPayloadPreview();
  const pending = markCandidatePayloadWorkflowPayloadPendingConsent(workflow);
  assert.equal(pending.ok, true);
  workflow = pending.workflow;
  const preview = buildPreview(workflow.payloadRequest!, "workflow-payload-envelope");
  const { consent, ack } = allowPreview(preview, "workflow-payload-consent");
  const allowed = markCandidatePayloadWorkflowPayloadAllowed(workflow);
  assert.equal(allowed.ok, true);
  workflow = allowed.workflow;
  const execution = buildCandidatePayloadExecutionRequest(preview, ack, {
    now: EXECUTE_AT,
    executionId: "workflow-payload-execution",
    eventId: "workflow-payload-execution-event",
  });
  assert.equal(execution.ok, true, execution.ok ? undefined : execution.errors.join(" "));
  if (!execution.ok) throw new Error("Expected payload execution request.");
  let handoffCalls = 0;
  const executed = await executeInboundCandidatePayloadRequest(
    execution.event,
    consent,
    createPeerConsentConsumptionState(),
    resolvedCandidatePayload,
    async () => {
      handoffCalls += 1;
      return { queued: true as const };
    },
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  const completed = receiveCandidatePayloadWorkflowHandoffResult(workflow, executed.result);
  assert.equal(completed.ok, true, completed.ok ? undefined : completed.errors.join(" "));
  assert.equal(completed.workflow.snapshot.state, "handoff_queued");
  assert.equal(completed.workflow.snapshot.handoff?.status, "handoff_queued");
  assert.equal(completed.workflow.snapshot.handoff?.transferredBytes, 0);
  assert.equal(handoffCalls, 1);
  const serialized = JSON.stringify(completed.workflow.snapshot);
  assert.equal(serialized.includes("/receiver/local"), false);
  assert.equal(serialized.includes("absolutePath"), false);
  assert.equal(serialized.includes("localPath"), false);
  assert.equal(serialized.includes("contents"), false);
  assert.equal(serialized.includes("transferQueueId"), false);
  assert.equal(serialized.includes("handoffId"), false);
});

test("discovery consent and payload consent remain separate authorities", async () => {
  const { workflow, searchConsent, searchAck } = await workflowWithCandidates();
  const payloadPreview = buildCandidatePayloadWorkflowPayloadPreview(
    workflow,
    { candidateId: "workflow-search-request-opaque-1", selectedByUser: true },
    buildMockAiContextSnapshot(),
    {
      now: EXECUTE_AT,
      requestId: "workflow-payload-request-authority",
      nonce: "workflow-payload-nonce-authority",
      sourceDeviceRef: SOURCE,
      pendingId: "workflow-payload-pending-authority",
    },
  );
  assert.equal(payloadPreview.ok, true);
  if (!payloadPreview.ok) throw new Error("Expected payload preview.");
  const payloadRoomPreview = buildPreview(payloadPreview.request, "workflow-payload-authority-envelope");
  const { consent: payloadConsent, ack: payloadAck } = allowPreview(payloadRoomPreview, "workflow-payload-authority-consent");
  const payloadExecution = buildCandidatePayloadExecutionRequest(payloadRoomPreview, payloadAck, { now: EXECUTE_AT });
  assert.equal(payloadExecution.ok, true);
  if (!payloadExecution.ok) throw new Error("Expected payload execution.");

  const searchCannotAuthorizePayload = await executeInboundCandidatePayloadRequest(
    payloadExecution.event,
    searchConsent,
    createPeerConsentConsumptionState(),
    resolvedCandidatePayload,
    async () => ({ queued: true as const }),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(searchCannotAuthorizePayload.executed, false);
  assert.equal(searchCannotAuthorizePayload.result.errorCode, "consent_binding_mismatch");
  assert.equal(buildFileCandidateExecutionRequest(payloadRoomPreview, payloadAck).ok, false);
  assert.equal(buildCandidatePayloadExecutionRequest(buildPreview(workflow.searchRequest!, "workflow-search-authority-envelope"), searchAck).ok, false);
  assert.equal(payloadConsent.binding.capability, "transfer.request_candidate_payload");
});

test("wrong selected candidate source binding fails before payload preview", async () => {
  const { workflow } = await workflowWithCandidates();
  const tampered: CandidatePayloadWorkflow = {
    ...workflow,
    snapshot: {
      ...workflow.snapshot,
      candidates: workflow.snapshot.candidates?.map((candidate) => ({
        ...candidate,
        sourceRequestId: "other-search-request",
      })),
    },
  };
  const selected = buildCandidatePayloadWorkflowPayloadPreview(
    tampered,
    { candidateId: "workflow-search-request-opaque-1", selectedByUser: true },
    buildMockAiContextSnapshot(),
  );
  assert.equal(selected.ok, false);
  assert.match(selected.errors.join(" "), /active source request/);
});

test("denied search and denied payload stop before enqueue", async () => {
  const confirmed = confirmSearchWorkflow();
  const deniedSearch = receiveCandidatePayloadWorkflowSearchResult(confirmed.workflow, {
    ...searchHostResult({
      schemaVersion: "filesystem-find-file-candidates-execution-request-v1",
      executionId: "execution",
      consentId: "consent",
      sourcePreviewEventId: "preview",
      envelopeId: "envelope",
      requestId: confirmed.request.requestId,
      requestPayloadHash: confirmed.request.requestPayloadHash,
      roomRef: ROOM,
      sourceDeviceRef: SOURCE,
      targetPeerRef: TARGET,
      capability: "filesystem.find_file_candidates",
      executorKind: "filesystem_find_candidates_host",
      input: confirmed.request.input,
      createdAt: EXECUTE_AT.toISOString(),
      expiresAt: new Date(EXECUTE_AT.getTime() + 60_000).toISOString(),
    }),
    status: "rejected",
    candidates: [],
    errorCode: "policy_rejected",
  });
  assert.equal(deniedSearch.ok, false);
  const afterDeniedSearch = buildCandidatePayloadWorkflowPayloadPreview(
    deniedSearch.workflow,
    { candidateId: "workflow-search-request-opaque-1", selectedByUser: true },
    buildMockAiContextSnapshot(),
  );
  assert.equal(afterDeniedSearch.ok, false);

  let workflow = await workflowWithPayloadPreview();
  const pending = markCandidatePayloadWorkflowPayloadPendingConsent(workflow);
  assert.equal(pending.ok, true);
  workflow = pending.workflow;
  const preview = buildPreview(workflow.payloadRequest!, "workflow-payload-denied-envelope");
  const review = evaluatePeerCapabilityPreview(preview, {
    roomRef: ROOM,
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    session: createPeerConsentSessionState(),
    now: ALLOWED_AT,
    consentId: "workflow-payload-denied-consent",
  });
  assert.equal(review.status, "reviewable");
  if (review.status !== "reviewable") throw new Error("Expected reviewable payload preview.");
  const denied = denyPeerCapability(review.binding, createPeerConsentSessionState(), { now: ALLOWED_AT });
  assert.equal(denied.ok, true);
  if (!denied.ok) throw new Error("Expected denial.");
  const deniedAck = buildPeerConsentStatusEvent(preview, denied.record, { now: ALLOWED_AT });
  assert.equal(deniedAck.ok, true);
  if (!deniedAck.ok) throw new Error("Expected denied status event.");
  assert.equal(deniedAck.event.kind, "capability_preview_deny");
});

test("expired, changed, or deleted candidates do not enqueue and replayed consent fails", async () => {
  for (const scenario of [
    { label: "expired", reason: "expired" as const, status: "candidate_expired" },
    { label: "changed", reason: "changed" as const, status: "candidate_changed" },
    { label: "deleted", reason: "not_found" as const, status: "candidate_not_found" },
  ]) {
    const workflow = await workflowWithPayloadPreview();
    const preview = buildPreview(workflow.payloadRequest!, `workflow-payload-${scenario.label}-envelope`);
    const { consent, ack } = allowPreview(preview, `workflow-payload-${scenario.label}-consent`);
    const execution = buildCandidatePayloadExecutionRequest(preview, ack, {
      now: EXECUTE_AT,
      executionId: `workflow-payload-${scenario.label}-execution`,
    });
    assert.equal(execution.ok, true);
    if (!execution.ok) throw new Error("Expected payload execution.");
    let handoffCalls = 0;
    const executed = await executeInboundCandidatePayloadRequest(
      execution.event,
      consent,
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
    assert.equal(executed.result.transferredBytes, 0, scenario.label);
    assert.equal(handoffCalls, 0, scenario.label);
    const pendingWorkflow = markCandidatePayloadWorkflowPayloadPendingConsent(workflow);
    assert.equal(pendingWorkflow.ok, true, scenario.label);
    const completed = receiveCandidatePayloadWorkflowHandoffResult(pendingWorkflow.workflow, executed.result);
    assert.equal(completed.ok, false, scenario.label);
  }

  const workflow = await workflowWithPayloadPreview();
  const preview = buildPreview(workflow.payloadRequest!, "workflow-payload-replay-envelope");
  const { consent, ack } = allowPreview(preview, "workflow-payload-replay-consent");
  const execution = buildCandidatePayloadExecutionRequest(preview, ack, { now: EXECUTE_AT });
  assert.equal(execution.ok, true);
  if (!execution.ok) throw new Error("Expected payload execution.");
  const first = await executeInboundCandidatePayloadRequest(
    execution.event,
    consent,
    createPeerConsentConsumptionState(),
    resolvedCandidatePayload,
    async () => ({ queued: true as const }),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  const replay = await executeInboundCandidatePayloadRequest(
    execution.event,
    consent,
    first.state,
    resolvedCandidatePayload,
    async () => ({ queued: true as const }),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(replay.executed, false);
  assert.equal(replay.result.status, "already_consumed");
});

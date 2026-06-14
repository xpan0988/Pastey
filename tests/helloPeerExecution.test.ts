import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  buildCapabilityRequestPreviewEnvelope,
  buildHelloPeerRequestFromPendingAction,
  buildMockAiContextSnapshot,
  buildMockHelloPeerPlan,
  confirmPendingAiAction,
  createPendingAiAction,
  evaluateAiPolicy,
} from "../src/lib/ai";
import {
  allowPeerCapabilityOnce,
  buildHelloPeerExecutionRequest,
  buildPeerConsentStatusEvent,
  buildSessionBoundCapabilityPreviewControlEvent,
  checkAndRecordRoomControlEvent,
  createControlQueueState,
  createPeerConsentConsumptionState,
  createPeerConsentSessionState,
  createRoomControlEventSessionState,
  denyPeerCapability,
  enqueueRoomControlEvent,
  evaluatePeerCapabilityPreview,
  executeHelloPeerTemplate,
  executeInboundHelloPeerRequest,
  hasOutgoingControlWindowDemand,
  matchExecutionResultToRequest,
  preservePeerConsentConsumptionState,
  validateHelloPeerExecutionResult,
  validateRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type PeerConsentRecord,
} from "../src/lib/agentBridge";

const NOW = new Date("2026-06-13T00:00:00.000Z");
const ALLOWED_AT = new Date("2026-06-13T00:00:10.000Z");
const EXECUTE_AT = new Date("2026-06-13T00:00:20.000Z");
const ROOM = "room";
const SOURCE = "room-session:source";
const TARGET = "room-session:target";

function chain(): {
  preview: CapabilityPreviewRoomControlEvent;
  consent: PeerConsentRecord;
  ack: CapabilityPreviewAckRoomControlEvent;
  request: CapabilityExecuteRequestRoomControlEvent;
} {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = confirmPendingAiAction(
    createPendingAiAction(plan, policy, {
      now: new Date("2026-06-12T23:59:00.000Z"),
      ttlMs: 300_000,
      pendingId: "pending",
    }),
    new Date("2026-06-12T23:59:30.000Z"),
  );
  const request = buildHelloPeerRequestFromPendingAction(pending, {
    now: NOW,
    ttlMs: 120_000,
    requestId: "request",
    nonce: "nonce",
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
  });
  assert.equal(request.ok, true);
  if (!request.ok) throw new Error("Expected request.");
  const envelope = buildCapabilityRequestPreviewEnvelope(request.request, {
    roomRef: ROOM,
    now: NOW,
    ttlMs: 120_000,
    envelopeId: "envelope",
  });
  assert.equal(envelope.ok, true);
  if (!envelope.ok) throw new Error("Expected envelope.");
  const preview = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, {
    roomId: ROOM,
    localSessionRef: SOURCE,
    peerSessionRef: TARGET,
    peerConnected: true,
  }, { now: NOW });
  assert.equal(preview.ok, true);
  if (!preview.ok || preview.event.kind !== "capability_preview") throw new Error("Expected preview.");
  const review = evaluatePeerCapabilityPreview(preview.event, {
    roomRef: ROOM,
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    session: createPeerConsentSessionState(),
    now: ALLOWED_AT,
    consentId: "consent",
  });
  assert.equal(review.status, "reviewable");
  if (review.status !== "reviewable") throw new Error("Expected review.");
  const consent = allowPeerCapabilityOnce(review.binding, createPeerConsentSessionState(), {
    now: ALLOWED_AT,
  });
  assert.equal(consent.ok, true);
  if (!consent.ok) throw new Error("Expected consent.");
  const ack = buildPeerConsentStatusEvent(preview.event, consent.record, {
    now: ALLOWED_AT,
    eventId: "ack",
  });
  assert.equal(ack.ok, true);
  if (!ack.ok || ack.event.kind !== "capability_preview_ack") throw new Error("Expected ack.");
  const execution = buildHelloPeerExecutionRequest(preview.event, ack.event, {
    now: EXECUTE_AT,
    executionId: "execution",
    eventId: "execution-event",
  });
  assert.equal(execution.ok, true);
  if (!execution.ok) throw new Error("Expected execution request.");
  return { preview: preview.event, consent: consent.record, ack: ack.event, request: execution.event };
}

test("allowed-once ack builds one exact bounded execution request", () => {
  const value = chain();
  assert.equal(value.request.payload.capability, "runtime.execute_hello_template");
  assert.equal(value.request.payload.exactMessage, "hello peer!");
  assert.equal(value.request.payload.consentId, value.consent.binding.consentId);
  assert.equal(value.request.payload.requestPayloadHash, value.consent.binding.requestPayloadHash);
  assert.equal(value.request.previewOnly, false);
  assert.equal(validateRoomControlEvent(value.request, { now: EXECUTE_AT }).valid, true);
  for (const field of ["command", "script", "code", "path", "stdin", "environment", "arguments"]) {
    assert.equal(field in value.request.payload, false);
  }
});

test("denied, expired, or mismatched consent grant cannot build an execution request", () => {
  const value = chain();
  const review = evaluatePeerCapabilityPreview(value.preview, {
    roomRef: ROOM,
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    session: createPeerConsentSessionState(),
    now: ALLOWED_AT,
    consentId: "denied-consent",
  });
  assert.equal(review.status, "reviewable");
  if (review.status !== "reviewable") return;
  const denied = denyPeerCapability(review.binding, createPeerConsentSessionState(), { now: ALLOWED_AT });
  assert.equal(denied.ok, true);
  if (!denied.ok) return;
  const denyEvent = buildPeerConsentStatusEvent(value.preview, denied.record, { now: ALLOWED_AT });
  assert.equal(denyEvent.ok, true);
  assert.equal(denyEvent.ok && denyEvent.event.kind === "capability_preview_deny", true);
  if (denyEvent.ok) {
    assert.equal(buildHelloPeerExecutionRequest(
      value.preview,
      denyEvent.event as unknown as CapabilityPreviewAckRoomControlEvent,
      { now: EXECUTE_AT },
    ).ok, false);
  }

  const mismatchedAck = {
    ...value.ack,
    payload: {
      ...value.ack.payload,
      consent: { ...value.ack.payload.consent!, requestPayloadHash: "wrong-hash" },
    },
  };
  assert.equal(buildHelloPeerExecutionRequest(value.preview, mismatchedAck, { now: EXECUTE_AT }).ok, false);
  assert.equal(buildHelloPeerExecutionRequest(value.preview, value.ack, {
    now: new Date("2026-06-13T00:03:00.000Z"),
  }).ok, false);
});

test("receiver consumes exact consent once and executes the fixed template once", () => {
  const value = chain();
  const first = executeInboundHelloPeerRequest(
    value.request,
    value.consent,
    createPeerConsentConsumptionState(),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(first.executed, true);
  assert.equal(first.result.status, "succeeded");
  assert.equal(first.result.output, "hello peer!");
  assert.equal(new TextEncoder().encode(first.result.output).byteLength <= 64, true);
  assert.deepEqual(first.state.consumedConsentIds, ["consent"]);
  const second = executeInboundHelloPeerRequest(
    value.request,
    value.consent,
    first.state,
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(second.executed, false);
  assert.equal(second.result.status, "already_consumed");
  assert.equal(second.result.output, undefined);
});

test("binding mismatch and session change fail closed", () => {
  const value = chain();
  const wrongConsent = {
    ...value.consent,
    binding: { ...value.consent.binding, requestPayloadHash: "wrong" },
  };
  const rejected = executeInboundHelloPeerRequest(
    value.request,
    wrongConsent,
    createPeerConsentConsumptionState(),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(rejected.executed, false);
  assert.equal(rejected.result.status, "rejected");
  const missing = executeInboundHelloPeerRequest(
    value.request,
    undefined,
    createPeerConsentConsumptionState(),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.equal(missing.executed, false);
  assert.equal(missing.result.errorCode, "missing_consent");
  assert.deepEqual(
    preservePeerConsentConsumptionState(rejected.state, "old-session", "new-session"),
    createPeerConsentConsumptionState(),
  );
});

test("wrong capability, message, and unknown request fields are rejected before execution", () => {
  const value = chain();
  for (const payload of [
    { ...value.request.payload, capability: "unknown" },
    { ...value.request.payload, exactMessage: "arbitrary" },
    { ...value.request.payload, arguments: [] },
  ]) {
    const candidate = { ...value.request, payload };
    assert.equal(validateRoomControlEvent(candidate, { now: EXECUTE_AT }).valid, false);
  }
});

test("timeout after execution begins still consumes consent", () => {
  const value = chain();
  const ticks = [0, 1_001];
  const result = executeInboundHelloPeerRequest(
    value.request,
    value.consent,
    createPeerConsentConsumptionState(),
    {
      roomRef: ROOM,
      sourceDeviceRef: SOURCE,
      targetPeerRef: TARGET,
      now: EXECUTE_AT,
      nowMs: () => ticks.shift() ?? 1_001,
    },
  );
  assert.equal(result.executed, true);
  assert.equal(result.result.status, "failed");
  assert.equal(result.result.errorCode, "execution_timeout");
  assert.deepEqual(result.state.consumedExecutionIds, ["execution"]);
});

test("fixed executor accepts no parameters and source contains no platform execution APIs", () => {
  assert.equal(executeHelloPeerTemplate.length, 0);
  assert.equal(executeHelloPeerTemplate(), "hello peer!");
  const source = readFileSync("src/lib/agentBridge/helloPeerExecution.ts", "utf8");
  assert.doesNotMatch(source, /\b(child_process|node:process|node:fs|node:net|node:http|node:https|eval|Command|spawn|execFile|stdout|stderr|exitCode|MCP|clipboard)\b/);
});

test("result schema permits only fixed success output and bounded errors", () => {
  const value = chain();
  const executed = executeInboundHelloPeerRequest(
    value.request,
    value.consent,
    createPeerConsentConsumptionState(),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  assert.deepEqual(validateHelloPeerExecutionResult(executed.result), []);
  assert.notDeepEqual(validateHelloPeerExecutionResult({ ...executed.result, output: "arbitrary" }), []);
  assert.notDeepEqual(validateHelloPeerExecutionResult({ ...executed.result, stdout: "no" }), []);
  assert.notDeepEqual(validateHelloPeerExecutionResult({ ...executed.result, stderr: "no" }), []);
  assert.notDeepEqual(validateHelloPeerExecutionResult({ ...executed.result, exitCode: 0 }), []);
  assert.notDeepEqual(validateHelloPeerExecutionResult({
    ...executed.result,
    status: "failed",
    output: undefined,
    errorCode: "x".repeat(65),
  }), []);
  assert.equal(matchExecutionResultToRequest(executed.resultEvent, value.request, EXECUTE_AT), true);
});

test("execution request/result use the existing queue, activate demand, and reject replay", () => {
  const value = chain();
  const outbound = enqueueRoomControlEvent(createControlQueueState(), value.request, "outbound", {
    now: EXECUTE_AT,
  });
  assert.equal(outbound.ok, true);
  if (!outbound.ok) return;
  assert.equal(hasOutgoingControlWindowDemand(outbound.state, { status: "idle" }, { now: EXECUTE_AT }), true);
  assert.equal("retry" in outbound.item, false);
  const executed = executeInboundHelloPeerRequest(
    value.request,
    value.consent,
    createPeerConsentConsumptionState(),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTE_AT },
  );
  const inboundResult = enqueueRoomControlEvent(outbound.state, executed.resultEvent, "inbound", {
    now: EXECUTE_AT,
  });
  assert.equal(inboundResult.ok, true);
  if (!inboundResult.ok) return;
  assert.equal(inboundResult.state.inbound[0]?.event.kind, "capability_execution_result");
  assert.equal(enqueueRoomControlEvent(inboundResult.state, executed.resultEvent, "inbound", {
    now: EXECUTE_AT,
  }).ok, false);

  const replayState = checkAndRecordRoomControlEvent(
    executed.resultEvent,
    createRoomControlEventSessionState(),
    { now: EXECUTE_AT },
  );
  assert.equal(replayState.ok, true);
  if (!replayState.ok) return;
  assert.equal(checkAndRecordRoomControlEvent(executed.resultEvent, replayState.state, { now: EXECUTE_AT }).ok, false);
});

test("UI exposes explicit bounded execution controls outside chat and no generic fields", () => {
  const panel = readFileSync("src/components/agentBridge/RoomControlPanel.tsx", "utf8");
  assert.match(panel, /data-testid="agent-bridge-request-hello-execution"/);
  assert.match(panel, /Request Hello Peer execution/);
  assert.match(panel, /data-testid="agent-bridge-execution-result-card"/);
  assert.match(panel, /One-time consent consumed\. Hello Peer demo executed once\./);
  assert.match(panel, /item\.event\.kind === "capability_preview_ack" && item\.event\.payload\.consent/);
  assert.match(panel, /Date\.parse\(senderExecutionAck\.payload\.consent\.expiresAt\) > controlDemandNowMs/);
  assert.doesNotMatch(panel, /Run again|Always allow|command input|script editor|working directory|environment field/);
});

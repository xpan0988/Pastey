import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  buildArtifactTransformRequest,
  buildCapabilityRequestPreviewEnvelope,
  validateArtifactTransformExecutionResult,
  validateAskBridgeNaturalV1Plan,
} from "../src/lib/ai";
import {
  allowPeerCapabilityOnce,
  buildArtifactTransformExecutionRequest,
  buildCapabilityPreviewControlEvent,
  buildPeerConsentStatusEvent,
  createPeerConsentConsumptionState,
  createPeerConsentSessionState,
  evaluatePeerCapabilityPreview,
  executeInboundArtifactTransformRequest,
  unavailableTransformExecutor,
} from "../src/lib/agentBridge";

const NOW = new Date("2026-07-11T00:00:00.000Z");
const ROOM = "room-transform";
const SOURCE = "room-session:source";
const TARGET = "room-session:target";

function chain() {
  const request = buildArtifactTransformRequest({
    sourceCapability: "filesystem.find_file_candidates",
    sourceRequestId: "search-1",
    candidateId: "candidate-1",
    candidateKind: "filesystem_file",
    resultContract: "typed_transform_result",
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    now: NOW,
    requestId: "transform-request",
    nonce: "transform-nonce",
  });
  assert.equal(request.ok, true, request.ok ? "" : request.errors.join(" "));
  if (!request.ok) throw new Error(request.errors.join(" "));
  const envelope = buildCapabilityRequestPreviewEnvelope(request.request, { roomRef: ROOM, now: NOW, envelopeId: "transform-envelope" });
  if (!envelope.ok) throw new Error(`envelope: ${envelope.errors.join(" ")}`);
  const preview = buildCapabilityPreviewControlEvent(envelope.envelope, { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: NOW, eventId: "transform-preview" });
  if (!preview.ok || preview.event.kind !== "capability_preview") throw new Error(`preview: ${preview.ok ? "wrong kind" : preview.errors.join(" ")}`);
  const review = evaluatePeerCapabilityPreview(preview.event, { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, session: createPeerConsentSessionState(), now: new Date("2026-07-11T00:00:05.000Z"), consentId: "transform-consent" });
  if (review.status !== "reviewable") throw new Error(review.errors.join(" "));
  const allowed = allowPeerCapabilityOnce(review.binding, createPeerConsentSessionState(), { now: new Date("2026-07-11T00:00:05.000Z") });
  if (!allowed.ok) throw new Error(allowed.errors.join(" "));
  const acknowledgement = buildPeerConsentStatusEvent(preview.event, allowed.record, { now: new Date("2026-07-11T00:00:05.000Z"), eventId: "transform-ack" });
  if (!acknowledgement.ok || acknowledgement.event.kind !== "capability_preview_ack") throw new Error("ack");
  const execution = buildArtifactTransformExecutionRequest(preview.event, acknowledgement.event, { now: new Date("2026-07-11T00:00:10.000Z"), executionId: "transform-execution", eventId: "transform-execution-event" });
  if (!execution.ok) throw new Error(`execution: ${execution.errors.join(" ")}`);
  return { request: request.request, preview: preview.event, consent: allowed.record, execution };
}

test("host-built Transform request binds the selected candidate and returns a typed result without transfer", async () => {
  const value = chain();
  const executed = await executeInboundArtifactTransformRequest(
    value.execution.event,
    value.consent,
    createPeerConsentConsumptionState(),
    async () => "claimed",
    async (request) => ({
      schemaVersion: "artifact-transform-selected-result-v1",
      capability: "artifact.transform_selected",
      executionId: request.executionId,
      requestId: request.requestId,
      consentId: request.consentId,
      status: "completed",
      result: { kind: "typed_transform_result", output: { kind: "process_output", stdout: "test-only", stderr: "", exitCode: 0, durationMs: 1, timedOut: false, stdoutTruncated: false, stderrTruncated: false } },
      createdAt: NOW.toISOString(),
    }),
    { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: new Date("2026-07-11T00:00:10.000Z") },
  );
  assert.equal(executed.result.status, "completed");
  assert.equal(executed.resultEvent.payload.capability, "artifact.transform_selected");
  assert.equal("handoffQueued" in executed.resultEvent.payload, false);
  assert.deepEqual(executed.state.consumedConsentIds, ["transform-consent"]);
});

test("Transform rejects mismatched contract, claimed candidate, replay, and unavailable production execution", async () => {
  const value = chain();
  const mismatch = structuredClone(value.execution.event);
  mismatch.payload.resultContract = "other" as "typed_transform_result";
  const rejected = await executeInboundArtifactTransformRequest(mismatch, value.consent, createPeerConsentConsumptionState(), async () => "claimed", unavailableTransformExecutor, { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: new Date("2026-07-11T00:00:10.000Z") });
  assert.equal(rejected.result.status, "rejected");
  const unavailable = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), async () => "claimed", unavailableTransformExecutor, { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: new Date("2026-07-11T00:00:10.000Z") });
  assert.equal(unavailable.result.errorCode, "sandbox_unavailable");
  const replay = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, unavailable.state, async () => "claimed", unavailableTransformExecutor, { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: new Date("2026-07-11T00:00:10.000Z") });
  assert.equal(replay.result.status, "already_consumed");
});

test("result validator permits process output only in a completed Transform result", () => {
  const valid = { schemaVersion: "artifact-transform-selected-result-v1", capability: "artifact.transform_selected", executionId: "e", requestId: "r", consentId: "c", status: "completed", result: { kind: "typed_transform_result", output: { kind: "process_output", stdout: "", stderr: "", exitCode: 0, durationMs: 0, timedOut: false, stdoutTruncated: false, stderrTruncated: false } }, createdAt: NOW.toISOString() };
  assert.equal(validateArtifactTransformExecutionResult(valid).valid, true);
  assert.equal(validateArtifactTransformExecutionResult({ ...valid, stdout: "leak" }).valid, false);
  assert.equal(validateArtifactTransformExecutionResult({ ...valid, status: "failed", errorCode: "executor_failed" }).valid, false);
});

test("provider Transform remains bounded and production code contains no fake or process fallback", () => {
  const unsupported = { schemaVersion: "ask-bridge-natural-v1", title: "Search Transform Return", status: "unsupported_future", unsupportedReason: "unsupported", requiresUserConfirmation: true, steps: [
    { primitive: "Search", filenameHint: "report", extensions: ["txt"], safeScopes: ["downloads"] },
    { primitive: "Transform", transformKind: "python" },
    { primitive: "Return", destination: "this_device", returnKind: "typed_transform_result", requiresSecondConsent: false },
  ] };
  assert.equal(validateAskBridgeNaturalV1Plan(unsupported).valid, true, JSON.stringify(validateAskBridgeNaturalV1Plan(unsupported)));
  const source = readFileSync("src/lib/agentBridge/transformExecution.ts", "utf8");
  assert.doesNotMatch(source, /child_process|node:process|spawn\(|execFile|Command::new|deterministicFake|fakeExecutor/i);
});

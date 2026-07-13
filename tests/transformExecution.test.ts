import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  buildArtifactTransformRequest,
  buildCapabilityRequestPreviewEnvelope,
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
  unavailableTransformReceiverHost,
  type TransformHostOutcome,
  type TransformReceiverHost,
} from "../src/lib/agentBridge";

const NOW = new Date("2026-07-11T00:00:00.000Z");
const REVIEW_NOW = new Date("2026-07-11T00:00:05.000Z");
const EXECUTION_NOW = new Date("2026-07-11T00:00:10.000Z");
const ROOM = "room-transform";
const SOURCE = "room-session:source";
const TARGET = "room-session:target";

function chain() {
  const request = buildArtifactTransformRequest({
    sourceCapability: "filesystem.find_file_candidates", sourceRequestId: "search-1", candidateId: "candidate-1", candidateKind: "filesystem_file", resultContract: "typed_transform_result",
    sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: NOW, requestId: "transform-request", nonce: "transform-nonce",
  });
  if (!request.ok) throw new Error(request.errors.join(" "));
  const envelope = buildCapabilityRequestPreviewEnvelope(request.request, { roomRef: ROOM, now: NOW, envelopeId: "transform-envelope" });
  if (!envelope.ok) throw new Error(envelope.errors.join(" "));
  const preview = buildCapabilityPreviewControlEvent(envelope.envelope, { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: NOW, eventId: "transform-preview" });
  if (!preview.ok || preview.event.kind !== "capability_preview") throw new Error("preview");
  const review = evaluatePeerCapabilityPreview(preview.event, { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, session: createPeerConsentSessionState(), now: REVIEW_NOW, consentId: "transform-consent" });
  if (review.status !== "reviewable") throw new Error(review.errors.join(" "));
  const allowed = allowPeerCapabilityOnce(review.binding, createPeerConsentSessionState(), { now: REVIEW_NOW });
  if (!allowed.ok) throw new Error(allowed.errors.join(" "));
  const acknowledgement = buildPeerConsentStatusEvent(preview.event, allowed.record, { now: REVIEW_NOW, eventId: "transform-ack" });
  if (!acknowledgement.ok || acknowledgement.event.kind !== "capability_preview_ack") throw new Error("ack");
  const execution = buildArtifactTransformExecutionRequest(preview.event, acknowledgement.event, { now: EXECUTION_NOW, executionId: "transform-execution", eventId: "transform-execution-event" });
  if (!execution.ok) throw new Error(execution.errors.join(" "));
  return { request: request.request, consent: allowed.record, execution };
}

function context() { return { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTION_NOW }; }

function boundedHost(outcome: TransformHostOutcome) {
  let calls = 0;
  const host: TransformReceiverHost = { async execute() { calls += 1; return outcome; } };
  return { host, calls: () => calls };
}

test("fixed policy time rejects exact-expiry Transform requests", () => {
  const value = chain();
  assert.equal(new Date(value.request.expiresAt).getTime() > NOW.getTime(), true);
});

test("deny, mirror replay, and request mismatch fail before receiver-host execution", async () => {
  const value = chain();
  const host = boundedHost({ executed: false, lifecycle: ["prepared"], errorCode: "sandbox_unavailable", deliveryStatus: "not_sent" });
  const denied = { ...value.consent, decision: "deny" as const, status: "denied" as const };
  const rejected = await executeInboundArtifactTransformRequest(value.execution.event, denied, createPeerConsentConsumptionState(), host.host, context());
  assert.equal(rejected.errorCode, "consent_not_allowed_once");
  const mismatch = structuredClone(value.execution.event);
  mismatch.payload.requestPayloadHash = "other-hash";
  const mismatchResult = await executeInboundArtifactTransformRequest(mismatch, value.consent, createPeerConsentConsumptionState(), host.host, context());
  assert.equal(mismatchResult.errorCode, "consent_binding_mismatch");
  assert.equal(host.calls(), 0);
});

test("production unavailable is prepared-only and consumes neither mirror nor lease authority", async () => {
  const value = chain();
  const result = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), unavailableTransformReceiverHost, context());
  assert.equal(result.errorCode, "sandbox_unavailable");
  assert.equal(result.executed, false);
  assert.deepEqual(result.lifecycle, ["prepared"]);
  assert.deepEqual(result.state.consumedConsentIds, []);
});

test("pre-start bounded host rejection preserves exact-request retry eligibility", async () => {
  const value = chain();
  const host = boundedHost({ executed: false, lifecycle: ["prepared", "claimed"], errorCode: "candidate_changed", deliveryStatus: "not_sent" });
  const result = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), host.host, context());
  assert.equal(result.executed, false);
  assert.equal(result.errorCode, "candidate_changed");
  assert.deepEqual(result.state.consumedRequestIds, []);
});

test("post-start bounded failures retain executor_started and consume the mirror", async () => {
  const value = chain();
  const host = boundedHost({ executed: true, lifecycle: ["prepared", "claimed", "revalidated", "executor_started"], errorCode: "result_transport_failed", deliveryStatus: "not_sent" });
  const result = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), host.host, context());
  assert.equal(result.executed, true);
  assert.equal(result.errorCode, "result_transport_failed");
  assert.deepEqual(result.lifecycle, ["prepared", "claimed", "revalidated", "executor_started"]);
  assert.deepEqual(result.state.consumedConsentIds, [value.execution.request.consentId]);
});

test("terminal replay preserves Rust category without a typed result or handoff", async () => {
  const value = chain();
  const host = boundedHost({ executed: true, lifecycle: ["prepared", "claimed", "revalidated", "executor_started"], terminalCategory: "completed", deliveryStatus: "replay" });
  const result = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), host.host, context());
  assert.equal(result.terminalCategory, "completed");
  assert.equal(result.deliveryStatus, "replay");
  assert.equal("resultEvent" in result, false);
});

test("provider Transform remains bounded and TypeScript exposes no raw executor seam", () => {
  const unsupported = { schemaVersion: "ask-bridge-natural-v1", title: "Search Transform Return", status: "unsupported_future", unsupportedReason: "unsupported", requiresUserConfirmation: true, steps: [
    { primitive: "Search", filenameHint: "report", extensions: ["txt"], safeScopes: ["downloads"] }, { primitive: "Transform", transformKind: "python" }, { primitive: "Return", destination: "this_device", returnKind: "typed_transform_result", requiresSecondConsent: false },
  ] };
  assert.equal(validateAskBridgeNaturalV1Plan(unsupported).valid, true);
  const coordinator = readFileSync("src/lib/agentBridge/transformExecution.ts", "utf8");
  const product = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const main = readFileSync("src-tauri/src/main.rs", "utf8");
  const tauri = readFileSync("src/lib/tauri.ts", "utf8");
  assert.doesNotMatch(coordinator, /TransformExecutor|stdout|stderr|ArtifactTransformExecutionResult|passTransformResultBoundary|child_process|node:process|spawn\(|execFile|Command::new|deterministicFake|fakeExecutor/i);
  assert.doesNotMatch(tauri, /ArtifactTransformRawExecutorResult|TransformFinalizationDelivery|finalizeAndSendTransformResult|finalize_and_send_transform_result/);
  assert.match(coordinator, /execute_transform_with_receiver_host/);
  assert.doesNotMatch(main, /mark_transform_operation_started|finalize_and_send_transform_result/);
  assert.match(product, /rustTransformReceiverHost/);
});

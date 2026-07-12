import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  buildArtifactTransformRequest,
  buildCapabilityRequestPreviewEnvelope,
  validateArtifactTransformExecutionResult,
  validateArtifactTransformRequest,
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
  unavailableTransformLeaseHost,
  type TransformExecutor,
  type TransformLeaseHost,
} from "../src/lib/agentBridge";

const NOW = new Date("2026-07-11T00:00:00.000Z");
const REVIEW_NOW = new Date("2026-07-11T00:00:05.000Z");
const EXECUTION_NOW = new Date("2026-07-11T00:00:10.000Z");
const ROOM = "room-transform";
const SOURCE = "room-session:source";
const TARGET = "room-session:target";

function chain(options: { ttlMs?: number } = {}) {
  const request = buildArtifactTransformRequest({
    sourceCapability: "filesystem.find_file_candidates", sourceRequestId: "search-1", candidateId: "candidate-1", candidateKind: "filesystem_file", resultContract: "typed_transform_result",
    sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: NOW, ttlMs: options.ttlMs, requestId: "transform-request", nonce: "transform-nonce",
  });
  assert.equal(request.ok, true, request.ok ? "" : request.errors.join(" "));
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
  return { request: request.request, preview: preview.event, review, consent: allowed.record, execution };
}

function completed(request: { executionId: string; requestId: string; consentId: string }, stdout = "test-only") {
  return { schemaVersion: "artifact-transform-selected-result-v1" as const, capability: "artifact.transform_selected" as const, executionId: request.executionId, requestId: request.requestId, consentId: request.consentId, status: "completed" as const, result: { kind: "typed_transform_result" as const, output: { kind: "process_output" as const, stdout, stderr: "", exitCode: 0, durationMs: 1, timedOut: false, stdoutTruncated: false, stderrTruncated: false } }, createdAt: EXECUTION_NOW.toISOString() };
}

function receiverHost(status: "revalidated" | "candidate_changed" = "revalidated") {
  const calls: string[] = [];
  const host: TransformLeaseHost = {
    async acquire() { calls.push("acquire"); return "leased"; },
    async revalidate() { calls.push("revalidate"); return status; },
    async release() { calls.push("release"); },
  };
  return { host, calls };
}

function readyExecutor(onStart?: () => void): TransformExecutor {
  return {
    async prepare() { return { status: "ready" }; },
    async start(request) { onStart?.(); return { status: "started", result: completed(request) }; },
  };
}

function context() { return { roomRef: ROOM, sourceDeviceRef: SOURCE, targetPeerRef: TARGET, now: EXECUTION_NOW }; }
const testOnlyResultBoundary = async (_request: unknown, output: ReturnType<typeof completed>) => ({ ok: true as const, result: output });

test("fixed policy time accepts valid Transform preview and rejects exact-expiry and expired requests", () => {
  const value = chain();
  assert.equal(value.review.status, "reviewable");
  assert.equal(validateArtifactTransformRequest(value.request, { now: new Date(value.request.expiresAt) }).valid, false);
  assert.equal(validateArtifactTransformRequest(value.request, { now: new Date(Date.parse(value.request.expiresAt) + 1) }).valid, false);
});

test("deny, replay, request mismatch, and consent mismatch fail closed", async () => {
  const value = chain();
  const denied = { ...value.consent, decision: "deny" as const, status: "denied" as const };
  const rejected = await executeInboundArtifactTransformRequest(value.execution.event, denied, createPeerConsentConsumptionState(), receiverHost().host, readyExecutor(), context());
  assert.equal(rejected.result.errorCode, "consent_not_allowed_once");
  assert.equal(rejected.executed, false);
  const mismatch = structuredClone(value.execution.event);
  mismatch.payload.requestPayloadHash = "other-hash";
  const mismatchResult = await executeInboundArtifactTransformRequest(mismatch, value.consent, createPeerConsentConsumptionState(), receiverHost().host, readyExecutor(), context());
  assert.equal(mismatchResult.result.errorCode, "consent_binding_mismatch");
  const host = receiverHost();
  const first = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), host.host, readyExecutor(), context());
  const replay = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, first.state, host.host, readyExecutor(), context());
  assert.equal(replay.result.status, "already_consumed");
  assert.equal(replay.executed, false);
});

test("lease revalidation blocks mutation before executor start and releases the lease", async () => {
  const value = chain();
  const receiver = receiverHost("candidate_changed");
  let starts = 0;
  const result = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), receiver.host, readyExecutor(() => { starts += 1; }), { ...context(), resultBoundary: testOnlyResultBoundary });
  assert.equal(result.result.errorCode, "candidate_changed");
  assert.equal(result.executed, false);
  assert.equal(starts, 0);
  assert.deepEqual(receiver.calls, ["acquire", "revalidate", "release"]);
  assert.deepEqual(result.lifecycle, ["prepared", "claimed"]);
  assert.deepEqual(result.state.consumedConsentIds, []);
  assert.deepEqual(result.state.consumedRequestIds, []);
});

test("successful injected receiver-host flow acknowledges execution start and never queues a handoff", async () => {
  const value = chain();
  const receiver = receiverHost();
  let starts = 0;
  const result = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), receiver.host, readyExecutor(() => { starts += 1; }), { ...context(), resultBoundary: testOnlyResultBoundary });
  assert.equal(result.result.status, "completed");
  assert.equal(result.executed, true);
  assert.equal(starts, 1);
  assert.deepEqual(result.lifecycle, ["prepared", "claimed", "revalidated", "executor_started", "completed"]);
  assert.deepEqual(receiver.calls, ["acquire", "revalidate", "release"]);
  assert.equal("handoffQueued" in result.resultEvent.payload, false);
});

test("unavailable production executor does not acquire or consume a candidate lease", async () => {
  const value = chain();
  let acquireCalls = 0;
  const unavailableLease: TransformLeaseHost = { async acquire() { acquireCalls += 1; return "leased"; }, async revalidate() { return "revalidated"; }, async release() {} };
  const result = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), unavailableLease, unavailableTransformExecutor, context());
  assert.equal(result.result.errorCode, "sandbox_unavailable");
  assert.equal(result.executed, false);
  assert.equal(acquireCalls, 0);
  assert.deepEqual(result.lifecycle, ["prepared"]);
  assert.deepEqual(result.state.consumedConsentIds, []);
  assert.equal(unavailableTransformLeaseHost.acquire !== undefined, true);
});

test("post-start finalization failures preserve executor_started and consumed mirror state", async () => {
  const value = chain();
  const receiver = receiverHost();
  const result = await executeInboundArtifactTransformRequest(
    value.execution.event,
    value.consent,
    createPeerConsentConsumptionState(),
    receiver.host,
    readyExecutor(),
    { ...context(), resultBoundary: async () => { throw new Error("transport failed"); } },
  );
  assert.equal(result.executed, true);
  assert.deepEqual(result.lifecycle, ["prepared", "claimed", "revalidated", "executor_started"]);
  assert.equal(result.result?.errorCode, "executor_failed");
  assert.deepEqual(result.state.consumedConsentIds, [value.execution.request.consentId]);
});

test("Rust-finalized replay metadata is terminal metadata, not an invalid executor result", async () => {
  const value = chain();
  const result = await executeInboundArtifactTransformRequest(
    value.execution.event,
    value.consent,
    createPeerConsentConsumptionState(),
    receiverHost().host,
    readyExecutor(),
    { ...context(), resultBoundary: async () => ({ ok: true as const, deliveredByRust: true as const, terminalCategory: "completed" as const }) },
  );
  assert.equal(result.executed, true);
  assert.equal(result.terminalCategory, "completed");
  assert.equal(result.result, undefined);
  assert.equal(result.resultEvent, undefined);
  assert.deepEqual(result.lifecycle, ["prepared", "claimed", "revalidated", "executor_started", "completed"]);
});

test("completed results are bounded, completed-only, and receiver-host sanitation can reject private values", async () => {
  const valid = completed({ executionId: "e", requestId: "r", consentId: "c" });
  assert.equal(validateArtifactTransformExecutionResult(valid).valid, true);
  assert.equal(validateArtifactTransformExecutionResult({ ...valid, status: "failed", errorCode: "executor_failed" }).valid, false);
  assert.equal(validateArtifactTransformExecutionResult(completed({ executionId: "e", requestId: "r", consentId: "c" }, "file:///ordinary/unrelated-path")).valid, true);
  const tooLarge = completed({ executionId: "e", requestId: "r", consentId: "c" }, "x".repeat(16 * 1024 + 1));
  assert.equal(validateArtifactTransformExecutionResult(tooLarge).valid, false);
  const inconsistent = completed({ executionId: "e", requestId: "r", consentId: "c" });
  inconsistent.result.output.stdoutTruncated = true;
  assert.equal(validateArtifactTransformExecutionResult(inconsistent).valid, false);
  const value = chain();
  const privateResultExecutor: TransformExecutor = {
    async prepare() { return { status: "ready" }; },
    async start(request) { return { status: "started", result: completed(request, "receiver-local-digest") }; },
  };
  const receiverLocalValues = new Set(["receiver-local-digest", "receiver-local-lease"]);
  const result = await executeInboundArtifactTransformRequest(value.execution.event, value.consent, createPeerConsentConsumptionState(), receiverHost().host, privateResultExecutor, { ...context(), resultBoundary: async (_request, output) => receiverLocalValues.has(output.result?.output.stdout ?? "") ? { ok: false } : { ok: true, result: output } });
  assert.equal(result.result.errorCode, "invalid_executor_result");
});

test("provider Transform remains bounded and production code cannot wire a test executor or process fallback", () => {
  const unsupported = { schemaVersion: "ask-bridge-natural-v1", title: "Search Transform Return", status: "unsupported_future", unsupportedReason: "unsupported", requiresUserConfirmation: true, steps: [
    { primitive: "Search", filenameHint: "report", extensions: ["txt"], safeScopes: ["downloads"] }, { primitive: "Transform", transformKind: "python" }, { primitive: "Return", destination: "this_device", returnKind: "typed_transform_result", requiresSecondConsent: false },
  ] };
  assert.equal(validateAskBridgeNaturalV1Plan(unsupported).valid, true);
  const coordinator = readFileSync("src/lib/agentBridge/transformExecution.ts", "utf8");
  const product = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  assert.doesNotMatch(coordinator, /passTransformResultBoundary|child_process|node:process|spawn\(|execFile|Command::new|deterministicFake|fakeExecutor/i);
  assert.match(coordinator, /finalize_and_send_transform_result/);
  assert.doesNotMatch(coordinator, /sanitize_and_finalize_transform_operation/);
  assert.doesNotMatch(readFileSync("src-tauri/src/main.rs", "utf8"), /mark_transform_operation_started|sanitize_and_finalize_transform_operation/);
  assert.match(product, /unavailableTransformExecutor/);
  assert.doesNotMatch(product, /readyExecutor|receiverHost|resultBoundary|passTransformResultBoundary/);
});

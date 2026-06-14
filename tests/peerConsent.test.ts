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
  applyInboundPeerStatusToOutboundQueue,
  buildPeerConsentStatusEvent,
  buildSessionBoundCapabilityPreviewControlEvent,
  createControlQueueState,
  createPeerConsentSessionState,
  denyPeerCapability,
  enqueueRoomControlEvent,
  evaluatePeerCapabilityPreview,
  expirePeerConsent,
  hasOutgoingControlWindowDemand,
  markControlQueueItemStatus,
  preservePeerConsentSessionState,
  validatePeerConsentRecord,
  type CapabilityPreviewRoomControlEvent,
  type PeerConsentBinding,
} from "../src/lib/agentBridge";

const NOW = new Date("2026-06-13T00:00:00.000Z");
const DECISION_TIME = new Date("2026-06-13T00:00:10.000Z");
const ROOM = "room-1";
const SOURCE = "room-session:source";
const TARGET = "room-session:target";

function previewEvent(eventId = "preview-event"): CapabilityPreviewRoomControlEvent {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = confirmPendingAiAction(
    createPendingAiAction(plan, policy, {
      now: new Date("2026-06-12T23:59:00.000Z"),
      ttlMs: 300_000,
      pendingId: `pending-${eventId}`,
    }),
    new Date("2026-06-12T23:59:30.000Z"),
  );
  const request = buildHelloPeerRequestFromPendingAction(pending, {
    now: NOW,
    ttlMs: 120_000,
    requestId: `request-${eventId}`,
    nonce: `nonce-${eventId}`,
    sourceDeviceRef: SOURCE,
  });
  assert.equal(request.ok, true);
  if (!request.ok) throw new Error("Expected request.");
  const envelope = buildCapabilityRequestPreviewEnvelope(request.request, {
    roomRef: "mock-room-preview",
    now: NOW,
    ttlMs: 120_000,
    envelopeId: `envelope-${eventId}`,
  });
  assert.equal(envelope.ok, true, envelope.ok ? "" : envelope.errors.join(" "));
  if (!envelope.ok) throw new Error("Expected envelope.");
  const controlEvent = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, {
    roomId: ROOM,
    localSessionRef: SOURCE,
    peerSessionRef: TARGET,
    peerConnected: true,
  }, { now: NOW });
  assert.equal(controlEvent.ok, true, controlEvent.ok ? "" : controlEvent.errors.join(" "));
  if (!controlEvent.ok || controlEvent.event.kind !== "capability_preview") {
    throw new Error("Expected preview event.");
  }
  return { ...controlEvent.event, eventId };
}

function review(event = previewEvent(), session = createPeerConsentSessionState()) {
  return evaluatePeerCapabilityPreview(event, {
    roomRef: ROOM,
    sourceDeviceRef: SOURCE,
    targetPeerRef: TARGET,
    session,
    now: DECISION_TIME,
    consentId: `consent-${event.eventId}`,
  });
}

function binding(): PeerConsentBinding {
  const result = review();
  assert.equal(result.status, "reviewable");
  if (result.status !== "reviewable") throw new Error("Expected reviewable.");
  return result.binding;
}

test("valid exact Hello Peer preview becomes reviewable", () => {
  const result = review();
  assert.equal(result.status, "reviewable");
  if (result.status !== "reviewable") return;
  assert.equal(result.binding.capability, "runtime.execute_hello_template");
  assert.equal(result.binding.exactMessage, "hello peer!");
  assert.equal(result.binding.requestPayloadHash, previewEvent().payload.request.requestPayloadHash);
  assert.equal(result.binding.previewOnly, true);
  assert.ok(Date.parse(result.binding.expiresAt) <= Date.parse(previewEvent().expiresAt));
});

test("PolicyGate rejects wrong capability, message, expiry, binding mismatch, unsafe fields, and decided request", () => {
  const event = previewEvent();
  const cases = [
    { ...event, payload: { ...event.payload, request: { ...event.payload.request, capability: "other" } } },
    { ...event, payload: { ...event.payload, request: { ...event.payload.request, input: { message: "not hello" } } } },
    { ...event, expiresAt: NOW.toISOString() },
    { ...event, roomRef: "wrong-room" },
    { ...event, sourceDeviceRef: "wrong-source" },
    { ...event, targetPeerRef: "wrong-target" },
    { ...event, payload: { ...event.payload, command: "no" } },
  ];
  for (const candidate of cases) {
    assert.equal(review(candidate as CapabilityPreviewRoomControlEvent).status, "rejected");
  }
  const decided = createPeerConsentSessionState();
  decided.decidedRequestIds.push(event.payload.request.requestId);
  assert.equal(review(event, decided).status, "rejected");
});

test("allow once and deny create exact immutable one-request bindings", () => {
  const exactBinding = binding();
  const allowed = allowPeerCapabilityOnce(exactBinding, createPeerConsentSessionState(), {
    now: DECISION_TIME,
  });
  assert.equal(allowed.ok, true);
  if (!allowed.ok) return;
  assert.deepEqual(allowed.record.binding, exactBinding);
  assert.equal(Object.isFrozen(allowed.record), true);
  assert.equal(Object.isFrozen(allowed.record.binding), true);
  assert.equal(allowed.record.status, "allowed_once");
  assert.equal(validatePeerConsentRecord(allowed.record, {
    now: DECISION_TIME,
    expectedBinding: exactBinding,
  }).valid, true);

  const denied = denyPeerCapability(exactBinding, createPeerConsentSessionState(), {
    now: DECISION_TIME,
  });
  assert.equal(denied.ok, true);
  if (!denied.ok) return;
  assert.equal(denied.record.status, "denied");
  assert.deepEqual(denied.record.binding, exactBinding);
});

test("second decision, binding reuse, and replay after expiry fail closed", () => {
  const exactBinding = binding();
  const allowed = allowPeerCapabilityOnce(exactBinding, createPeerConsentSessionState(), {
    now: DECISION_TIME,
  });
  assert.equal(allowed.ok, true);
  if (!allowed.ok) return;
  assert.equal(denyPeerCapability(exactBinding, allowed.state, { now: DECISION_TIME }).ok, false);
  assert.equal(allowPeerCapabilityOnce({
    ...exactBinding,
    sourceEventId: "different-event",
  }, allowed.state, { now: DECISION_TIME }).ok, false);
  assert.equal(
    allowPeerCapabilityOnce(exactBinding, createPeerConsentSessionState(), {
      now: new Date("2026-06-13T00:03:00.000Z"),
    }).ok,
    false,
  );
  assert.equal(validatePeerConsentRecord({
    ...allowed.record,
    binding: { ...allowed.record.binding, requestId: "different-request" },
  }, {
    now: DECISION_TIME,
    expectedBinding: exactBinding,
  }).valid, false);
  assert.equal(validatePeerConsentRecord({
    ...allowed.record,
    binding: { ...allowed.record.binding, requestPayloadHash: "different-payload" },
  }, {
    now: DECISION_TIME,
    expectedBinding: exactBinding,
  }).valid, false);
  assert.equal(expirePeerConsent(allowed.record, new Date("2026-06-13T00:03:00.000Z")).status, "expired");
  assert.deepEqual(preservePeerConsentSessionState(allowed.state, "old-session", "new-session"), {
    decidedRequestIds: [],
    decidedEnvelopeIds: [],
    decidedEventIds: [],
    consentIds: [],
  });
  assert.equal(preservePeerConsentSessionState(allowed.state, "same-session", "same-session"), allowed.state);
});

test("consent lifetime is bounded by the source event and creates no persistent trust", () => {
  const exactBinding = binding();
  assert.ok(Date.parse(exactBinding.expiresAt) <= Date.parse(previewEvent().expiresAt));
  assert.equal("trustedPeerIds" in createPeerConsentSessionState(), false);
  assert.equal("alwaysAllow" in exactBinding, false);
});

test("allow once and deny build bounded outbound preview status events", () => {
  const source = previewEvent();
  for (const decision of ["allow", "deny"] as const) {
    const policy = review(source);
    assert.equal(policy.status, "reviewable");
    if (policy.status !== "reviewable") continue;
    const result = decision === "allow"
      ? allowPeerCapabilityOnce(policy.binding, createPeerConsentSessionState(), { now: DECISION_TIME })
      : denyPeerCapability(policy.binding, createPeerConsentSessionState(), { now: DECISION_TIME });
    assert.equal(result.ok, true);
    if (!result.ok) continue;
    const status = buildPeerConsentStatusEvent(source, result.record, {
      now: DECISION_TIME,
      eventId: `${decision}-status`,
    });
    assert.equal(status.ok, true);
    if (!status.ok) continue;
    assert.equal(status.event.kind, decision === "allow" ? "capability_preview_ack" : "capability_preview_deny");
    assert.equal("stdout" in status.event.payload, false);
    assert.equal("stderr" in status.event.payload, false);
    assert.equal("exitCode" in status.event.payload, false);
    const queued = enqueueRoomControlEvent(createControlQueueState(), status.event, "outbound", {
      now: DECISION_TIME,
    });
    assert.equal(queued.ok, true);
    if (!queued.ok) continue;
    assert.equal(hasOutgoingControlWindowDemand(queued.state, { status: "idle" }, { now: DECISION_TIME }), true);
  }
});

test("sender matches ack or deny without execution, completion, or retry", () => {
  const source = previewEvent();
  const queued = enqueueRoomControlEvent(createControlQueueState(), source, "outbound", { now: NOW });
  assert.equal(queued.ok, true);
  if (!queued.ok) return;
  const sending = markControlQueueItemStatus(queued.state, queued.item.queueId, "selected", { now: NOW });
  assert.equal(sending.ok, true);
  if (!sending.ok) return;
  const transportSending = markControlQueueItemStatus(sending.state, sending.item.queueId, "transport_sending", { now: NOW });
  assert.equal(transportSending.ok, true);
  if (!transportSending.ok) return;
  const delivered = markControlQueueItemStatus(transportSending.state, transportSending.item.queueId, "transport_delivered", { now: NOW });
  assert.equal(delivered.ok, true);
  if (!delivered.ok) return;

  for (const decision of ["allow", "deny"] as const) {
    const policy = review(source);
    assert.equal(policy.status, "reviewable");
    if (policy.status !== "reviewable") continue;
    const consent = decision === "allow"
      ? allowPeerCapabilityOnce(policy.binding, createPeerConsentSessionState(), { now: DECISION_TIME })
      : denyPeerCapability(policy.binding, createPeerConsentSessionState(), { now: DECISION_TIME });
    assert.equal(consent.ok, true);
    if (!consent.ok) continue;
    const status = buildPeerConsentStatusEvent(source, consent.record, { now: DECISION_TIME });
    assert.equal(status.ok, true);
    if (!status.ok) continue;
    const applied = applyInboundPeerStatusToOutboundQueue(delivered.state, status.event, { now: DECISION_TIME });
    assert.equal(applied.ok, true);
    if (!applied.ok) continue;
    assert.equal(applied.item.status, decision === "allow" ? "acknowledged_preview_only" : "denied");
    assert.doesNotMatch(applied.item.reason ?? "", /executed|completed|retrying/i);
  }
});

test("receiver UI exposes only explicit one-time review controls and bounded sender wording", () => {
  const panel = readFileSync("src/components/agentBridge/RoomControlPanel.tsx", "utf8");
  const app = readFileSync("src/components/AiSlotPreview.tsx", "utf8");
  assert.match(panel, /data-testid="agent-bridge-peer-consent-review"/);
  assert.match(panel, /data-testid="agent-bridge-allow-once"/);
  assert.match(panel, /data-testid="agent-bridge-deny-peer-preview"/);
  assert.match(panel, /Allow once applies only to this exact request and does not execute it yet\./);
  assert.match(panel, /Peer allowed this exact preview once\. No execution has occurred\./);
  assert.match(panel, /Peer denied the preview\. No retry will be attempted\./);
  assert.doesNotMatch(panel, /Always allow|Remember this peer|Trust room|Execute now|Approve all/);
  assert.ok(app.indexOf("<RoomControlPanel room={room} envelope={envelopePreview?.envelope} />") >= 0);
});

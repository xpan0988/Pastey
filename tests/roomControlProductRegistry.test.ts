import assert from "node:assert/strict";
import test from "node:test";

import {
  buildCapabilityRequestPreviewEnvelope,
  buildFileCandidateRequestFromPendingAction,
  buildMockAiContextSnapshot,
  buildMockFileCandidatePlan,
  confirmPendingAiAction,
  createPendingAiAction,
  evaluateAiPolicy,
} from "../src/lib/ai";
import {
  buildCapabilityPreviewStatusControlEvent,
  buildHelloPeerStdoutProductPreview,
  buildSessionBoundCapabilityPreviewControlEvent,
  type CapabilityPreviewRoomControlEvent,
} from "../src/lib/agentBridge";
import {
  createRoomControlProductRegistry,
  registerOutboundCapabilityPreview,
  routeRoomControlInboxEvents,
} from "../src/lib/agentBridge/roomControlProductRegistry";
import type { RoomControlSessionContext } from "../src/lib/types";

const NOW = new Date("2026-07-09T00:00:00.000Z");
const DECISION_AT = new Date("2026-07-09T00:00:10.000Z");
const SESSION: RoomControlSessionContext = {
  roomId: "room",
  localSessionRef: "room-session:sender",
  peerSessionRef: "room-session:receiver",
  peerRouteRef: "peer-route",
  peerConnected: true,
};

function helloPreview(): CapabilityPreviewRoomControlEvent {
  const result = buildHelloPeerStdoutProductPreview(SESSION, { now: NOW });
  assert.equal(result.ok, true, result.ok ? undefined : result.errors.join(" "));
  if (!result.ok) throw new Error("Expected Hello preview.");
  return result.preview.previewEvent;
}

function filePreview(): CapabilityPreviewRoomControlEvent {
  const plan = buildMockFileCandidatePlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = confirmPendingAiAction(
    createPendingAiAction(plan, policy, {
      now: NOW,
      ttlMs: 120_000,
      pendingId: "file-pending",
    }),
    NOW,
  );
  const request = buildFileCandidateRequestFromPendingAction(pending, {
    now: NOW,
    ttlMs: 120_000,
    requestId: "file-request",
    nonce: "file-nonce",
    sourceDeviceRef: SESSION.localSessionRef,
  });
  assert.equal(request.ok, true, request.ok ? undefined : request.errors.join(" "));
  if (!request.ok) throw new Error("Expected file request.");
  const envelope = buildCapabilityRequestPreviewEnvelope(request.request, {
    roomRef: SESSION.roomId,
    now: NOW,
    ttlMs: 120_000,
    envelopeId: "file-envelope",
  });
  assert.equal(envelope.ok, true, envelope.ok ? undefined : envelope.errors.join(" "));
  if (!envelope.ok) throw new Error("Expected file envelope.");
  const preview = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, SESSION, { now: NOW });
  assert.equal(preview.ok, true, preview.ok ? undefined : preview.errors.join(" "));
  if (!preview.ok || preview.event.kind !== "capability_preview") {
    throw new Error("Expected file preview.");
  }
  return preview.event;
}

function decision(preview: CapabilityPreviewRoomControlEvent, denied = false) {
  const built = buildCapabilityPreviewStatusControlEvent(
    preview,
    denied ? "denied" : "acknowledged_preview_only",
    { now: DECISION_AT },
  );
  assert.equal(built.ok, true, built.ok ? undefined : built.errors.join(" "));
  if (!built.ok) throw new Error("Expected decision event.");
  return built.event;
}

function route(registry: ReturnType<typeof createRoomControlProductRegistry>, events: readonly unknown[]) {
  return routeRoomControlInboxEvents(registry, events, {
    now: DECISION_AT,
    expectedRoomRef: SESSION.roomId,
    expectedSourceDeviceRef: SESSION.peerSessionRef,
    expectedTargetPeerRef: SESSION.localSessionRef,
  });
}

test("Hello success followed by Request file search stays product-scoped in one Bridge session", () => {
  const hello = helloPreview();
  const file = filePreview();
  let registry = registerOutboundCapabilityPreview(
    createRoomControlProductRegistry(),
    hello,
    "hello_peer",
    NOW,
  );
  const helloRouted = route(registry, [decision(hello)]);
  assert.equal(helloRouted.helloPeer.length, 1);
  assert.equal(helloRouted.requestFile.length, 0);

  registry = registerOutboundCapabilityPreview(helloRouted.registry, file, "request_file", NOW);
  const fileRouted = route(registry, [decision(file)]);
  assert.equal(fileRouted.helloPeer.length, 0);
  assert.equal(fileRouted.requestFile.length, 1);
});

test("Hello deny followed by Request file search does not leak the deny into Request file", () => {
  const hello = helloPreview();
  const file = filePreview();
  let registry = registerOutboundCapabilityPreview(
    createRoomControlProductRegistry(),
    hello,
    "hello_peer",
    NOW,
  );
  const denied = route(registry, [decision(hello, true)]);
  assert.equal(denied.helloPeer.length, 1);
  assert.equal(denied.requestFile.length, 0);

  registry = registerOutboundCapabilityPreview(denied.registry, file, "request_file", NOW);
  const searched = route(registry, [decision(file)]);
  assert.equal(searched.requestFile.length, 1);
});

test("outbound preview registry survives panel close/reopen until expiry", () => {
  const file = filePreview();
  const registry = registerOutboundCapabilityPreview(
    createRoomControlProductRegistry(),
    file,
    "request_file",
    NOW,
  );
  const reopened = route(registry, [decision(file)]);
  assert.equal(reopened.requestFile.length, 1);
});

test("Request file result remains routable after the panel closes and reopens", () => {
  const resultEvent = {
    schemaVersion: "pastey-room-control-event-v1",
    eventId: "file-result-event",
    kind: "capability_execution_result",
    roomRef: SESSION.roomId,
    sourceDeviceRef: SESSION.peerSessionRef,
    targetPeerRef: SESSION.localSessionRef,
    createdAt: DECISION_AT.toISOString(),
    expiresAt: new Date(DECISION_AT.getTime() + 60_000).toISOString(),
    previewOnly: false,
    payload: {
      schemaVersion: "filesystem-find-file-candidates-result-v1",
      capability: "filesystem.find_file_candidates",
      executionId: "file-execution",
      requestId: "file-request",
      consentId: "file-consent",
      status: "completed",
      queryEcho: {
        filenameHint: "notes",
        extensions: ["txt"],
        searchMode: "filename_metadata_only",
      },
      candidates: [],
      omitted: {
        tooManyMatches: false,
        hiddenFilesSkipped: true,
        symlinksSkipped: true,
        scopesSkipped: [],
      },
      durationMs: 1,
      truncated: false,
      errorCode: null,
      createdAt: DECISION_AT.toISOString(),
    },
  };
  const reopened = route(createRoomControlProductRegistry(), [resultEvent]);
  assert.equal(reopened.requestFile.length, 1);
  assert.equal(reopened.helloPeer.length, 0);
});

test("the same room-control event is processed once", () => {
  const hello = helloPreview();
  const registry = registerOutboundCapabilityPreview(
    createRoomControlProductRegistry(),
    hello,
    "hello_peer",
    NOW,
  );
  const event = decision(hello);
  const first = route(registry, [event]);
  const second = route(first.registry, [event]);
  assert.equal(first.helloPeer.length, 1);
  assert.equal(second.helloPeer.length, 0);
  assert.deepEqual(second.ignoredEventIds, [event.eventId]);
});

test("receiver without an outbound preview ignores outbound-only status matching", () => {
  const hello = helloPreview();
  const routed = route(createRoomControlProductRegistry(), [decision(hello)]);
  assert.equal(routed.helloPeer.length, 0);
  assert.equal(routed.requestFile.length, 0);
});

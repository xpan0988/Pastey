import {
  buildCapabilityRequestPreviewEnvelope,
  buildHelloStdoutRequestFromPendingAction,
  buildMockAiContextSnapshot,
  buildMockHelloStdoutPlan,
  confirmPendingAiAction,
  createPendingAiAction,
  evaluateAiPolicy,
  type AiActionPlan,
  type CapabilityRequestPreviewEnvelope,
  type HelloStdoutRequest,
  type PendingAiAction,
} from "../ai";
import type { RoomControlSessionContext } from "../types";
import {
  buildSessionBoundCapabilityPreviewControlEvent,
} from "./roomControlTransport";
import {
  type CapabilityExecutionResultRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
} from "./roomControlEvent";

export const HELLO_PEER_DEMO_ACTION_LABEL = "Run Hello diagnostic";
export const HELLO_PEER_DEMO_DESCRIPTION =
  "Ask the selected device to run Pastey's built-in hello runtime and return stdout.";
export const HELLO_PEER_REQUIRES_ONE_SELECTED_DEVICE = "Hello Peer requires one selected device.";

export const HELLO_PEER_LIFECYCLE_STEPS = [
  "Plan prepared",
  "Host validated",
  "You confirmed",
  "Peer requested",
  "Peer approved",
  "Peer denied",
  "Runtime executed",
  "Result returned",
] as const;

export type HelloPeerLifecycleStep = typeof HELLO_PEER_LIFECYCLE_STEPS[number];

export interface HelloPeerStdoutProductPreview {
  plan: AiActionPlan;
  pending: PendingAiAction;
  request: HelloStdoutRequest;
  envelope: CapabilityRequestPreviewEnvelope;
  previewEvent: CapabilityPreviewRoomControlEvent;
}

export interface HelloPeerStdoutProductResult {
  title: "Hello Peer completed";
  stdout: "hello peer";
  exitCode: 0;
}

export function buildHelloPeerStdoutProductPreview(
  session: RoomControlSessionContext,
  options: { now?: Date; ttlMs?: number } = {},
): { ok: true; preview: HelloPeerStdoutProductPreview } | { ok: false; errors: string[] } {
  const now = options.now ?? new Date();
  const ttlMs = options.ttlMs ?? 120_000;
  const plan = buildHelloPeerStdoutPlan(session.peerSessionRef);
  const context = buildMockAiContextSnapshot();
  const policy = evaluateAiPolicy(plan, {
    ...context,
    room: {
      hasActiveRoom: true,
      trustedRoom: true,
      peerCount: 1,
    },
    peers: [{
      peerRef: session.peerSessionRef,
      visible: session.peerConnected,
      trusted: true,
      capabilities: ["runtime.hello_stdout"],
    }],
  });
  if (policy.status !== "accepted") {
    return { ok: false, errors: policy.reasons };
  }
  const pending = confirmPendingAiAction(
    createPendingAiAction(plan, policy, { now, ttlMs }),
    now,
  );
  const request = buildHelloStdoutRequestFromPendingAction(pending, {
    now,
    ttlMs,
    sourceDeviceRef: session.localSessionRef,
  });
  if (!request.ok) {
    return { ok: false, errors: request.errors };
  }
  const envelope = buildCapabilityRequestPreviewEnvelope(request.request, {
    roomRef: session.roomId,
    sourceDeviceRef: session.localSessionRef,
    targetPeerRef: session.peerSessionRef,
    now,
    ttlMs,
  });
  if (!envelope.ok) {
    return { ok: false, errors: envelope.errors };
  }
  const previewEvent = buildSessionBoundCapabilityPreviewControlEvent(envelope.envelope, session, { now });
  if (!previewEvent.ok || previewEvent.event.kind !== "capability_preview") {
    return { ok: false, errors: previewEvent.ok ? ["Hello Peer preview builder produced the wrong event kind."] : previewEvent.errors };
  }
  return {
    ok: true,
    preview: {
      plan,
      pending,
      request: request.request,
      envelope: envelope.envelope,
      previewEvent: previewEvent.event,
    },
  };
}

export function formatHelloPeerStdoutProductResult(
  event: CapabilityExecutionResultRoomControlEvent,
): HelloPeerStdoutProductResult | null {
  const result = event.payload;
  if (
    !("capability" in result) ||
    result.capability !== "runtime.hello_stdout" ||
    result.status !== "succeeded" ||
    !("stdout" in result) ||
    result.stdout !== "hello peer" ||
    !("exitCode" in result) ||
    result.exitCode !== 0
  ) {
    return null;
  }
  return {
    title: "Hello Peer completed",
    stdout: result.stdout,
    exitCode: result.exitCode,
  };
}

function buildHelloPeerStdoutPlan(targetPeerRef: string): AiActionPlan {
  const plan = buildMockHelloStdoutPlan();
  return {
    ...plan,
    title: HELLO_PEER_DEMO_ACTION_LABEL,
    explanation: HELLO_PEER_DEMO_DESCRIPTION,
    references: [{ kind: "peer", ref: targetPeerRef }],
    proposedInput: {
      ...plan.proposedInput,
      targetPeerRef,
    },
  };
}

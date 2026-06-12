import {
  buildCapabilityPreviewControlEvent,
  type RoomControlEventBuildResult,
} from "./roomControlEvent";
import {
  hashHelloPeerRequestPayload,
  validateCapabilityRequestPreviewEnvelope,
  type CapabilityRequestPreviewEnvelope,
  type HelloPeerRequest,
} from "../ai";
import type { RoomControlSessionContext } from "../types";

export function buildSessionBoundCapabilityPreviewControlEvent(
  envelope: CapabilityRequestPreviewEnvelope,
  session: RoomControlSessionContext,
  options: { now?: Date } = {},
): RoomControlEventBuildResult {
  const { requestPayloadHash: _requestPayloadHash, ...requestWithoutHash } =
    envelope.request;
  const reboundRequestWithoutHash: Omit<HelloPeerRequest, "requestPayloadHash"> = {
    ...requestWithoutHash,
    sourceDeviceRef: session.localSessionRef,
    targetPeerRef: session.peerSessionRef,
  };
  const request: HelloPeerRequest = {
    ...reboundRequestWithoutHash,
    requestPayloadHash: hashHelloPeerRequestPayload(reboundRequestWithoutHash),
  };
  const reboundEnvelope: CapabilityRequestPreviewEnvelope = {
    ...envelope,
    roomRef: session.roomId,
    sourceDeviceRef: session.localSessionRef,
    targetPeerRef: session.peerSessionRef,
    request,
  };
  const validation = validateCapabilityRequestPreviewEnvelope(reboundEnvelope, {
    now: options.now,
    expectedRoomRef: session.roomId,
    expectedTargetPeerRef: session.peerSessionRef,
  });
  if (!validation.valid) {
    return { ok: false, errors: validation.errors };
  }
  return buildCapabilityPreviewControlEvent(validation.value, {
    roomRef: session.roomId,
    sourceDeviceRef: session.localSessionRef,
    targetPeerRef: session.peerSessionRef,
    now: options.now,
  });
}

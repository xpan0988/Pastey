import type { BridgePeerSessionId, BridgeTarget } from "./bridgeRouting";

export type BridgeDeliveryOutcomeStatus =
  | "accepted_for_delivery"
  | "delivered"
  | "failed"
  | "rejected"
  | "cancelled"
  | "interrupted"
  | "unsupported";

export type BridgeDeliveryTargetKind = BridgeTarget["kind"];

export type BridgeDeliveryContentKind =
  | "text"
  | "file"
  | "image"
  | "pasted_image"
  | "control_event";

export interface BridgeDeliveryOutcome {
  operationId: string;
  bridgeSessionRef: string;
  peerSessionRef: BridgePeerSessionId;
  targetKind: BridgeDeliveryTargetKind;
  contentKind: BridgeDeliveryContentKind;
  status: BridgeDeliveryOutcomeStatus;
  errorCode?: string;
  createdAt: string;
  updatedAt: string;
}

export type BridgeSendAggregateStatus =
  | "pending"
  | "partial"
  | "completed"
  | "failed"
  | "cancelled"
  | "unsupported";

export interface BridgeSendOperation {
  operationId: string;
  bridgeSessionRef: string;
  target: BridgeTarget;
  resolvedPeerSessionRefs: readonly BridgePeerSessionId[];
  contentKind: BridgeDeliveryContentKind;
  aggregateStatus: BridgeSendAggregateStatus;
  outcomes: readonly BridgeDeliveryOutcome[];
  createdAt: string;
  updatedAt: string;
}

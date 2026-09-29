/** Physical DTOs are review/status data; the renderer has no authority types. */
export type PhysicalPayload = string | number | boolean | PhysicalPayload[] | { [key: string]: PhysicalPayload };
export interface PhysicalScope {
  requester: string;
  executor: string;
  environment: { environment: string; evidenceClass: "simulation" | "hardware"; offerExpiry: number };
  /** Opaque capability payload; its schema and meaning belong to the device binding. */
  intent: { capabilityId: string; payload: PhysicalPayload; payloadDigest: string };
  execution: { actionDurationUs: number; leaseDurationUs: number; totalExecutionUs: number; actionCount: number };
  qualification: { requiredEnforcementClass: "adapter_isolation_only" | "native_fence"; expiresAt: number };
  completion: unknown;
  [field: string]: unknown;
}
export interface PhysicalReview {
  reviewId: string;
  revision: number;
  scopeDigest: string;
  scope: PhysicalScope;
  state: "draft" | "reviewed" | "approved" | "rejected" | "expired";
  approval: { approvalId: string; expiresAt: number } | null;
}
export interface PhysicalStatus {
  review: string;
  authority: "pending" | "open" | "closed";
  installation: "pending" | "active" | "quarantined";
  dispatch: "not_sent" | "intent_committed" | "acknowledged" | "refused" | "unknown";
  consequence: "unobserved" | "partial" | "verified" | "contradicted" | "outcome_unknown";
  acceptance: "pending" | "accepted" | "rejected" | "cancelled";
  reconciliation: "pending" | "recorded";
  enforcementPending: boolean;
  quarantined: boolean;
}
export interface PhysicalProductView {
  offers: { scopeDigest: string; scope: PhysicalScope }[];
  review: PhysicalReview | null;
  start: string | null;
  status: PhysicalStatus | null;
  deliveryPending: boolean;
  availability?: "qualified" | "released" | "qualification_unavailable" | "qualification_expired" | "environment_unavailable";
}
export type PhysicalProductRequest =
  | { kind: "discover" | "snapshot" }
  | { kind: "compose"; offer_digest: string }
  | { kind: "approve" | "start"; review_id: string; scope_digest: string }
  | { kind: "status" | "cancel" | "reconcile"; start: string };
export function physicalReviewFresh(review: PhysicalReview | null, now = Date.now()): boolean {
  return Boolean(review && now < review.scope.environment.offerExpiry && now < review.scope.qualification.expiresAt
    && (!review.approval || now < review.approval.expiresAt));
}
/** Flatten a payload into pointer = value lines for review, without interpreting it. */
export function physicalPayloadEntries(payload: PhysicalPayload, prefix = ""): [string, string][] {
  if (payload !== null && typeof payload === "object") {
    return Object.entries(payload).flatMap(([key, value]) => physicalPayloadEntries(value, `${prefix}/${key}`));
  }
  return [[prefix || "/", String(payload)]];
}

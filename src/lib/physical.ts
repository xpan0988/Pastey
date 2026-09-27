/** Physical DTOs are review/status data; the renderer has no authority types. */
export interface PhysicalScope {
  requester: string;
  executor: string;
  environment: { environment: string; evidenceClass: "simulation" | "hardware"; offerExpiry: number };
  intent: { kind: "micro_duck_velocity_v1"; parameters: { vxMps: number; vyMps: number; vyawRadps: number; frame: string } };
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

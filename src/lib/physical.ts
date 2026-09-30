/** Physical DTOs are review/status data; the renderer has no authority types. */
export interface PhysicalScope {
  requester: string;
  executor: string;
  environment: { environment: string; evidenceClass: "simulation" | "hardware"; offerExpiry: number };
  /** The approved options (payloads stay with the binding), rate and observation flow.
   * A single reviewed action is a one-option stream with actionCount 1. */
  stream: {
    options: string[];
    minDecisionIntervalUs: number;
    observation: { fields: string[]; minIntervalUs: number; destination: string };
    onCompletion: "automatic" | "await_review";
    effectBound:
      | { verification: "witnessed"; predicate: { id: string }; requiredWitness: string }
      | { verification: "intent_only" };
  };
  execution: { actionDurationUs: number; leaseDurationUs: number; totalExecutionUs: number; actionCount: number };
  qualification: { requiredEnforcementClass: "adapter_isolation_only" | "native_fence"; expiresAt: number };
  completion: { predicate: { id: string }; requiredWitness: string; evaluationWindowUs: number };
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
const seconds = (us: number) => `${us / 1e6} s`;
/** Review rows for a scope: where, what, how often, how much, until when,
 * who judges and what flows back. */
export function physicalScopeSummary(scope: PhysicalScope): [string, string][] {
  const s = scope.stream;
  const o = s.observation;
  const e = s.effectBound;
  return [
    ["Executor Host", scope.executor],
    ["Approved options", s.options.join(", ")],
    ["Decision rate", `at most one decision per ${seconds(s.minDecisionIntervalUs)}`],
    ["Per action", `≤ ${seconds(scope.execution.actionDurationUs)}`],
    ["In total", `≤ ${seconds(scope.execution.totalExecutionUs)} · ≤ ${scope.execution.actionCount} action${scope.execution.actionCount === 1 ? "" : "s"}`],
    ["Completion", `${scope.completion.predicate.id} within ${seconds(scope.completion.evaluationWindowUs)}`],
    ["Witness class", scope.completion.requiredWitness.replace(/_/g, " ")],
    ["Observations sent", o.fields.length === 0
      ? "none"
      : `${o.fields.join(", ")} to ${o.destination}, at most one per ${seconds(o.minIntervalUs)}`],
    ["Effect bound", e.verification === "witnessed"
      ? `${e.predicate.id}, checked by a ${e.requiredWitness.replace(/_/g, " ")} witness`
      : "Intent only: constrains the brain's choices, not what the body does"],
    ["On completion", s.onCompletion === "automatic" ? "Accepted automatically" : "Awaits a review decision"],
  ];
}

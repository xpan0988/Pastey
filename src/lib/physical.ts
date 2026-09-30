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
    idleLeaseUs: number;
    approvalLifetimeUs: number;
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
/** One decision record: who proposed which option, and the executor's admission. */
export interface PhysicalDecision {
  sequence: number;
  proposer: string;
  option: string;
  allowed: boolean;
  reason: string;
}
/** The witness's latest verdict; Core's own conclusion is `consequence`. */
export interface PhysicalWitnessConclusion {
  witnessClass: string;
  result: "verified" | "partial" | "contradicted" | "unknown";
  reason: string;
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
  root?: string | null;
  session?: string | null;
  action?: string | null;
  consequenceReason?: string | null;
  witness?: PhysicalWitnessConclusion | null;
  decisions?: PhysicalDecision[];
}
/** What an agent's MCP configuration runs to drive a started stream. */
export interface PhysicalMcpConnection {
  command: string;
  args: string[];
  grantPath: string;
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
    ["Approval valid for", `${seconds(s.approvalLifetimeUs)} after approval`],
    ["Idle lease", `the stream ends after ${seconds(s.idleLeaseUs)} without a tool call`],
  ];
}
const words = (value: string) => value.replace(/_/g, " ");
/** Status rows, without the decision records and the witness verdict. */
export function physicalStatusRows(status: PhysicalStatus): [string, string][] {
  return ([
    ["Authority", status.authority],
    ["Installation", status.installation],
    ["Dispatch", status.dispatch],
    ["Consequence", status.consequenceReason ? `${status.consequence} (${status.consequenceReason})` : status.consequence],
    ["Acceptance", status.acceptance],
    ["Reconciliation", status.reconciliation],
    ["Enforcement pending", status.enforcementPending ? "yes" : "no"],
    ["Quarantined", status.quarantined ? "yes" : "no"],
  ] as [string, string][]).map(([label, value]) => [label, words(value)]);
}
/** The witness's verdict, apart from Core's conclusion. */
export function physicalWitnessSummary(status: PhysicalStatus): string {
  const w = status.witness;
  return w ? `${words(w.witnessClass)} witness: ${w.result} (${words(w.reason)})` : "No witness verdict yet";
}
/** One decision record as a table row: sequence, proposer, option, admission. */
export function physicalDecisionRow(d: PhysicalDecision): [string, string, string, string] {
  return [String(d.sequence), d.proposer, d.option, d.allowed ? "allowed" : `refused: ${d.reason}`];
}
/** The MCP server entry an agent configuration needs. */
export function physicalMcpConfig(c: PhysicalMcpConnection): string {
  return JSON.stringify({ mcpServers: { "pastey-physical": { command: c.command, args: c.args } } }, null, 2);
}

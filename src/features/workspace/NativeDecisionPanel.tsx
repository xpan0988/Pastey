import { useEffect, useState } from "react";
import { respondRemoteNativeDecision, type NativeAgentTaskStatus, type NativeDecision, type NativeDecisionReview } from "../../lib/tauri";
import { nativeDecisionPresentation, nativeFileChangeLabel, type NativeDecisionDelivery } from "../../lib/nativeDecisions";

/** Native requests of one task awaiting the user who started it. Request
 * content is the native Agent's own and untrusted: every field is rendered
 * as escaped text, never as markup, a link or an action. Approve once and
 * Deny appear only where the executing device offers them, and each sends
 * one explicit decision for exactly what is shown. */
export function NativeDecisionPanel({ status, roomId, remote, hostName }: {
  status: NativeAgentTaskStatus;
  roomId: string;
  remote: boolean;
  hostName: string;
}) {
  const [now, setNow] = useState(() => Date.now() / 1000);
  const [delivery, setDelivery] = useState<Record<string, NativeDecisionDelivery>>({});
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now() / 1000), 15_000);
    return () => window.clearInterval(timer);
  }, []);
  const reviews = status.state === "running" ? status.nativeDecisions ?? [] : [];
  if (reviews.length === 0) return null;

  async function decide(review: NativeDecisionReview, decision: NativeDecision) {
    setError(null);
    setDelivery((current) => ({ ...current, [review.reviewRef]: "sending" }));
    try {
      await respondRemoteNativeDecision(roomId, status.taskId, review.reviewRef, review.contentDigest, decision);
      setDelivery((current) => ({ ...current, [review.reviewRef]: "sent" }));
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
      setDelivery((current) => {
        const next = { ...current };
        delete next[review.reviewRef];
        return next;
      });
    }
  }

  return <section className="v2-task-open" aria-label="Codex requests">
    {reviews.map((review) => {
      const view = nativeDecisionPresentation(review, { now, remote, hostName, delivery: review.state === "pending" ? delivery[review.reviewRef] : undefined });
      const content = review.content;
      return <section className="v2-agent-response" key={review.reviewRef} aria-label={view.title}>
        <strong>{view.title} · on {hostName}</strong>
        <small>{view.stateLabel}</small>
        {content?.command !== undefined ? <pre className="v2-agent-response-text">{content.command}</pre> : null}
        {content?.cwd !== undefined ? <small>In folder: {content.cwd}</small> : null}
        {content?.networkHost !== undefined ? <small>Network access: {content.networkProtocol ?? "unknown"} to {content.networkHost}</small> : null}
        {content?.reason !== undefined ? <small>Codex's stated reason (not verified): {content.reason}</small> : null}
        {(content?.fileChanges ?? []).map((change, index) => <div key={index}>
          <small>{nativeFileChangeLabel(change)}</small>
          {change.diff ? <pre className="v2-agent-response-text">{change.diff}</pre> : null}
        </div>)}
        {view.note ? <small className="v2-error">{view.note}</small> : null}
        {view.canAccept || view.canDecline ? <footer>
          {view.canDecline ? <button type="button" className="v2-button" onClick={() => void decide(review, "decline")}>Deny</button> : null}
          {view.canAccept ? <button type="button" className="v2-button primary" onClick={() => void decide(review, "accept")}>Approve once</button> : null}
        </footer> : null}
      </section>;
    })}
    <small>Approve once allows only the request shown. Stop Codex interrupts the turn; it does not undo what Codex already did.</small>
    {error ? <small className="v2-error" role="alert">{error}</small> : null}
  </section>;
}

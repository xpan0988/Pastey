import { useEffect, useRef, useState } from "react";
import { physicalProductCommand } from "../lib/tauri";
import { physicalReviewFresh, type PhysicalProductRequest, type PhysicalProductView } from "../lib/physical";
import type { BridgePeerSession } from "../lib/bridgePeers";

export function PhysicalReviewPanel({ roomId, peers }: { roomId: string; peers: BridgePeerSession[] }) {
  const [selected, setSelected] = useState("");
  const peer = peers.find((p) => p.peerSessionId === selected && p.hostRef);
  return <details className="v2-physical-panel">
    <summary>Physical environment</summary>
    <label>Executor Host <select value={selected} onChange={(e) => setSelected(e.target.value)}>
      <option value="">Select a Host</option>
      {peers.filter((p) => p.hostRef).map((p) => <option key={p.peerSessionId} value={p.peerSessionId}>{p.displayName}</option>)}
    </select></label>
    {peer?.hostRef ? <PhysicalHostReview key={`${roomId}:${peer.peerSessionId}:${peer.hostRef}`} roomId={roomId} host={peer.hostRef} /> : <p>Select a current Host to discover its physical environments.</p>}
  </details>;
}
function PhysicalHostReview({ roomId, host }: { roomId: string; host: string }) {
  const [view, setView] = useState<PhysicalProductView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const mounted = useRef(true);
  const [, updateExpiry] = useState(0);
  const reviewExpiry = view?.review ? Math.min(view.review.scope.environment.offerExpiry, view.review.scope.qualification.expiresAt, view.review.approval?.expiresAt ?? Infinity) : null;
  useEffect(() => {
    if (reviewExpiry === null) return;
    const delay = reviewExpiry - Date.now();
    if (delay <= 0) return;
    const timer = window.setTimeout(() => updateExpiry((n) => n + 1), Math.min(delay + 1, 2147483647));
    return () => window.clearTimeout(timer);
  }, [reviewExpiry]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function command(request: PhysicalProductRequest) {
    setBusy(true); setError("");
    try { const result = await physicalProductCommand(roomId, host, request); if (mounted.current) setView(result); }
    catch (e) { if (mounted.current) setError(String(e)); }
    finally { if (mounted.current) setBusy(false); }
  }
  const r = view?.review ?? null;
  const scope = r?.scope;
  const fresh = physicalReviewFresh(r);
  const status = view?.status;
  return <section aria-label="Physical review and status" aria-busy={busy}>
    <p>Host <code>{host}</code></p>
    <button type="button" disabled={busy} onClick={() => void command({ kind: "discover" })}>Discover environments</button>
    <button type="button" disabled={busy} onClick={() => void command({ kind: "snapshot" })}>Refresh saved state</button>
    {view ? <>
      {view.offers.length === 0 ? <p>No current qualified physical environment is offered. Real Gate A qualification may be pending.</p> : <ul>{view.offers.map((offer) => <li key={offer.scopeDigest}>
        {offer.scope.environment.environment} · {offer.scope.environment.evidenceClass} · {offer.scope.qualification.requiredEnforcementClass.replace(/_/g, " ")}
        <button type="button" disabled={busy || Date.now() >= offer.scope.environment.offerExpiry} onClick={() => void command({ kind: "compose", offer_digest: offer.scopeDigest })}>Compose exact review</button>
      </li>)}</ul>}
      {scope && r ? <article>
        <strong>Exact physical review · {r.state}</strong>
        <p>Environment {scope.environment.environment} · {scope.environment.evidenceClass} · {scope.qualification.requiredEnforcementClass.replace(/_/g, " ")}</p>
        <p>Intent: {scope.intent.kind} · {scope.intent.parameters.vxMps} m/s forward, {scope.intent.parameters.vyMps} m/s lateral, {scope.intent.parameters.vyawRadps} rad/s yaw · {scope.intent.parameters.frame}</p>
        <p>Action duration ≤ {scope.execution.actionDurationUs / 1e6} s · budget ≤ {scope.execution.totalExecutionUs / 1e6} s · {scope.execution.actionCount} action</p>
        <details><summary>Completion predicate and exact effects</summary><pre>{JSON.stringify(scope, null, 2)}</pre></details>
        {!fresh ? <p role="status">This review is stale. Discover and compose a fresh review.</p> : null}
        <button type="button" disabled={busy || !fresh || r.state !== "reviewed"} onClick={() => void command({ kind: "approve", review_id: r.reviewId, scope_digest: r.scopeDigest })}>Approve exact scope</button>
        <button type="button" disabled={busy || !fresh || r.state !== "approved" || view.start !== null} onClick={() => void command({ kind: "start", review_id: r.reviewId, scope_digest: r.scopeDigest })}>Start approved action</button>
      </article> : null}
      <div aria-live="polite">
        {view.deliveryPending ? <p>Delivery or semantic reply pending. Execution and consequences may be unknown.</p> : null}
        {status ? <dl>{Object.entries(status).map(([key, value]) => <div key={key}><dt>{key.replace(/([A-Z])/g, " $1")}</dt><dd>{String(value ?? "unknown").replace(/_/g, " ")}</dd></div>)}</dl> : null}
      </div>
      {view.start ? <div>
        <button type="button" disabled={busy} onClick={() => void command({ kind: "status", start: view.start! })}>Query executor status</button>
        <button type="button" disabled={busy} onClick={() => void command({ kind: "cancel", start: view.start! })}>Cancel task authority</button>
        <button type="button" disabled={busy} onClick={() => void command({ kind: "reconcile", start: view.start! })}>Reconcile consequences</button>
        <p>Cancellation delivery may be uncertain. A stop acknowledgement does not prove physical rest.</p>
      </div> : null}
    </> : null}
    {error ? <p role="alert">{error}</p> : null}
  </section>;
}

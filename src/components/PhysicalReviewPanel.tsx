import { useEffect, useRef, useState } from "react";
import { physicalMcpConnection, physicalProductCommand } from "../lib/tauri";
import { physicalDecisionRow, physicalMcpConfig, physicalReviewFresh, physicalScopeSummary, physicalStatusRows, physicalWitnessSummary, type PhysicalMcpConnection, type PhysicalProductRequest, type PhysicalProductView } from "../lib/physical";
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
  const [mcp, setMcp] = useState<PhysicalMcpConnection | null>(null);
  const mounted = useRef(true);
  const [, updateExpiry] = useState(0);
  const reviewExpiry = view ? Math.min(
    view.review ? Math.min(view.review.scope.environment.offerExpiry, view.review.scope.qualification.expiresAt, view.review.approval?.expiresAt ?? Infinity) : Infinity,
    ...view.offers.map((offer) => Math.min(offer.scope.environment.offerExpiry, offer.scope.qualification.expiresAt)).filter((expiry) => expiry > Date.now()),
  ) : null;
  useEffect(() => {
    if (reviewExpiry === null || !Number.isFinite(reviewExpiry)) return;
    const delay = reviewExpiry - Date.now();
    if (delay <= 0) return;
    const timer = window.setTimeout(() => updateExpiry((n) => n + 1), Math.min(delay + 1, 2147483647));
    return () => window.clearTimeout(timer);
  }, [reviewExpiry]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function connectBrain(start: string) {
    setBusy(true); setError("");
    try { const connection = await physicalMcpConnection(roomId, host, start); if (mounted.current) setMcp(connection); }
    catch (e) { if (mounted.current) setError(String(e)); }
    finally { if (mounted.current) setBusy(false); }
  }
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
      <p role="status">{({ qualified: "Qualified simulation environment", released: "Qualified and released NativeFence simulation profile", qualification_unavailable: "Qualification unavailable or withdrawn", qualification_expired: "Qualification expired", environment_unavailable: "Environment unavailable" })[view.offers.length > 0 && view.offers.every((offer) => Date.now() >= Math.min(offer.scope.environment.offerExpiry, offer.scope.qualification.expiresAt)) ? "qualification_expired" : view.availability ?? "environment_unavailable"]}</p>
      {view.offers.length === 0 ? <p>No executable environment is offered. NativeFence simulation requires an exact, current simulator/controller qualification.</p> : <ul>{view.offers.map((offer) => <li key={offer.scopeDigest}>
        {offer.scope.environment.environment} · {offer.scope.environment.evidenceClass} · {offer.scope.qualification.requiredEnforcementClass.replace(/_/g, " ")}
        <button type="button" disabled={busy || Date.now() >= Math.min(offer.scope.environment.offerExpiry, offer.scope.qualification.expiresAt)} onClick={() => void command({ kind: "compose", offer_digest: offer.scopeDigest })}>Compose review</button>
      </li>)}</ul>}
      {scope && r ? <article>
        <strong>Physical review · {r.state}</strong>
        <p>Environment {scope.environment.environment} · {scope.environment.evidenceClass} · {scope.qualification.requiredEnforcementClass.replace(/_/g, " ")}</p>
        <dl>{physicalScopeSummary(scope).map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
        <details><summary>Complete reviewed scope</summary><pre>{JSON.stringify(scope, null, 2)}</pre></details>
        {!fresh ? <p role="status">This review is stale. Discover and compose a fresh review.</p> : null}
        <button type="button" disabled={busy || !fresh || r.state !== "reviewed"} onClick={() => void command({ kind: "approve", review_id: r.reviewId, scope_digest: r.scopeDigest })}>Approve this scope</button>
        <button type="button" disabled={busy || !fresh || r.state !== "approved" || view.start !== null} onClick={() => void command({ kind: "start", review_id: r.reviewId, scope_digest: r.scopeDigest })}>Start approved action</button>
      </article> : null}
      <div aria-live="polite">
        {view.deliveryPending ? <p>Delivery or semantic reply pending. Execution and consequences may be unknown.</p> : null}
        {status ? <>
          <dl>{physicalStatusRows(status).map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
          <p>{physicalWitnessSummary(status)}</p>
          {status.decisions && status.decisions.length > 0 ? <table>
            <caption>Decision records (latest {status.decisions.length})</caption>
            <thead><tr><th>#</th><th>Proposer</th><th>Option</th><th>Admission</th></tr></thead>
            <tbody>{status.decisions.map((d) => { const [n, who, option, admission] = physicalDecisionRow(d); return <tr key={n}><td>{n}</td><td>{who}</td><td>{option}</td><td>{admission}</td></tr>; })}</tbody>
          </table> : <p>No decision records yet.</p>}
        </> : null}
      </div>
      {view.start ? <div>
        <button type="button" disabled={busy} onClick={() => void command({ kind: "status", start: view.start! })}>Query executor status</button>
        <button type="button" disabled={busy} onClick={() => void command({ kind: "cancel", start: view.start! })}>Cancel task authority</button>
        <button type="button" disabled={busy} onClick={() => void command({ kind: "reconcile", start: view.start! })}>Reconcile consequences</button>
        <p>Cancellation delivery may be uncertain. A stop acknowledgement does not prove physical rest.</p>
        <button type="button" disabled={busy} onClick={() => void connectBrain(view.start!)}>Connect an MCP brain</button>
        {mcp ? <div>
          <p>Add this server to the agent's MCP configuration. It is good for one connection; closing it ends the stream as a crashed brain would.</p>
          <pre>{physicalMcpConfig(mcp)}</pre>
        </div> : null}
      </div> : null}
    </> : null}
    {error ? <p role="alert">{error}</p> : null}
  </section>;
}

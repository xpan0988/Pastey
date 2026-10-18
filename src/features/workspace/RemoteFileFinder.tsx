import { useEffect, useState } from "react";
import { Dot, Icon, ListRow, Notice } from "./ui";
import type { BridgeRoute } from "../../lib/bridgeRouting";
import { bridgeRoutePayload } from "../../lib/bridgeRoutingRuntime";
import { candidateMetadata } from "../../lib/bridgePlanSearchResults";
import { availableRevisionId, FINDER_SCOPES, remoteFindBlocks, remoteFindGoal, remoteFindProgress, type RemoteFindInput, type RemoteFindProgress } from "../../lib/remoteFileFinder";
import {
  approveBridgePlan,
  bindBridgePlanToSession,
  createComposedFileBridgePlan,
  listBridgePlanWorkspace,
  listReceivedRoomControlEvents,
  selectBridgePlanSearchCandidate,
  startBridgePlanAttempt,
  withdrawBridgePlanRevision,
} from "../../lib/tauri";

const POLL_MS = 1_500;

/** Find one file on the selected device and receive only that file in this
 * device's Inbox. It reuses the reviewed Bridge Plan flow: this device
 * approves the exact Search and Transfer, the other device searches only
 * the places named here (its own shared folders are decided there), and
 * only the file chosen from its results is sent. No folder or project moves. */
export function RemoteFileFinder({ roomId, peerName, route }: { roomId: string; peerName: string | null; route: BridgeRoute | null }) {
  const [input, setInput] = useState<RemoteFindInput>({ filenameHint: "", extension: "mp4", scopes: ["authorized_folders"] });
  const [revisionId, setRevisionId] = useState<string | null>(null);
  const [attemptId, setAttemptId] = useState<string | null>(null);
  const [selectedCandidateId, setSelectedCandidateId] = useState<string | null>(null);
  const [progress, setProgress] = useState<RemoteFindProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const device = peerName ?? "the selected device";
  const controlRoute = route ? bridgeRoutePayload(route, "pastey-bridge-control-route-v1") : null;

  useEffect(() => {
    if (!attemptId) return;
    if (progress && ["delivered", "no_matches", "search_failed", "transfer_failed"].includes(progress.phase)) return;
    let cancelled = false;
    const poll = async () => {
      try {
        const [workspace, events] = await Promise.all([listBridgePlanWorkspace(roomId), listReceivedRoomControlEvents(roomId)]);
        if (!cancelled) setProgress(remoteFindProgress(attemptId, workspace, events, selectedCandidateId));
      } catch (error) {
        if (!cancelled) setMessage(error instanceof Error ? error.message : "Pastey could not read the search progress.");
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), POLL_MS);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [attemptId, progress?.phase, roomId, selectedCandidateId]);

  function reset() {
    if (revisionId && !attemptId) void withdrawBridgePlanRevision(roomId, revisionId).catch(() => undefined);
    setRevisionId(null);
    setAttemptId(null);
    setSelectedCandidateId(null);
    setProgress(null);
    setMessage(null);
  }

  async function review() {
    const composed = remoteFindBlocks(input);
    if (!composed.blocks) { setMessage(composed.error); return; }
    setBusy(true); setMessage(null);
    try {
      const workspace = await createComposedFileBridgePlan({ roomId, originalUserGoal: remoteFindGoal(input, device), blocks: composed.blocks });
      const revision = availableRevisionId(workspace);
      if (!revision) throw new Error("Pastey did not return the search to review.");
      setRevisionId(revision);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not prepare the search.");
    } finally { setBusy(false); }
  }

  async function approveAndSearch() {
    if (!revisionId || !controlRoute) return;
    setBusy(true); setMessage(null);
    try {
      const approvalId = `bridge-plan-approval-${crypto.randomUUID()}`;
      await approveBridgePlan(revisionId, approvalId);
      await bindBridgePlanToSession(approvalId, controlRoute);
      const nextAttemptId = `bridge-plan-attempt-${crypto.randomUUID()}`;
      await startBridgePlanAttempt(approvalId, nextAttemptId, controlRoute);
      setAttemptId(nextAttemptId);
      setProgress({ phase: "searching" });
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not start the search.");
    } finally { setBusy(false); }
  }

  async function choose(candidateId: string) {
    if (!attemptId || !controlRoute || selectedCandidateId) return;
    setBusy(true); setMessage(null);
    try {
      await selectBridgePlanSearchCandidate(roomId, attemptId, candidateId, controlRoute);
      setSelectedCandidateId(candidateId);
      setProgress({ phase: "sending" });
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not request that file. Its result may have expired; search again.");
    } finally { setBusy(false); }
  }

  const editing = !revisionId;
  const scopeLabels = FINDER_SCOPES.filter((scope) => input.scopes.includes(scope.value)).map((scope) => scope.label).join(", ");
  return <div className="stack-lg">
    {editing ? <>
      <div className="field">
        <label className="field-label" htmlFor="finder-name">File name contains</label>
        <input id="finder-name" className="input" value={input.filenameHint} onChange={(event) => setInput({ ...input, filenameHint: event.target.value })} onKeyDown={(event) => { if (event.key === "Enter" && input.filenameHint.trim() && route && !busy) void review(); }} placeholder="validation" aria-label="File name to find" autoFocus />
      </div>
      <div className="field">
        <label className="field-label" htmlFor="finder-type">File type <span className="subtle">(optional)</span></label>
        <input id="finder-type" className="input" style={{ maxWidth: 160 }} value={input.extension} onChange={(event) => setInput({ ...input, extension: event.target.value })} aria-label="File type" placeholder="mp4" />
      </div>
      <fieldset className="field" style={{ border: 0, margin: 0, padding: 0 }}>
        <legend className="field-label" style={{ marginBottom: 6 }}>Search on {device} in</legend>
        {FINDER_SCOPES.map((scope) => <label className="check" key={scope.value}><input type="checkbox" checked={input.scopes.includes(scope.value)} onChange={(event) => setInput({ ...input, scopes: event.target.checked ? [...input.scopes, scope.value] : input.scopes.filter((value) => value !== scope.value) })} /><span>{scope.label}{scope.detail ? <small>{scope.detail}</small> : null}</span></label>)}
      </fieldset>
      <div className="row"><span className="spacer xsmall subtle">Next you review the search before anything runs.</span><button type="button" className="btn btn-primary" disabled={busy || !route || !input.filenameHint.trim()} onClick={() => void review()}>{busy ? "Preparing…" : "Review search"}</button></div>
    </> : null}
    {revisionId && !attemptId ? <section className="card">
      <header className="card-header"><h3>Review</h3></header>
      <div className="card-body">
        <p className="small">{remoteFindGoal(input, device)}</p>
        <ul className="small muted"><li>{device} searches only: {scopeLabels}.</li><li>You choose one result. Only that file is sent, encrypted, to this device's Inbox.</li><li>Nothing else moves, and nothing on {device} is changed.</li></ul>
      </div>
      <footer className="card-footer"><button type="button" className="btn" disabled={busy} onClick={reset}>Edit</button><button type="button" className="btn btn-primary" disabled={busy || !route} onClick={() => void approveAndSearch()}>{busy ? "Starting…" : "Approve and search"}</button></footer>
    </section> : null}
    {progress?.phase === "searching" ? <p className="small muted row"><Dot tone="live" pulse />Searching {device}…</p> : null}
    {progress?.phase === "no_matches" ? <Notice>No matching files on {device} in those places.</Notice> : null}
    {progress?.phase === "search_failed" || progress?.phase === "transfer_failed" ? <Notice tone="bad">{progress.message}</Notice> : null}
    {progress?.phase === "candidates" ? <section className="card" aria-label="Matching files"><div className="list">{progress.candidates.map((candidate) => {
      const metadata = candidateMetadata(candidate);
      return <ListRow key={candidate.candidateId} leading={<span className="file-icon" aria-hidden="true"><Icon name="file" /></span>} title={candidate.displayName} meta={`${metadata.size}${metadata.modifiedAt ? ` · modified ${metadata.modifiedAt}` : ""}${metadata.redactedLocation ? ` · ${metadata.redactedLocation}` : ""}`} side={<button type="button" className="btn btn-sm" disabled={busy || Boolean(selectedCandidateId)} onClick={() => void choose(candidate.candidateId)}>Get this file</button>} />;
    })}</div><footer className="card-footer"><span className="spacer xsmall subtle">Results expire after 10 minutes.</span></footer></section> : null}
    {progress?.phase === "sending" ? <p className="small muted row"><Dot tone="live" pulse />Receiving the chosen file from {device}…</p> : null}
    {progress?.phase === "delivered" ? <Notice tone="ok">Received. The file is in your Inbox.</Notice> : null}
    {message ? <Notice tone="bad">{message}</Notice> : null}
    {attemptId ? <div className="row"><span className="spacer" /><button type="button" className="btn" disabled={busy} onClick={reset}>New search</button></div> : null}
  </div>;
}

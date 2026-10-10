import { useEffect, useState } from "react";
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
  return <div className="v2-task-open v2-file-finder">
    <strong>Find a file on {device}</strong>
    {editing ? <>
      <input value={input.filenameHint} onChange={(event) => setInput({ ...input, filenameHint: event.target.value })} placeholder="Part of the file name, e.g. validation" aria-label="File name to find" />
      <label>File type <input value={input.extension} onChange={(event) => setInput({ ...input, extension: event.target.value })} aria-label="File type" placeholder="mp4" /></label>
      <fieldset className="v2-finder-scopes"><legend>Search on {device} in</legend>{FINDER_SCOPES.map((scope) => <label key={scope.value}><input type="checkbox" checked={input.scopes.includes(scope.value)} onChange={(event) => setInput({ ...input, scopes: event.target.checked ? [...input.scopes, scope.value] : input.scopes.filter((value) => value !== scope.value) })} /> {scope.label}{scope.detail ? <small> {scope.detail}</small> : null}</label>)}</fieldset>
      <footer><button type="button" className="v2-button primary" disabled={busy || !route || !input.filenameHint.trim()} onClick={() => void review()}>{busy ? "Preparing…" : "Review search"}</button></footer>
    </> : null}
    {revisionId && !attemptId ? <section className="v2-task-open">
      <small>{remoteFindGoal(input, device)}</small>
      <ul><li>{device} searches only: {FINDER_SCOPES.filter((scope) => input.scopes.includes(scope.value)).map((scope) => scope.label).join(", ")}.</li><li>You choose one result. Only that file is sent, encrypted, to this device's Inbox.</li><li>Nothing else moves, and nothing on {device} is changed.</li></ul>
      <footer><button type="button" className="v2-button" disabled={busy} onClick={reset}>Edit</button><button type="button" className="v2-button primary" disabled={busy || !route} onClick={() => void approveAndSearch()}>{busy ? "Starting…" : "Approve and search"}</button></footer>
    </section> : null}
    {progress?.phase === "searching" ? <small>Searching {device}…</small> : null}
    {progress?.phase === "no_matches" ? <small>No matching files on {device} in those places.</small> : null}
    {progress?.phase === "search_failed" || progress?.phase === "transfer_failed" ? <small className="v2-error">{progress.message}</small> : null}
    {progress?.phase === "candidates" ? <ul className="v2-finder-results" aria-label="Matching files">{progress.candidates.map((candidate) => {
      const metadata = candidateMetadata(candidate);
      return <li key={candidate.candidateId}><span><strong>{candidate.displayName}</strong><small>{metadata.size}{metadata.modifiedAt ? ` · modified ${metadata.modifiedAt}` : ""}{metadata.redactedLocation ? ` · ${metadata.redactedLocation}` : ""}</small></span><button type="button" className="v2-button" disabled={busy || Boolean(selectedCandidateId)} onClick={() => void choose(candidate.candidateId)}>Send to this device</button></li>;
    })}<small>Results expire after 10 minutes.</small></ul> : null}
    {progress?.phase === "sending" ? <small>Sending the chosen file from {device}…</small> : null}
    {progress?.phase === "delivered" ? <small>Received. The file is in this Bridge's Inbox.</small> : null}
    {message ? <small className="v2-error">{message}</small> : null}
    {attemptId ? <footer><button type="button" className="v2-button" disabled={busy} onClick={reset}>New search</button></footer> : null}
  </div>;
}

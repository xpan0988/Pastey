import { useState, type ReactNode } from "react";
import {
  continueNativeSession,
  continueRemoteNativeSession,
  type NativeAgentTaskStatus,
  type SharedWorkspace,
} from "../../lib/tauri";
import {
  continuationAvailability,
  evidenceGroups,
  originLabel,
  providerLabel,
  relativeTime,
  resumeLabel,
  sessionHoldLine,
  sessionPersistenceLine,
  sessionStatusPresentation,
  snapshotActivityLine,
  type SessionView,
} from "../../lib/nativeSessions";
import type { ContinuedSession } from "./AgentTaskLifecycle";
import type { SessionSnapshot } from "./NativeSessionsCard";
import { Badge, Dot, Icon, Notice, type Tone } from "./ui";

const STATUS_TONE: Record<string, Tone> = { live: "live", done: "ok", warning: "warn", unknown: "neutral" };

export type ContinueRoute = { roomId: string; peerSessionId: string; hostRef: string } | null;

/** One session, read only, laid out for the question people open it with:
 * can I continue it, and what happens if I do? Its state follows in plain
 * words; the evidence behind every statement is one step away. */
export function SessionDetail({ snapshot, hostName, remote, folder, route, share, busy, run, onBack, onLookAgain, onContinued }: {
  snapshot: SessionSnapshot;
  hostName: string;
  remote: boolean;
  /** This device's folder the session was read in (this device only). */
  folder: string;
  route: ContinueRoute;
  /** On another device: the share of the session's folder, as that device
   * last listed it (`null`: no longer listed; omitted: not known). */
  share?: SharedWorkspace | null;
  busy: boolean;
  /** The task continuing this session, when there is one: shown here, in
   * place of the composer, while it runs. */
  run: { node: ReactNode; active: boolean } | null;
  onBack: () => void;
  onLookAgain: () => void;
  onContinued?: (status: NativeAgentTaskStatus, hostRef: string | null, session: ContinuedSession) => void;
}) {
  const session = snapshot.session;
  const status = sessionStatusPresentation(session);
  const hold = sessionHoldLine(session, hostName);
  const workspace = snapshot.workspace;
  return <section className="card session-detail" aria-label="Session report">
    <header className="card-header">
      <button type="button" className="btn btn-ghost btn-sm" onClick={onBack}><Icon name="back" small />All sessions</button>
      <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={onLookAgain}><Icon name="refresh" small />{busy ? "Looking…" : "Look again"}</button>
    </header>
    <div className="card-body">
      <div className="session-title">
        <div className="stack" style={{ gap: 2, minWidth: 0 }}>
          <h3>{session.title ?? "(untitled)"}</h3>
          <span className="xsmall subtle">{providerLabel(session.provider)} on {hostName} · {session.workspaceLabel}{session.gitBranch ? ` · ${session.gitBranch}` : ""} · started in the {originLabel(session.origin)} · active {relativeTime(session.lastActivityAtMs)}</span>
        </div>
        <span className="row">{session.origin === "pastey" ? <Badge tone="accent">Started by Pastey</Badge> : null}<span title={status.explanation}><Badge tone={STATUS_TONE[status.tone]}>{status.label}</Badge></span></span>
      </div>

      {run?.active ? run.node : <>
        {run ? run.node : null}
        <ContinueSession session={session} remote={remote} folder={folder} onContinued={onContinued} route={route} hostName={hostName} share={share} />
      </>}

      {run?.active ? <p className="xsmall subtle">Below is the session as Pastey read it before your continuation; it is read again when the continuation ends.</p> : null}
      <dl className="facts session-facts">
        <dt>State</dt><dd>{status.explanation}</dd>
        <dt>Open anywhere?</dt><dd>{hold.text}</dd>
        <dt>History</dt><dd>{snapshotActivityLine(snapshot)}{snapshot.activity.changedFiles.length ? ` Changed: ${snapshot.activity.changedFiles.join(", ")}.` : ""}</dd>
        <dt>Folder now</dt><dd>{workspace.exists ? workspace.gitBranch ? `${workspace.gitBranch} at ${workspace.head ?? "?"}, ${workspace.uncommittedPaths ?? "?"} uncommitted path(s)${workspace.commitsSinceLastActivity != null ? `, ${workspace.commitsSinceLastActivity} commit(s) since the session's last activity` : ""}.` : "Not a Git repository." : "The folder no longer exists."}</dd>
      </dl>

      {snapshot.excerptsShared
        ? snapshot.recent.length ? <div className="evidence-group"><h4>Recent conversation</h4><p className="xsmall subtle">Replies are the agent's own account, not verified.</p>{snapshot.recent.map((entry, index) => <div className="excerpt" key={index}><small>{entry.role === "user" ? "Prompt" : "Agent said"} · {relativeTime(entry.atMs)}</small>{entry.text}</div>)}</div> : null
        : <p className="xsmall subtle">{hostName} shares statuses only, not excerpts of the conversation.</p>}

      <SessionEvidence snapshot={snapshot} hostName={hostName} howOpen={hold.how} />
    </div>
  </section>;
}

/** Where each statement above comes from. Collapsed: needed to check a
 * statement or report a problem, not to decide. */
function SessionEvidence({ snapshot, hostName, howOpen }: { snapshot: SessionSnapshot; hostName: string; howOpen: string }) {
  const session = snapshot.session;
  const evidence = evidenceGroups(snapshot.evidence);
  return <details className="disclosure">
    <summary><Icon name="chevron" small />Evidence and how Pastey knows</summary>
    <div className="disclosure-body stack">
      <p className="small muted">{sessionPersistenceLine(session, hostName)}</p>
      <dl className="facts">
        <dt>Open anywhere?</dt><dd>{howOpen} {resumeLabel(session)}, {relativeTime(session.live.observedAtMs)}. Continuing checks again at the moment it starts.</dd>
        <dt>Live state</dt><dd><Dot tone={session.live.state === "running" ? "live" : "neutral"} /> {session.live.state.replace("_", " ")} · from {session.live.source}{session.live.detail ? ` · ${session.live.detail}` : ""}</dd>
        <dt>Last turn</dt><dd>{session.lastTurn.outcome.replace("_", " ")} ({session.lastTurn.source}{session.lastTurn.atMs ? `, ${relativeTime(session.lastTurn.atMs)}` : ""})</dd>
        <dt>Ids</dt><dd className="mono">{session.provider === "codex" ? "thread" : "session"} {session.sessionId}{session.lastTurn.marker ? ` · ends with ${session.lastTurn.marker}` : ""}</dd>
      </dl>
      <div className="evidence">
        {evidence.facts.length ? <div className="evidence-group"><h4><Dot tone="ok" />Facts</h4><ul>{evidence.facts.map((item) => <li key={item.statement}>{item.statement} <small>({item.source})</small></li>)}</ul></div> : null}
        {evidence.inferences.length ? <div className="evidence-group"><h4><Dot tone="warn" />Inferred</h4><ul>{evidence.inferences.map((item) => <li key={item.statement}>{item.statement} <small>({item.source})</small></li>)}</ul></div> : null}
        {evidence.unknowns.length ? <div className="evidence-group"><h4><Dot />Not known</h4><ul>{evidence.unknowns.map((item) => <li key={item.statement}>{item.statement}</li>)}</ul></div> : null}
      </div>
    </div>
  </details>;
}

/** Continues this session in the session itself, with one new prompt. The
 * Host checks again at launch; nothing is retried, and no new session is
 * ever made in its place. Its progress then replaces this form. */
function ContinueSession({ session, remote, folder, onContinued, route, hostName, share }: { session: SessionView; remote: boolean; folder: string; onContinued?: (status: NativeAgentTaskStatus, hostRef: string | null, session: ContinuedSession) => void; route: ContinueRoute; hostName: string; share?: SharedWorkspace | null }) {
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [started, setStarted] = useState(false);
  const availability = remote && !route
    ? { allowed: false, reason: `${hostName} is not connected right now.` }
    : continuationAvailability(session, !remote, hostName, share);

  async function submit() {
    if (!availability.allowed || !prompt.trim() || busy || started) return;
    setBusy(true); setError(null);
    try {
      const status = route
        ? await continueRemoteNativeSession({ roomId: route.roomId, peerSessionId: route.peerSessionId, targetHostRef: route.hostRef, provider: session.provider, sessionId: session.sessionId, scopeRef: session.scopeRef, prompt: prompt.trim(), expectedMarker: session.lastTurn.marker ?? null })
        : await continueNativeSession({ workspaces: [folder], provider: session.provider, sessionId: session.sessionId, prompt: prompt.trim(), expectedMarker: session.lastTurn.marker ?? null });
      setStarted(true);
      const continued: ContinuedSession = { provider: session.provider, sessionId: session.sessionId, title: session.title, workspaceLabel: session.workspaceLabel };
      onContinued?.(status, route?.hostRef ?? null, continued);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally { setBusy(false); }
  }

  const steps = remote
    ? [`${hostName} asks its user to Accept.`, `${providerLabel(session.provider)} there reopens this thread; it refuses if the Codex app or a terminal has it open.`, "Pastey checks it still ends with the turn you see here, then sends your prompt as one turn.", "You follow it here, with its reply."]
    : session.provider === "codex"
      ? ["Codex reopens this thread; it refuses if the Codex app or a terminal has it open.", "Pastey checks it still ends with the turn you see here, then sends your prompt as one turn.", "You follow it here, with its reply."]
      : ["Claude Code resumes this session only if nothing holds it.", "Pastey checks it still ends with the entry you see here, then sends your prompt.", "You follow it here, with its reply."];

  return <div className={`continue-panel${availability.allowed ? "" : " blocked"}`} aria-label="Continue this session">
    <div className="row"><Dot tone={availability.allowed ? session.resume.verdict === "safe" ? "ok" : "neutral" : "warn"} /><strong className="spacer">{!availability.allowed ? "You can't continue this session now" : session.resume.verdict === "safe" ? "You can continue this session" : `You can try: ${providerLabel(session.provider)} decides when it starts`}</strong></div>
    {availability.allowed ? <>
      <textarea className="textarea" value={prompt} onChange={(event) => setPrompt(event.target.value)} placeholder={`What should ${providerLabel(session.provider)} do next in this session?`} aria-label="Next prompt" disabled={busy || started} />
      <div className="row"><details className="disclosure spacer"><summary><Icon name="chevron" small />What happens</summary><ol className="small disclosure-body">{steps.map((step) => <li key={step}>{step}</li>)}</ol></details><button type="button" className="btn btn-primary" disabled={busy || started || !prompt.trim()} onClick={() => void submit()}>{busy ? "Sending…" : "Continue session"}</button></div>
      <p className="xsmall subtle">{availability.reason}</p>
    </> : <p className="small muted">{availability.reason}</p>}
    {error ? <Notice tone="bad" title={remote ? undefined : "Nothing ran."}>{error}</Notice> : null}
  </div>;
}

import { useEffect, useRef, useState } from "react";
import type { BridgePeerSession } from "../../lib/bridgePeers";
import {
  continueNativeSession,
  continueRemoteNativeSession,
  inspectNativeSession,
  inspectRemoteNativeSession,
  type SharedWorkspace,
} from "../../lib/tauri";
import {
  continuationAvailability,
  evidenceGroups,
  localSessionView,
  originLabel,
  providerLabel,
  relativeTime,
  remoteOutcomeMessage,
  resumeLabel,
  sessionHoldLine,
  sessionPersistenceLine,
  sessionStatusPresentation,
  snapshotActivityLine,
} from "../../lib/nativeSessions";
import { continuedSessionKey, type ContinuedSession } from "./AgentTaskLifecycle";
import { PastTurnView, TaskTurn } from "./AgentTaskTracker";
import { Bubble, Composer, ConversationHeader, ConversationThread, SystemLine } from "./Conversation";
import type { NativeAgentController } from "./NativeAgentPanel";
import type { SessionRef, SessionSnapshot } from "./NativeSessionsCard";
import { Badge, Dot, Icon, type Tone } from "./ui";

const STATUS_TONE: Record<string, Tone> = { live: "live", done: "ok", warning: "warn", unknown: "neutral" };

/** One existing Claude Code or Codex session as a conversation: what it
 * holds (as much of it as this device may read), then what this device
 * sends it and what comes back, then a composer that continues it. The
 * session is always the one named here, by provider and id; continuing it
 * runs in the session itself, once, only while it still ends where it was
 * read, and on another device only after its user accepts. */
export function SessionConversation({ roomId, agent, peers, members, target, session, folder, share, onBack }: {
  roomId: string;
  agent: NativeAgentController;
  peers: readonly BridgePeerSession[];
  members: readonly BridgePeerSession[];
  /** "local" or the chosen device's HostRef. */
  target: string;
  session: SessionRef;
  /** This device's folder the session was read in (this device only). */
  folder: string;
  /** On another device: the share of the session's folder, as that device
   * last listed it (`null`: no longer listed; omitted: not known). */
  share?: SharedWorkspace | null;
  onBack?: () => void;
}) {
  const remote = target === "local" ? null : peers.find((peer) => peer.hostRef === target) ?? null;
  const hostName = target === "local" ? "this device" : remote?.displayName ?? members.find((peer) => peer.hostRef === target)?.displayName ?? "the other device";
  const route = remote ? { roomId, peerSessionId: remote.peerSessionId, hostRef: remote.hostRef! } : null;
  const key = continuedSessionKey(session);
  const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
  // The session as read when this device began sending to it: shown above
  // what it sent since, so a turn is never shown twice.
  const [base, setBase] = useState<SessionSnapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("");
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [details, setDetails] = useState(false);

  async function read() {
    setBusy(true); setMessage(null);
    try {
      if (route) {
        const response = await inspectRemoteNativeSession(roomId, route.hostRef, session.provider, session.sessionId);
        const problem = remoteOutcomeMessage(response, hostName);
        if (problem || !response.snapshot) { setMessage(problem ?? `${hostName} sent no report.`); return; }
        setSnapshot(response.snapshot);
      } else if (target === "local") {
        const local = await inspectNativeSession([folder], session.provider, session.sessionId);
        setSnapshot({ ...local, session: localSessionView(local.session), excerptsShared: true });
      } else {
        setMessage(`${hostName} is not connected right now.`);
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally { setBusy(false); }
  }

  useEffect(() => { setSnapshot(null); setBase(null); setSendError(null); void read(); }, [key, target]); // eslint-disable-line react-hooks/exhaustive-deps

  // This device's turns in this session: the one it follows, and the ones
  // before it in this sitting.
  const ours = agent.origin?.kind === "session" && continuedSessionKey(agent.origin.session) === key && (agent.taskHostRef ?? "local") === target;
  const active = ours && !!agent.status && ["queued", "running"].includes(agent.status.state);
  const otherActive = !ours && !!agent.status && ["queued", "running"].includes(agent.status.state);

  // When its turn ends, read the session again: it now ends with a new
  // turn, which the next continuation must name.
  const wasActive = useRef(false);
  useEffect(() => {
    if (active) { wasActive.current = true; return; }
    if (wasActive.current) { wasActive.current = false; void read(); }
  }, [active]); // eslint-disable-line react-hooks/exhaustive-deps

  const view = snapshot?.session ?? null;
  const shown = ours && base ? base : snapshot;
  const status = view ? sessionStatusPresentation(view) : null;
  const availability = !view ? null
    : remote === null && target !== "local" ? { allowed: false, reason: `${hostName} is not connected right now.` }
      : continuationAvailability(view, !route, hostName, share);
  const certain = view?.resume.verdict === "safe";

  async function send() {
    if (!view || !availability?.allowed || !prompt.trim() || sending || active || otherActive || !snapshot) return;
    setSending(true); setSendError(null);
    const text = prompt.trim();
    try {
      const started = route
        ? await continueRemoteNativeSession({ roomId: route.roomId, peerSessionId: route.peerSessionId, targetHostRef: route.hostRef, provider: view.provider, sessionId: view.sessionId, scopeRef: view.scopeRef, prompt: prompt.trim(), expectedMarker: view.lastTurn.marker ?? null })
        : await continueNativeSession({ workspaces: [folder], provider: view.provider, sessionId: view.sessionId, prompt: prompt.trim(), expectedMarker: view.lastTurn.marker ?? null });
      const continued: ContinuedSession = { provider: view.provider, sessionId: view.sessionId, title: view.title, workspaceLabel: view.workspaceLabel, scopeRef: view.scopeRef };
      if (!ours || !base) setBase(snapshot);
      agent.adoptTask(started, route?.hostRef ?? null, continued, text);
      setPrompt("");
    } catch (error) {
      setSendError(error instanceof Error ? error.message : String(error));
    } finally { setSending(false); }
  }

  const blocked = !availability ? (busy ? "Reading the session…" : message ?? "")
    : !availability.allowed ? availability.reason
      : active ? `${providerLabel(session.provider)} is working on your last message. Wait for it, or stop it.`
        : otherActive ? "Codex is working on another task in this Bridge. Wait for it, or stop it there." : null;

  return <section className="conversation" aria-label="Session">
    <ConversationHeader
      onBack={onBack}
      title={view?.title ?? (busy ? "Opening…" : "(untitled)")}
      subtitle={view ? <>{providerLabel(view.provider)} · {view.workspaceLabel}{view.gitBranch ? ` · ${view.gitBranch}` : ""} · {hostName}</> : hostName}
      side={<>
        {status ? <span title={status.explanation}><Badge tone={STATUS_TONE[status.tone]}>{status.label}</Badge></span> : null}
        <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label="Look again" title="Read the session again" disabled={busy} onClick={() => void read()}><Icon name="refresh" /></button>
        <button type="button" className={`btn btn-ghost btn-icon btn-sm${details ? " active" : ""}`} aria-label="Details" title="Details and evidence" aria-pressed={details} onClick={() => setDetails(!details)}><Icon name="info" /></button>
      </>}
    />
    {details && snapshot ? <SessionDetails snapshot={snapshot} hostName={hostName} /> : null}
    <ConversationThread follow={`${shown?.recent.length}:${agent.pastTurns.length}:${agent.status?.state}:${ours}`}>
      {shown ? <SessionHistory snapshot={shown} hostName={hostName} /> : message ? <SystemLine tone="bad">{message}</SystemLine> : <SystemLine>Reading the session on {hostName}…</SystemLine>}
      {ours ? <>
        {agent.pastTurns.map((turn) => <PastTurnView key={turn.taskId} turn={turn} hostName={hostName} />)}
        <TaskTurn roomId={roomId} agent={agent} peers={peers} members={members} />
      </> : null}
    </ConversationThread>
    <Composer
      label="Continue this session"
      value={prompt}
      onChange={setPrompt}
      onSend={() => void send()}
      canSend={!blocked && !!prompt.trim() && !sending}
      disabled={!availability?.allowed || active || otherActive}
      sending={sending}
      placeholder={availability?.allowed ? `Continue this ${providerLabel(session.provider)} session…` : "This session can't be continued right now"}
      bar={<><Icon name="agent" small />{providerLabel(session.provider)} · in this session{route ? ` · ${hostName}` : ""}</>}
      hint={sendError ? sendError : blocked ? blocked : certain ? availability?.reason : <>You can try: {providerLabel(session.provider)} decides when it starts. {availability?.reason}</>}
      hintTone={sendError ? "bad" : blocked ? "warn" : undefined}
    />
  </section>;
}

/** What the session holds, as a conversation, with how much of it is here. */
function SessionHistory({ snapshot, hostName }: { snapshot: SessionSnapshot; hostName: string }) {
  const session = snapshot.session;
  const prompts = snapshot.activity.userTurns;
  const shownPrompts = snapshot.recent.filter((entry) => entry.role === "user").length;
  const summary = `${prompts} prompt${prompts === 1 ? "" : "s"}, ${snapshot.activity.toolCalls} tool call${snapshot.activity.toolCalls === 1 ? "" : "s"}`;
  return <>
    <SystemLine>Started in the {originLabel(session.origin)}{session.createdAtMs ? ` · ${relativeTime(session.createdAtMs)}` : ""} · {snapshot.historyComplete ? summary : `at least ${summary}`}</SystemLine>
    {!snapshot.excerptsShared
      ? <SystemLine>{hostName} shares this session's status only, not its messages.</SystemLine>
      : snapshot.recent.length && (shownPrompts < prompts || !snapshot.historyComplete)
        ? <SystemLine>Earlier messages are not shown here: {hostName === "this device" ? "the newest" : `${hostName} shares only short excerpts of the newest`} messages.</SystemLine>
        : null}
    {snapshot.excerptsShared ? snapshot.recent.map((entry, index) => <Bubble key={index} from={entry.role === "user" ? "me" : "agent"} meta={<>{entry.role === "user" ? "Prompt" : `${providerLabel(session.provider)} said`}{entry.atMs ? ` · ${relativeTime(entry.atMs)}` : ""}{entry.role === "user" ? "" : " · its own account, not verified"}</>}>{entry.text}</Bubble>) : null}
    <SystemLine>{sessionStatusPresentation(session).label}{session.lastTurn.atMs ? ` · ${relativeTime(session.lastTurn.atMs)}` : ""}</SystemLine>
  </>;
}

/** Everything the report knows and where it comes from. On request: it
 * is needed to check a statement or report a problem, not to continue. */
function SessionDetails({ snapshot, hostName }: { snapshot: SessionSnapshot; hostName: string }) {
  const session = snapshot.session;
  const status = sessionStatusPresentation(session);
  const hold = sessionHoldLine(session, hostName);
  const evidence = evidenceGroups(snapshot.evidence);
  const workspace = snapshot.workspace;
  return <div className="conversation-details" aria-label="Session details">
    <p className="small muted">{sessionPersistenceLine(session, hostName)}</p>
    <dl className="facts">
      <dt>State</dt><dd>{status.explanation}</dd>
      <dt>Open anywhere?</dt><dd>{hold.text} <span className="subtle">({hold.how} {resumeLabel(session)}, {relativeTime(session.live.observedAtMs)}. Continuing checks again when it starts.)</span></dd>
      <dt>History</dt><dd>{snapshotActivityLine(snapshot)}{snapshot.activity.changedFiles.length ? ` Changed: ${snapshot.activity.changedFiles.join(", ")}.` : ""}</dd>
      <dt>Folder now</dt><dd>{workspace.exists ? workspace.gitBranch ? `${workspace.gitBranch} at ${workspace.head ?? "?"}, ${workspace.uncommittedPaths ?? "?"} uncommitted path(s)${workspace.commitsSinceLastActivity != null ? `, ${workspace.commitsSinceLastActivity} commit(s) since the session's last activity` : ""}.` : "Not a Git repository." : "The folder no longer exists."}</dd>
      <dt>Live state</dt><dd><Dot tone={session.live.state === "running" ? "live" : "neutral"} /> {session.live.state.replace("_", " ")} · from {session.live.source}{session.live.detail ? ` · ${session.live.detail}` : ""}</dd>
      <dt>Last turn</dt><dd>{session.lastTurn.outcome.replace("_", " ")} ({session.lastTurn.source}{session.lastTurn.atMs ? `, ${relativeTime(session.lastTurn.atMs)}` : ""})</dd>
      <dt>Ids</dt><dd className="mono">{session.provider === "codex" ? "thread" : "session"} {session.sessionId}{session.lastTurn.marker ? ` · ends with ${session.lastTurn.marker}` : ""}</dd>
    </dl>
    <div className="evidence">
      {evidence.facts.length ? <div className="evidence-group"><h4><Dot tone="ok" />Facts</h4><ul>{evidence.facts.map((item) => <li key={item.statement}>{item.statement} <small>({item.source})</small></li>)}</ul></div> : null}
      {evidence.inferences.length ? <div className="evidence-group"><h4><Dot tone="warn" />Inferred</h4><ul>{evidence.inferences.map((item) => <li key={item.statement}>{item.statement} <small>({item.source})</small></li>)}</ul></div> : null}
      {evidence.unknowns.length ? <div className="evidence-group"><h4><Dot />Not known</h4><ul>{evidence.unknowns.map((item) => <li key={item.statement}>{item.statement}</li>)}</ul></div> : null}
    </div>
  </div>;
}

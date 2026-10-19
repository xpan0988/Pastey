import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import type { SessionReadingKnowledge } from "../../lib/accessGuidance";
import {
  discoverNativeSessions,
  discoverRemoteNativeSessions,
  inspectNativeSession,
  inspectRemoteNativeSession,
  type RemoteNativeSessionSnapshot,
} from "../../lib/tauri";
import {
  ACTIVITY_FILTERS,
  evidenceGroups,
  localSessionView,
  originLabel,
  providerLabel,
  relativeTime,
  remoteOutcomeMessage,
  resumeLabel,
  sessionFilters,
  sessionStatusPresentation,
  snapshotActivityLine,
  type SessionView,
} from "../../lib/nativeSessions";
import { Badge, Dot, EmptyState, Icon, ListRow, Notice, type Tone } from "./ui";

type Peer = { peerSessionId: string; displayName: string; hostRef?: string | null };
type Snapshot = Omit<RemoteNativeSessionSnapshot, "session" | "activity" | "excerptsShared"> & {
  session: SessionView;
  activity: RemoteNativeSessionSnapshot["activity"] | Omit<RemoteNativeSessionSnapshot["activity"], "changedElsewhere">;
  excerptsShared: boolean;
};

const STATUS_TONE: Record<string, Tone> = { live: "live", done: "ok", warning: "warn", unknown: "neutral" };

/** Claude Code and Codex sessions on this device or a connected device,
 * read only. Another device answers only for folders its own user shares
 * with this device, and sends statuses and evidence, not transcripts.
 * Looking never starts, continues or stops a session. */
export function NativeSessionsCard({ roomId, peers, members, target, onReading }: {
  roomId: string;
  peers: Peer[];
  members?: Peer[];
  /** "local" or the chosen device's HostRef, chosen by the Agents workspace. */
  target: string;
  /** What looking learned about the other device's reading permission. */
  onReading?: (knowledge: SessionReadingKnowledge) => void;
}) {
  const [folder, setFolder] = useState("");
  const [form, setForm] = useState<{ provider: string; text: string; activeWithinSecs: number | null; scopeRef: string }>({ provider: "", text: "", activeWithinSecs: null, scopeRef: "" });
  const [sessions, setSessions] = useState<SessionView[] | null>(null);
  const [scopes, setScopes] = useState<Array<{ scopeRef: string; label: string; shareExcerpts: boolean }>>([]);
  const [notes, setNotes] = useState<string[]>([]);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Chosen by durable HostRef, so a reconnect keeps the choice.
  const remote = target === "local" ? null : peers.find((peer) => peer.hostRef === target) ?? null;
  const targetName = target === "local" ? "this device" : (members ?? peers).find((peer) => peer.hostRef === target)?.displayName ?? "the other device";
  const hostName = remote?.displayName ?? targetName;
  const firstTarget = useRef(true);

  useEffect(() => {
    if (firstTarget.current) { firstTarget.current = false; return; }
    setSessions(null); setSnapshot(null); setScopes([]); setNotes([]); setMessage(null); setForm((current) => ({ ...current, scopeRef: "" }));
  }, [target]);

  async function run(action: () => Promise<void>) {
    setBusy(true); setMessage(null);
    try { await action(); }
    catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  }

  const look = () => run(async () => {
    setSnapshot(null);
    const filters = sessionFilters(form);
    if (remote) {
      const response = await discoverRemoteNativeSessions(roomId, remote.hostRef!, filters);
      setScopes(response.scopes);
      const problem = remoteOutcomeMessage(response, hostName);
      if (response.outcome === "not_authorized") onReading?.({ state: "not_allowed" });
      else if (response.outcome === "ok") onReading?.({ state: "allowed", folders: response.scopes.map((scope) => scope.label) });
      if (problem) { setSessions(null); setMessage(problem); return; }
      setSessions(response.discovery?.sessions ?? []);
      setNotes([...(response.discovery?.providers ?? []).flatMap((provider) => provider.note ? [`${providerLabel(provider.provider)}: ${provider.note}`] : []), ...(response.discovery?.truncated ? ["Only the newest sessions were sent; narrow the search to see others."] : [])]);
    } else {
      if (!folder) { setMessage("Choose a folder on this device first."); return; }
      const discovery = await discoverNativeSessions([folder], filters);
      setSessions(discovery.sessions.map(localSessionView));
      setNotes(discovery.providers.flatMap((provider) => provider.note ? [`${providerLabel(provider.provider)}: ${provider.note}`] : []));
    }
  });

  const inspect = (session: SessionView) => run(async () => {
    if (remote) {
      const response = await inspectRemoteNativeSession(roomId, remote.hostRef!, session.provider, session.sessionId);
      const problem = remoteOutcomeMessage(response, hostName);
      if (problem || !response.snapshot) { setMessage(problem ?? `${hostName} sent no report.`); return; }
      setSnapshot(response.snapshot);
    } else {
      const local = await inspectNativeSession([folder], session.provider, session.sessionId);
      setSnapshot({ ...local, session: localSessionView(local.session), excerptsShared: true });
    }
  });

  async function chooseFolder() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") { setFolder(chosen); setSessions(null); setSnapshot(null); }
  }

  const disconnected = target !== "local" && !remote;

  return <div className="stack-lg" aria-label="Agent sessions">
    <section className="card">
      <div className="card-body">
        <div className="filters">
          <select className="select" value={form.provider} onChange={(event) => setForm({ ...form, provider: event.target.value })} aria-label="Agent"><option value="">Claude Code and Codex</option><option value="claude_code">Claude Code</option><option value="codex">Codex</option></select>
          <select className="select" value={form.activeWithinSecs ?? ""} onChange={(event) => setForm({ ...form, activeWithinSecs: event.target.value ? Number(event.target.value) : null })} aria-label="Last activity">{ACTIVITY_FILTERS.map((filter) => <option key={filter.label} value={filter.seconds ?? ""}>{filter.label}</option>)}</select>
        </div>
        {target === "local"
          ? <div className="row"><input className="input input-mono" readOnly value={folder} placeholder="Choose a folder on this device…" aria-label="Folder" /><button type="button" className="btn" disabled={busy} onClick={() => void chooseFolder()}><Icon name="folder" />Choose…</button></div>
          : scopes.length > 1
            ? <select className="select" value={form.scopeRef} onChange={(event) => setForm({ ...form, scopeRef: event.target.value })} aria-label="Shared folder"><option value="">All folders {hostName} shares</option>{scopes.map((scope) => <option key={scope.scopeRef} value={scope.scopeRef}>{scope.label}</option>)}</select>
            : <p className="small muted">{hostName} answers only for folders it shares with this device under its Settings → Session reading.</p>}
        <div className="row">
          <input className="input" value={form.text} onChange={(event) => setForm({ ...form, text: event.target.value })} onKeyDown={(event) => { if (event.key === "Enter") void look(); }} placeholder="Title, folder or branch contains…" aria-label="Search sessions" />
          <button type="button" className="btn btn-primary" disabled={busy || disconnected} onClick={() => void look()}><Icon name="search" />{busy ? "Looking…" : sessions ? "Look again" : "Look for sessions"}</button>
        </div>
        {disconnected ? <Notice tone="warn">{targetName} is not connected right now.</Notice> : null}
        {notes.map((note) => <p key={note} className="xsmall subtle">{note}</p>)}
        {message ? <Notice tone="warn">{message}</Notice> : null}
      </div>
    </section>

    {sessions && sessions.length === 0 ? <EmptyState title="No sessions match">Try a wider time range or a different search.</EmptyState> : null}
    {sessions && sessions.length > 0 ? <section className="card">
      <header className="card-header"><h3>{sessions.length === 1 ? "1 session" : `${sessions.length} sessions`}</h3>{sessions.length > 1 ? <span className="xsmall subtle">Choose one; Pastey does not pick for you.</span> : null}</header>
      <div className="list">
        {sessions.map((session) => {
          const status = sessionStatusPresentation(session);
          return <ListRow
            key={`${session.provider}-${session.sessionId}`}
            selected={snapshot?.session.sessionId === session.sessionId}
            disabled={busy}
            onClick={() => void inspect(session)}
            title={session.title ?? "(untitled)"}
            meta={`${providerLabel(session.provider)} · ${session.workspaceLabel}${session.gitBranch ? ` · ${session.gitBranch}` : ""} · ${originLabel(session.origin)} · ${relativeTime(session.lastActivityAtMs)}`}
            side={<>{session.origin === "pastey" ? <Badge tone="accent">Started by Pastey</Badge> : null}<span title={status.explanation}><Badge tone={STATUS_TONE[status.tone]}>{status.label}</Badge></span></>}
          />;
        })}
      </div>
    </section> : null}

    {snapshot ? <SessionSnapshotView snapshot={snapshot} hostName={hostName} remote={target !== "local"} /> : null}
  </div>;
}

function SessionSnapshotView({ snapshot, hostName, remote }: { snapshot: Snapshot; hostName: string; remote: boolean }) {
  const session = snapshot.session;
  const status = sessionStatusPresentation(session);
  const evidence = evidenceGroups(snapshot.evidence);
  const workspace = snapshot.workspace;
  return <section className="card" aria-label="Session report">
    <header className="card-header">
      <div className="stack" style={{ gap: 2, minWidth: 0 }}>
        <h3>{session.title ?? "(untitled)"}</h3>
        <span className="xsmall subtle">{providerLabel(session.provider)} on {hostName} · {session.workspaceLabel} · session <span className="mono">{session.sessionId.slice(0, 8)}</span></span>
      </div>
      <Badge tone={STATUS_TONE[status.tone]}>{status.label}</Badge>
    </header>
    <div className="card-body">
      <p className="small muted">{status.explanation}</p>
      <dl className="facts">
        <dt>Live state</dt><dd><Dot tone={session.live.state === "running" ? "live" : "neutral"} /> {session.live.state.replace("_", " ")} · from {session.live.source} · checked {relativeTime(session.live.observedAtMs)}</dd>
        <dt>Last turn</dt><dd>{session.lastTurn.outcome.replace("_", " ")} ({session.lastTurn.source}{session.lastTurn.atMs ? `, ${relativeTime(session.lastTurn.atMs)}` : ""})</dd>
        <dt>Activity</dt><dd>{snapshotActivityLine(snapshot)}</dd>
        {snapshot.activity.changedFiles.length ? <><dt>Changed</dt><dd>{snapshot.activity.changedFiles.join(", ")}</dd></> : null}
        <dt>Folder now</dt><dd>{workspace.exists ? workspace.gitBranch ? `${workspace.gitBranch} at ${workspace.head ?? "?"}, ${workspace.uncommittedPaths ?? "?"} uncommitted path(s)${workspace.commitsSinceLastActivity != null ? `, ${workspace.commitsSinceLastActivity} commit(s) since the session's last activity` : ""}.` : "Not a Git repository." : "The folder no longer exists."}</dd>
        <dt>Continuing</dt><dd>{remote
          ? `Not supported from another device. On ${hostName}: ${resumeLabel(session).toLowerCase()}; continue it in ${providerLabel(session.provider)} there.`
          : `${resumeLabel(session)}. This view does not continue sessions; continue it in ${providerLabel(session.provider)} itself.`}</dd>
      </dl>
      <div className="evidence">
        {evidence.facts.length ? <div className="evidence-group"><h4><Dot tone="ok" />Facts</h4><ul>{evidence.facts.map((item) => <li key={item.statement}>{item.statement} <small>({item.source})</small></li>)}</ul></div> : null}
        {evidence.inferences.length ? <div className="evidence-group"><h4><Dot tone="warn" />Inferred</h4><ul>{evidence.inferences.map((item) => <li key={item.statement}>{item.statement} <small>({item.source})</small></li>)}</ul></div> : null}
        {evidence.unknowns.length ? <div className="evidence-group"><h4><Dot />Not known</h4><ul>{evidence.unknowns.map((item) => <li key={item.statement}>{item.statement}</li>)}</ul></div> : null}
      </div>
      {snapshot.excerptsShared
        ? snapshot.recent.length ? <div className="evidence-group"><h4>Recent conversation</h4><p className="xsmall subtle">Replies are the agent's own account, not verified.</p>{snapshot.recent.map((entry, index) => <div className="excerpt" key={index}><small>{entry.role === "user" ? "Prompt" : "Agent said"} · {relativeTime(entry.atMs)}</small>{entry.text}</div>)}</div> : null
        : <p className="xsmall subtle">{hostName} shares statuses only, not excerpts of the conversation.</p>}
    </div>
  </section>;
}

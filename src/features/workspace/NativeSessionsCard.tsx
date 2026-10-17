import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
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

type Peer = { peerSessionId: string; displayName: string; hostRef?: string | null };
type Snapshot = Omit<RemoteNativeSessionSnapshot, "session" | "activity" | "excerptsShared"> & {
  session: SessionView;
  activity: RemoteNativeSessionSnapshot["activity"] | Omit<RemoteNativeSessionSnapshot["activity"], "changedElsewhere">;
  excerptsShared: boolean;
};

/** Claude Code and Codex sessions on this device or a connected device,
 * read only. Another device answers only for folders its own user shares
 * with this device, and sends statuses and evidence, not transcripts. */
export function NativeSessionsCard({ roomId, peers }: { roomId: string; peers: Peer[] }) {
  const [target, setTarget] = useState("local");
  const [folder, setFolder] = useState("");
  const [form, setForm] = useState<{ provider: string; text: string; activeWithinSecs: number | null; scopeRef: string }>({ provider: "", text: "", activeWithinSecs: null, scopeRef: "" });
  const [sessions, setSessions] = useState<SessionView[] | null>(null);
  const [scopes, setScopes] = useState<Array<{ scopeRef: string; label: string; shareExcerpts: boolean }>>([]);
  const [notes, setNotes] = useState<string[]>([]);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const remote = peers.find((peer) => peer.peerSessionId === target && peer.hostRef) ?? null;
  const hostName = remote?.displayName ?? "this device";

  async function run(action: () => Promise<void>) {
    setBusy(true); setMessage(null);
    try { await action(); }
    catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  }

  function reset(nextTarget: string) {
    setTarget(nextTarget); setSessions(null); setSnapshot(null); setScopes([]); setNotes([]); setMessage(null); setForm({ ...form, scopeRef: "" });
  }

  const look = () => run(async () => {
    setSnapshot(null);
    const filters = sessionFilters(form);
    if (remote) {
      const response = await discoverRemoteNativeSessions(roomId, remote.hostRef!, filters);
      setScopes(response.scopes);
      const problem = remoteOutcomeMessage(response, hostName);
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

  return <div className="v2-task-open" aria-label="Agent sessions">
    <strong>Agent sessions</strong>
    <small>Claude Code and Codex sessions that already exist, with what is known about them. Looking changes nothing.</small>
    <label className="v2-target-select"><select value={target} onChange={(event) => reset(event.target.value)} disabled={busy} aria-label="Device"><option value="local">This device</option>{peers.filter((peer) => peer.hostRef).map((peer) => <option key={peer.peerSessionId} value={peer.peerSessionId}>{peer.displayName}</option>)}</select></label>
    {remote
      ? scopes.length > 1 ? <label className="v2-target-select"><select value={form.scopeRef} onChange={(event) => setForm({ ...form, scopeRef: event.target.value })} aria-label="Shared folder"><option value="">All folders {hostName} shares</option>{scopes.map((scope) => <option key={scope.scopeRef} value={scope.scopeRef}>{scope.label}</option>)}</select></label> : <small>{hostName} answers only for folders it shares with this device under its Settings → Session reading.</small>
      : <button type="button" className="v2-button" disabled={busy} onClick={() => void chooseFolder()}>{folder ? `Folder: ${folder}` : "Choose a folder on this device…"}</button>}
    <label className="v2-target-select"><select value={form.provider} onChange={(event) => setForm({ ...form, provider: event.target.value })} aria-label="Agent"><option value="">Claude Code and Codex</option><option value="claude_code">Claude Code</option><option value="codex">Codex</option></select></label>
    <label className="v2-target-select"><select value={form.activeWithinSecs ?? ""} onChange={(event) => setForm({ ...form, activeWithinSecs: event.target.value ? Number(event.target.value) : null })} aria-label="Last activity">{ACTIVITY_FILTERS.map((filter) => <option key={filter.label} value={filter.seconds ?? ""}>{filter.label}</option>)}</select></label>
    <input value={form.text} onChange={(event) => setForm({ ...form, text: event.target.value })} placeholder="Title, folder or branch contains…" aria-label="Search sessions" />
    <footer><button type="button" className="v2-button primary" disabled={busy} onClick={() => void look()}>{busy ? "Looking…" : sessions ? "Look again" : "Look for sessions"}</button></footer>
    {notes.map((note) => <small key={note}>{note}</small>)}
    {sessions && sessions.length === 0 ? <small>No sessions match.</small> : null}
    {sessions && sessions.length > 1 ? <small>{sessions.length} sessions match. Choose one; Pastey does not pick for you.</small> : null}
    {sessions?.map((session) => {
      const status = sessionStatusPresentation(session);
      return <button type="button" key={`${session.provider}-${session.sessionId}`} className={`v2-session-row tone-${status.tone}${snapshot?.session.sessionId === session.sessionId ? " selected" : ""}`} disabled={busy} onClick={() => void inspect(session)}>
        <span><strong>{session.title ?? "(untitled)"}</strong><small>{providerLabel(session.provider)} · {session.workspaceLabel}{session.gitBranch ? ` · ${session.gitBranch}` : ""} · {originLabel(session.origin)} · {relativeTime(session.lastActivityAtMs)}</small></span>
        <b title={status.explanation}>{status.label}</b>
      </button>;
    })}
    {snapshot ? <SessionSnapshotView snapshot={snapshot} hostName={hostName} /> : null}
    {message ? <small className="v2-error" role="alert">{message}</small> : null}
  </div>;
}

function SessionSnapshotView({ snapshot, hostName }: { snapshot: Snapshot; hostName: string }) {
  const session = snapshot.session;
  const status = sessionStatusPresentation(session);
  const evidence = evidenceGroups(snapshot.evidence);
  const workspace = snapshot.workspace;
  return <section className="v2-task-open" aria-label="Session report">
    <strong>{session.title ?? "(untitled)"}</strong>
    <small>{providerLabel(session.provider)} on {hostName} · {session.workspaceLabel} · session {session.sessionId.slice(0, 8)}</small>
    <div className="v2-settings-row"><span><strong>{status.label}</strong><small>{status.explanation}</small></span><b>{resumeLabel(session)}</b></div>
    <small>Live state: {session.live.state.replace("_", " ")} · from {session.live.source} · checked {relativeTime(session.live.observedAtMs)}</small>
    <small>History: last recorded turn {session.lastTurn.outcome.replace("_", " ")} ({session.lastTurn.source}{session.lastTurn.atMs ? `, ${relativeTime(session.lastTurn.atMs)}` : ""})</small>
    <small>{snapshotActivityLine(snapshot)}</small>
    {snapshot.activity.changedFiles.length ? <small>Changed: {snapshot.activity.changedFiles.join(", ")}</small> : null}
    <small>{workspace.exists ? workspace.gitBranch ? `Folder now: ${workspace.gitBranch} at ${workspace.head ?? "?"}, ${workspace.uncommittedPaths ?? "?"} uncommitted path(s)${workspace.commitsSinceLastActivity != null ? `, ${workspace.commitsSinceLastActivity} commit(s) since the session's last activity` : ""}.` : "The folder is not a Git repository." : "The folder no longer exists."}</small>
    {evidence.facts.length ? <><strong>Facts</strong><ul>{evidence.facts.map((item) => <li key={item.statement}>{item.statement} <small>({item.source})</small></li>)}</ul></> : null}
    {evidence.inferences.length ? <><strong>Inferred</strong><ul>{evidence.inferences.map((item) => <li key={item.statement}>{item.statement} <small>({item.source})</small></li>)}</ul></> : null}
    {evidence.unknowns.length ? <><strong>Not known</strong><ul>{evidence.unknowns.map((item) => <li key={item.statement}>{item.statement}</li>)}</ul></> : null}
    {snapshot.excerptsShared
      ? snapshot.recent.length ? <><strong>Recent conversation</strong><small>Replies are the agent's own account, not verified.</small><ul>{snapshot.recent.map((entry, index) => <li key={index}><small>{entry.role === "user" ? "Prompt" : "Agent said"} · {relativeTime(entry.atMs)}</small><br />{entry.text}</li>)}</ul></> : null
      : <small>{hostName} shares statuses only, not excerpts of the conversation.</small>}
  </section>;
}

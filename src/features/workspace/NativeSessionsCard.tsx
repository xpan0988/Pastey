import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  discoverNativeSessions,
  discoverRemoteNativeSessions,
  inspectNativeSession,
  inspectRemoteNativeSession,
  type NativeAgentTaskStatus,
  type NativeSessionProvider,
  type RemoteNativeSessionSnapshot,
} from "../../lib/tauri";
import { peerShareGuidance } from "../../lib/workspaceShares";
import type { PeerShares } from "./BridgeWorkspaces";
import {
  ACTIVITY_FILTERS,
  continuationChip,
  localSessionView,
  originLabel,
  providerLabel,
  relativeTime,
  remoteOutcomeMessage,
  sessionFilters,
  sessionStatusPresentation,
  type SessionView,
} from "../../lib/nativeSessions";
import { continuedSessionKey, type ContinuedSession } from "./AgentTaskLifecycle";
import { SessionDetail } from "./SessionDetail";
import { Badge, EmptyState, Icon, ListRow, Notice, type Tone } from "./ui";

type Peer = { peerSessionId: string; displayName: string; hostRef?: string | null };
export type SessionSnapshot = Omit<RemoteNativeSessionSnapshot, "session" | "activity" | "excerptsShared"> & {
  session: SessionView;
  activity: RemoteNativeSessionSnapshot["activity"] | Omit<RemoteNativeSessionSnapshot["activity"], "changedElsewhere">;
  excerptsShared: boolean;
};

const CHIP_TONE: Record<string, Tone> = { ok: "ok", live: "live", warn: "warn", neutral: "neutral" };

/** A request to open one session, from the task that continues it. */
export interface OpenSessionRequest {
  provider: NativeSessionProvider;
  sessionId: string;
  nonce: number;
}

/** Claude Code and Codex sessions on this device or a connected device,
 * read only. Another device answers only for folders its own user shares
 * with this Bridge with “Sessions” on, and sends statuses and evidence, not
 * transcripts. Looking never starts, continues or stops a session; only
 * Continue session in one session's view does, once, as the user wrote it. */
export function NativeSessionsCard({ roomId, peers, members, target, shares, active = true, onContinued, continued, openRequest, onOpenChange }: {
  roomId: string;
  peers: Peer[];
  members?: Peer[];
  /** "local" or the chosen device's HostRef, chosen by the Agents workspace. */
  target: string;
  /** What the chosen device shares with this Bridge, as it last said. */
  shares?: PeerShares;
  /** Shown now: only then does it look by itself. */
  active?: boolean;
  /** A continuation this device started from one session's view. */
  onContinued?: (status: NativeAgentTaskStatus, hostRef: string | null, session: ContinuedSession) => void;
  /** The task continuing a session, shown inside that session's view. */
  continued?: { key: string; ended: boolean; node: ReactNode } | null;
  openRequest?: OpenSessionRequest | null;
  /** The session whose view is open, by provider and id, or null. */
  onOpenChange?: (key: string | null) => void;
}) {
  const [folder, setFolder] = useState("");
  const [form, setForm] = useState<{ provider: string; text: string; activeWithinSecs: number | null; scopeRef: string }>({ provider: "", text: "", activeWithinSecs: null, scopeRef: "" });
  const [sessions, setSessions] = useState<SessionView[] | null>(null);
  const [notes, setNotes] = useState<string[]>([]);
  const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [lookedAt, setLookedAt] = useState<number | null>(null);
  const [opening, setOpening] = useState<string | null>(null);
  // Chosen by durable HostRef, so a reconnect keeps the choice.
  const remote = target === "local" ? null : peers.find((peer) => peer.hostRef === target) ?? null;
  const targetName = target === "local" ? "this device" : (members ?? peers).find((peer) => peer.hostRef === target)?.displayName ?? "the other device";
  const hostName = remote?.displayName ?? targetName;
  const firstTarget = useRef(true);
  const autoLooked = useRef<string | null>(null);

  useEffect(() => {
    if (firstTarget.current) { firstTarget.current = false; return; }
    setSessions(null); setSnapshot(null); setNotes([]); setMessage(null); setLookedAt(null); setForm((current) => ({ ...current, scopeRef: "" }));
    autoLooked.current = null;
  }, [target]);

  const openKey = snapshot ? continuedSessionKey(snapshot.session) : null;
  useEffect(() => { onOpenChange?.(openKey); }, [openKey, onOpenChange]);

  async function run(action: () => Promise<void>) {
    setBusy(true); setMessage(null);
    try { await action(); }
    catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  }

  const look = (nextForm = form) => run(async () => {
    const filters = sessionFilters(nextForm);
    if (remote) {
      const response = await discoverRemoteNativeSessions(roomId, remote.hostRef!, filters);
      // Every answer lists what that device shares now.
      if (response.outcome === "ok" || response.outcome === "not_authorized") shares?.adopt(response.scopes);
      const problem = response.outcome === "not_authorized"
        ? peerShareGuidance(hostName, { state: "known", workspaces: response.scopes }, "read") ?? remoteOutcomeMessage(response, hostName)
        : remoteOutcomeMessage(response, hostName);
      if (problem) { setSessions(null); setMessage(problem); return; }
      setSessions(response.discovery?.sessions ?? []);
      setNotes([...(response.discovery?.providers ?? []).flatMap((provider) => provider.note ? [`${providerLabel(provider.provider)}: ${provider.note}`] : []), ...(response.discovery?.truncated ? ["Only the newest sessions were sent; narrow the search to see others."] : [])]);
    } else {
      if (!folder) { setMessage("Choose a folder on this device first."); return; }
      const discovery = await discoverNativeSessions([folder], filters);
      setSessions(discovery.sessions.map(localSessionView));
      setNotes(discovery.providers.flatMap((provider) => provider.note ? [`${providerLabel(provider.provider)}: ${provider.note}`] : []));
    }
    setLookedAt(Date.now());
  });

  const inspect = (session: { provider: NativeSessionProvider; sessionId: string }) => run(async () => {
    setOpening(continuedSessionKey(session));
    try { await read(session); } finally { setOpening(null); }
  });

  const read = async (session: { provider: NativeSessionProvider; sessionId: string }) => {
    if (remote) {
      const response = await inspectRemoteNativeSession(roomId, remote.hostRef!, session.provider, session.sessionId);
      const problem = remoteOutcomeMessage(response, hostName);
      if (problem || !response.snapshot) { setMessage(problem ?? `${hostName} sent no report.`); return; }
      setSnapshot(response.snapshot);
    } else {
      const local = await inspectNativeSession([folder], session.provider, session.sessionId);
      setSnapshot({ ...local, session: localSessionView(local.session), excerptsShared: true });
    }
  };

  const disconnected = target !== "local" && !remote;
  const knowledge = shares?.knowledge ?? { state: "unknown" as const };
  const readable = knowledge.state === "known" ? knowledge.workspaces.filter((share) => share.readSessions) : [];
  const readingGuidance = target === "local" ? null : peerShareGuidance(hostName, knowledge, "read");

  // Looking is read only, so the list fills itself once there is something
  // to look in: a folder chosen here, or a folder that device shares for
  // sessions. Each such scope is looked in once; Look again repeats it.
  const scopeKey = target === "local" ? (folder ? `local|${folder}` : null)
    : remote && readable.length ? `${target}|${readable.map((share) => share.scopeRef).join(",")}` : null;
  useEffect(() => {
    if (!active || busy || !scopeKey || autoLooked.current === scopeKey) return;
    autoLooked.current = scopeKey;
    void look();
  }, [active, scopeKey, busy]); // eslint-disable-line react-hooks/exhaustive-deps

  // The task continuing a session asks to open it.
  const handledRequest = useRef<number | null>(null);
  useEffect(() => {
    if (!openRequest || handledRequest.current === openRequest.nonce || disconnected) return;
    handledRequest.current = openRequest.nonce;
    void inspect(openRequest);
  }, [openRequest, disconnected]); // eslint-disable-line react-hooks/exhaustive-deps

  // When the continuation of the open session ends, read it again: it now
  // ends with a new turn, which the next continuation must name.
  const continuedHere = continued && openKey === continued.key ? continued : null;
  const wasRunning = useRef(false);
  useEffect(() => {
    if (!continuedHere) { wasRunning.current = false; return; }
    if (!continuedHere.ended) { wasRunning.current = true; return; }
    if (wasRunning.current && snapshot) { wasRunning.current = false; void inspect(snapshot.session); }
  }, [continuedHere?.ended, continuedHere?.key]); // eslint-disable-line react-hooks/exhaustive-deps

  async function chooseFolder() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") { setFolder(chosen); setSessions(null); setSnapshot(null); }
  }

  function setFilter(next: typeof form) {
    setForm(next);
    if (sessions) void look(next);
  }

  if (snapshot) {
    return <div className="stack-lg" aria-label="Agent sessions">
      {message ? <Notice tone="warn">{message}</Notice> : null}
      <SessionDetail
        snapshot={snapshot}
        hostName={hostName}
        remote={target !== "local"}
        folder={folder}
        route={remote ? { roomId, peerSessionId: remote.peerSessionId, hostRef: remote.hostRef! } : null}
        share={knowledge.state === "known" ? knowledge.workspaces.find((share) => share.scopeRef === snapshot.session.scopeRef) ?? null : undefined}
        busy={busy}
        run={continuedHere ? { node: continuedHere.node, active: !continuedHere.ended } : null}
        onBack={() => { setSnapshot(null); setMessage(null); }}
        onLookAgain={() => void inspect(snapshot.session)}
        onContinued={onContinued}
      />
    </div>;
  }

  return <div className="stack-lg" aria-label="Agent sessions">
    <section className="card">
      <div className="card-body">
        <div className="session-toolbar">
          {target === "local"
            ? <div className="row spacer"><input className="input input-mono" readOnly value={folder} placeholder="Choose a folder on this device…" aria-label="Folder" /><button type="button" className="btn" disabled={busy} onClick={() => void chooseFolder()}><Icon name="folder" />Choose…</button></div>
            : readable.length > 1
              ? <select className="select spacer" value={form.scopeRef} onChange={(event) => setFilter({ ...form, scopeRef: event.target.value })} aria-label="Shared folder"><option value="">All folders {hostName} shares for sessions</option>{readable.map((scope) => <option key={scope.scopeRef} value={scope.scopeRef}>{scope.label}</option>)}</select>
              : readable.length === 1
                ? <p className="small muted spacer"><Icon name="folder" small /> {readable[0].label}, the folder {hostName} shares for sessions{readable[0].shareExcerpts ? ", with short excerpts" : ", statuses only"}.</p>
                : <span className="spacer" />}
          <button type="button" className="btn" disabled={busy || disconnected || !scopeKey} onClick={() => void look()}><Icon name="refresh" />{busy && !opening ? "Looking…" : sessions ? "Look again" : "Look for sessions"}</button>
        </div>
        {target !== "local" && !readable.length ? readingGuidance ? <Notice tone="info">{readingGuidance}</Notice>
          : knowledge.state === "unknown" && !disconnected ? <p className="small muted">{shares?.message ?? `Asking ${hostName} which folders it shares…`}</p>
            : <p className="small muted">{hostName} answers only for folders it shares with this Bridge with “Sessions” on.</p> : null}
        {sessions || form.text || form.provider || form.activeWithinSecs ? <div className="filters">
          <input className="input" value={form.text} onChange={(event) => setForm({ ...form, text: event.target.value })} onKeyDown={(event) => { if (event.key === "Enter") void look(); }} placeholder="Title, folder or branch contains…" aria-label="Search sessions" />
          <select className="select" value={form.provider} onChange={(event) => setFilter({ ...form, provider: event.target.value })} aria-label="Agent"><option value="">Claude Code and Codex</option><option value="claude_code">Claude Code</option><option value="codex">Codex</option></select>
          <select className="select" value={form.activeWithinSecs ?? ""} onChange={(event) => setFilter({ ...form, activeWithinSecs: event.target.value ? Number(event.target.value) : null })} aria-label="Last activity">{ACTIVITY_FILTERS.map((filter) => <option key={filter.label} value={filter.seconds ?? ""}>{filter.label}</option>)}</select>
        </div> : null}
        {disconnected ? <Notice tone="warn">{targetName} is not connected right now.</Notice> : null}
        {message ? <Notice tone="warn">{message}</Notice> : null}
      </div>
    </section>

    {sessions && sessions.length === 0 ? <EmptyState title="No sessions match">{form.text || form.provider || form.activeWithinSecs ? "Try a wider time range or a different search." : `Claude Code and Codex have no saved session for ${target === "local" ? "this folder" : "these folders"} on ${hostName}.`}</EmptyState> : null}
    {sessions && sessions.length > 0 ? <section className="card">
      <header className="card-header"><h3>{sessions.length === 1 ? "1 session" : `${sessions.length} sessions`}<span className="xsmall subtle"> · on {hostName}{lookedAt ? `, ${relativeTime(lookedAt)}` : ""}</span></h3>{sessions.length > 1 ? <span className="xsmall subtle">Newest first. Choose one; Pastey does not pick for you.</span> : null}</header>
      <div className="list">
        {sessions.map((session) => {
          const status = sessionStatusPresentation(session);
          const share = knowledge.state === "known" ? knowledge.workspaces.find((candidate) => candidate.scopeRef === session.scopeRef) ?? null : undefined;
          const chip = continuationChip(session, target === "local", hostName, share);
          return <ListRow
            key={`${session.provider}-${session.sessionId}`}
            disabled={busy}
            onClick={() => void inspect(session)}
            title={session.title ?? "(untitled)"}
            meta={`${providerLabel(session.provider)} · ${session.workspaceLabel}${session.gitBranch ? ` · ${session.gitBranch}` : ""} · ${originLabel(session.origin)} · ${relativeTime(session.lastActivityAtMs)} · ${status.label}`}
            side={<>{session.origin === "pastey" ? <Badge tone="accent">Started by Pastey</Badge> : null}{opening === continuedSessionKey(session) ? <span className="xsmall subtle">Opening…</span> : <Badge tone={CHIP_TONE[chip.tone]}>{chip.label}</Badge>}<Icon name="chevron" small /></>}
          />;
        })}
      </div>
      {notes.length ? <details className="disclosure session-notes"><summary><Icon name="chevron" small />How these were read</summary><div className="disclosure-body stack">{notes.map((note) => <p key={note} className="xsmall subtle">{note}</p>)}</div></details> : null}
    </section> : null}
  </div>;
}

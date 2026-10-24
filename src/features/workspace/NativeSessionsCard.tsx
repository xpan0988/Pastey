import { useCallback, useEffect, useRef, useState } from "react";
import {
  discoverNativeSessions,
  discoverRemoteNativeSessions,
  type NativeSessionProvider,
  type RemoteNativeSessionSnapshot,
  type SharedWorkspace,
} from "../../lib/tauri";
import { peerShareGuidance } from "../../lib/workspaceShares";
import type { PeerShares } from "./BridgeWorkspaces";
import {
  continuationChip,
  localSessionView,
  providerLabel,
  relativeTime,
  remoteOutcomeMessage,
  sessionFilters,
  sessionStatusPresentation,
  type SessionView,
} from "../../lib/nativeSessions";
import { continuedSessionKey } from "./AgentTaskLifecycle";
import type { PlaceChoice } from "./NativeAgentPanel";
import { Dot, type Tone } from "./ui";

type Peer = { peerSessionId: string; displayName: string; hostRef?: string | null };
export type SessionSnapshot = Omit<RemoteNativeSessionSnapshot, "session" | "activity" | "excerptsShared"> & {
  session: SessionView;
  activity: RemoteNativeSessionSnapshot["activity"] | Omit<RemoteNativeSessionSnapshot["activity"], "changedElsewhere">;
  excerptsShared: boolean;
};

const CHIP_TONE: Record<string, Tone> = { ok: "ok", live: "live", warn: "warn", neutral: "neutral" };

/** One session, named by what identifies it: its provider and id. */
export interface SessionRef {
  provider: NativeSessionProvider;
  sessionId: string;
}

/** The Claude Code and Codex sessions of one workspace on the chosen
 * device, read only. Another device answers only for a folder its own user
 * shares with this Bridge with “Sessions” on, and sends statuses and
 * evidence, not transcripts. Looking never starts, continues or stops a
 * session. It looks by itself once per workspace while shown. */
export function useSessionList({ roomId, peers, members, target, shares, choice, active }: {
  roomId: string;
  peers: readonly Peer[];
  members: readonly Peer[];
  /** "local" or the chosen device's HostRef. */
  target: string;
  shares?: PeerShares;
  choice: PlaceChoice | null;
  active: boolean;
}) {
  const [sessions, setSessions] = useState<SessionView[] | null>(null);
  const [notes, setNotes] = useState<string[]>([]);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [lookedAt, setLookedAt] = useState<number | null>(null);
  // Chosen by durable HostRef, so a reconnect keeps the choice.
  const remote = target === "local" ? null : peers.find((peer) => peer.hostRef === target) ?? null;
  const hostName = target === "local" ? "this device" : remote?.displayName ?? members.find((peer) => peer.hostRef === target)?.displayName ?? "the other device";
  const knowledge = shares?.knowledge ?? { state: "unknown" as const };
  const share: SharedWorkspace | null = choice?.kind === "shared" && knowledge.state === "known" ? knowledge.workspaces.find((candidate) => candidate.scopeRef === choice.scopeRef) ?? null : null;

  // What this workspace's sessions are looked for in, or why they cannot be.
  const scopeKey = choice?.kind === "local" ? `local|${choice.path}`
    : choice?.kind === "shared" && remote && share?.readSessions ? `${target}|${choice.scopeRef}` : null;
  const unavailable = choice?.kind === "path" || choice?.kind === "send"
    ? `Sessions are listed only for folders ${hostName} shares with “Sessions” on.`
    : choice?.kind === "shared" && share && !share.readSessions ? peerShareGuidance(hostName, knowledge, "read", share)
      : choice?.kind === "shared" && !remote ? `${hostName} is not connected right now.` : null;

  const look = useCallback(async () => {
    if (!scopeKey || !choice) return;
    setBusy(true); setMessage(null);
    try {
      if (choice.kind === "shared" && remote) {
        const response = await discoverRemoteNativeSessions(roomId, remote.hostRef!, sessionFilters({ provider: "", text: "", activeWithinSecs: null, scopeRef: choice.scopeRef }));
        // Every answer lists what that device shares now.
        if (response.outcome === "ok" || response.outcome === "not_authorized") shares?.adopt(response.scopes);
        const problem = response.outcome === "not_authorized"
          ? peerShareGuidance(hostName, { state: "known", workspaces: response.scopes }, "read") ?? remoteOutcomeMessage(response, hostName)
          : remoteOutcomeMessage(response, hostName);
        if (problem) { setSessions(null); setMessage(problem); return; }
        setSessions(response.discovery?.sessions ?? []);
        setNotes([...(response.discovery?.providers ?? []).flatMap((provider) => provider.note ? [`${providerLabel(provider.provider)}: ${provider.note}`] : []), ...(response.discovery?.truncated ? ["Only the newest sessions were sent."] : [])]);
      } else if (choice.kind === "local") {
        const discovery = await discoverNativeSessions([choice.path], {});
        setSessions(discovery.sessions.map(localSessionView));
        setNotes(discovery.providers.flatMap((provider) => provider.note ? [`${providerLabel(provider.provider)}: ${provider.note}`] : []));
      }
      setLookedAt(Date.now());
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally { setBusy(false); }
  }, [choice, hostName, remote, roomId, scopeKey, shares]);

  const looked = useRef<string | null>(null);
  useEffect(() => {
    if (looked.current !== scopeKey) { setSessions(null); setNotes([]); setMessage(null); setLookedAt(null); }
    if (!active || busy || !scopeKey || looked.current === scopeKey) return;
    looked.current = scopeKey;
    void look();
  }, [active, scopeKey, busy]); // eslint-disable-line react-hooks/exhaustive-deps

  return { sessions, notes, message, busy, lookedAt, look, unavailable, share, hostName, remote };
}

export type SessionList = ReturnType<typeof useSessionList>;

/** The sessions of one workspace, newest first. Pastey never picks one:
 * each row says what it is and whether it can be continued from here. */
export function SessionListView({ list, target, openKey, liveKey, onOpen }: {
  list: SessionList;
  target: string;
  openKey: string | null;
  /** The session the followed task continues, if any. */
  liveKey: string | null;
  onOpen: (session: SessionView) => void;
}) {
  const [filter, setFilter] = useState("");
  if (list.unavailable) return <p className="nav-note">{list.unavailable}</p>;
  if (list.message) return <p className="nav-note warn-text">{list.message}</p>;
  if (!list.sessions) return <p className="nav-note">{list.busy ? "Looking for sessions…" : "Sessions appear here."}</p>;
  if (!list.sessions.length) return <p className="nav-note">No Claude Code or Codex session is saved for this folder on {list.hostName} yet.</p>;
  const text = filter.trim().toLowerCase();
  const shown = text ? list.sessions.filter((session) => [session.title, session.gitBranch, providerLabel(session.provider)].some((field) => field?.toLowerCase().includes(text))) : list.sessions;
  return <>
    {list.sessions.length > 6 ? <input className="input input-sm nav-filter" value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Find a session…" aria-label="Find a session" /> : null}
    <ul className="nav-sessions" aria-label="Sessions">
      {shown.map((session) => {
        const key = continuedSessionKey(session);
        const chip = continuationChip(session, target === "local", list.hostName, list.share ?? undefined);
        const status = sessionStatusPresentation(session);
        return <li key={key}>
          <button type="button" className={`nav-session${openKey === key ? " selected" : ""}`} aria-current={openKey === key ? "page" : undefined} onClick={() => onOpen(session)}>
            <span className="nav-session-title">{liveKey === key ? <Dot tone="live" pulse /> : null}{session.title ?? "(untitled)"}</span>
            <span className="nav-session-meta"><span className={`nav-chip ${CHIP_TONE[chip.tone]}`}>{chip.label}</span> · {providerLabel(session.provider)}{session.origin === "pastey" ? " · by Pastey" : ""} · {relativeTime(session.lastActivityAtMs)} · <span title={status.explanation}>{status.label}</span></span>
          </button>
        </li>;
      })}
    </ul>
    {list.notes.length ? <details className="disclosure nav-notes"><summary>How these were read{list.lookedAt ? `, ${relativeTime(list.lookedAt)}` : ""}</summary><div className="disclosure-body stack">{list.notes.map((note) => <p key={note} className="xsmall subtle">{note}</p>)}</div></details> : null}
  </>;
}

import { useCallback, useEffect, useRef, useState } from "react";
import { createLookGate, mergeSessionRead, reconcileWithCurrent, type NativeSessionIdentity, type SessionListState } from "../../lib/sessionList";
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
 * session. It looks each time a workspace is shown (showing what it last
 * found there meanwhile), when asked, and when its owner says something
 * may have changed. A read that fails keeps what was found before, said
 * as such; an older answer never replaces a newer one. */
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
  const [found, setFound] = useState<SessionListState | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Chosen by durable HostRef, so a reconnect keeps the choice.
  const remote = target === "local" ? null : peers.find((peer) => peer.hostRef === target) ?? null;
  const hostName = target === "local" ? "this device" : remote?.displayName ?? members.find((peer) => peer.hostRef === target)?.displayName ?? "the other device";
  const knowledge = shares?.knowledge ?? { state: "unknown" as const };
  const share: SharedWorkspace | null = choice?.kind === "shared" && knowledge.state === "known" ? knowledge.workspaces.find((candidate) => candidate.scopeRef === choice.scopeRef) ?? null : null;

  // What this workspace's sessions are looked for in, or why they cannot be.
  const scopeKey = choice?.kind === "local" ? `local|${choice.path}`
    : choice?.kind === "shared" && remote && share?.readSessions ? `${target}|${choice.scopeRef}` : null;
  const unavailable = choice?.kind === "path" || choice?.kind === "send"
    ? knowledge.state === "known" && knowledge.fullAccess
      ? `To see sessions here, add this folder to ${hostName}'s folders by its path (Full Access).`
      : `Sessions are listed only for folders ${hostName} shares with “Sessions” on.`
    : choice?.kind === "shared" && share && !share.readSessions ? peerShareGuidance(hostName, knowledge, "read", share)
      : choice?.kind === "shared" && !remote ? `${hostName} is not connected right now.` : null;

  // What each workspace showed last, by workspace and device, and which
  // look of each is the newest. Kept for this view only.
  const known = useRef(new Map<string, { found: SessionListState | null; message: string | null }>());
  const gate = useRef(createLookGate());
  const shown = useRef(scopeKey);
  shown.current = scopeKey;

  const look = useCallback(async () => {
    if (!scopeKey || !choice) return;
    const key = scopeKey;
    const lookNumber = gate.current.begin(key);
    if (shown.current === key) setBusy(true);
    const earlier = known.current.get(key)?.found ?? null;
    let next: { found: SessionListState | null; message: string | null };
    try {
      if (choice.kind === "shared" && remote) {
        const response = await discoverRemoteNativeSessions(roomId, remote.hostRef!, sessionFilters({ provider: "", text: "", activeWithinSecs: null, scopeRef: choice.scopeRef }));
        // Every answer lists what that device shares now.
        if (response.outcome === "ok" || response.outcome === "not_authorized") shares?.adopt(response.scopes, response.fullAccess === true);
        if (response.outcome === "not_authorized") {
          // Withdrawn authority, not a failed read: nothing earlier stays.
          next = { found: null, message: peerShareGuidance(hostName, { state: "known", workspaces: response.scopes, fullAccess: response.fullAccess === true }, "read") ?? remoteOutcomeMessage(response, hostName) };
        } else if (response.outcome !== "ok") {
          next = { found: earlier, message: remoteOutcomeMessage(response, hostName) };
        } else {
          const discovery = response.discovery;
          next = { found: mergeSessionRead(earlier, key, {
            providers: discovery?.providers ?? [],
            sessions: discovery?.sessions ?? [],
            at: Date.now(),
            notes: [...(discovery?.providers ?? []).flatMap((provider) => provider.note ? [`${providerLabel(provider.provider)}: ${provider.note}`] : []), ...(discovery?.truncated ? ["Only the newest sessions were sent."] : [])],
          }), message: null };
        }
      } else if (choice.kind === "local") {
        const discovery = await discoverNativeSessions([choice.path], {});
        next = { found: mergeSessionRead(earlier, key, {
          providers: discovery.providers,
          sessions: discovery.sessions.map(localSessionView),
          at: Date.now(),
          notes: discovery.providers.flatMap((provider) => provider.note ? [`${providerLabel(provider.provider)}: ${provider.note}`] : []),
        }), message: null };
      } else return;
    } catch (error) {
      next = { found: earlier, message: error instanceof Error ? error.message : String(error) };
    }
    // A newer look of this workspace answers instead.
    if (!gate.current.isNewest(key, lookNumber)) return;
    known.current.set(key, next);
    if (shown.current === key) { setFound(next.found); setMessage(next.message); setBusy(false); }
  }, [choice, hostName, remote, roomId, scopeKey, shares]);

  // Each time a workspace is shown: what it showed last, then a new look.
  useEffect(() => {
    const last = scopeKey ? known.current.get(scopeKey) : undefined;
    setFound(last?.found ?? null); setMessage(last?.message ?? null); setBusy(false);
    if (active && scopeKey) void look();
  }, [active, scopeKey]); // eslint-disable-line react-hooks/exhaustive-deps

  return {
    sessions: found?.sessions ?? null,
    notes: found?.notes ?? [],
    unread: found?.unread ?? [],
    message,
    busy,
    lookedAt: found?.lookedAt ?? null,
    /** Each provider's native default in this workspace, from the newest read. */
    defaults: found?.defaults ?? {},
    look,
    unavailable,
    share,
    hostName,
    remote,
  };
}

export type SessionList = ReturnType<typeof useSessionList>;

/** The sessions of one workspace, newest first. Pastey never picks one:
 * each row says what it is and whether it can be continued from here. The
 * session the followed conversation runs in (`current`) is that
 * conversation's own row, so it is not listed again. */
export function SessionListView({ list, target, openKey, liveKey, current = null, onOpen }: {
  list: SessionList;
  target: string;
  openKey: string | null;
  /** The session the followed task continues, if any. */
  liveKey: string | null;
  /** The native session of the conversation shown above the list, if known. */
  current?: NativeSessionIdentity | null;
  onOpen: (session: SessionView) => void;
}) {
  const [filter, setFilter] = useState("");
  if (list.unavailable) return <p className="nav-note">{list.unavailable}</p>;
  const refresh = <button type="button" className="btn btn-link small" disabled={list.busy} onClick={() => void list.look()}>{list.busy ? "Reading…" : "Refresh"}</button>;
  if (!list.sessions) {
    return list.message
      ? <p className="nav-note warn-text" role="alert">{list.message} {refresh}</p>
      : <p className="nav-note">{list.busy ? "Looking for sessions…" : "Sessions appear here."}</p>;
  }
  const { listed } = reconcileWithCurrent(list.sessions, current);
  const sessions = listed ?? [];
  const text = filter.trim().toLowerCase();
  const shown = text ? sessions.filter((session) => [session.title, session.gitBranch, providerLabel(session.provider)].some((field) => field?.toLowerCase().includes(text))) : sessions;
  return <>
    {list.message ? <p className="nav-note warn-text" role="alert">Could not read the sessions again: {list.message} Showing what was found {list.lookedAt ? relativeTime(list.lookedAt) : "earlier"}.</p> : null}
    {list.unread.map((unread) => <p key={unread.provider} className="nav-note warn-text" role="status" title={unread.note ?? undefined}>
      {providerLabel(unread.provider)} history could not be read just now{unread.lastReadAt ? `; its sessions are as found ${relativeTime(unread.lastReadAt)}` : ""}. Nothing was removed.
    </p>)}
    {!sessions.length && !current && !list.unread.length ? <p className="nav-note">No Claude Code or Codex session is saved for this folder on {list.hostName} yet.</p> : null}
    {sessions.length > 6 ? <input className="input input-sm nav-filter" value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Find a session…" aria-label="Find a session" /> : null}
    {shown.length ? <ul className="nav-sessions" aria-label="Sessions">
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
    </ul> : null}
    <p className="nav-note nav-sessions-read">{list.lookedAt ? `Read ${relativeTime(list.lookedAt)}` : "Not read yet"} · {refresh}</p>
    {list.notes.length ? <details className="disclosure nav-notes"><summary>How these were read{list.lookedAt ? `, ${relativeTime(list.lookedAt)}` : ""}</summary><div className="disclosure-body stack">{list.notes.map((note) => <p key={note} className="xsmall subtle">{note}</p>)}</div></details> : null}
  </>;
}

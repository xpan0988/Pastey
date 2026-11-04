/** What the Agents sidebar knows of one workspace's native sessions, and how
 * it is kept honest: a session is named by its provider and id only (never
 * by its title or time), a read that fails is never taken as a deletion, and
 * an older answer never replaces a newer one. */
import type { NativeSessionProvider } from "./tauri";
import type { SessionView } from "./nativeSessions";

/** One native session, by what identifies it. */
export interface NativeSessionIdentity {
  provider: NativeSessionProvider;
  sessionId: string;
}

export function nativeSessionKey(session: NativeSessionIdentity): string {
  return `${session.provider}:${session.sessionId}`;
}

export function sameNativeSession(a: NativeSessionIdentity | null | undefined, b: NativeSessionIdentity | null | undefined): boolean {
  return !!a && !!b && a.provider === b.provider && a.sessionId === b.sessionId;
}

/** One provider's part of a read: whether its history could be read. */
export interface ProviderRead {
  provider: NativeSessionProvider;
  historyReadable: boolean;
  note: string | null;
}

/** One answer from session discovery for one workspace. */
export interface SessionRead {
  providers: readonly ProviderRead[];
  sessions: readonly SessionView[];
  /** When this device received the answer. */
  at: number;
  notes?: readonly string[];
}

/** A provider whose history the last read could not reach. */
export interface UnreadProvider {
  provider: NativeSessionProvider;
  note: string | null;
  /** When its history was last read, if ever, in this view. */
  lastReadAt: number | null;
}

export interface SessionListState {
  /** The workspace on the chosen device these sessions belong to. */
  scopeKey: string;
  /** Newest activity first. A provider that could not be read keeps the
   * sessions an earlier read found. */
  sessions: SessionView[];
  unread: UnreadProvider[];
  /** When each provider's history was last read. */
  readAt: Partial<Record<NativeSessionProvider, number>>;
  lookedAt: number;
  notes: string[];
}

function newestFirst(a: SessionView, b: SessionView): number {
  return (b.lastActivityAtMs ?? 0) - (a.lastActivityAtMs ?? 0);
}

/** A new read of one workspace, over what an earlier read of the same
 * workspace found. Sessions of a provider that answered are exactly what it
 * answered; those of a provider that could not be read are kept from the
 * earlier read and the provider is named as unread. Nothing from another
 * workspace or device is ever carried over. */
export function mergeSessionRead(previous: SessionListState | null, scopeKey: string, read: SessionRead): SessionListState {
  const earlier = previous?.scopeKey === scopeKey ? previous : null;
  const unreadable = read.providers.filter((provider) => !provider.historyReadable);
  const unreadNames = new Set(unreadable.map((provider) => provider.provider));
  const answered = read.sessions.filter((session) => !unreadNames.has(session.provider));
  const answeredKeys = new Set(answered.map(nativeSessionKey));
  const kept = (earlier?.sessions ?? []).filter((session) => unreadNames.has(session.provider) && !answeredKeys.has(nativeSessionKey(session)));
  const readAt: SessionListState["readAt"] = { ...(earlier?.readAt ?? {}) };
  for (const provider of read.providers) {
    if (provider.historyReadable) readAt[provider.provider] = read.at;
  }
  return {
    scopeKey,
    sessions: [...answered, ...kept].sort(newestFirst),
    unread: unreadable.map((provider) => ({ provider: provider.provider, note: provider.note, lastReadAt: readAt[provider.provider] ?? null })),
    readAt,
    lookedAt: read.at,
    notes: [...(read.notes ?? [])],
  };
}

/** Hands out a number per look and says whether an answer is still the
 * newest look of its workspace: an answer that arrives after a newer look
 * began is dropped, whatever order the answers arrive in. */
export function createLookGate() {
  let next = 0;
  const newest = new Map<string, number>();
  return {
    begin(scopeKey: string): number {
      next += 1;
      newest.set(scopeKey, next);
      return next;
    },
    isNewest(scopeKey: string, look: number): boolean {
      return newest.get(scopeKey) === look;
    },
  };
}

/** The discovered sessions beside the conversation this device follows. The
 * native session that conversation runs in is that conversation: it is shown
 * once, as the conversation (with its live task), never again as a
 * discovered row. Only an exact provider-and-id match counts. */
export function reconcileWithCurrent(sessions: readonly SessionView[] | null, current: NativeSessionIdentity | null): { listed: SessionView[] | null; currentSession: SessionView | null } {
  if (!sessions) return { listed: null, currentSession: null };
  if (!current) return { listed: [...sessions], currentSession: null };
  const key = nativeSessionKey(current);
  return {
    listed: sessions.filter((session) => nativeSessionKey(session) !== key),
    currentSession: sessions.find((session) => nativeSessionKey(session) === key) ?? null,
  };
}

/** A conversation's name: the one its provider records (a name its user
 * or the provider gave it, else what began it), else its first prompt. A
 * follow-up never renames it. */
export function conversationTitle(discovered: Pick<SessionView, "title"> | null, firstPrompt: string | null): string {
  return discovered?.title?.trim() || firstPrompt?.trim() || "Conversation";
}

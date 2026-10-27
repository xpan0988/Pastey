import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { remoteAccessGuidance, type AccessLine } from "../../lib/accessGuidance";
import { peerShareGuidance, shareSummary, type PeerShareKnowledge } from "../../lib/workspaceShares";
import type { BridgePeerSession } from "../../lib/bridgePeers";
import { getDeviceIdentity, getDeviceProfile, openRemoteFolder, presentDeviceIdentity, type NativeAgentCapability, type SharedWorkspace } from "../../lib/tauri";
import type { RoomInfo } from "../../lib/types";
import {
  continuedSessionKey,
  nativeAgentRecoveryRequiresAction,
  nativeAgentTargetAvailability,
  useRemoteNativeCapabilities,
  type ConversationPlace,
  type NativeAgentTargetAvailability,
} from "./AgentTaskLifecycle";
import { TaskBanner } from "./AgentTaskTracker";
import { availabilityCopy, folderLabel, NewConversation, type NativeAgentController, type PlaceChoice } from "./NativeAgentPanel";
import { SessionListView, useSessionList, type SessionList, type SessionRef } from "./NativeSessionsCard";
import { SessionConversation } from "./SessionDetail";
import { usePeerSharedWorkspaces } from "./BridgeWorkspaces";
import { Badge, Dialog, Dot, EmptyState, Icon, Notice } from "./ui";

/** What the main pane shows: a new conversation, the conversation this
 * Bridge follows, or one existing session, named by provider and id. */
type AgentView = { kind: "new" } | { kind: "current" } | { kind: "session"; session: SessionRef };

const PLACE_KEY = "pastey:agents-place-v1";
const LOCAL_FOLDERS_KEY = "pastey:agents-local-folders-v1";

function readStored<T>(key: string, fallback: T): T {
  try { return (JSON.parse(window.localStorage.getItem(key) ?? "null") as T | null) ?? fallback; }
  catch { return fallback; }
}

function writeStored(key: string, value: unknown) {
  try { window.localStorage.setItem(key, JSON.stringify(value)); }
  catch { /* a convenience only */ }
}

function placeKey(choice: PlaceChoice | null): string | null {
  if (!choice) return null;
  switch (choice.kind) {
    case "shared": return `shared:${choice.scopeRef}`;
    case "local": return `local:${choice.path}`;
    default: return choice.kind;
  }
}

function choiceOf(place: ConversationPlace): PlaceChoice {
  switch (place.kind) {
    case "shared": return { kind: "shared", scopeRef: place.scopeRef };
    case "local": return { kind: "local", path: place.path };
    default: return { kind: place.kind };
  }
}

/** Agent work on one device of this Bridge, laid out as the user thinks of
 * it: that device, the folders it lets this device work in (its
 * workspaces, each with what it allows), the sessions in a workspace, and
 * one open conversation. A new conversation and an existing session are
 * different things and never stand in for each other. Running and reading
 * stay separate authorities, decided by the device that holds them. */
export function AgentWorkspace({ room, agent, peers, members, onOpenDevice, onOpenDetails }: {
  room: RoomInfo;
  agent: NativeAgentController;
  peers: readonly BridgePeerSession[];
  members: readonly BridgePeerSession[];
  onOpenDevice: (hostRef: string) => void;
  onOpenDetails?: () => void;
}) {
  // Prefer the other device: running there is why most people open this.
  const [target, setTarget] = useState<string>(() => members.find((peer) => peer.hostRef)?.hostRef ?? "local");
  const [choice, setChoice] = useState<PlaceChoice | null>(null);
  const [view, setView] = useState<AgentView | null>(null);
  const [accessOpen, setAccessOpen] = useState(false);
  const [localFolders, setLocalFolders] = useState<string[]>(() => readStored<string[]>(LOCAL_FOLDERS_KEY, []));
  const [waitedForAccept, setWaitedForAccept] = useState<Record<string, boolean>>({});
  const options = useMemo(() => members.filter((peer) => peer.hostRef), [members]);
  const targetHostRef = target === "local" ? null : target;
  const targetName = targetHostRef ? options.find((peer) => peer.hostRef === targetHostRef)?.displayName ?? "the other device" : "this device";
  const remote = targetHostRef ? peers.find((peer) => peer.hostRef === targetHostRef) ?? null : null;
  const connected = targetHostRef ? !!remote : true;
  const shares = usePeerSharedWorkspaces(room.id, targetHostRef, connected, remote?.peerSessionId ?? null);
  const remoteCapabilities = useRemoteNativeCapabilities(room.id, remote ? targetHostRef : null, remote?.peerSessionId ?? null);
  const availability: NativeAgentTargetAvailability = targetHostRef
    ? nativeAgentTargetAvailability("agent.coding.codex", remoteCapabilities.observation)
    : localAvailability(agent.capabilities.find((capability) => capability.agentId === "agent.coding.codex"));
  const sharedList = shares.knowledge.state === "known" ? shares.knowledge.workspaces : [];
  const fullAccess = shares.knowledge.state === "known" && shares.knowledge.fullAccess === true;
  const [addingFolder, setAddingFolder] = useState(false);

  // The workspace shown: the one last chosen while it is still offered,
  // else the first shared one (sessions and agents first).
  const storedKey = `${room.id}:${target}`;
  useEffect(() => {
    const remembered = readStored<Record<string, PlaceChoice>>(PLACE_KEY, {})[storedKey] ?? null;
    const offered = (candidate: PlaceChoice | null) => !!candidate && (candidate.kind === "shared"
      ? sharedList.some((share) => share.scopeRef === candidate.scopeRef)
      : candidate.kind === "local" ? !targetHostRef : !!targetHostRef);
    const best = [...sharedList].sort((a, b) => Number(b.readSessions && b.agentRequests) - Number(a.readSessions && a.agentRequests))[0];
    setChoice((current) => offered(current) ? current : offered(remembered) ? remembered : targetHostRef
      ? best ? { kind: "shared", scopeRef: best.scopeRef } : null
      : localFolders[0] ? { kind: "local", path: localFolders[0] } : null);
  }, [storedKey, shares.knowledge, localFolders]); // eslint-disable-line react-hooks/exhaustive-deps

  const choose = useCallback((next: PlaceChoice) => {
    setChoice(next);
    setView(null);
    writeStored(PLACE_KEY, { ...readStored<Record<string, PlaceChoice>>(PLACE_KEY, {}), [storedKey]: next });
  }, [storedKey]);

  function changeTarget(next: string) {
    setTarget(next);
    setChoice(null);
    setView(null);
  }

  async function addLocalFolder() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    const next = [chosen, ...localFolders.filter((folder) => folder !== chosen)].slice(0, 8);
    setLocalFolders(next);
    writeStored(LOCAL_FOLDERS_KEY, next);
    choose({ kind: "local", path: chosen });
  }

  // A run that waits in Review tells this device that the other one has no
  // Full Access for it there. Nothing else is inferred.
  useEffect(() => {
    if (agent.taskHostRef && agent.status?.state === "queued" && agent.status.code === "native_agent_review_required") {
      setWaitedForAccept((current) => current[agent.taskHostRef!] ? current : { ...current, [agent.taskHostRef!]: true });
    }
  }, [agent.status, agent.taskHostRef]);

  // A message sent from a new-conversation draft begins that conversation:
  // show it, rather than a fresh draft.
  const conversationId = agent.origin?.kind === "new" ? agent.origin.conversationId : null;
  useEffect(() => {
    if (conversationId) setView((current) => current?.kind === "new" ? { kind: "current" } : current);
  }, [conversationId]);

  const list = useSessionList({ roomId: room.id, peers, members, target, shares, choice, active: true });
  const share = choice?.kind === "shared" ? sharedList.find((candidate) => candidate.scopeRef === choice.scopeRef) ?? null : null;

  // The followed task: which conversation it belongs to, and whether that
  // conversation is the one open. It is shown once, in its conversation;
  // elsewhere a banner leads back to it while it needs attention.
  const taskTarget = agent.taskHostRef ?? "local";
  const ownsCurrent = agent.origin?.kind === "new" && taskTarget === target && placeKey(choiceOf(agent.origin.place)) === placeKey(choice);
  const taskActive = !!agent.status && ["queued", "running"].includes(agent.status.state);
  const liveKey = agent.origin?.kind === "session" && taskTarget === target ? continuedSessionKey(agent.origin.session) : null;
  const shownView: AgentView | null = view ?? (ownsCurrent ? { kind: "current" } : null);
  const openKey = shownView?.kind === "session" ? continuedSessionKey(shownView.session) : null;
  const taskShown = (shownView?.kind === "current" && ownsCurrent) || (openKey !== null && openKey === liveKey);
  const taskNeedsAttention = !!agent.movement || nativeAgentRecoveryRequiresAction(agent.status, agent.movement)
    || taskActive;

  function openTask() {
    const origin = agent.origin;
    if (taskTarget !== target) setTarget(taskTarget);
    if (origin?.kind === "session") {
      if (origin.session.scopeRef) setChoice(taskTarget === "local" ? { kind: "local", path: origin.session.scopeRef } : { kind: "shared", scopeRef: origin.session.scopeRef });
      setView({ kind: "session", session: { provider: origin.session.provider, sessionId: origin.session.sessionId } });
    } else {
      if (origin?.kind === "new") setChoice(choiceOf(origin.place));
      setView({ kind: "current" });
    }
  }

  const openSession = (session: SessionRef) => setView({ kind: "session", session: { provider: session.provider, sessionId: session.sessionId } });
  const placeBody = (canRun: boolean, runBlocked: string | null) => <PlaceBody canRun={canRun} runBlocked={runBlocked} agent={agent} ownsCurrent={ownsCurrent} shownView={shownView} onNew={() => setView({ kind: "new" })} onCurrent={() => setView({ kind: "current" })} list={list} target={target} openKey={openKey} liveKey={taskActive ? liveKey : null} onOpenSession={openSession} />;
  const back = () => setView(null);
  const main = shownView?.kind === "session"
    ? <SessionConversation key={`${target}:${openKey}`} roomId={room.id} agent={agent} peers={peers} members={members} target={target} session={shownView.session} folder={choice?.kind === "local" ? choice.path : ""} share={targetHostRef ? share : undefined} onBack={back} />
    : choice || shownView?.kind === "current"
      ? <NewConversation key={`${target}:${placeKey(choice)}:${shownView?.kind === "current"}`} roomId={room.id} agent={agent} peers={peers} members={members} target={target} choice={choice} shares={shares} availability={availability} onRecheck={() => void (remote ? remoteCapabilities.refresh() : agent.refreshCapabilities())} current={shownView?.kind === "current"} onNewDraft={() => setView({ kind: "new" })} onBack={back} />
      : <div className="conversation conversation-empty"><EmptyState title={targetHostRef ? `Nothing to work in on ${targetName} yet` : "Choose a folder"}>{targetHostRef
        ? peerShareGuidance(targetName, shares.knowledge, "run") ?? `Choose a workspace ${targetName} shares, or a folder by path.`
        : "Add a folder on this device to see its sessions and start conversations there."}</EmptyState></div>;

  return <div className="agents"><div className="agents-layout" data-open={view !== null || undefined}>
    <aside className="agents-nav" aria-label="Agents">
      {taskNeedsAttention && !taskShown ? <TaskBanner agent={agent} members={members} onOpen={openTask} /> : null}

      <div className="nav-host">
        <label className="nav-label" htmlFor="agent-device">Agents on</label>
        <select id="agent-device" className="select" value={target} onChange={(event) => changeTarget(event.target.value)}>
          {options.map((peer) => <option key={peer.hostRef} value={peer.hostRef}>{peer.displayName}{peers.some((routeable) => routeable.hostRef === peer.hostRef) ? "" : " (not connected)"}</option>)}
          <option value="local">This device</option>
        </select>
        <div className="nav-host-status" role="status">
          <Dot tone={!connected ? "neutral" : availability.state === "available" ? "ok" : availability.state === "checking" ? "neutral" : "warn"} pulse={availability.state === "checking"} />
          <span>{!connected ? "Not connected" : availability.state === "available" ? targetHostRef ? `Connected · Codex ready${fullAccess ? " · Full Access" : ""}` : "Codex ready" : availabilityCopy(availability, targetHostRef ? targetName : null, false)}</span>
        </div>
        {targetHostRef ? <div className="row nav-host-actions">
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => setAccessOpen(true)}><Icon name="shield" small />Access</button>
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => onOpenDevice(targetHostRef)}><Icon name="device" small />Device</button>
        </div> : null}
      </div>

      <div className="nav-section-head">
        <span>{targetHostRef ? fullAccess ? `Workspaces on ${targetName}` : "Shared workspaces" : "Folders on this device"}</span>
        {targetHostRef ? <span className="row">
          {fullAccess ? <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label={`Add a folder on ${targetName}`} title={`Add a folder on ${targetName} by its path (Full Access)`} disabled={!connected} onClick={() => setAddingFolder(true)}><Icon name="plus" small /></button> : null}
          <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label="Ask again" title="Ask again which folders it shares" disabled={shares.loading || !connected} onClick={() => void shares.refresh()}><Icon name="refresh" small /></button>
        </span>
          : <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label="Add a folder" title="Add a folder on this device" onClick={() => void addLocalFolder()}><Icon name="plus" small /></button>}
      </div>
      <ul className="nav-places">
        {targetHostRef
          ? sharedList.map((workspace) => <PlaceItem key={workspace.scopeRef} label={workspace.label} grants={<Grants share={workspace} />} title={shareSummary(workspace)} selected={choice?.kind === "shared" && choice.scopeRef === workspace.scopeRef} onSelect={() => choose({ kind: "shared", scopeRef: workspace.scopeRef })}>
            {placeBody(workspace.agentRequests, peerShareGuidance(targetName, shares.knowledge, "run", workspace))}
          </PlaceItem>)
          : localFolders.map((folder) => <PlaceItem key={folder} label={folderLabel(folder)} grants={<span className="mono">{folder}</span>} title={folder} selected={choice?.kind === "local" && choice.path === folder} onSelect={() => choose({ kind: "local", path: folder })}>
            {placeBody(true, null)}
          </PlaceItem>)}
      </ul>
      {targetHostRef && !sharedList.length ? <p className="nav-note">{!connected ? `${targetName} is not connected right now.` : shares.knowledge.state === "unknown" ? shares.message ?? `Asking ${targetName} which folders it shares…` : <>{peerShareGuidance(targetName, shares.knowledge, "read")}{onOpenDetails ? <> <button type="button" className="btn btn-link small" onClick={onOpenDetails}>Bridge details</button></> : null}</>}</p> : null}
      {!targetHostRef && !localFolders.length ? <p className="nav-note">Add a folder on this device to see its sessions and start conversations there. <button type="button" className="btn btn-link small" onClick={() => void addLocalFolder()}>Add a folder…</button></p> : null}

      {targetHostRef ? <>
        <div className="nav-section-head"><span>Other places</span></div>
        <ul className="nav-places">
          <PlaceItem label={`A folder by path on ${targetName}`} grants={<span>{fullAccess ? "Type its path · add it above for sessions" : "Type its path · no sessions"}</span>} selected={choice?.kind === "path"} onSelect={() => choose({ kind: "path" })}>
            {placeBody(true, null)}
          </PlaceItem>
          <PlaceItem label="A folder from this device" grants={<span>Sent after your review · no sessions</span>} selected={choice?.kind === "send"} onSelect={() => choose({ kind: "send" })}>
            {placeBody(true, null)}
          </PlaceItem>
        </ul>
      </> : null}
    </aside>

    <main className="agents-main">{main}</main>

    {addingFolder && targetHostRef ? <AddRemoteFolderDialog roomId={room.id} hostRef={targetHostRef} name={targetName} onClose={() => setAddingFolder(false)} onAdded={(scopes, added) => {
      shares.adopt(scopes, true);
      setAddingFolder(false);
      const chosen = added ?? scopes.find((scope) => !sharedList.some((known) => known.scopeRef === scope.scopeRef));
      if (chosen) choose({ kind: "shared", scopeRef: chosen.scopeRef });
    }} /> : null}
    {accessOpen && targetHostRef ? <Dialog title={`Access on ${targetName}`} description={`What this device may do on ${targetName}, authority by authority, and where each is decided. Nothing here can be granted from this device.`} onClose={() => setAccessOpen(false)}>
      <RemoteAccessSummary room={room} hostRef={targetHostRef} name={targetName} connected={connected} paired={(room.pairing_state ?? "pairing_required") === "paired"} sharing={shares.knowledge} waitedForAccept={waitedForAccept[targetHostRef] ?? null} onOpenDetails={onOpenDetails ? () => { setAccessOpen(false); onOpenDetails(); } : undefined} />
    </Dialog> : null}
  </div></div>;
}

/** One workspace in the navigator: its name and what it allows; selected,
 * it opens to show its conversations and sessions. */
function PlaceItem({ label, grants, title, selected, onSelect, children }: { label: string; grants: ReactNode; title?: string; selected: boolean; onSelect: () => void; children: ReactNode }) {
  return <li className={`nav-place${selected ? " selected" : ""}`}>
    <button type="button" className="nav-place-head" aria-expanded={selected} title={title} onClick={onSelect}>
      <Icon name="folder" small />
      <span className="nav-place-text"><span className="nav-place-name">{label}</span><span className="nav-place-grants">{grants}</span></span>
    </button>
    {selected ? <div className="nav-place-body">{children}</div> : null}
  </li>;
}

/** Under the Full Access another device gave this one: names one of its
 * folders by full path. That device checks the folder against its own
 * rules, lists it for its user, and answers with its workspaces. */
function AddRemoteFolderDialog({ roomId, hostRef, name, onClose, onAdded }: {
  roomId: string;
  hostRef: string;
  name: string;
  onClose: () => void;
  onAdded: (scopes: SharedWorkspace[], added: SharedWorkspace | null) => void;
}) {
  const [folder, setFolder] = useState("");
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  async function add() {
    setPending(true); setProblem(null);
    try {
      const response = await openRemoteFolder(roomId, hostRef, folder.trim());
      if (response.outcome === "ok") {
        const leaf = folder.trim().split(/[\\/]/).filter(Boolean).pop() ?? "";
        onAdded(response.scopes, response.scopes.find((scope) => scope.fullAccess && scope.label === leaf) ?? null);
        return;
      }
      setProblem(response.outcome === "folder_refused"
        ? response.detail ?? `${name} would not open that folder.`
        : response.outcome === "busy" ? `${name} is still answering an earlier request. Try again in a moment.` : `${name} could not open it now.`);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally { setPending(false); }
  }
  return <Dialog title={`Add a folder on ${name}`} labelledBy="add-remote-folder-title" onClose={() => { if (!pending) onClose(); }} footer={<><button type="button" className="btn" disabled={pending} onClick={onClose}>Cancel</button><button type="button" className="btn btn-primary" disabled={pending || !folder.trim()} onClick={() => void add()}>{pending ? "Adding…" : "Add"}</button></>}>
    <p className="small">Type the folder's full path on {name}, for example <span className="mono">{"C:\\Users\\you\\Projects\\app"}</span> or <span className="mono">/Users/you/Projects/app</span>. You can then see its sessions, continue them and run Codex there.</p>
    <input className="input input-mono" autoFocus value={folder} disabled={pending} onChange={(event) => setFolder(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter" && folder.trim()) void add(); }} aria-label={`Folder path on ${name}`} placeholder="Full path on that device" />
    <p className="xsmall subtle">{name} lists the folder for its user, who can remove it. Folders too broad to share (a drive root, your home folder, app data) are refused there.</p>
    {problem ? <p className="error-text" role="alert">{problem}</p> : null}
  </Dialog>;
}

/** What a workspace allows this device, in words. Each is the Host's own
 * switch for this Bridge; continuing a session needs both. Under Full
 * Access everything is allowed. */
function Grants({ share }: { share: SharedWorkspace }) {
  if (share.fullAccess) return <span className="grant on">Full Access</span>;
  return <>
    <span className={share.readSessions ? "grant on" : "grant"}>{share.readSessions ? "Sessions" : "No sessions"}</span>
    <span className={share.agentRequests ? "grant on" : "grant"}>{share.agentRequests ? "Agents" : "No agents"}</span>
  </>;
}

/** Inside the selected workspace: start something new, return to the
 * conversation this device began there, or open one of its sessions. */
function PlaceBody({ canRun, runBlocked, agent, ownsCurrent, shownView, onNew, onCurrent, list, target, openKey, liveKey, onOpenSession }: {
  canRun: boolean;
  runBlocked: string | null;
  agent: NativeAgentController;
  ownsCurrent: boolean;
  shownView: AgentView | null;
  onNew: () => void;
  onCurrent: () => void;
  list: SessionList;
  target: string;
  openKey: string | null;
  liveKey: string | null;
  onOpenSession: (session: SessionRef) => void;
}) {
  const currentTitle = agent.pastTurns[0]?.prompt ?? agent.prompt ?? "Conversation";
  const active = !!agent.status && ["queued", "running"].includes(agent.status.state);
  return <>
    <button type="button" className={`nav-new${shownView === null || shownView.kind === "new" ? " selected" : ""}`} disabled={!canRun} title={canRun ? undefined : runBlocked ?? undefined} onClick={onNew}><Icon name="plus" small />New conversation</button>
    {!canRun && runBlocked ? <p className="nav-note">{runBlocked}</p> : null}
    {ownsCurrent ? <button type="button" className={`nav-session${shownView?.kind === "current" ? " selected" : ""}`} onClick={onCurrent}>
      <span className="nav-session-title">{active ? <Dot tone="live" pulse /> : null}{currentTitle}</span>
      <span className="nav-session-meta">Codex · your new conversation, from this device</span>
    </button> : null}
    <SessionListView list={list} target={target} openKey={openKey} liveKey={liveKey} onOpen={onOpenSession} />
  </>;
}

const ACCESS_TONE: Record<AccessLine["state"], "ok" | "warn" | "neutral" | "live"> = { ok: "ok", needs_action: "warn", decided_there: "live", unknown: "neutral" };
const ACCESS_LABEL: Record<AccessLine["state"], string> = { ok: "Ready", needs_action: "Needs action", decided_there: "Decided there", unknown: "Not known yet" };

/** Per authority: what is known here, what the other device decides, and
 * the exact steps there. No single "trust" switch. */
function RemoteAccessSummary({ room, hostRef, name, connected, paired, sharing, waitedForAccept, onOpenDetails }: {
  room: RoomInfo;
  hostRef: string;
  name: string;
  connected: boolean;
  paired: boolean;
  sharing: PeerShareKnowledge;
  waitedForAccept: boolean | null;
  onOpenDetails?: () => void;
}) {
  const [me, setMe] = useState<{ name: string; fingerprint: string | null }>({ name: "this device", fingerprint: null });
  const [shown, setShown] = useState<{ text: string; ok: boolean } | null>(null);
  useEffect(() => {
    let cancelled = false;
    void Promise.all([getDeviceProfile().catch(() => null), getDeviceIdentity().catch(() => null)]).then(([profile, identity]) => {
      if (!cancelled) setMe({ name: profile?.device_name || "this device", fingerprint: identity?.displayFingerprint ?? null });
    });
    return () => { cancelled = true; };
  }, []);
  const lines = remoteAccessGuidance({ remoteName: name, thisDeviceName: me.name, thisFingerprint: me.fingerprint, paired, connected, lastRunWaitedForAccept: waitedForAccept, sharing });

  async function showDevice() {
    setShown(null);
    try { await presentDeviceIdentity(room.id, hostRef); setShown({ text: `Shown. ${name} can now trust ${me.name} in this Bridge's Workspaces there.`, ok: true }); }
    catch (error) { setShown({ text: error instanceof Error ? error.message : `Pastey could not reach ${name}.`, ok: false }); }
  }

  return <div className="stack access-summary">
    <ul className="access-lines">
      {lines.map((line) => <li key={line.id}>
        <div className="row"><Dot tone={ACCESS_TONE[line.state] === "live" ? "live" : ACCESS_TONE[line.state]} /><strong className="spacer">{line.label}</strong><Badge tone={ACCESS_TONE[line.state]}>{ACCESS_LABEL[line.state]}</Badge></div>
        <p className="small muted">{line.detail}</p>
        {line.steps.length && line.state !== "ok" ? <details className="disclosure"><summary><Icon name="chevron" small />How to allow it</summary><ol className="small disclosure-body">{line.steps.map((step) => <li key={step}>{step}</li>)}</ol></details> : null}
      </li>)}
    </ul>
    <div className="row"><button type="button" className="btn btn-sm" disabled={!connected} onClick={() => void showDevice()}>Show this device to {name} again</button><span className="xsmall subtle spacer">{connected ? `Pastey does this when the Bridge opens, so ${name}'s user can trust this device. It grants nothing.` : `${name} is not connected right now.`}</span></div>
    {onOpenDetails ? <button type="button" className="btn btn-link small" onClick={onOpenDetails}>This device's own shares: Bridge details → Workspaces</button> : null}
    {shown ? <Notice tone={shown.ok ? "ok" : "bad"}>{shown.text}</Notice> : null}
  </div>;
}

function localAvailability(codex: NativeAgentCapability | undefined): NativeAgentTargetAvailability {
  return { state: codex?.state ?? "unavailable", reason: codex?.reason ?? null, detail: codex?.detail ?? null, backend: codex?.backend ?? null };
}

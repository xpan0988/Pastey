import { useCallback, useEffect, useMemo, useState } from "react";
import { remoteAccessGuidance, type AccessLine } from "../../lib/accessGuidance";
import { shareSummary, type PeerShareKnowledge } from "../../lib/workspaceShares";
import type { BridgePeerSession } from "../../lib/bridgePeers";
import { getDeviceIdentity, getDeviceProfile, presentDeviceIdentity } from "../../lib/tauri";
import type { RoomInfo } from "../../lib/types";
import { continuedSessionKey } from "./AgentTaskLifecycle";
import { AgentTaskTracker } from "./AgentTaskTracker";
import { NativeAgentPanel, type NativeAgentController } from "./NativeAgentPanel";
import { NativeSessionsCard, type OpenSessionRequest } from "./NativeSessionsCard";
import { usePeerSharedWorkspaces } from "./BridgeWorkspaces";
import { Badge, Dot, Icon, Notice, Segmented } from "./ui";

type AgentMode = "new" | "sessions";
const MODE_KEY = "pastey:agents-mode-v1";

function rememberedMode(roomId: string): AgentMode {
  try { return (JSON.parse(window.localStorage.getItem(MODE_KEY) ?? "{}") as Record<string, AgentMode>)[roomId] === "sessions" ? "sessions" : "new"; }
  catch { return "new"; }
}

function rememberMode(roomId: string, mode: AgentMode) {
  try { window.localStorage.setItem(MODE_KEY, JSON.stringify({ ...(JSON.parse(window.localStorage.getItem(MODE_KEY) ?? "{}") as Record<string, AgentMode>), [roomId]: mode })); }
  catch { /* a convenience only */ }
}

/** Agent work on one device of this Bridge. The page answers, top down:
 * which device the agents work on (and what it allows), the one task this
 * Bridge follows and where it is, then one of two things to do there:
 * start a new task, or continue a session that already exists. Running and
 * reading stay separate authorities, each decided by the device that holds
 * them; both use the folders it shares with this Bridge, by name. */
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
  const [mode, setModeState] = useState<AgentMode>(() => rememberedMode(room.id));
  const [waitedForAccept, setWaitedForAccept] = useState<Record<string, boolean>>({});
  const [openSession, setOpenSession] = useState<string | null>(null);
  const [openRequest, setOpenRequest] = useState<OpenSessionRequest | null>(null);
  const options = useMemo(() => members.filter((peer) => peer.hostRef), [members]);
  const targetHostRef = target === "local" ? null : target;
  const targetName = targetHostRef ? options.find((peer) => peer.hostRef === targetHostRef)?.displayName ?? "the other device" : "this device";
  const connected = targetHostRef ? peers.some((peer) => peer.hostRef === targetHostRef) : true;
  const shares = usePeerSharedWorkspaces(room.id, targetHostRef, connected);

  const setMode = useCallback((next: AgentMode) => { setModeState(next); rememberMode(room.id, next); }, [room.id]);

  // A run that waits in Review tells this device that the other one has no
  // unattended permission for it there. Nothing else is inferred.
  useEffect(() => {
    if (agent.taskHostRef && agent.status?.state === "queued" && agent.status.code === "native_agent_review_required") {
      setWaitedForAccept((current) => current[agent.taskHostRef!] ? current : { ...current, [agent.taskHostRef!]: true });
    }
  }, [agent.status, agent.taskHostRef]);

  // The followed task is shown once: inside the session it continues while
  // that session is open on its own device, otherwise here at the top.
  const continuedKey = agent.continues ? continuedSessionKey(agent.continues) : null;
  const taskOnTarget = (agent.taskHostRef ?? "local") === target;
  const trackerInSession = mode === "sessions" && taskOnTarget && !!continuedKey && openSession === continuedKey;
  const taskEnded = !!agent.status && !["queued", "running"].includes(agent.status.state);
  const openContinuedSession = agent.continues ? () => {
    setTarget(agent.taskHostRef ?? "local");
    setMode("sessions");
    setOpenRequest({ provider: agent.continues!.provider, sessionId: agent.continues!.sessionId, nonce: Date.now() });
  } : undefined;

  return <div className="panel-inner">
    <section className="agents-head" aria-label="Device">
      <div className="agents-device">
        <label className="field-label" htmlFor="agent-device">Agents on</label>
        <select id="agent-device" className="select" value={target} onChange={(event) => setTarget(event.target.value)}>
          <option value="local">This device</option>
          {options.map((peer) => <option key={peer.hostRef} value={peer.hostRef}>{peer.displayName}{peers.some((routeable) => routeable.hostRef === peer.hostRef) ? "" : " (not connected)"}</option>)}
        </select>
        {targetHostRef ? <span className="row xsmall subtle"><Dot tone={connected ? "ok" : "neutral"} />{connected ? "Connected" : "Not connected"}</span> : null}
        {targetHostRef ? <button type="button" className="btn btn-ghost btn-sm" onClick={() => onOpenDevice(targetHostRef)}><Icon name="device" small />Device details</button> : null}
      </div>
      <p className="small muted">{targetHostRef
        ? `You are on this device. Tasks and sessions below are on ${targetName}: it runs them and reads them, and it decides what this device may do there.`
        : "Tasks and sessions below are on this device, in folders you choose here."}</p>
      {targetHostRef ? <SharedWorkspacesLine name={targetName} knowledge={shares.knowledge} connected={connected} onOpenDetails={onOpenDetails} /> : null}
      {targetHostRef ? <RemoteAccessSummary room={room} hostRef={targetHostRef} name={targetName} connected={connected} paired={(room.pairing_state ?? "pairing_required") === "paired"} sharing={shares.knowledge} waitedForAccept={waitedForAccept[targetHostRef] ?? null} /> : null}
    </section>

    {trackerInSession ? null : <AgentTaskTracker roomId={room.id} agent={agent} peers={peers} members={members} onOpenSession={openContinuedSession} />}

    <div className="agents-modes">
      <Segmented<AgentMode> label="What to do" value={mode} onChange={setMode} options={[{ value: "new", label: "New task" }, { value: "sessions", label: "Continue a session" }]} />
      <span className="small muted">{mode === "new"
        ? <>Give Codex a task in a folder on {targetName}. <Badge tone="live">Execution</Badge></>
        : <>Pick up a Claude Code or Codex session already saved on {targetName}. Looking changes nothing. <Badge>Read, then continue</Badge></>}</span>
    </div>

    <section className="section" aria-label="Run a task" hidden={mode !== "new"}>
      <NativeAgentPanel roomId={room.id} agent={agent} peers={peers} members={members} target={target} shares={shares} onOpenDetails={onOpenDetails} />
    </section>

    <section className="section" aria-label="Existing sessions" hidden={mode !== "sessions"}>
      <NativeSessionsCard
        roomId={room.id}
        peers={[...peers]}
        members={[...members]}
        target={target}
        shares={shares}
        active={mode === "sessions"}
        onContinued={agent.adoptTask}
        openRequest={openRequest}
        onOpenChange={setOpenSession}
        continued={continuedKey && taskOnTarget ? { key: continuedKey, ended: taskEnded, node: <AgentTaskTracker roomId={room.id} agent={agent} peers={peers} members={members} inSession onLookAgain={openContinuedSession} /> } : null}
      />
    </section>
  </div>;
}

/** The folders the chosen device shares with this Bridge, at a glance. */
function SharedWorkspacesLine({ name, knowledge, connected, onOpenDetails }: { name: string; knowledge: PeerShareKnowledge; connected: boolean; onOpenDetails?: () => void }) {
  const text = !connected ? `${name} is not connected right now.`
    : knowledge.state === "unknown" ? `Asking ${name} which folders it shares…`
      : knowledge.workspaces.length ? knowledge.workspaces.map((share) => `${share.label} (${shareSummary(share).toLowerCase()})`).join(" · ")
        : `${name} shares no folder with this Bridge yet. On ${name}, open this Bridge and choose Share a workspace in Bridge details.`;
  return <div className="row"><Icon name="folder" small /><span className="small spacer"><strong>Shared by {name}:</strong> {text}</span>{onOpenDetails ? <button type="button" className="btn btn-link small" onClick={onOpenDetails}>Workspaces</button> : null}</div>;
}

const ACCESS_TONE: Record<AccessLine["state"], "ok" | "warn" | "neutral" | "live"> = { ok: "ok", needs_action: "warn", decided_there: "live", unknown: "neutral" };
const ACCESS_LABEL: Record<AccessLine["state"], string> = { ok: "Ready", needs_action: "Needs action", decided_there: "Decided there", unknown: "Not known yet" };

/** Per authority: what is known here, what the other device decides, and
 * the exact steps there. No single "trust" switch. */
function RemoteAccessSummary({ room, hostRef, name, connected, paired, sharing, waitedForAccept }: {
  room: RoomInfo;
  hostRef: string;
  name: string;
  connected: boolean;
  paired: boolean;
  sharing: PeerShareKnowledge;
  waitedForAccept: boolean | null;
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

  return <details className="disclosure access-summary" open={lines.some((line) => line.state === "needs_action") || undefined}>
    <summary><Icon name="chevron" small />Access on {name}{connected ? "" : " (not connected)"}</summary>
    <div className="disclosure-body stack">
      <ul className="access-lines">
        {lines.map((line) => <li key={line.id}>
          <div className="row"><Dot tone={ACCESS_TONE[line.state] === "live" ? "live" : ACCESS_TONE[line.state]} /><strong className="spacer">{line.label}</strong><Badge tone={ACCESS_TONE[line.state]}>{ACCESS_LABEL[line.state]}</Badge></div>
          <p className="small muted">{line.detail}</p>
          {line.steps.length && line.state !== "ok" ? <details className="disclosure"><summary><Icon name="chevron" small />How to allow it</summary><ol className="small disclosure-body">{line.steps.map((step) => <li key={step}>{step}</li>)}</ol></details> : null}
        </li>)}
      </ul>
      <div className="row"><button type="button" className="btn btn-sm" disabled={!connected} onClick={() => void showDevice()}>Show this device to {name} again</button><span className="xsmall subtle spacer">{connected ? `Pastey does this when the Bridge opens, so ${name}'s user can trust this device. It grants nothing.` : `${name} is not connected right now.`}</span></div>
      {shown ? <Notice tone={shown.ok ? "ok" : "bad"}>{shown.text}</Notice> : null}
    </div>
  </details>;
}

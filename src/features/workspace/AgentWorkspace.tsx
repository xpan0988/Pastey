import { useEffect, useMemo, useState } from "react";
import { remoteAccessGuidance, type AccessLine } from "../../lib/accessGuidance";
import { shareSummary, type PeerShareKnowledge } from "../../lib/workspaceShares";
import type { BridgePeerSession } from "../../lib/bridgePeers";
import { getDeviceIdentity, getDeviceProfile, presentDeviceIdentity } from "../../lib/tauri";
import type { RoomInfo } from "../../lib/types";
import { NativeAgentPanel, type NativeAgentController } from "./NativeAgentPanel";
import { NativeSessionsCard } from "./NativeSessionsCard";
import { usePeerSharedWorkspaces } from "./BridgeWorkspaces";
import { Badge, Dot, Icon, Notice } from "./ui";

/** Agent work on one device of this Bridge, in one place, with its two
 * authorities kept apart: running a task (Execution, which the executing
 * device admits) and reading sessions that already exist (Observation,
 * which the holding device allows separately). Both use the folders that
 * device shares with this Bridge, by name; it resolves its own paths. */
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
  const [waitedForAccept, setWaitedForAccept] = useState<Record<string, boolean>>({});
  const options = useMemo(() => members.filter((peer) => peer.hostRef), [members]);
  const targetHostRef = target === "local" ? null : target;
  const targetName = targetHostRef ? options.find((peer) => peer.hostRef === targetHostRef)?.displayName ?? "the other device" : "this device";
  const connected = targetHostRef ? peers.some((peer) => peer.hostRef === targetHostRef) : true;
  const shares = usePeerSharedWorkspaces(room.id, targetHostRef, connected);

  // A run that waits in Review tells this device that the other one has no
  // unattended permission for it there. Nothing else is inferred.
  useEffect(() => {
    if (agent.taskHostRef && agent.status?.state === "queued" && agent.status.code === "native_agent_review_required") {
      setWaitedForAccept((current) => current[agent.taskHostRef!] ? current : { ...current, [agent.taskHostRef!]: true });
    }
  }, [agent.status, agent.taskHostRef]);

  return <div className="panel-inner">
    <div className="panel-intro">
      <h2>Agents</h2>
      <p>Run Codex on a device of this Bridge, or look at the Claude Code and Codex sessions already on it. Running and reading are allowed separately, by the device that does them.</p>
    </div>

    <section className="card device-bar" aria-label="Device">
      <div className="card-body">
        <div className="row-wrap">
          <label className="field-label" htmlFor="agent-device">Device</label>
          <select id="agent-device" className="select" style={{ flex: "1 1 200px", width: "auto" }} value={target} onChange={(event) => setTarget(event.target.value)}>
            <option value="local">This device</option>
            {options.map((peer) => <option key={peer.hostRef} value={peer.hostRef}>{peer.displayName}{peers.some((routeable) => routeable.hostRef === peer.hostRef) ? "" : " (not connected)"}</option>)}
          </select>
          {targetHostRef ? <button type="button" className="btn btn-sm" onClick={() => onOpenDevice(targetHostRef)}><Icon name="device" small />Device details</button> : null}
        </div>
        {targetHostRef ? <SharedWorkspacesLine name={targetName} knowledge={shares.knowledge} connected={connected} onOpenDetails={onOpenDetails} /> : null}
        {targetHostRef ? <RemoteAccessSummary room={room} hostRef={targetHostRef} name={targetName} connected={connected} paired={(room.pairing_state ?? "pairing_required") === "paired"} sharing={shares.knowledge} waitedForAccept={waitedForAccept[targetHostRef] ?? null} /> : null}
      </div>
    </section>

    <section className="section" aria-label="Run a task">
      <div className="section-head">
        <h2 className="row">Run a task <Badge tone="live">Execution</Badge></h2>
        <p>Pastey starts Codex on {targetName} and follows this run: its approvals, its result, and any folder it moves. A run continues your earlier Pastey conversation in that folder unless you start a new one.</p>
      </div>
      <NativeAgentPanel roomId={room.id} agent={agent} peers={peers} members={members} target={target} shares={shares} onOpenDetails={onOpenDetails} />
    </section>

    <section className="section" aria-label="Existing sessions">
      <div className="section-head">
        <h2 className="row">Sessions on {targetName} <Badge>Look or continue</Badge></h2>
        <p>Sessions Claude Code and Codex already keep there, including ones Pastey started (marked). Looking never changes a session. You can continue an idle session in the session itself (Codex threads on another device after its user accepts); the run then appears under Run a task.</p>
      </div>
      <NativeSessionsCard roomId={room.id} peers={[...peers]} members={[...members]} target={target} shares={shares} onContinued={agent.adoptTask} />
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

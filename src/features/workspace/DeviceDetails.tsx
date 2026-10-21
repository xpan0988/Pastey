import { Fragment, useEffect, useRef, useState } from "react";
import { benchmarkFacts, benchmarkScope, type FactRow } from "../../lib/deviceDiagnostics";
import {
  getBridgeNodeListProjection,
  getBridgePeerAddress,
  getDeviceIdentity,
  getLastBenchmarkResults,
  getRoomControlDiagnostics,
  listKnownDevices,
  listRemoteNativeAgentCapabilities,
  listSeenPeerDevices,
  runBridgeDeviceDiagnostics,
  runBridgeDeviceSelfCheck,
  trustPeerDevice,
  revokeTrustedDevice,
  type NativeAgentCapability,
  type RoomControlDiagnostics,
  type SeenPeerDevice,
} from "../../lib/tauri";
import { controlClassLabel, controlHealth, controlReasonLabel, refusalLine } from "../../lib/controlDiagnostics";
import type { BridgeDeviceDiagnostics, BridgeNodeListProjectionV1, DiagnosticState, KnownDevice, LinkBenchmarkResult, RoomInfo } from "../../lib/types";
import { codexUnavailableCopy } from "./NativeAgentPanel";
import { Badge, Dialog, Dot, Icon, Notice, Tabs, type Tone } from "./ui";
import { bridgeCode, roomMembers } from "./workspaceViewModel";

type Section = "overview" | "connection" | "performance" | "agents" | "trust" | "advanced";

/** One device of a Bridge, in one place: who it is, how it is connected,
 * what was measured, what it can run, and what each side allows. Every
 * check runs only when asked, against this exact device's current session.
 * This device's own global facts live in Settings → This device. */
export function DeviceDetails({ room, hostRef, onClose, onOpenSettings }: {
  room: RoomInfo;
  hostRef: string;
  onClose: () => void;
  onOpenSettings: (route: "settings-permissions" | "settings-devices" | "settings-this-device") => void;
}) {
  const [section, setSection] = useState<Section>("overview");
  const member = roomMembers(room).find((peer) => peer.hostRef === hostRef) ?? null;
  const name = member?.displayName ?? "Bridge device";
  const [nodeList, setNodeList] = useState<BridgeNodeListProjectionV1 | null>(null);
  const [seen, setSeen] = useState<SeenPeerDevice | null>(null);
  const [known, setKnown] = useState<KnownDevice[]>([]);
  const [address, setAddress] = useState<string | null>(null);
  const [myFingerprint, setMyFingerprint] = useState<string | null>(null);

  async function refreshTrust() {
    const [devices, knownDevices] = await Promise.all([listSeenPeerDevices().catch(() => []), listKnownDevices().catch(() => [])]);
    setSeen(devices.find((device) => device.bridgeId === room.id) ?? null);
    setKnown(knownDevices);
  }

  useEffect(() => {
    let cancelled = false;
    void getBridgeNodeListProjection(room.id).then((projection) => { if (!cancelled) setNodeList(projection); }).catch(() => undefined);
    void getBridgePeerAddress(room.id).then((value) => { if (!cancelled) setAddress(value); }).catch(() => undefined);
    void getDeviceIdentity().then((identity) => { if (!cancelled) setMyFingerprint(identity.displayFingerprint); }).catch(() => undefined);
    void refreshTrust();
    return () => { cancelled = true; };
  }, [room.id, hostRef]); // eslint-disable-line react-hooks/exhaustive-deps

  const node = nodeList?.nodes.find((candidate) => candidate.hostRef === hostRef) ?? null;
  const link = nodeList?.links.find((candidate) => candidate.targetHostRef === hostRef) ?? null;
  const liveness = node?.currentSession?.liveness ?? member?.liveness ?? "disconnected";
  const connected = liveness === "connected";

  return <Dialog wide title={name} description={<>Bridge <span className="mono">{bridgeCode(room)}</span> · {connected ? "Connected now" : liveness}</>} onClose={onClose}>
    <Tabs label="Device details" value={section} onChange={setSection} tabs={[
      { value: "overview", label: "Overview" },
      { value: "connection", label: "Connection" },
      { value: "performance", label: "Performance" },
      { value: "agents", label: "Agents" },
      { value: "trust", label: "Trust & permissions" },
      { value: "advanced", label: "Advanced" },
    ]} />
    {section === "overview" ? <Facts rows={[
      ["Name", name],
      ["Connection", connected ? "Connected now" : String(liveness)],
      ["Pairing", room.pairing_state === "paired" ? "Paired with this Bridge" : room.pairing_state === "awaiting_peer" ? "Waiting for the other device" : "Pairing required"],
      ["Identity", seen ? seen.displayFingerprint : "Not shown in this session"],
      ["Trusted here", seen ? (seen.trusted ? "Yes" : "No") : "Unknown until it shows its identity"],
      ["Paired before", seen ? (known.some((device) => device.fingerprint === seen.fingerprint) ? "Yes · connects without a code nearby" : "No") : "Unknown"],
    ]} /> : null}
    {section === "connection" ? <ConnectionSection connected={connected} liveness={String(liveness)} observedAt={node?.currentSession?.observedAt ?? null} link={link} address={address} /> : null}
    {section === "performance" ? <PerformanceSection room={room} hostRef={hostRef} name={name} connected={connected} linkBenchmark={link?.benchmark ?? null} /> : null}
    {section === "agents" ? <AgentsSection room={room} hostRef={hostRef} name={name} connected={connected} capabilities={node?.capabilities ?? []} /> : null}
    {section === "trust" ? <TrustSection room={room} name={name} seen={seen} myFingerprint={myFingerprint} onChanged={refreshTrust} onOpenSettings={onOpenSettings} /> : null}
    {section === "advanced" ? <AdvancedSection room={room} hostRef={hostRef} name={name} connected={connected} /> : null}
  </Dialog>;
}

function Facts({ rows }: { rows: FactRow[] }) {
  return <dl className="facts">{rows.map(([label, value]) => <Fragment key={label}><dt>{label}</dt><dd>{value}</dd></Fragment>)}</dl>;
}

function ConnectionSection({ connected, liveness, observedAt, link, address }: { connected: boolean; liveness: string; observedAt: number | null; link: BridgeNodeListProjectionV1["links"][number] | null; address: string | null }) {
  return <div className="stack-lg">
    <Facts rows={[
      ["Session", connected ? "Current, verified" : liveness],
      ["Last observed", observedAt ? new Date(observedAt * 1000).toLocaleString() : "Not observed"],
      ["Control channel", link ? diagnosticStateLabel(link.connection.controlChannel) : "Not observed"],
      ["Data path", link ? diagnosticStateLabel(link.connection.dataPath) : "Not observed"],
      ["Private-network address", address ?? "None · this network only"],
    ]} />
    <p className="xsmall subtle">Observed from this device's current session; nothing here is a measurement. Pair, re-pair or set the address in Bridge details.</p>
  </div>;
}

/** Real measurements only, each with what it exercised and when. */
function PerformanceSection({ room, hostRef, name, connected, linkBenchmark }: { room: RoomInfo; hostRef: string; name: string; connected: boolean; linkBenchmark: LinkBenchmarkResult | null }) {
  const [last, setLast] = useState<LinkBenchmarkResult | null>(linkBenchmark);
  const [result, setResult] = useState<BridgeDeviceDiagnostics | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);

  useEffect(() => {
    if (linkBenchmark) return;
    void getLastBenchmarkResults().then((results) => setLast(results.find((candidate) => candidate.peer_id === room.id) ?? null)).catch(() => undefined);
  }, [linkBenchmark, room.id]);

  async function testConnection() {
    if (inFlight.current) return;
    inFlight.current = true;
    setRunning(true); setError(null);
    try {
      const diagnostics = await runBridgeDeviceDiagnostics(room.id, hostRef);
      setResult(diagnostics);
      if (diagnostics.linkBenchmark) setLast(diagnostics.linkBenchmark);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Pastey could not test this connection.");
    } finally { inFlight.current = false; setRunning(false); }
  }

  const shown = last ? benchmarkFacts(last) : null;
  return <div className="stack-lg">
    <div className="row-wrap">
      <button type="button" className="btn btn-primary" disabled={running || !connected} onClick={() => void testConnection()}><Icon name="refresh" small />{running ? "Testing…" : "Test connection"}</button>
      <span className="xsmall subtle spacer">About one second. Sends test data to {name} in memory; no files are read or written.</span>
    </div>
    {!connected ? <Notice tone="warn">{name} is not connected, so nothing can be measured now.</Notice> : null}
    {error ? <Notice tone="bad">{error}</Notice> : null}
    {shown && last ? <section className="card"><div className="card-body">
      <div className="row"><strong className="spacer" style={{ fontSize: "var(--text-lg)" }}>{shown.headline}</strong><Badge tone={last.link_quality === "Excellent" || last.link_quality === "Good" ? "ok" : "warn"}>{last.link_quality}</Badge></div>
      <p className="xsmall subtle">{benchmarkScope(last.benchmark_mode, true)}. Real file transfers also depend on both disks.</p>
      <Facts rows={shown.facts} />
    </div></section> : <p className="small muted">No measurement for this Bridge yet. Pastey does not estimate speed; test the connection to measure it.</p>}
    {result ? <DiagnosticGroup title="Connection checks" facts={[
      ["Identity", result.connection.identity],
      ["Secure session", result.connection.secureSession],
      ["Control channel", result.connection.controlChannel],
      ["Data path", result.connection.dataPath],
    ]} /> : null}
  </div>;
}

function AgentsSection({ room, hostRef, name, connected, capabilities }: { room: RoomInfo; hostRef: string; name: string; connected: boolean; capabilities: BridgeNodeListProjectionV1["nodes"][number]["capabilities"] }) {
  const [native, setNative] = useState<NativeAgentCapability[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  async function check() {
    setError(null); setNative(null);
    try { setNative(await listRemoteNativeAgentCapabilities(room.id, hostRef)); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : "No answer."); setNative([]); }
  }
  useEffect(() => { if (connected) void check(); }, [connected]); // eslint-disable-line react-hooks/exhaustive-deps
  const codex = native?.find((capability) => capability.agentId === "agent.coding.codex");
  return <div className="stack-lg">
    <section className="card"><div className="list">
      <div className="list-row"><span className="list-row-main"><span className="list-row-title">Codex</span><span className="list-row-meta">{!connected ? `${name} is not connected.` : native === null ? "Checking…" : error ? `Could not check: ${error}` : codex?.state === "available" ? `Available on ${name}.` : codexUnavailableCopy(codex?.reason ?? (codex?.state === "incompatible" ? "native_agent_incompatible" : null), name)}</span></span>
        <span className="list-row-side"><Badge tone={codex?.state === "available" ? "ok" : native === null ? "neutral" : "warn"}>{codex?.state === "available" ? "Available" : native === null ? "…" : "Unavailable"}</Badge><button type="button" className="btn btn-sm" disabled={!connected || native === null} onClick={() => void check()}>Check again</button></span></div>
    </div></section>
    {capabilities.length ? <section className="stack"><span className="subhead">Reported capabilities</span><div className="card"><div className="list">
      {capabilities.map((capability) => <div className="list-row" key={capability.capabilityId}><span className="list-row-main"><span className="list-row-title mono">{capability.capabilityId}</span>{capability.unavailableReason ? <span className="list-row-meta">{capability.unavailableReason}</span> : null}</span><span className="list-row-side"><Badge tone={capability.available ? "ok" : "neutral"}>{capability.available ? "Available" : "Unavailable"}</Badge></span></div>)}
    </div></div></section> : null}
    <p className="xsmall subtle">What {name} reports about itself in this session. Running anything there still needs {name}'s Accept or its own unattended permission.</p>
  </div>;
}

/** Both directions, each decided on its own device. */
function TrustSection({ room, name, seen, myFingerprint, onChanged, onOpenSettings }: {
  room: RoomInfo;
  name: string;
  seen: SeenPeerDevice | null;
  myFingerprint: string | null;
  onChanged: () => Promise<void>;
  onOpenSettings: (route: "settings-permissions" | "settings-devices" | "settings-this-device") => void;
}) {
  const [confirm, setConfirm] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function act(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await onChanged(); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }
  return <div className="stack-lg">
    <section className="stack">
      <span className="subhead">On this device · you decide here</span>
      <div className="card"><div className="card-body">
        {seen ? <>
          <div className="row"><span className="spacer"><strong>{seen.trusted ? `${name} is trusted here` : `${name} is not trusted here`}</strong><br /><span className="xsmall subtle mono">{seen.displayFingerprint}</span></span>
            {seen.trusted ? <button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void act(() => revokeTrustedDevice(seen.fingerprint))}>Revoke trust</button> : <button type="button" className="btn btn-sm" disabled={pending} onClick={() => setConfirm(true)}>Trust…</button>}</div>
          <p className="xsmall subtle">Trust lets you share folders with {name} (Bridge details → Workspaces) and give it other permissions here. It allows nothing by itself.</p>
        </> : <p className="small muted">{name} has not shown its identity in this session, so it cannot be trusted yet. It does as soon as this Bridge is open on {name}.</p>}
        <div><button type="button" className="btn btn-link small" onClick={() => onOpenSettings("settings-permissions")}>What {name} may do here: Settings → Permissions</button></div>
      </div></div>
    </section>
    <section className="stack">
      <span className="subhead">On {name} · its user decides there</span>
      <div className="card"><div className="card-body">
        <p className="small">Running Codex on {name} waits for someone there to click Accept, unless {name}'s user lets this device run in a folder without asking. Picking {name}'s folders by name, and reading its sessions, need {name} to share those folders with this Bridge, from this Bridge's Workspaces on {name}. None of this can be granted from here.</p>
        <p className="small muted">This device's fingerprint, to compare on {name}: <span className="mono">{myFingerprint ?? "…"}</span></p>
      </div></div>
    </section>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
    {confirm && seen ? <Dialog title={`Trust ${seen.peerName}?`} labelledBy="trust-device-title" onClose={() => setConfirm(false)} footer={<><button type="button" className="btn" onClick={() => setConfirm(false)}>Cancel</button><button type="button" className="btn btn-primary" disabled={pending} onClick={() => { setConfirm(false); void act(() => trustPeerDevice(seen.fingerprint)); }}>It matches. Trust</button></>}>
      <p>{`Only continue if ${seen.peerName} shows exactly this identity under Settings → This device:`}</p>
      <p className="mono fingerprint-callout">{seen.displayFingerprint}</p>
      <p className="small muted">Trusting it lets you give it limited permissions. It does not let it run anything by itself.</p>
    </Dialog> : null}
  </div>;
}

/** The heavier, explicit managed self-check. */
function AdvancedSection({ room, hostRef, name, connected }: { room: RoomInfo; hostRef: string; name: string; connected: boolean }) {
  const [check, setCheck] = useState<{ result?: BridgeDeviceDiagnostics; unavailable?: boolean } | null>(null);
  const inFlight = useRef(false);
  async function runCheck() {
    if (inFlight.current) return;
    inFlight.current = true;
    setCheck({});
    try { setCheck({ result: await runBridgeDeviceSelfCheck(room.id, hostRef) }); }
    catch { setCheck({ unavailable: true }); }
    finally { inFlight.current = false; }
  }
  const loading = check !== null && !check.result && !check.unavailable;
  return <div className="stack-lg">
    <div className="row-wrap"><button type="button" className="btn" disabled={loading || !connected} onClick={() => void runCheck()}>{loading ? "Checking…" : check ? "Run again" : "Run managed self-check"}</button><span className="xsmall subtle spacer">Tests the connection, then runs a small managed Plan end to end on {name}. Use it to diagnose Plans.</span></div>
    {check?.unavailable ? <Notice tone="warn">Diagnostics for {name} are temporarily unavailable.</Notice> : null}
    <ControlChannelSection room={room} name={name} />
    {check?.result ? <>
      <DiagnosticGroup title="Managed readiness" facts={[
        ["Provider", check.result.managedReadiness.provider],
        ["Runtime", check.result.managedReadiness.runtime],
        ["Execution world", check.result.managedReadiness.executionWorld],
        ["Managed execution", check.result.managedReadiness.managedExecution],
      ]} />
      {check.result.managedE2e ? <div className="stack"><span className="subhead">Managed E2E self-check</span><dl className="facts"><dt>Result</dt><dd><Badge tone={check.result.managedE2e.outcome === "PASS" ? "ok" : check.result.managedE2e.outcome === "BLOCKED" ? "neutral" : "bad"}>{check.result.managedE2e.outcome}</Badge></dd>{check.result.managedE2e.coreTerminalState ? <><dt>Core terminal state</dt><dd>{check.result.managedE2e.coreTerminalState}</dd></> : null}{check.result.managedE2e.failureCode ? <><dt>Reason</dt><dd>{check.result.managedE2e.failureCode}</dd></> : null}</dl></div> : null}
    </> : null}
  </div>;
}

/** This Bridge's Room Control as this device sees it: what it accepted
 * and refused by kind, any backpressure now, what it could not deliver,
 * and the requests waiting for Accept here. Counts only; read on request. */
function ControlChannelSection({ room, name }: { room: RoomInfo; name: string }) {
  const [diagnostics, setDiagnostics] = useState<RoomControlDiagnostics | null>(null);
  const [error, setError] = useState<string | null>(null);
  async function read() {
    setError(null);
    try { setDiagnostics(await getRoomControlDiagnostics(room.id)); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
  }
  const now = diagnostics?.observedAt ?? Date.now() / 1000;
  const health = diagnostics ? controlHealth(diagnostics) : null;
  const refusals = diagnostics ? [["Last refused here", refusalLine(diagnostics.lastRefusal, now)], [`Last not delivered to ${name}`, refusalLine(diagnostics.lastOutboundFailure, now)]] as const : [];
  return <section className="stack" aria-label="Control channel">
    <div className="row-wrap"><span className="subhead spacer">Control channel</span><button type="button" className="btn btn-sm" onClick={() => void read()}>{diagnostics ? "Read again" : "Check control channel"}</button></div>
    <p className="xsmall subtle">How this device handles this Bridge's control messages (checks, agent requests and statuses, session queries): what it accepted and refused, and why. Use it when a check or a request to {name} fails.</p>
    {error ? <Notice tone="bad">{error}</Notice> : null}
    {diagnostics && health ? <>
      <Notice tone={health.tone === "ok" ? "ok" : "warn"}>{health.summary}</Notice>
      <dl className="facts">
        {diagnostics.classes.filter((entry) => entry.accepted || entry.refused.length).map((entry) => <Fragment key={entry.class}>
          <dt>{controlClassLabel(entry.class)}</dt>
          <dd>{entry.accepted} accepted · {entry.lastMinute} of {entry.perMinute} in the last minute{entry.refused.length ? ` · ${entry.refused.map(([reason, count]) => `${count} ${controlReasonLabel(reason)}`).join(", ")}` : ""}</dd>
        </Fragment>)}
        <dt>Waiting for Accept here</dt><dd>{diagnostics.nativeReviews.pending} of {diagnostics.nativeReviews.limit} · {diagnostics.nativeReviews.started} started, {diagnostics.nativeReviews.denied} denied, {diagnostics.nativeReviews.expired} expired, {diagnostics.nativeReviews.revoked} ended by revoked access</dd>
        <dt>Kept for display</dt><dd>{diagnostics.historyDepth} of {diagnostics.historyLimit} Bridge Plan events{diagnostics.historyDropped ? ` · ${diagnostics.historyDropped} older ones dropped` : ""}</dd>
        {diagnostics.outboundFailures.length ? <><dt>Not delivered to {name}</dt><dd>{diagnostics.outboundFailures.map((failure) => `${failure.count} × ${controlClassLabel(failure.class).toLowerCase()}: ${controlReasonLabel(failure.failure)}`).join("; ")}</dd></> : null}
        {refusals.map(([label, line]) => line ? <Fragment key={label}><dt>{label}</dt><dd>{line}</dd></Fragment> : null)}
      </dl>
    </> : null}
  </section>;
}

function DiagnosticGroup({ title, facts }: { title: string; facts: Array<[string, DiagnosticState]> }) {
  return <div className="stack"><span className="subhead">{title}</span><dl className="facts">{facts.map(([label, state]) => <Fragment key={label}><dt>{label}</dt><dd><Badge tone={diagnosticTone(state)}>{diagnosticStateLabel(state)}</Badge></dd></Fragment>)}</dl></div>;
}

function diagnosticTone(state: DiagnosticState): Tone {
  return state === "healthy" || state === "available" ? "ok" : state === "unavailable" ? "bad" : "neutral";
}

export function diagnosticStateLabel(state: DiagnosticState): string {
  if (state === "not_configured") return "Not configured";
  return state.charAt(0).toUpperCase() + state.slice(1);
}

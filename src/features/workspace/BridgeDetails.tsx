import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import {
  getBridgeNodeListProjection,
  listAuthorizedSearchFolders,
  listSessionReadGrants,
  listUnattendedGrants,
  revokeAuthorizedSearchFolder,
  revokeSessionReadGrant,
  revokeUnattendedGrant,
  runBridgeDeviceSelfCheck,
  type AuthorizedSearchFolder,
  type SessionReadGrant,
  type UnattendedGrant,
} from "../../lib/tauri";
import type { BridgeDeviceDiagnostics, BridgeNodeListProjectionV1, DiagnosticState, RoomInfo } from "../../lib/types";
import { expiryLabel } from "../../lib/unattendedAccess";
import { BridgeConnectionSection, ShowDeviceIdentity } from "./BridgeNetworkPanel";
import { Badge, Dialog, Dot, Icon, ListRow, type Tone } from "./ui";
import { bridgeCode, roomMembers } from "./workspaceViewModel";

/** Everything about the current Bridge that is not the work itself:
 * connection, members, what this device allows over it, and Burn. */
export function BridgeDetails({ room, onClose, onRefresh, onBurn, onOpenSettings }: {
  room: RoomInfo;
  onClose: () => void;
  onRefresh: () => Promise<void>;
  onBurn: (room: RoomInfo) => Promise<void>;
  onOpenSettings: () => void;
}) {
  const [confirmBurn, setConfirmBurn] = useState(false);
  const [burning, setBurning] = useState(false);
  return <aside className="inspector" aria-label="Bridge details">
    <header className="inspector-header">
      <h2>Bridge {bridgeCode(room)}</h2>
      <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label="Close details" onClick={onClose}><Icon name="close" /></button>
    </header>
    <div className="inspector-body">
      <BridgeConnectionSection room={room} onPairingChanged={onRefresh} />
      <BridgeDevices room={room} />
      <BridgePermissions room={room} onOpenSettings={onOpenSettings} />
      <ShowDeviceIdentity room={room} />
      <section className="inspector-section" aria-label="Burn">
        <h3>Burn this Bridge</h3>
        <p className="small muted">Ends the Bridge on this device for good. Files you saved stay where they are.</p>
        <div><button type="button" className="btn btn-danger btn-sm" onClick={() => setConfirmBurn(true)}><Icon name="flame" small />Burn Bridge…</button></div>
      </section>
    </div>
    {confirmBurn ? <Dialog
      title={`Burn Bridge ${bridgeCode(room)}?`}
      onClose={() => { if (!burning) setConfirmBurn(false); }}
      footer={<><button type="button" className="btn" disabled={burning} onClick={() => setConfirmBurn(false)}>Cancel</button><button type="button" className="btn btn-danger-solid" disabled={burning} onClick={() => { setBurning(true); void onBurn(room).finally(() => setBurning(false)); }}>{burning ? "Burning…" : "Burn Bridge"}</button></>}
    >
      <p>This permanently removes the Bridge content, membership, authority, and current-session state from this device. Explicitly saved user-owned files remain outside Bridge storage.</p>
      <p className="small muted">This cannot be undone. To work with this device again, pair a new Bridge.</p>
    </Dialog> : null}
  </aside>;
}

/** Members of this Bridge and an explicit, per-device check. Inspection
 * only: devices join a Bridge only by pairing. */
export function BridgeDevices({ room }: { room: RoomInfo }) {
  const peers = useMemo(() => roomMembers(room), [room]);
  const [nodeList, setNodeList] = useState<BridgeNodeListProjectionV1 | null>(null);
  const [checks, setChecks] = useState<Record<string, { result?: BridgeDeviceDiagnostics; unavailable?: boolean }>>({});
  const inFlight = useRef(new Set<string>());

  async function runDiagnostics(hostRef: string) {
    const bridgeId = room.id;
    const checkKey = deviceCheckKey(bridgeId, hostRef);
    if (inFlight.current.has(checkKey)) return;
    inFlight.current.add(checkKey);
    setChecks((current) => ({ ...current, [checkKey]: {} }));
    try {
      const result = await runBridgeDeviceSelfCheck(bridgeId, hostRef);
      setChecks((current) => ({ ...current, [checkKey]: { result } }));
    } catch {
      setChecks((current) => ({ ...current, [checkKey]: { unavailable: true } }));
    } finally {
      inFlight.current.delete(checkKey);
      setChecks((current) => ({ ...current }));
    }
  }

  useEffect(() => {
    let cancelled = false;
    void getBridgeNodeListProjection(room.id)
      .then((projection) => { if (!cancelled) setNodeList(projection); })
      .catch(() => { if (!cancelled) setNodeList(null); });
    return () => { cancelled = true; };
  }, [room.id]);

  function deviceRow(key: string, name: string, meta: string, state: string, hostRef: string | null) {
    const checkKey = hostRef ? deviceCheckKey(room.id, hostRef) : null;
    const check = checkKey ? checks[checkKey] : undefined;
    const loading = checkKey ? inFlight.current.has(checkKey) : false;
    return <div key={key}>
      <ListRow
        leading={<Dot tone={state === "connected" ? "ok" : state === "reconnecting" ? "warn" : "neutral"} />}
        title={name}
        meta={meta}
        side={hostRef ? <button type="button" className="btn btn-sm" aria-label={`Run diagnostics for ${name}`} disabled={loading} onClick={() => void runDiagnostics(hostRef)}>{loading ? "Checking…" : check ? "Retry" : "Check"}</button> : null}
      />
      {check ? <DeviceDiagnosticsResult check={check} /> : null}
    </div>;
  }

  return <section className="inspector-section" aria-label="Devices">
    <h3>Devices</h3>
    <div className="card"><div className="list">
      {nodeList ? nodeList.nodes.map((node) => {
        const isLocal = node.hostRef === nodeList.links[0]?.sourceHostRef || node.deviceProfile !== undefined;
        const state = node.currentSession?.liveness ?? "disconnected";
        return isLocal
          ? deviceRow(node.hostRef, "This device", "You", "connected", null)
          : deviceRow(node.hostRef, node.displayName ?? "Bridge device", state === "connected" ? "Connected now" : "Member · not connected", state, node.hostRef);
      }) : <>
        {deviceRow("local", "This device", "You", "connected", null)}
        {peers.map((peer) => deviceRow(peer.peerSessionId, peer.displayName, peer.liveness === "connected" ? "Connected now" : `Member · ${peer.liveness}`, peer.liveness, peer.hostRef ?? null))}
      </>}
    </div></div>
    {peers.length === 0 ? <p className="small muted">No other device has joined yet.</p> : null}
    <p className="xsmall subtle">Devices join a Bridge only by pairing. Check runs once, when you ask, against the device's current session.</p>
  </section>;
}

function deviceCheckKey(bridgeId: string, hostRef: string): string {
  return JSON.stringify([bridgeId, hostRef]);
}

function DeviceDiagnosticsResult({ check }: { check: { result?: BridgeDeviceDiagnostics; unavailable?: boolean } }) {
  if (check.unavailable) {
    return <p className="small muted" role="status" style={{ padding: "0 16px 12px" }}>Diagnostics for this device are temporarily unavailable.</p>;
  }
  if (!check.result) {
    return <p className="small muted" role="status" style={{ padding: "0 16px 12px" }}>Checking the device's current session…</p>;
  }
  const { connection, managedReadiness, linkBenchmark, managedE2e } = check.result;
  return <div className="stack" role="status" style={{ padding: "0 16px 14px" }}>
    <DiagnosticGroup title="Connection" facts={[
      ["Identity", connection.identity],
      ["Secure session", connection.secureSession],
      ["Control channel", connection.controlChannel],
      ["Data path", connection.dataPath],
    ]} />
    <DiagnosticGroup title="Managed readiness" facts={[
      ["Provider", managedReadiness.provider],
      ["Runtime", managedReadiness.runtime],
      ["Execution world", managedReadiness.executionWorld],
      ["Managed execution", managedReadiness.managedExecution],
    ]} />
    {linkBenchmark ? <small className="subtle">Pipeline baseline · {linkBenchmark.average_MBps.toFixed(1)} MB/s</small> : null}
    {managedE2e ? <div className="stack"><span className="subhead">Managed E2E self-check</span><dl className="facts"><dt>Result</dt><dd><Badge tone={managedE2e.outcome === "PASS" ? "ok" : managedE2e.outcome === "BLOCKED" ? "neutral" : "bad"}>{managedE2e.outcome}</Badge></dd>{managedE2e.coreTerminalState ? <><dt>Core terminal state</dt><dd>{managedE2e.coreTerminalState}</dd></> : null}{managedE2e.failureCode ? <><dt>Reason</dt><dd>{managedE2e.failureCode}</dd></> : null}</dl></div> : null}
  </div>;
}

function DiagnosticGroup({ title, facts }: { title: string; facts: Array<[string, DiagnosticState]> }) {
  return <div className="stack"><span className="subhead">{title}</span><dl className="facts">{facts.map(([label, state]) => <Fragment key={label}><dt>{label}</dt><dd><Badge tone={diagnosticTone(state)}>{diagnosticStateLabel(state)}</Badge></dd></Fragment>)}</dl></div>;
}

function diagnosticTone(state: DiagnosticState): Tone {
  return state === "healthy" || state === "available" ? "ok" : state === "unavailable" ? "bad" : "neutral";
}

function diagnosticStateLabel(state: DiagnosticState): string {
  if (state === "not_configured") return "Not configured";
  return state.charAt(0).toUpperCase() + state.slice(1);
}

/** What this device lets the other devices of this Bridge do here, with
 * Revoke beside each. Permissions are created only in Settings, by this
 * device's own user. */
function BridgePermissions({ room, onOpenSettings }: { room: RoomInfo; onOpenSettings: () => void }) {
  const [unattended, setUnattended] = useState<UnattendedGrant[]>([]);
  const [reading, setReading] = useState<SessionReadGrant[]>([]);
  const [folders, setFolders] = useState<AuthorizedSearchFolder[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const [nextUnattended, nextReading, nextFolders] = await Promise.all([
      listUnattendedGrants().catch(() => []),
      listSessionReadGrants().catch(() => []),
      listAuthorizedSearchFolders().catch(() => []),
    ]);
    setUnattended(nextUnattended.filter((grant) => grant.bridgeId === room.id && grant.state === "active"));
    setReading(nextReading.filter((grant) => grant.bridgeId === room.id && grant.state === "active"));
    setFolders(nextFolders.filter((folder) => folder.bridgeId === room.id));
  }
  useEffect(() => { void refresh(); }, [room.id]); // eslint-disable-line react-hooks/exhaustive-deps

  async function revoke(action: () => Promise<unknown>) {
    setPending(true); setError(null);
    try { await action(); await refresh(); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  const none = unattended.length === 0 && reading.length === 0 && folders.length === 0;
  return <section className="inspector-section" aria-label="Allowed on this device">
    <h3>Allowed on this device</h3>
    {none
      ? <p className="small muted">Nothing. Over this Bridge, other devices cannot search folders here, read sessions here, or start Codex here without your Accept.</p>
      : <div className="card"><div className="list">
        {unattended.map((grant) => <ListRow key={grant.grantId} title="Start Codex without asking" meta={`${grant.deviceLabel} · ${grant.workspace} · ${grant.runs} of ${grant.maxRuns} runs used · ${expiryLabel(grant.expiresAt)}`} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void revoke(() => revokeUnattendedGrant(grant.grantId))}>Revoke</button>} />)}
        {reading.map((grant) => <ListRow key={grant.grantId} title="Read Agent sessions" meta={`${grant.deviceLabel} · ${grant.workspace} · ${grant.shareExcerpts ? "statuses and excerpts" : "statuses only"} · ${expiryLabel(grant.expiresAt)}`} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void revoke(() => revokeSessionReadGrant(grant.grantId))}>Revoke</button>} />)}
        {folders.map((folder) => <ListRow key={folder.folderId} title={`Find files in ${folder.displayName}`} meta={folder.unavailable ? "Moved or replaced; not searched" : "Any device on this Bridge, by file name"} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void revoke(() => revokeAuthorizedSearchFolder(folder.folderId))}>Stop sharing</button>} />)}
      </div></div>}
    <div><button type="button" className="btn btn-link small" onClick={onOpenSettings}>Manage permissions in Settings</button></div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
  </section>;
}

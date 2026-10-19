import { Fragment, useEffect, useState } from "react";
import { benchmarkFacts, benchmarkScope, deviceProfileFacts, runtimeFacts, type FactRow } from "../../lib/deviceDiagnostics";
import { getDeviceCapabilities, getDeviceProfile, getLastBenchmarkResults, runLoopbackBenchmark } from "../../lib/tauri";
import type { BenchmarkMode, CapabilityProbeMode, DeviceCapabilities, DeviceProfile, LinkBenchmarkResult } from "../../lib/types";
import { DeviceIdentitySettings } from "./DeviceTrustSettings";
import { Badge, Icon, Notice, Segmented } from "./ui";

/** This device's own facts and checks: who it is, what it has, and what it
 * can do on its own. Connections to other devices are measured per device,
 * in that Bridge's Device Details. */
export function ThisDeviceSettings() {
  return <>
    <DeviceIdentitySettings />
    <DeviceProfileSection />
    <LocalSpeedTest />
  </>;
}

function FactList({ rows }: { rows: FactRow[] }) {
  return <dl className="facts card-body">{rows.map(([label, value]) => <Fragment key={label}><dt>{label}</dt><dd>{value}</dd></Fragment>)}</dl>;
}

function DeviceProfileSection() {
  const [profile, setProfile] = useState<DeviceProfile | null>(null);
  const [capabilities, setCapabilities] = useState<DeviceCapabilities | null>(null);
  const [busy, setBusy] = useState<"profile" | CapabilityProbeMode | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void getDeviceProfile().then(setProfile).catch((nextError) => setError(nextError instanceof Error ? nextError.message : String(nextError)));
    void getDeviceCapabilities({ probeMode: "quick" }).then(setCapabilities).catch(() => undefined);
  }, []);

  async function refreshProfile() {
    setBusy("profile"); setError(null);
    try { setProfile(await getDeviceProfile({ forceRefresh: true })); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setBusy(null); }
  }

  async function probe(mode: CapabilityProbeMode) {
    setBusy(mode); setError(null);
    try { setCapabilities(await getDeviceCapabilities({ forceRefresh: true, probeMode: mode })); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setBusy(null); }
  }

  return <>
    <section className="section" aria-label="Device">
      <div className="section-head-row"><div className="section-head"><h2>Device</h2><p>As this device reports itself to Pastey and to the devices it pairs with.</p></div>
        <button type="button" className="btn btn-sm" disabled={busy !== null} onClick={() => void refreshProfile()}><Icon name="refresh" small />{busy === "profile" ? "Refreshing…" : "Refresh"}</button></div>
      <div className="card">{profile ? <FactList rows={deviceProfileFacts(profile)} /> : <p className="list-empty">{error ?? "Loading…"}</p>}</div>
    </section>
    <section className="section" aria-label="Runtimes">
      <div className="section-head-row"><div className="section-head"><h2>Runtimes</h2><p>What Pastey found installed. A quick check reads known locations; a full check also asks each runtime its version.</p></div>
        <div className="row"><button type="button" className="btn btn-sm" disabled={busy !== null} onClick={() => void probe("quick")}>{busy === "quick" ? "Checking…" : "Quick check"}</button><button type="button" className="btn btn-sm" disabled={busy !== null} onClick={() => void probe("full")}>{busy === "full" ? "Checking…" : "Full check"}</button></div></div>
      <div className="card">{capabilities ? <FactList rows={[...runtimeFacts(capabilities), ...(capabilities.gpu_acceleration.cuda_available || capabilities.gpu_acceleration.metal_available ? [["GPU acceleration", [capabilities.gpu_acceleration.metal_available ? "Metal" : null, capabilities.gpu_acceleration.cuda_available ? "CUDA" : null].filter(Boolean).join(", ")] as FactRow] : [])]} /> : <p className="list-empty">Not checked yet.</p>}</div>
      {error && profile ? <p className="error-text">{error}</p> : null}
    </section>
  </>;
}

const DURATIONS: ReadonlyArray<{ value: string; label: string }> = [{ value: "1", label: "1 s" }, { value: "5", label: "5 s" }, { value: "15", label: "15 s" }];

/** A real measurement of this device alone; never a network speed. */
function LocalSpeedTest() {
  const [mode, setMode] = useState<BenchmarkMode>("pastey_pipeline");
  const [duration, setDuration] = useState("5");
  const [result, setResult] = useState<LinkBenchmarkResult | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void getLastBenchmarkResults().then((results) => setResult(results.find((candidate) => candidate.peer_id === "loopback") ?? null)).catch(() => undefined);
  }, []);

  async function run() {
    setRunning(true); setError(null);
    try { setResult(await runLoopbackBenchmark({ mode, durationSeconds: Number(duration) })); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setRunning(false); }
  }

  const shown = result ? benchmarkFacts(result) : null;
  return <section className="section" aria-label="Local speed test">
    <div className="section-head"><h2>Local speed test</h2><p>How fast this device alone can move data through Pastey, with no network involved. To measure the connection to another device, open that device from its Bridge.</p></div>
    <div className="card">
      <div className="card-body">
        <div className="row-wrap">
          <Segmented label="What to measure" value={mode} onChange={setMode} disabled={running} options={[{ value: "pastey_pipeline", label: "Pastey pipeline" }, { value: "raw_memory", label: "Raw memory" }]} />
          <Segmented label="How long" value={duration} onChange={setDuration} disabled={running} options={DURATIONS} />
          <span className="spacer" />
          <button type="button" className="btn btn-primary" disabled={running} onClick={() => void run()}>{running ? "Running…" : "Run local test"}</button>
        </div>
        <p className="xsmall subtle">{benchmarkScope(mode, false)}.</p>
        {error ? <Notice tone="bad">{error}</Notice> : null}
        {shown && result ? <>
          <div className="row"><strong className="spacer">{shown.headline}</strong><Badge tone={result.link_quality === "Excellent" || result.link_quality === "Good" ? "ok" : "warn"}>{result.link_quality}</Badge></div>
          <p className="xsmall subtle">Last result: {benchmarkScope(result.benchmark_mode, false)}.</p>
          <dl className="facts">{shown.facts.map(([label, value]) => <Fragment key={label}><dt>{label}</dt><dd>{value}</dd></Fragment>)}</dl>
        </> : <p className="small muted">Not measured yet.</p>}
      </div>
    </div>
  </section>;
}

import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState, type ChangeEvent, type ReactNode } from "react";
import { prettifyShortcut } from "../lib/format";
import {
  checkForUpdates,
  copyLastError,
  getDeviceCapabilities,
  getDeviceProfile,
  getLastBenchmarkResults,
  openLogsFolder,
  runLoopbackBenchmark,
  updateConfig
} from "../lib/tauri";
import type { AppConfig, BenchmarkMode, DeviceCapabilities, DeviceProfile, LinkBenchmarkResult } from "../lib/types";

interface SettingsPageProps {
  config: AppConfig;
  onConfigChange: (config: AppConfig) => void;
  onJoinWithCode: () => void;
}

const PRESET_WINDOWS = [1, 2, 4, 8, 16];
const DEFAULT_WINDOW = 8;
const DIAGNOSTICS_FRONTEND_TTL_MS = 60_000;

interface DiagnosticsSnapshot {
  profile: DeviceProfile | null;
  capabilities: DeviceCapabilities | null;
  benchmark: LinkBenchmarkResult | null;
  cachedAt: number;
}

let cachedDiagnostics: DiagnosticsSnapshot | null = null;
let diagnosticsRequest: Promise<DiagnosticsSnapshot> | null = null;
let diagnosticsRequestSequence = 0;

export function SettingsPage({ config, onConfigChange, onJoinWithCode }: SettingsPageProps) {
  const [windowValue, setWindowValue] = useState(windowSelectionFromConfig(config.transfer_window_override, PRESET_WINDOWS));
  const [customWindow, setCustomWindow] = useState(customWindowFromConfig(config.transfer_window_override, PRESET_WINDOWS));
  const [logActionMessage, setLogActionMessage] = useState<string | null>(null);
  const [deviceProfile, setDeviceProfile] = useState<DeviceProfile | null>(null);
  const [deviceCapabilities, setDeviceCapabilities] = useState<DeviceCapabilities | null>(null);
  const [lastBenchmark, setLastBenchmark] = useState<LinkBenchmarkResult | null>(null);
  const [benchmarkMode, setBenchmarkMode] = useState<BenchmarkMode>("raw_memory");
  const [benchmarkDuration, setBenchmarkDuration] = useState(5);
  const [diagnosticMessage, setDiagnosticMessage] = useState<string | null>(null);
  const [diagnosticsLoading, setDiagnosticsLoading] = useState(false);
  const [benchmarkRunning, setBenchmarkRunning] = useState(false);

  useEffect(() => {
    setWindowValue(windowSelectionFromConfig(config.transfer_window_override, PRESET_WINDOWS));
    setCustomWindow(customWindowFromConfig(config.transfer_window_override, PRESET_WINDOWS));
  }, [config.transfer_window_override]);

  useEffect(() => {
    void refreshDiagnostics(false);
  }, []);

  async function save(next: AppConfig) {
    const saved = await updateConfig(next);
    onConfigChange(saved);
  }

  async function saveDevToolsEnabled(enabled: boolean) {
    await save({ ...config, dev_tools_enabled: enabled });
  }

  async function saveMicroFlowGroupMode(mode: AppConfig["micro_flow_group_mode"]) {
    await save({ ...config, micro_flow_group_mode: mode });
  }

  async function saveTransferWindow(nextValue: string) {
    setWindowValue(nextValue);
    if (nextValue === "custom") {
      const currentCustom = validCustomWindow(customWindow) ?? DEFAULT_WINDOW;
      setCustomWindow(String(currentCustom));
      await save({ ...config, transfer_window_override: currentCustom });
      return;
    }

    await save({
      ...config,
      transfer_window_override: nextValue === "default" ? null : Number(nextValue)
    });
  }

  async function saveCustomWindow() {
    const nextCustom = validCustomWindow(customWindow);
    if (!nextCustom) {
      setCustomWindow(customWindowFromConfig(config.transfer_window_override, PRESET_WINDOWS) || String(DEFAULT_WINDOW));
      return;
    }

    setCustomWindow(String(nextCustom));
    setWindowValue("custom");
    await save({ ...config, transfer_window_override: nextCustom });
  }

  async function chooseInbox() {
    const selected = await open({
      directory: true,
      multiple: false,
      defaultPath: config.inbox_dir ?? undefined
    });

    if (typeof selected === "string") {
      await save({ ...config, inbox_dir: selected });
    }
  }

  async function handleOpenLogsFolder() {
    setLogActionMessage(null);
    try {
      await openLogsFolder();
    } catch (err) {
      setLogActionMessage(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleCopyLastError() {
    setLogActionMessage(null);
    try {
      const copied = await copyLastError();
      setLogActionMessage(copied ? "Last error copied." : "No transfer error logged yet.");
    } catch (err) {
      setLogActionMessage(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleCheckForUpdates() {
    setLogActionMessage(null);
    try {
      await checkForUpdates();
    } catch (err) {
      setLogActionMessage(err instanceof Error ? err.message : String(err));
    }
  }

  async function refreshDiagnostics(forceRefresh = false) {
    if (!forceRefresh && cachedDiagnostics && diagnosticsSnapshotFresh(cachedDiagnostics)) {
      setDeviceProfile(cachedDiagnostics.profile);
      setDeviceCapabilities(cachedDiagnostics.capabilities);
      setLastBenchmark(cachedDiagnostics.benchmark);
      return;
    }

    setDiagnosticMessage(null);
    setDiagnosticsLoading(true);
    const request = !forceRefresh && diagnosticsRequest ? diagnosticsRequest : loadDiagnosticsSnapshot(forceRefresh);
    const requestSequence = ++diagnosticsRequestSequence;
    diagnosticsRequest = request;
    try {
      const { profile, capabilities, benchmark } = await request;
      if (requestSequence !== diagnosticsRequestSequence) return;
      setDeviceProfile(profile);
      setDeviceCapabilities(capabilities);
      setLastBenchmark(benchmark);
    } catch (err) {
      if (requestSequence !== diagnosticsRequestSequence) return;
      setDiagnosticMessage(err instanceof Error ? err.message : String(err));
    } finally {
      if (requestSequence === diagnosticsRequestSequence) {
        setDiagnosticsLoading(false);
      }
      if (diagnosticsRequest === request) {
        diagnosticsRequest = null;
      }
    }
  }

  async function handleRunLoopbackBenchmark() {
    setBenchmarkRunning(true);
    setDiagnosticMessage(null);
    try {
      const result = await runLoopbackBenchmark({
        mode: benchmarkMode,
        durationSeconds: benchmarkDuration,
        windowSize: config.transfer_window_override ?? undefined
      });
      setLastBenchmark(result);
    } catch (err) {
      setDiagnosticMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBenchmarkRunning(false);
    }
  }

  return (
    <div className="workstation-view settings-workstation" aria-label="Settings">
      <div className="settings-card-grid">
        <SettingsCard title="General" icon="gear">
          <SettingsControlRow label="Device name" value={deviceProfile?.device_name || "This device"} />
          <SettingsControlRow label="Theme" control={<DisabledSelect value="System" />} />
          <SettingsControlRow label="Startup behavior" value="Not available yet" disabled />
          <SettingsControlRow label="Downloads location" value={config.inbox_dir ? "Custom" : "Inbox"} actionLabel="Choose" onAction={chooseInbox} />
          <SettingsControlRow label="Global shortcut" value={prettifyShortcut(config.shortcut)} />
        </SettingsCard>

        <SettingsCard title="Transfers" icon="drive">
          <SettingsControlRow label="Max concurrent transfers" value="Not available yet" disabled />
          <SettingsControlRow
            label="Burn defaults"
            control={<Switch checked={config.auto_burn_after_download} onChange={(event) => void save({ ...config, auto_burn_after_download: event.target.checked })} />}
          />
          <SettingsControlRow label="Receiver approval defaults" value="Not available yet" disabled />
          <SettingsControlRow
            label="Save received files"
            control={<Switch checked={config.save_received_files_to_inbox} onChange={(event) => void save({ ...config, save_received_files_to_inbox: event.target.checked })} />}
          />
          <SettingsControlRow
            label="Save received images"
            control={<Switch checked={config.save_received_images_to_inbox} onChange={(event) => void save({ ...config, save_received_images_to_inbox: event.target.checked })} />}
          />
          <SettingsControlRow label="Compression" value="Not available yet" disabled />
          <SettingsControlRow label="Bandwidth limit" value="Not available yet" disabled />
        </SettingsCard>

        <SettingsCard title="Security" icon="shield">
          <SettingsControlRow label="Encryption status" value="Enabled" status="Secure" />
          <SettingsControlRow label="Key verification" value="Room scoped" />
          <SettingsControlRow label="Trusted peers" value="Current-session only" />
          <SettingsControlRow label="Approval policy" value="Per request" />
          <p className="settings-card-note">There is no generic runtime or reusable trust authority.</p>
        </SettingsCard>

        <SettingsCard title="Discovery" icon="nearby">
          <SettingsControlRow label="Local network discovery" value="Enabled" status="Stable" />
          <SettingsControlRow label="Join with code" actionLabel="Open" onAction={onJoinWithCode} />
          <SettingsControlRow label="Presence TTL" value="Current discovery default" />
          <SettingsControlRow
            label="Diagnostics logging"
            control={<Switch checked={config.dev_tools_enabled} onChange={(event) => void saveDevToolsEnabled(event.target.checked)} />}
          />
        </SettingsCard>

        <SettingsCard title="Notifications" icon="bell">
          <SettingsControlRow label="Transfer completed" value="Not available yet" disabled />
          <SettingsControlRow label="Approval requests" value="Not available yet" disabled />
          <SettingsControlRow label="Errors" value="Not available yet" disabled />
        </SettingsCard>

        <SettingsCard title="Advanced diagnostics" icon="wrench">
          <SettingsControlRow
            label="Diagnostics logging"
            value={config.dev_tools_enabled ? "Enabled" : "Disabled"}
            control={<Switch checked={config.dev_tools_enabled} onChange={(event) => void saveDevToolsEnabled(event.target.checked)} />}
          />
          <SettingsControlRow label="Local profile" value={diagnosticsLoading ? "Loading..." : deviceProfile ? deviceTitle(deviceProfile) : "Not loaded"} actionLabel="Refresh" onAction={() => refreshDiagnostics(true)} disabled={diagnosticsLoading} />
          <SettingsControlRow label="Capability probe" value={deviceCapabilities ? availableRuntimeTitle(deviceCapabilities) : "Not probed"} />
          <SettingsControlRow label="Last benchmark" value={lastBenchmark ? `${lastBenchmark.average_MBps.toFixed(1)} MB/s` : "Not run"} />
          {diagnosticMessage ? <p className="muted">{diagnosticMessage}</p> : null}
        </SettingsCard>

        <SettingsCard title="Transfer diagnostics" icon="drive">
          <SettingsControlRow label="Advanced transfer grouping" detail="Uses the existing transfer planner setting." control={
            <select
              value={config.micro_flow_group_mode}
              onChange={(event) => void saveMicroFlowGroupMode(event.target.value as AppConfig["micro_flow_group_mode"])}
            >
              <option value="dynamic">Dynamic</option>
              <option value="fixed">Fixed</option>
            </select>
          } />
          <SettingsControlRow label="Transfer window" detail="Existing binary transfer pipeline depth." control={
            <select value={windowValue} onChange={(event) => void saveTransferWindow(event.target.value)}>
              <option value="default">Default / Auto</option>
              <option value="1">1</option>
              <option value="2">2</option>
              <option value="4">4</option>
              <option value="8">8</option>
              <option value="16">16</option>
              <option value="custom">Custom</option>
            </select>
          } />
          {windowValue === "custom" ? (
            <SettingsControlRow label="Custom window" detail="1 to 16" control={
              <input
                type="number"
                min={1}
                max={16}
                step={1}
                value={customWindow}
                placeholder={String(DEFAULT_WINDOW)}
                onChange={(event) => setCustomWindow(event.target.value)}
                onBlur={() => void saveCustomWindow()}
                onKeyDown={(event) => {
                  if (event.key === "Enter") event.currentTarget.blur();
                }}
              />
            } />
          ) : null}
        </SettingsCard>

        <SettingsCard title="Device diagnostics" icon="nearby">
          <p className="muted diagnostics-note">
            Loopback tests stay on this device. They do not measure Wi-Fi, Ethernet, school network, or internet speed.
          </p>
          <div className="diagnostic-grid">
            <DiagnosticBlock title="Device Profile" rows={[
              ["Device", deviceProfile ? deviceTitle(deviceProfile) : "Unknown"],
              ["Platform", deviceProfile ? platformTitle(deviceProfile) : "Unknown"],
              ["Power", deviceProfile ? powerTitle(deviceProfile) : "Unknown"]
            ]} />
            <DiagnosticBlock title="Capabilities" rows={[
              ["CPU", deviceProfile ? cpuTitle(deviceProfile) : "Unknown"],
              ["GPU", deviceCapabilities ? gpuTitle(deviceCapabilities) : "Unknown"],
              ["Runtimes", deviceCapabilities ? availableRuntimeTitle(deviceCapabilities) : "Unknown"]
            ]} />
            <DiagnosticBlock title="Last Benchmark" rows={[
              ["Mode", lastBenchmark ? benchmarkModeTitle(lastBenchmark.benchmark_mode) : "Unknown"],
              ["Quality", lastBenchmark ? lastBenchmark.link_quality : "Not run"],
              ["Average", lastBenchmark ? `${lastBenchmark.average_MBps.toFixed(1)} MB/s` : "Not run"],
              ["Latency", lastBenchmark?.latency_ms != null ? `${lastBenchmark.latency_ms.toFixed(1)} ms` : "Unknown"]
            ]} />
          </div>
        </SettingsCard>

        <SettingsCard title="Local diagnostic tools" icon="wrench">
          <div className="settings-diagnostics-body">
            <div className="benchmark-controls">
              <select value={benchmarkMode} onChange={(event) => setBenchmarkMode(event.target.value as BenchmarkMode)}>
                <option value="raw_memory">Loopback raw memory</option>
                <option value="pastey_pipeline">Loopback Pastey pipeline</option>
              </select>
              <select value={benchmarkDuration} onChange={(event) => setBenchmarkDuration(Number(event.target.value))}>
                <option value={1}>Target 1s quick</option>
                <option value={5}>Target 5s standard</option>
                <option value={15}>Target 15s extended</option>
              </select>
              <button className="secondary-button" disabled={benchmarkRunning} onClick={() => void handleRunLoopbackBenchmark()}>
                {benchmarkRunning ? "Running..." : "Run local test"}
              </button>
            </div>
            <p className="muted diagnostics-note">{benchmarkModeDescription(benchmarkMode)}</p>
            {logActionMessage ? <p className="muted">{logActionMessage}</p> : null}
            <div className="diagnostic-actions">
              <button className="secondary-button" onClick={() => void handleOpenLogsFolder()}>
                Open logs
              </button>
              <button className="secondary-button" onClick={() => void handleCopyLastError()}>
                Copy last error
              </button>
              <button className="secondary-button" onClick={() => void handleCheckForUpdates()}>
                Check updates
              </button>
            </div>
          </div>
        </SettingsCard>
      </div>
    </div>
  );
}

function SettingsCard({ title, icon, children }: { title: string; icon: string; children: ReactNode }) {
  return (
    <section className="settings-workstation-card">
      <div className="settings-group-title">
        <span className={`section-icon ${icon}`} aria-hidden="true" />
        <h2>{title}</h2>
      </div>
      <div className="settings-control-list">{children}</div>
    </section>
  );
}

function DiagnosticBlock({ title, rows }: { title: string; rows: Array<[string, string]> }) {
  return (
    <div className="diagnostic-block">
      <strong>{title}</strong>
      {rows.map(([label, value]) => (
        <div className="diagnostic-metric" key={label}>
          <span>{label}</span>
          <span>{value}</span>
        </div>
      ))}
    </div>
  );
}

function deviceTitle(profile: DeviceProfile): string {
  const memory = profile.memory_total_gb ? `${profile.memory_total_gb}GB` : null;
  return [profile.device_name || platformDeviceFallback(profile), memory].filter(Boolean).join(" · ");
}

function platformTitle(profile: DeviceProfile): string {
  return [profile.platform, profile.os_version, profile.arch].filter(Boolean).join(" · ");
}

function powerTitle(profile: DeviceProfile): string {
  const label = profile.power_state === "plugged_in" ? "Plugged in" : profile.power_state === "on_battery" ? "Battery mode" : "Unknown";
  return profile.battery_percent == null ? label : `${label} · ${profile.battery_percent}%`;
}

function cpuTitle(profile: DeviceProfile): string {
  const name = profile.cpu_name || profile.arch || "Unknown";
  const physical = profile.cpu_physical_core_count ?? null;
  const logical = profile.cpu_logical_processor_count ?? profile.cpu_core_count ?? null;
  if (physical && logical && physical !== logical) return `${name} · ${physical}C/${logical}T`;
  if (logical) return `${name} · ${logical} cores`;
  return name;
}

function gpuTitle(capabilities: DeviceCapabilities): string {
  const primary = capabilities.gpu_acceleration.gpu_names[0];
  const labels = [
    primary,
    capabilities.gpu_acceleration.cuda_available ? "CUDA" : null,
    capabilities.gpu_acceleration.metal_available ? "Metal" : null
  ].filter(Boolean);
  if (labels.length) return labels.join(" · ");
  return "Unknown";
}

function availableRuntimeTitle(capabilities: DeviceCapabilities): string {
  const names = capabilities.runtimes
    .filter((runtime) => runtime.available)
    .map((runtime) => {
      const version = runtime.version?.trim();
      return version ? `${runtime.name} (${version})` : runtime.name;
    });
  if (names.length) return names.join(", ");
  return capabilities.runtimes.length ? "None detected" : "Not probed";
}

function benchmarkModeDescription(mode: BenchmarkMode): string {
  if (mode === "pastey_pipeline") {
    return "Localhost encrypted/framed pipeline. Measures Pastey overhead, not LAN speed.";
  }

  return "Localhost memory/socket baseline. Does not use LAN or internet.";
}

function benchmarkModeTitle(mode?: BenchmarkMode | null): string {
  if (mode === "raw_memory") return "Loopback raw memory";
  if (mode === "pastey_pipeline") return "Loopback Pastey pipeline";
  return "Unknown";
}

function platformDeviceFallback(profile: DeviceProfile): string {
  if (profile.platform === "macos") return "Mac";
  if (profile.platform === "windows") return "Windows PC";
  return "This device";
}

async function loadDiagnosticsSnapshot(forceRefresh: boolean): Promise<DiagnosticsSnapshot> {
  const [profileResult, capabilitiesResult, benchmarkResult] = await Promise.allSettled([
    getDeviceProfile({ forceRefresh }),
    getDeviceCapabilities({ forceRefresh, probeMode: "full" }),
    getLastBenchmarkResults()
  ]);
  const profile = profileResult.status === "fulfilled" ? profileResult.value : cachedDiagnostics?.profile ?? null;
  const capabilities = capabilitiesResult.status === "fulfilled" ? capabilitiesResult.value : cachedDiagnostics?.capabilities ?? null;
  const benchmark = benchmarkResult.status === "fulfilled" ? benchmarkResult.value[0] ?? null : cachedDiagnostics?.benchmark ?? null;

  cachedDiagnostics = {
    profile,
    capabilities,
    benchmark,
    cachedAt: Date.now()
  };
  return cachedDiagnostics;
}

function diagnosticsSnapshotFresh(snapshot: DiagnosticsSnapshot): boolean {
  return Date.now() - snapshot.cachedAt <= DIAGNOSTICS_FRONTEND_TTL_MS;
}

function SettingsControlRow({
  label,
  detail,
  value,
  control,
  actionLabel,
  onAction,
  status,
  disabled,
}: {
  label: string;
  detail?: string;
  value?: string;
  control?: ReactNode;
  actionLabel?: string;
  onAction?: () => void | Promise<void>;
  status?: string;
  disabled?: boolean;
}) {
  const content = (
    <>
      <div className="settings-control-copy">
        <strong>{label}</strong>
        {detail ? <p className="muted">{detail}</p> : null}
      </div>
      <div className="settings-control-trailing">
        {status ? <span className="status-chip success">{status}</span> : null}
        {control ? <div className="settings-control">{control}</div> : null}
        {value ? <span className="settings-value">{value}</span> : null}
        {onAction ? <span className="link-like-action">{actionLabel ?? "Open"}</span> : null}
      </div>
    </>
  );

  return onAction ? (
    <button className={`settings-control-row settings-control-button ${disabled ? "disabled" : ""}`} disabled={disabled} onClick={() => void onAction()}>
      {content}
    </button>
  ) : (
    <div className={`settings-control-row ${disabled ? "disabled" : ""}`}>{content}</div>
  );
}

function DisabledSelect({ value }: { value: string }) {
  return (
    <select value={value} disabled>
      <option value={value}>{value}</option>
    </select>
  );
}

function Switch({
  checked,
  disabled,
  readOnly,
  onChange
}: {
  checked: boolean;
  disabled?: boolean;
  readOnly?: boolean;
  onChange?: (event: ChangeEvent<HTMLInputElement>) => void;
}) {
  return (
    <label className="switch">
      <input type="checkbox" checked={checked} disabled={disabled} readOnly={readOnly} onChange={onChange} />
      <span aria-hidden="true" />
    </label>
  );
}

export function windowSelectionFromConfig(value: number | null | undefined, presets: number[]): string {
  if (!value || !Number.isFinite(value) || value <= 0) return "default";
  return presets.includes(value) ? String(value) : "custom";
}

export function customWindowFromConfig(value: number | null | undefined, presets: number[]): string {
  if (!value || !Number.isFinite(value) || value <= 0 || presets.includes(value)) return "";
  return String(value);
}

export function validCustomWindow(value: string): number | null {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) return null;
  return Math.min(16, Math.max(1, Math.trunc(parsed)));
}

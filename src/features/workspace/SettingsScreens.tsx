import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { readAppearance, saveAppearance, type Appearance } from "../../lib/appearance";
import {
  authorizeSearchFolder,
  checkForUpdates,
  checkManagedWorkerProviderHealth,
  copyLastError,
  createManagedWorkerProvider,
  deleteManagedWorkerProvider,
  getManagedWorkerProviderSettings,
  listAuthorizedSearchFolders,
  listManagedRuntimeOptions,
  listNativeAgentCapabilities,
  listRooms,
  openLogsFolder,
  revealInFolder,
  revokeAuthorizedSearchFolder,
  selectManagedExecuteRuntime,
  selectManagedWorkerProvider,
  updateConfig,
  updateManagedWorkerProvider,
} from "../../lib/tauri";
import type { AuthorizedSearchFolder, ManagedRuntimeOption, ManagedWorkerProviderSettings, ManagedWorkerProviderSettingsSnapshot, NativeAgentCapability } from "../../lib/tauri";
import type { AppConfig, RoomInfo } from "../../lib/types";
import { DeviceTrustSettings } from "./DeviceTrustSettings";
import { KnownDevicesSettings } from "./KnownDevicesSettings";
import { availabilityCopy } from "./NativeAgentPanel";
import { SessionReadingSettings } from "./SessionReadingSettings";
import { ThisDeviceSettings } from "./ThisDeviceSettings";
import { UnattendedAccessSettings } from "./UnattendedAccessSettings";
import { Badge, Dot, Icon, ListRow, Notice, Segmented, Switch, type IconName } from "./ui";
import { bridgeCode } from "./workspaceViewModel";
import type { NavigateWorkspace, WorkspaceRoute } from "./workspaceTypes";

export type SettingsRoute = Extract<WorkspaceRoute, `settings${string}`>;

/** Device-wide settings only. What belongs to one Bridge (its connection,
 * devices, and what it is allowed here) is in that Bridge's details. */
const SECTIONS: ReadonlyArray<{ route: SettingsRoute; label: string; icon: IconName }> = [
  { route: "settings", label: "General", icon: "settings" },
  { route: "settings-this-device", label: "This device", icon: "device" },
  { route: "settings-devices", label: "Devices & trust", icon: "shield" },
  { route: "settings-permissions", label: "Permissions", icon: "key" },
  { route: "settings-agents", label: "Agents", icon: "agent" },
  { route: "settings-advanced", label: "Advanced", icon: "terminal" },
  { route: "settings-about", label: "About", icon: "info" },
];

interface SettingsProps {
  route: SettingsRoute;
  config: AppConfig;
  onNavigate: NavigateWorkspace;
  onConfigChange: (config: AppConfig) => void;
  menuButton?: ReactNode;
}

export function SettingsScreen({ route, config, onNavigate, onConfigChange, menuButton }: SettingsProps) {
  const section = SECTIONS.find((candidate) => candidate.route === route) ?? SECTIONS[0];
  return <section className="page" aria-label="Settings">
    {menuButton ? <header className="page-header narrow-only">{menuButton}<div className="page-header-main"><h1>Settings</h1></div></header> : null}
    <div className="settings">
      <nav className="settings-nav" aria-label="Settings sections">
        {SECTIONS.map((candidate) => <button key={candidate.route} type="button" className={`nav-item${candidate.route === section.route ? " active" : ""}`} aria-current={candidate.route === section.route ? "page" : undefined} onClick={() => onNavigate(candidate.route)}><Icon name={candidate.icon} />{candidate.label}</button>)}
      </nav>
      <div className="settings-content"><div className="content">
        <h1>{section.label}</h1>
        {section.route === "settings" ? <GeneralSettings config={config} onConfigChange={onConfigChange} /> : null}
        {section.route === "settings-this-device" ? <ThisDeviceSettings /> : null}
        {section.route === "settings-devices" ? <><DeviceTrustSettings /><KnownDevicesSettings /></> : null}
        {section.route === "settings-permissions" ? <PermissionsSettings /> : null}
        {section.route === "settings-agents" ? <AgentSettings /> : null}
        {section.route === "settings-advanced" ? <AdvancedSettings config={config} onConfigChange={onConfigChange} /> : null}
        {section.route === "settings-about" ? <AboutSettings config={config} /> : null}
      </div></div>
    </div>
  </section>;
}

type SavedConfigKey = "save_received_files_to_inbox" | "save_received_images_to_inbox";

function GeneralSettings({ config, onConfigChange }: { config: AppConfig; onConfigChange: (config: AppConfig) => void }) {
  const [appearance, setAppearance] = useState<Appearance>(readAppearance);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save(next: AppConfig) {
    if (pending) return;
    setPending(true); setError(null);
    try { onConfigChange(await updateConfig(next)); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  async function chooseFolder() {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") await save({ ...config, inbox_dir: chosen });
  }

  const toggle = (key: SavedConfigKey, value: boolean) => void save({ ...config, [key]: value });

  return <>
    <section className="section">
      <div className="card">
        <div className="setting-row"><span><strong>Appearance</strong><small>Light, dark, or follow the system.</small></span>
          <Segmented label="Appearance" value={appearance} onChange={(value) => { setAppearance(value); saveAppearance(value); }} options={[{ value: "system", label: "System" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} /></div>
        <div className="setting-row"><span><strong>Global shortcut</strong><small>Opens Pastey from anywhere.</small></span><span className="value mono">{config.shortcut}</span></div>
      </div>
    </section>
    <section className="section" aria-label="Receiving">
      <div className="section-head"><h2>Receiving</h2><p>Where text and files other devices send you are kept.</p></div>
      <div className="card">
        <div className="setting-row"><span><strong>Receiving folder</strong><small className="mono">{config.inbox_dir ?? "Pastey's default Inbox folder"}</small></span>
          <span className="row">{config.inbox_dir ? <button type="button" className="btn btn-sm" onClick={() => void revealInFolder(config.inbox_dir!)}><Icon name="folder" small />Open</button> : null}
            <button type="button" className="btn btn-sm" disabled={pending} onClick={() => void chooseFolder()}>Change…</button>
            {config.inbox_dir ? <button type="button" className="btn btn-sm" disabled={pending} onClick={() => void save({ ...config, inbox_dir: null })}>Use default</button> : null}</span></div>
        <div className="setting-row"><span><strong>Save received files</strong><small>Off: received files stay in Pastey's temporary storage instead of the receiving folder.</small></span><Switch label="Save received files" checked={config.save_received_files_to_inbox} disabled={pending} onChange={(value) => toggle("save_received_files_to_inbox", value)} /></div>
        <div className="setting-row"><span><strong>Save received images</strong><small>Off: received images stay in Pastey's temporary storage instead of the receiving folder.</small></span><Switch label="Save received images" checked={config.save_received_images_to_inbox} disabled={pending} onChange={(value) => toggle("save_received_images_to_inbox", value)} /></div>
      </div>
      {error ? <p className="error-text" role="alert">{error}</p> : null}
    </section>
  </>;
}

function PermissionsSettings() {
  return <>
    <p className="muted" style={{ marginBottom: "var(--space-6)" }}>What other devices may do on this device. Each permission names one device, one Bridge and one folder, and only you can create or revoke it here. A trusted device without a permission can do none of this.</p>
    <UnattendedAccessSettings />
    <SessionReadingSettings />
    <SharedSearchFolders />
  </>;
}

/** Folders this device's user shares for other Bridge devices' file search.
 * Sharing a folder lets a connected device find files in it by name and
 * request one; it never lets that device browse, change or delete anything,
 * and nothing it or an Agent sends can add a folder here. */
function SharedSearchFolders() {
  const [folders, setFolders] = useState<AuthorizedSearchFolder[] | null>(null);
  const [bridges, setBridges] = useState<RoomInfo[]>([]);
  const [bridgeId, setBridgeId] = useState("");
  const [adding, setAdding] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    void listAuthorizedSearchFolders().then(setFolders).catch(() => setFolders([]));
    void listRooms().then((rooms) => setBridges(rooms.filter((room) => room.status === "active"))).catch(() => setBridges([]));
  }, []);
  const bridgeName = (id: string | null) => {
    const bridge = bridges.find((room) => room.id === id);
    return bridge ? `Bridge ${bridgeCode(bridge)}${bridge.peer_device_name ? ` (${bridge.peer_device_name})` : ""}` : id ? "an ended Bridge" : null;
  };
  async function add() {
    if (!bridgeId) { setError("Choose the Bridge whose devices may search the folder."); return; }
    setPending(true); setError(null);
    try {
      const chosen = await open({ directory: true, multiple: false });
      if (typeof chosen !== "string") return;
      await authorizeSearchFolder(chosen, bridgeId);
      setFolders(await listAuthorizedSearchFolders());
      setAdding(false);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally { setPending(false); }
  }
  async function remove(folderId: string) {
    setPending(true); setError(null);
    try { setFolders(await revokeAuthorizedSearchFolder(folderId)); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }
  return <section className="section" aria-label="Shared search folders">
    <div className="section-head"><h2>Shared search folders</h2><p>Devices on the chosen Bridge can find files in a shared folder by name and ask for one. Other Bridges cannot. Nothing in the folder is changed or deleted, and burning the Bridge ends the share.</p></div>
    <div className="card">
      <div className="list">
        {folders === null ? <p className="list-empty">Checking…</p> : folders.length === 0 ? <p className="list-empty">No shared folders. Other devices cannot search this device. Downloads, Desktop, Documents and other places are never searchable until you share a specific folder here for one Bridge.</p> : folders.map((folder) => <ListRow key={folder.folderId} leading={<Icon name="folder" />} title={folder.displayName} meta={<>{folder.bridgeId ? `Shared with ${bridgeName(folder.bridgeId)}` : "Shared before Bridge scoping. Not searched; stop sharing and share it with one Bridge."}{folder.bridgeId && folder.unavailable ? " · moved or replaced; not searched" : ""}</>} side={<button type="button" className="btn btn-danger btn-sm" disabled={pending} onClick={() => void remove(folder.folderId)}>Stop sharing</button>} />)}
      </div>
      {adding ? <div className="add-form">
        <strong>Share a folder</strong>
        <div className="row-wrap">
          <select className="select" value={bridgeId} onChange={(event) => setBridgeId(event.target.value)} disabled={pending || bridges.length === 0} aria-label="Bridge to share with"><option value="">{bridges.length ? "Share with which Bridge?" : "No active Bridge"}</option>{bridges.map((bridge) => <option key={bridge.id} value={bridge.id}>Bridge {bridgeCode(bridge)}{bridge.peer_device_name ? ` · ${bridge.peer_device_name}` : ""}</option>)}</select>
          <button type="button" className="btn btn-primary" disabled={pending || !bridgeId} onClick={() => void add()}><Icon name="folder" small />Choose folder and share…</button>
        </div>
        <div className="row"><span className="spacer" /><button type="button" className="btn btn-sm" disabled={pending} onClick={() => setAdding(false)}>Cancel</button></div>
      </div> : <footer className="card-footer"><button type="button" className="btn btn-sm" onClick={() => setAdding(true)}><Icon name="plus" small />Share a folder…</button></footer>}
    </div>
    {error ? <p className="error-text" role="alert">{error}</p> : null}
  </section>;
}

function AgentSettings() {
  const [capabilities, setCapabilities] = useState<NativeAgentCapability[] | null>(null);
  const [checking, setChecking] = useState(false);
  async function check() {
    setChecking(true);
    try { setCapabilities(await listNativeAgentCapabilities()); } catch { setCapabilities([]); } finally { setChecking(false); }
  }
  useEffect(() => { void check(); }, []);
  const codex = capabilities?.find((capability) => capability.agentId === "agent.coding.codex");
  return <>
    <section className="section" aria-label="Native Agents on this device">
      <div className="section-head-row"><div className="section-head"><h2>On this device</h2><p>Native Agents keep their own sign-in, tools, sandbox and context. Pastey only starts them, moves files when you approve, and reports what they return.</p></div>
        <button type="button" className="btn btn-sm" disabled={checking} onClick={() => void check()}><Icon name="refresh" small />{checking ? "Checking…" : "Check again"}</button></div>
      <div className="card">
        <div className="setting-row"><span><strong>Codex</strong><small>{capabilities === null ? "Checking…" : availabilityCopy(codex ? { state: codex.state, reason: codex.reason, detail: codex.detail, backend: codex.backend } : { state: "unavailable" }, null, false)}</small></span>
          {capabilities === null ? <Dot pulse /> : <Badge tone={codex?.state === "available" ? "ok" : "warn"}>{codex?.state === "available" ? "Available" : codex?.state === "incompatible" ? "Needs update" : "Unavailable"}</Badge>}</div>
      </div>
    </section>
    <section className="section" aria-label="Approvals">
      <div className="section-head"><h2>Approvals</h2></div>
      <div className="card">
        <div className="setting-row"><span><strong>Codex requests from other devices</strong><small>Each request waits for your Accept on this device, unless an unattended permission under Permissions covers it.</small></span><span className="value">Always reviewed</span></div>
        <div className="setting-row"><span><strong>Plans</strong><small>A Plan runs only after you approve its exact revision, and each device admits it.</small></span><span className="value">Always reviewed</span></div>
        <div className="setting-row"><span><strong>Developer Mode terminals</strong><small>Each terminal request needs a separate Accept on the device it opens on.</small></span><span className="value">Always reviewed</span></div>
      </div>
    </section>
    <ProviderSettings />
  </>;
}

/** Host-local provider and runtime configuration for managed Plans. A
 * selection is a prerequisite only; it grants no Agent or step authority. */
function ProviderSettings() {
  const [runtimes, setRuntimes] = useState<ManagedRuntimeOption[] | null>(null);
  const [pendingRuntime, setPendingRuntime] = useState<ManagedRuntimeOption["runtimeId"] | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [providerSettings, setProviderSettings] = useState<ManagedWorkerProviderSettingsSnapshot | null>(null);
  const [pendingProviderAction, setPendingProviderAction] = useState<string | null>(null);
  const [providerError, setProviderError] = useState<string | null>(null);
  const [editingProvider, setEditingProvider] = useState<ManagedWorkerProviderSettings | "new" | null>(null);

  useEffect(() => {
    let cancelled = false;
    void listManagedRuntimeOptions()
      .then((options) => { if (!cancelled) setRuntimes(options); })
      .catch((error) => { if (!cancelled) setRuntimeError(error instanceof Error ? error.message : String(error)); });
    void getManagedWorkerProviderSettings()
      .then((settings) => { if (!cancelled) setProviderSettings(settings); })
      .catch((error) => { if (!cancelled) setProviderError(error instanceof Error ? error.message : String(error)); });
    return () => { cancelled = true; };
  }, []);

  async function selectRuntime(runtimeId: ManagedRuntimeOption["runtimeId"]) {
    if (pendingRuntime) return;
    setPendingRuntime(runtimeId);
    setRuntimeError(null);
    try {
      setRuntimes(await selectManagedExecuteRuntime(runtimeId));
    } catch (error) {
      setRuntimeError(error instanceof Error ? error.message : String(error));
    } finally {
      setPendingRuntime(null);
    }
  }

  async function runProviderAction(action: string, operation: () => Promise<ManagedWorkerProviderSettingsSnapshot>): Promise<boolean> {
    if (pendingProviderAction) return false;
    setPendingProviderAction(action);
    setProviderError(null);
    try {
      setProviderSettings(await operation());
      return true;
    } catch (error) {
      setProviderError(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      setPendingProviderAction(null);
    }
  }

  return <section className="section" aria-label="Plan provider">
    <div className="section-head"><h2>Plans</h2><p>The model provider and runtime this device uses for reviewed Plans. Selecting one is only a prerequisite: it grants no Agent or step authority.</p></div>
    <span className="subhead">Runtime</span>
    <div className="card"><div className="list">
      {runtimes ? runtimes.map((runtime) => <ListRow key={runtime.runtimeId} title={runtime.runtimeId === "python" ? "Python" : "Node.js"} meta="Found at a fixed location on this device; never from PATH." side={runtime.selected ? <Badge tone={runtime.selectionReady ? "ok" : "warn"}>{runtime.selectionReady ? "Selected" : "Re-select required"}</Badge> : <button type="button" className="btn btn-sm" disabled={!runtime.available || pendingRuntime !== null} onClick={() => void selectRuntime(runtime.runtimeId)}>{runtime.available ? "Select" : "Unavailable"}</button>} />) : <p className="list-empty">Checking…</p>}
    </div></div>
    {runtimeError ? <p className="error-text" role="alert">{runtimeError}</p> : null}
    <span className="subhead">Provider</span>
    <div className="card">
      <div className="list">
        {providerSettings ? providerSettings.providers.length === 0 ? <p className="list-empty">No provider configured.</p> : providerSettings.providers.map((provider) => <div key={provider.configRef.configDigest}>
          <ListRow title={`${provider.providerKind === "openai_compatible" ? "OpenAI-compatible" : provider.providerKind} · ${provider.model}`} meta={`${provider.baseUrl} · health ${provider.health} · ${provider.timeoutMillis} ms · ${provider.maxOutputTokens} tokens${provider.lastHealthCheckAt ? ` · checked ${new Date(provider.lastHealthCheckAt * 1000).toLocaleString()}` : ""}`} side={provider.selected ? <Badge tone="ok">Selected</Badge> : null} />
          <div className="row-wrap" style={{ padding: "0 16px 12px" }}>
            <button type="button" className="btn btn-sm" disabled={pendingProviderAction !== null} onClick={() => void runProviderAction(`health:${provider.configRef.configDigest}`, () => checkManagedWorkerProviderHealth(provider.configRef))}>Check connection</button>
            <button type="button" className="btn btn-sm" disabled={provider.selected || pendingProviderAction !== null} onClick={() => void runProviderAction(`select:${provider.configRef.configDigest}`, () => selectManagedWorkerProvider({ configRef: provider.configRef, model: provider.model }))}>Use for Plans</button>
            <button type="button" className="btn btn-sm" disabled={pendingProviderAction !== null} onClick={() => setEditingProvider(provider)}>Edit</button>
            <button type="button" className="btn btn-danger btn-sm" disabled={pendingProviderAction !== null} onClick={() => void runProviderAction(`delete:${provider.configRef.configDigest}`, () => deleteManagedWorkerProvider(provider.configRef))}>Remove</button>
          </div>
        </div>) : <p className="list-empty">Checking…</p>}
      </div>
      {editingProvider ? <ProviderForm provider={editingProvider} pending={pendingProviderAction !== null} onCancel={() => setEditingProvider(null)} onSubmit={(operation) => void runProviderAction("save", operation).then((saved) => { if (saved) setEditingProvider(null); })} />
        : <footer className="card-footer"><span className="spacer xsmall subtle">{providerSettings ? `Plan selection: ${providerSettings.managedSelection === "selected" ? "selected" : providerSettings.managedSelection === "stale" ? "re-select required" : "not configured"}.` : ""}</span><button type="button" className="btn btn-sm" disabled={pendingProviderAction !== null || !providerSettings} onClick={() => setEditingProvider("new")}><Icon name="plus" small />Add provider…</button></footer>}
    </div>
    {providerError ? <p className="error-text" role="alert">{providerError}</p> : null}
  </section>;
}

function ProviderForm({ provider, pending, onCancel, onSubmit }: { provider: ManagedWorkerProviderSettings | "new"; pending: boolean; onCancel: () => void; onSubmit: (operation: () => Promise<ManagedWorkerProviderSettingsSnapshot>) => void }) {
  const editing = provider !== "new";
  const [providerId, setProviderId] = useState(editing ? provider.configRef.providerId : "primary");
  const [baseUrl, setBaseUrl] = useState(editing ? provider.baseUrl : "https://api.openai.com/v1");
  const [model, setModel] = useState(editing ? provider.model : "");
  const [apiKey, setApiKey] = useState("");
  const [timeoutMillis, setTimeoutMillis] = useState(String(editing ? provider.timeoutMillis : 30_000));
  const [maxOutputTokens, setMaxOutputTokens] = useState(String(editing ? provider.maxOutputTokens : 1_024));

  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const timeout = Number(timeoutMillis);
    const maxTokens = Number(maxOutputTokens);
    if (editing) {
      const expectedRef = provider.configRef;
      onSubmit(() => updateManagedWorkerProvider({ expectedRef, baseUrl, model, replacementApiKey: apiKey || null, timeoutMillis: timeout, maxOutputTokens: maxTokens }));
    } else {
      onSubmit(() => createManagedWorkerProvider({ providerId, baseUrl, model, apiKey, timeoutMillis: timeout, maxOutputTokens: maxTokens }));
    }
    setApiKey("");
  }

  return <form className="add-form" onSubmit={submit}>
    <strong>{editing ? "Edit provider" : "Add an OpenAI-compatible provider"}</strong>
    <label className="field"><span className="field-label">Provider ID</span><input className="input" value={providerId} disabled={editing || pending} onChange={(event) => setProviderId(event.target.value)} /></label>
    <label className="field"><span className="field-label">Endpoint</span><input className="input input-mono" type="url" value={baseUrl} disabled={pending} onChange={(event) => setBaseUrl(event.target.value)} /></label>
    <label className="field"><span className="field-label">Model</span><input className="input" value={model} disabled={pending} onChange={(event) => setModel(event.target.value)} /></label>
    <label className="field"><span className="field-label">{editing ? "Replacement API key (optional)" : "API key"}</span><input className="input" type="password" autoComplete="new-password" required={!editing} value={apiKey} disabled={pending} onChange={(event) => setApiKey(event.target.value)} /></label>
    <div className="row-wrap">
      <label className="field" style={{ flex: "1 1 140px" }}><span className="field-label">Timeout (ms)</span><input className="input" type="number" min="1000" max="300000" value={timeoutMillis} disabled={pending} onChange={(event) => setTimeoutMillis(event.target.value)} /></label>
      <label className="field" style={{ flex: "1 1 140px" }}><span className="field-label">Max output tokens</span><input className="input" type="number" min="1" max="32768" value={maxOutputTokens} disabled={pending} onChange={(event) => setMaxOutputTokens(event.target.value)} /></label>
    </div>
    <p className="xsmall subtle">The key is sent only to this device's Pastey when you save, and cannot be viewed again.</p>
    <div className="row"><span className="spacer" /><button type="button" className="btn" disabled={pending} onClick={() => { setApiKey(""); onCancel(); }}>Cancel</button><button type="submit" className="btn btn-primary" disabled={pending}>Save</button></div>
  </form>;
}

function AdvancedSettings({ config, onConfigChange }: { config: AppConfig; onConfigChange: (config: AppConfig) => void }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  async function saveConfig(next: AppConfig) {
    if (pending) return;
    setPending(true); setError(null);
    try { onConfigChange(await updateConfig(next)); }
    catch (nextError) { setError(nextError instanceof Error ? nextError.message : String(nextError)); }
    finally { setPending(false); }
  }

  const setDevTools = (value: boolean) => saveConfig({ ...config, dev_tools_enabled: value });

  async function copyError() {
    try {
      const last = await copyLastError();
      setCopied(last ? "Copied the last error." : "No error recorded.");
    } catch (nextError) { setCopied(nextError instanceof Error ? nextError.message : String(nextError)); }
  }

  return <>
    <section className="section">
      <div className="card">
        <div className="setting-row"><span><strong>Developer tools</strong><small>Lets the transfer window override in Pastey's configuration take effect. It does not affect Developer Mode terminals, which always need the other device's Accept.</small></span><Switch label="Developer tools" checked={config.dev_tools_enabled} disabled={pending} onChange={(value) => void setDevTools(value)} /></div>
        <div className="setting-row"><span><strong>Transfer grouping</strong><small>How many small files are sent together. Dynamic adapts to the connection.</small></span>
          <select className="select select-sm" style={{ width: "auto" }} value={config.micro_flow_group_mode} disabled={pending} onChange={(event) => void saveConfig({ ...config, micro_flow_group_mode: event.target.value as AppConfig["micro_flow_group_mode"] })} aria-label="Transfer grouping"><option value="dynamic">Dynamic</option><option value="fixed">Fixed</option></select></div>
        <div className="setting-row"><span><strong>Transfer window</strong><small>{config.dev_tools_enabled ? "Chunks in flight per transfer. Automatic lets Pastey tune it." : "Chunks in flight per transfer. Turn on Developer tools to override Pastey's automatic tuning."}</small></span>
          <select className="select select-sm" style={{ width: "auto" }} value={config.transfer_window_override ? String(config.transfer_window_override) : "auto"} disabled={pending || !config.dev_tools_enabled} onChange={(event) => void saveConfig({ ...config, transfer_window_override: event.target.value === "auto" ? null : Number(event.target.value) })} aria-label="Transfer window"><option value="auto">Automatic</option>{[1, 2, 4, 8, 12, 16].map((value) => <option key={value} value={value}>{value}</option>)}</select></div>
      </div>
      {error ? <p className="error-text" role="alert">{error}</p> : null}
    </section>
    <section className="section">
      <div className="section-head"><h2>Diagnostics</h2><p>For reporting a problem. Logs stay on this device until you share them.</p></div>
      <div className="card">
        <div className="setting-row"><span><strong>Logs</strong><small>Open the folder with Pastey's logs.</small></span><button type="button" className="btn btn-sm" onClick={() => void openLogsFolder().catch(() => undefined)}><Icon name="folder" small />Open</button></div>
        <div className="setting-row"><span><strong>Last error</strong><small>{copied ?? "Copy the most recent error to the clipboard."}</small></span><button type="button" className="btn btn-sm" onClick={() => void copyError()}><Icon name="copy" small />Copy</button></div>
      </div>
    </section>
  </>;
}

function AboutSettings({ config }: { config: AppConfig }) {
  return <section className="section">
    <div className="card">
      <div className="setting-row"><span><strong>Version</strong></span><span className="value mono">{config.app_version}</span></div>
      <div className="setting-row"><span><strong>Application data</strong><small className="mono">{config.app_data_path || "—"}</small></span>{config.app_data_path ? <button type="button" className="btn btn-sm" onClick={() => void revealInFolder(config.app_data_path)}><Icon name="folder" small />Reveal</button> : null}</div>
      <div className="setting-row"><span><strong>Updates</strong><small>Opens the Pastey releases page.</small></span><button type="button" className="btn btn-sm" onClick={() => void checkForUpdates().catch(() => undefined)}>Check for updates</button></div>
    </div>
    <Notice tone="neutral">Native Agents own their workspace, sign-in, tools, sandbox and context. Pastey manages movement, authority, effects, completion and what it observes of their lifecycle.</Notice>
  </section>;
}

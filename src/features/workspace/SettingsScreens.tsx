import type { AppConfig } from "../../lib/types";
import type { NavigateWorkspace, WorkspaceRoute } from "./workspaceTypes";

interface SettingsProps { config: AppConfig; onNavigate: NavigateWorkspace }

export function SettingsOverview({ config, onNavigate }: SettingsProps) {
  return (
    <SettingsFrame title="Settings" description="Configure how Pastey works across your devices.">
      <div className="v2-settings-columns">
        <div>
          <SettingsGroup title="General">
            <SettingsRow title="Device name" description="Shown to other devices." value="This device" />
            <SettingsRow title="Theme" description="Uses the operating system appearance." value="System" />
            <SettingsRow title="Global shortcut" description="Quickly open Pastey." value={config.shortcut} />
          </SettingsGroup>
          <SettingsGroup title="Receiving">
            <SettingsRow title="Receiving folder" description="Current user-owned destination." value={config.inbox_dir ? "Custom" : "Default"} />
            <SettingsRow title="Save received files" description="Persist received files to the receiving folder." value={config.save_received_files_to_inbox ? "On" : "Off"} />
            <SettingsRow title="Save received images" description="Persist received images to the receiving folder." value={config.save_received_images_to_inbox ? "On" : "Off"} />
            <SettingsRow title="Open receiving folder" description="Open the current receiving destination." value="Unavailable" />
          </SettingsGroup>
          <SettingsGroup title="Transfers">
            <SettingsRow title="Max concurrent transfers" description="Pastey manages concurrency automatically." value={config.transfer_window_override ? String(config.transfer_window_override) : "Automatic"} />
            <SettingsRow title="Burn defaults" description="Default cleanup behavior for new Bridges." value={config.auto_burn_after_download ? "On" : "Off"} />
          </SettingsGroup>
        </div>
        <div>
          <SettingsGroup title="Approvals">
            <SettingsRow title="Agent Plan approval" description="Requester review is always required for an immutable Draft." value="Always required" status="ready" />
            <SettingsRow title="Developer Mode admission" description="Separate human approval for this terminal session." value="Always required" status="ready" />
          </SettingsGroup>
          <SettingsGroup title="Discovery"><SettingsRow title="Local network discovery" description="Current discovery state is Host-owned." value="Not projected" /></SettingsGroup>
          <SettingsGroup title="Tasks">
            <SettingsRow title="Task features" description="Reviewed cross-device managed Plan workflow." value="Available when Host ready" />
            <SettingsLink title="Task provider" description="Host-owned planning provider configuration." value="Open" route="settings-provider" onNavigate={onNavigate} />
          </SettingsGroup>
          <SettingsGroup title="Advanced">
            <SettingsLink title="Diagnostics" description="Logging and renderer-safe diagnostics." value="Open" route="settings-diagnostics" onNavigate={onNavigate} />
            <SettingsLink title="Transfer diagnostics" description="Existing transfer scheduler controls." value="Open" route="settings-transfer" onNavigate={onNavigate} />
            <SettingsLink title="Troubleshooting" description="Device state, errors, and local checks." value="Open" route="settings-troubleshooting" onNavigate={onNavigate} />
            <SettingsLink title="About" description="Version, app data, and updates." value="Open" route="settings-about" onNavigate={onNavigate} />
          </SettingsGroup>
        </div>
      </div>
    </SettingsFrame>
  );
}

export function DiagnosticsSettings({ onNavigate }: Pick<SettingsProps, "onNavigate">) {
  return <SettingsFrame title="Diagnostics" description="Runtime visibility and bounded diagnostic controls." onBack={() => onNavigate("settings")}><div className="v2-settings-narrow"><SettingsGroup title="Diagnostics logging"><SettingsRow title="Diagnostics logging" description="Bounded structured diagnostic mirrors." value="Backend-owned" /><SettingsRow title="Device diagnostics" description="Renderer-safe device profile is not wired to this screen." value="Unavailable" /></SettingsGroup><SettingsGroup title="Capability probe"><UnavailableRows names={["Python", "Git", "zsh", "bash"]} /></SettingsGroup><SettingsGroup title="Logs"><SettingsRow title="Logs folder" value="Unavailable" /><SettingsRow title="Copy last error" value="Unavailable" /></SettingsGroup></div></SettingsFrame>;
}

export function ProviderSettings({ onNavigate }: Pick<SettingsProps, "onNavigate">) {
  return <SettingsFrame title="Task Provider" description="Global runtime configuration for reviewed Bridge tasks." onBack={() => onNavigate("settings")}><div className="v2-settings-narrow"><SettingsGroup title="Agent runtime"><SettingsRow title="Enabled" description="Managed Plan support is controlled by the Host." value="State unavailable" /></SettingsGroup><SettingsGroup title="Provider"><section className="v2-provider-unavailable"><strong>Provider configuration is unavailable to this renderer.</strong><p>The current backend does not expose safe selected provider, model, health, or revocation state. Credentials remain hidden, and Pastey does not use a production fallback provider.</p><span>Not configured / unavailable</span></section></SettingsGroup><SettingsGroup title="Authority"><SettingsRow title="Provider selection" description="A ready provider proposes a Plan; it never grants Agent authority." value="No authority" /><SettingsRow title="Agent Plan approval" description="Requester approval remains independently required." value="Always required" /></SettingsGroup></div></SettingsFrame>;
}

export function TransferSettings({ config, onNavigate }: SettingsProps) {
  return <SettingsFrame title="Transfer Diagnostics" description="Advanced transport scheduling and pipeline controls." onBack={() => onNavigate("settings")}><div className="v2-settings-narrow"><SettingsGroup title="Transfer behaviour"><SettingsRow title="Transfer diagnostics" description="Advanced transfer behavior setting." value={config.micro_flow_group_mode === "dynamic" ? "Dynamic" : "Fixed"} /></SettingsGroup><SettingsGroup title="Pipeline depth"><SettingsRow title="Transfer window" description="Options: 1 · 2 · 4 · 8 · 16 · Custom" value={config.transfer_window_override ? String(config.transfer_window_override) : "Default / Auto"} /></SettingsGroup><p className="v2-settings-note">These controls tune the existing explicit transfer runtime; they do not create a shared filesystem or separate movement path.</p></div></SettingsFrame>;
}

export function TroubleshootingSettings({ onNavigate }: Pick<SettingsProps, "onNavigate">) {
  return <SettingsFrame title="Troubleshooting" description="Inspect this Host and run local diagnostics." onBack={() => onNavigate("settings")}><div className="v2-settings-narrow"><SettingsGroup title="This device"><SettingsRow title="Device" value="This device" /><SettingsRow title="Platform" value={navigator.platform || "Unavailable"} /><SettingsRow title="Power" value="Not projected" /></SettingsGroup><SettingsGroup title="Last local test"><UnavailableRows names={["Mode", "Quality", "Average"]} /></SettingsGroup><SettingsGroup title="Local benchmark"><SettingsRow title="Benchmark mode" description="Localhost baseline only; no LAN or internet." value="Unavailable" /><SettingsRow title="Duration" value="Target 5s standard" /></SettingsGroup><button type="button" className="v2-button primary" disabled>Run local test</button></div></SettingsFrame>;
}

export function AboutSettings({ config, onNavigate }: SettingsProps) {
  return <SettingsFrame title="About" description="Application information and maintenance." onBack={() => onNavigate("settings")}><div className="v2-settings-narrow"><SettingsGroup title="Pastey"><SettingsRow title="Version" value={config.app_version} /><SettingsRow title="Application data" description="Reveal" value={config.app_data_path || "Unavailable"} /></SettingsGroup><SettingsGroup title="Updates"><SettingsRow title="Update status" value="Not renderer-exposed" /><SettingsRow title="Check for updates" value="Unavailable" /></SettingsGroup><SettingsGroup title="Diagnostics"><SettingsLink title="Logs folder" value="Open" route="settings-diagnostics" onNavigate={onNavigate} /><SettingsLink title="Troubleshooting" value="Open" route="settings-troubleshooting" onNavigate={onNavigate} /></SettingsGroup></div></SettingsFrame>;
}

function SettingsFrame({ title, description, onBack, children }: { title: string; description: string; onBack?: () => void; children: React.ReactNode }) {
  return <section className="v2-screen v2-settings-screen"><header className="v2-settings-header">{onBack ? <button type="button" onClick={onBack}>Settings&nbsp; /&nbsp;</button> : null}<h1>{title}</h1><p>{description}</p></header><div className="v2-settings-body">{children}</div></section>;
}

function SettingsGroup({ title, children }: { title: string; children: React.ReactNode }) {
  return <section className="v2-settings-group"><h2>{title}</h2><div>{children}</div></section>;
}

function SettingsRow({ title, description, value, status }: { title: string; description?: string; value: string; status?: "ready" }) {
  return <div className="v2-settings-row"><span><strong>{title}</strong>{description ? <small>{description}</small> : null}</span><b>{status ? <i className="v2-dot connected" /> : null}{value}</b></div>;
}

function SettingsLink({ route, onNavigate, ...row }: { title: string; description?: string; value: string; route: WorkspaceRoute; onNavigate: NavigateWorkspace }) {
  return <button type="button" className="v2-settings-link" onClick={() => onNavigate(route)}><SettingsRow {...row} value={`${row.value} →`} /></button>;
}

function UnavailableRows({ names }: { names: string[] }) {
  return <>{names.map((name) => <SettingsRow key={name} title={name} value="Unavailable" />)}</>;
}

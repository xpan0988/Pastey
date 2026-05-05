import { open } from "@tauri-apps/plugin-dialog";
import { prettifyShortcut } from "../lib/format";
import { updateConfig } from "../lib/tauri";
import type { AppConfig } from "../lib/types";

interface SettingsPageProps {
  config: AppConfig;
  onConfigChange: (config: AppConfig) => void;
}

export function SettingsPage({ config, onConfigChange }: SettingsPageProps) {
  async function save(next: AppConfig) {
    const saved = await updateConfig(next);
    onConfigChange(saved);
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

  return (
    <div className="stack">
      <div className="panel subtle-stack">
        <h2>Settings</h2>
        <p className="muted">Only local utility settings live here. No cloud sync, no account data.</p>

        <label className="field">
          <span>Default expiry</span>
          <select
            value={config.default_expiry_minutes}
            onChange={(event) =>
              void save({ ...config, default_expiry_minutes: Number(event.target.value) })
            }
          >
            <option value={5}>5 min</option>
            <option value={15}>15 min</option>
            <option value={60}>1 hour</option>
            <option value={1440}>24 hours</option>
          </select>
        </label>

        <label className="toggle-row">
          <span>Keep automatic cleanup enabled</span>
          <input
            type="checkbox"
            checked={config.auto_burn_after_download}
            onChange={(event) =>
              void save({ ...config, auto_burn_after_download: event.target.checked })
            }
          />
        </label>

        <div className="field">
          <span>Inbox folder</span>
          <div className="row spread">
            <code className="path-box">{config.inbox_dir ?? "(using app inbox)"}</code>
            <button className="ghost-button" onClick={chooseInbox}>
              Choose
            </button>
          </div>
        </div>

        <div className="field">
          <span>Global shortcut</span>
          <code className="path-box">{prettifyShortcut(config.shortcut)}</code>
        </div>

        <div className="field">
          <span>App data path</span>
          <code className="path-box">{config.app_data_path}</code>
        </div>
      </div>
    </div>
  );
}

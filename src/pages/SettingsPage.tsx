import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { prettifyShortcut } from "../lib/format";
import { updateConfig } from "../lib/tauri";
import type { AppConfig } from "../lib/types";

interface SettingsPageProps {
  config: AppConfig;
  onConfigChange: (config: AppConfig) => void;
}

export function SettingsPage({ config, onConfigChange }: SettingsPageProps) {
  const presetSpeeds = [10, 50, 100];
  const [customSpeed, setCustomSpeed] = useState(config.speed_limit_mbps && !presetSpeeds.includes(config.speed_limit_mbps) ? String(config.speed_limit_mbps) : "");
  const speedValue = config.speed_limit_mbps
    ? presetSpeeds.includes(config.speed_limit_mbps)
      ? String(config.speed_limit_mbps)
      : "custom"
    : "unlimited";

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

        <div className="settings-grid">
          <div className="meta-card">
            <span className="meta-label">Max file size</span>
            <strong>10GB</strong>
          </div>
          <div className="meta-card">
            <span className="meta-label">App version</span>
            <strong>{config.app_version}</strong>
          </div>
        </div>

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

        <label className="field">
          <span>Transfer speed limit</span>
          <select
            value={speedValue}
            onChange={(event) =>
              void save({
                ...config,
                speed_limit_mbps:
                  event.target.value === "unlimited"
                    ? null
                    : event.target.value === "custom"
                      ? Number(customSpeed) || 25
                      : Number(event.target.value)
              })
            }
          >
            <option value="unlimited">Unlimited</option>
            <option value="10">10 MB/s</option>
            <option value="50">50 MB/s</option>
            <option value="100">100 MB/s</option>
            <option value="custom">Custom MB/s</option>
          </select>
        </label>

        {speedValue === "custom" ? (
          <label className="field">
            <span>Custom speed</span>
            <input
              type="number"
              min={1}
              step={1}
              value={customSpeed}
              placeholder="25"
              onChange={(event) => setCustomSpeed(event.target.value)}
              onBlur={() => void save({ ...config, speed_limit_mbps: Number(customSpeed) || 25 })}
            />
          </label>
        ) : null}

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

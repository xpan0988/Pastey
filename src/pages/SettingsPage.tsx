import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { prettifyShortcut } from "../lib/format";
import { checkForUpdates, copyLastError, openLogsFolder, updateConfig } from "../lib/tauri";
import type { AppConfig } from "../lib/types";

interface SettingsPageProps {
  config: AppConfig;
  onConfigChange: (config: AppConfig) => void;
}

const PRESET_SPEEDS = [10, 50, 100];

export function SettingsPage({ config, onConfigChange }: SettingsPageProps) {
  const [speedValue, setSpeedValue] = useState(speedSelectionFromConfig(config.speed_limit_mbps, PRESET_SPEEDS));
  const [customSpeed, setCustomSpeed] = useState(customSpeedFromConfig(config.speed_limit_mbps, PRESET_SPEEDS));
  const [logActionMessage, setLogActionMessage] = useState<string | null>(null);

  useEffect(() => {
    setSpeedValue(speedSelectionFromConfig(config.speed_limit_mbps, PRESET_SPEEDS));
    setCustomSpeed(customSpeedFromConfig(config.speed_limit_mbps, PRESET_SPEEDS));
  }, [config.speed_limit_mbps]);

  async function save(next: AppConfig) {
    const saved = await updateConfig(next);
    onConfigChange(saved);
  }

  async function saveSpeedLimit(nextValue: string) {
    setSpeedValue(nextValue);
    if (nextValue === "custom") {
      const currentCustom = validCustomSpeed(customSpeed) ?? 25;
      setCustomSpeed(String(currentCustom));
      await save({ ...config, speed_limit_mbps: currentCustom });
      return;
    }

    await save({
      ...config,
      speed_limit_mbps: nextValue === "unlimited" ? null : Number(nextValue)
    });
  }

  async function saveCustomSpeed() {
    const nextCustom = validCustomSpeed(customSpeed);
    if (!nextCustom) {
      setCustomSpeed(customSpeedFromConfig(config.speed_limit_mbps, PRESET_SPEEDS) || "25");
      return;
    }

    setCustomSpeed(String(nextCustom));
    setSpeedValue("custom");
    await save({ ...config, speed_limit_mbps: nextCustom });
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
            <strong>pastey {config.app_version}</strong>
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
            onChange={(event) => void saveSpeedLimit(event.target.value)}
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
              onBlur={() => void saveCustomSpeed()}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.currentTarget.blur();
                }
              }}
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

        <div className="field">
          <span>Diagnostics</span>
          <div className="row gap wrap">
            <button className="ghost-button" onClick={() => void handleOpenLogsFolder()}>
              Open Logs Folder
            </button>
            <button className="ghost-button" onClick={() => void handleCopyLastError()}>
              Copy Last Error
            </button>
            <button className="ghost-button" onClick={() => void handleCheckForUpdates()}>
              Check for updates
            </button>
          </div>
          {logActionMessage ? <p className="muted">{logActionMessage}</p> : null}
        </div>
      </div>
    </div>
  );
}

function speedSelectionFromConfig(value: number | null | undefined, presets: number[]): string {
  if (!value || !Number.isFinite(value) || value <= 0) return "unlimited";
  return presets.includes(value) ? String(value) : "custom";
}

function customSpeedFromConfig(value: number | null | undefined, presets: number[]): string {
  if (!value || !Number.isFinite(value) || value <= 0 || presets.includes(value)) return "";
  return String(value);
}

function validCustomSpeed(value: string): number | null {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) return null;
  return Math.min(10_000, Math.max(1, parsed));
}

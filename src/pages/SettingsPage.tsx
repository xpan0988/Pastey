import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { prettifyShortcut } from "../lib/format";
import { checkForUpdates, copyLastError, openLogsFolder, updateConfig } from "../lib/tauri";
import type { AppConfig } from "../lib/types";

interface SettingsPageProps {
  config: AppConfig;
  onConfigChange: (config: AppConfig) => void;
}

const PRESET_WINDOWS = [1, 2, 4, 8, 16];
const DEFAULT_WINDOW = 8;

export function SettingsPage({ config, onConfigChange }: SettingsPageProps) {
  const [windowValue, setWindowValue] = useState(windowSelectionFromConfig(config.transfer_window_override, PRESET_WINDOWS));
  const [customWindow, setCustomWindow] = useState(customWindowFromConfig(config.transfer_window_override, PRESET_WINDOWS));
  const [logActionMessage, setLogActionMessage] = useState<string | null>(null);

  useEffect(() => {
    setWindowValue(windowSelectionFromConfig(config.transfer_window_override, PRESET_WINDOWS));
    setCustomWindow(customWindowFromConfig(config.transfer_window_override, PRESET_WINDOWS));
  }, [config.transfer_window_override]);

  async function save(next: AppConfig) {
    const saved = await updateConfig(next);
    onConfigChange(saved);
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

        {config.dev_tools_enabled ? (
          <>
            <label className="field">
              <span>Transfer window</span>
              <select
                value={windowValue}
                onChange={(event) => void saveTransferWindow(event.target.value)}
              >
                <option value="default">Default / Auto (window 8)</option>
                <option value="1">window 1</option>
                <option value="2">window 2</option>
                <option value="4">window 4</option>
                <option value="8">window 8</option>
                <option value="16">window 16</option>
                <option value="custom">Custom window</option>
              </select>
            </label>

            {windowValue === "custom" ? (
              <label className="field">
                <span>Custom window</span>
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
                    if (event.key === "Enter") {
                      event.currentTarget.blur();
                    }
                  }}
                />
              </label>
            ) : null}
          </>
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

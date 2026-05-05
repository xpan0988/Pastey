export function formatCode(code?: string | null): string {
  if (!code) return "--------";
  const compact = code.replace(/\D/g, "").slice(0, 8);
  return compact.length === 8
    ? `${compact.slice(0, 4)}-${compact.slice(4)}`
    : compact;
}

export function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = size / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

export function formatRelativeExpiry(expiresAt: number): string {
  const diff = Math.max(0, expiresAt * 1000 - Date.now());
  const minutes = Math.round(diff / 60000);
  if (minutes < 1) return "less than a minute";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} hr`;
  return `${Math.round(hours / 24)} day`;
}

export function formatTimestamp(timestamp: number): string {
  return new Date(timestamp * 1000).toLocaleString();
}

export function prettifyShortcut(shortcut: string): string {
  return shortcut
    .replace("CommandOrControl", navigator.platform.includes("Mac") ? "Cmd" : "Ctrl")
    .replace(/\+/g, " + ");
}

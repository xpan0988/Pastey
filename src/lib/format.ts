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

export function formatSpeed(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "0 MB/s";
  return `${formatBytes(bytesPerSecond)}/s`;
}

export function formatDuration(seconds?: number | null): string {
  if (!seconds || !Number.isFinite(seconds) || seconds <= 0) return "--:--";
  const rounded = Math.max(0, Math.round(seconds));
  const minutes = Math.floor(rounded / 60);
  const remainingSeconds = rounded % 60;
  if (minutes >= 60) {
    const hours = Math.floor(minutes / 60);
    const remainingMinutes = minutes % 60;
    return `${hours}:${String(remainingMinutes).padStart(2, "0")}:${String(remainingSeconds).padStart(2, "0")}`;
  }
  return `${String(minutes).padStart(2, "0")}:${String(remainingSeconds).padStart(2, "0")}`;
}

export function fileTypeLabel(fileName?: string | null, mimeType?: string | null): string {
  const normalizedMime = mimeType?.trim().toLowerCase() ?? "";
  const extension = fileExtension(fileName);

  if (normalizedMime.startsWith("image/")) return "Image";
  if (normalizedMime.startsWith("video/")) return "Video";
  if (normalizedMime === "application/pdf") return "PDF";
  if (normalizedMime === "application/zip" || isArchiveExtension(extension)) return "Archive";

  return "File";
}

function fileExtension(fileName?: string | null): string {
  const name = fileName?.trim().toLowerCase();
  if (!name) return "";
  const lastDot = name.lastIndexOf(".");
  if (lastDot <= 0 || lastDot === name.length - 1) return "";
  return name.slice(lastDot + 1);
}

function isArchiveExtension(extension: string): boolean {
  return ["zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz"].includes(extension);
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

/** Presentation for device facts and measured link performance. Every number
 * shown comes from a real measurement or a Host report, with what was
 * measured and when; nothing here estimates or extrapolates. */
import type { BenchmarkMode, DeviceCapabilities, DeviceProfile, LinkBenchmarkResult } from "./types";

export type FactRow = [label: string, value: string];

/** What a measurement exercised, in the user's terms. */
export function benchmarkScope(mode: BenchmarkMode, overBridge: boolean): string {
  if (overBridge) {
    return "Pastey's transfer pipeline over this Bridge's current connection, in memory (no disk reads or writes)";
  }
  return mode === "raw_memory"
    ? "Raw in-memory throughput on this device only (no network)"
    : "Pastey's transfer pipeline on this device only, over loopback (no network)";
}

export function benchmarkModeLabel(mode: BenchmarkMode): string {
  return mode === "raw_memory" ? "Raw memory" : "Pastey pipeline";
}

/** One measured result: the headline average and the facts it came with. */
export function benchmarkFacts(result: LinkBenchmarkResult): { headline: string; facts: FactRow[] } {
  const facts: FactRow[] = [
    ["Peak", `${result.peak_MBps.toFixed(1)} MB/s`],
    ["Quality", result.link_quality],
    ["Measured for", `${(result.duration_ms / 1000).toFixed(1)} s, ${formatMegabytes(result.total_bytes)}`],
  ];
  if (result.latency_ms != null) facts.splice(1, 0, ["Latency", `${result.latency_ms.toFixed(1)} ms`]);
  if (result.failed_chunks > 0 || result.duplicate_chunks > 0) {
    facts.push(["Chunk errors", `${result.failed_chunks} failed, ${result.duplicate_chunks} duplicate`]);
  }
  facts.push(["Measured", measuredAgo(result.timestamp)]);
  return { headline: `${result.average_MBps.toFixed(1)} MB/s average`, facts };
}

export function measuredAgo(timestampSeconds: number, now = Date.now()): string {
  const seconds = Math.max(0, Math.round(now / 1000 - timestampSeconds));
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
  if (seconds < 2 * 86400) return `${Math.round(seconds / 3600)} h ago`;
  return `${Math.round(seconds / 86400)} days ago`;
}

function formatMegabytes(bytes: number): string {
  return bytes >= 1024 * 1024 * 1024 ? `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB` : `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Device facts as reported, leaving out what the Host did not report. */
export function deviceProfileFacts(profile: DeviceProfile): FactRow[] {
  const rows: FactRow[] = [
    ["Name", profile.device_name],
    ["System", [profile.platform, profile.os_version].filter(Boolean).join(" ")],
    ["Architecture", profile.arch],
  ];
  if (profile.cpu_name) {
    const cores = profile.cpu_physical_core_count ?? profile.cpu_core_count;
    rows.push(["Processor", `${profile.cpu_name}${cores ? ` · ${cores} cores` : ""}${profile.cpu_logical_processor_count ? ` · ${profile.cpu_logical_processor_count} threads` : ""}`]);
  }
  if (profile.memory_total_gb != null) rows.push(["Memory", `${profile.memory_total_gb.toFixed(0)} GB`]);
  if (profile.gpu_names.length) rows.push(["Graphics", profile.gpu_names.join(", ")]);
  rows.push(["Power", powerLabel(profile)]);
  return rows;
}

function powerLabel(profile: DeviceProfile): string {
  const state = String(profile.power_state).replace(/_/g, " ");
  const label = state.charAt(0).toUpperCase() + state.slice(1);
  return profile.battery_percent != null ? `${label} · ${Math.round(profile.battery_percent)}%` : label;
}

/** Runtimes the Host found, available ones first. */
export function runtimeFacts(capabilities: DeviceCapabilities): FactRow[] {
  return [...capabilities.runtimes]
    .sort((left, right) => Number(right.available) - Number(left.available) || left.name.localeCompare(right.name))
    .map((runtime) => [runtime.name, runtime.available ? runtime.version ?? "Available" : "Not found"]);
}

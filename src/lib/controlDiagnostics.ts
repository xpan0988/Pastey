/** Presentation of one Bridge's Room Control diagnostics. Rust counts; these
 * helpers only say what the counts mean, without guessing a cause. */
import type { ControlClass, ControlRefusal, RoomControlDiagnostics } from "./tauri";

const CLASS_LABELS: Record<ControlClass, string> = {
  capability_query: "Capability checks asked of this device",
  capability_answer: "Answers to this device's capability checks",
  native_agent: "Agent requests, statuses and session queries",
  bridge_plan: "Bridge Plans and Find file",
  membership: "Bridge membership",
};

const REASON_LABELS: Record<string, string> = {
  rate_limited: "refused for its rate budget (nothing was done)",
  event_replayed: "refused as already received",
  refused_after_admission: "refused as invalid or not allowed",
  not_sent: "not sent (no current session)",
  review_capacity: "refused: too many requests waiting for Accept",
  refused: "refused by the other device before it did anything",
  replayed: "already received by the other device",
  expired: "expired before delivery",
  inbox_full: "refused by an older Pastey whose inbox was full",
  rejected: "rejected by the other device",
  transport: "no answer (outcome unknown)",
};

export function controlClassLabel(value: ControlClass): string {
  return CLASS_LABELS[value] ?? value;
}

export function controlReasonLabel(reason: string): string {
  return REASON_LABELS[reason] ?? reason.replace(/_/g, " ");
}

function ago(at: number, now: number): string {
  const seconds = Math.max(0, Math.round(now - at));
  if (seconds < 60) return `${seconds} s ago`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
  return `${Math.round(seconds / 3600)} h ago`;
}

export function refusalLine(refusal: ControlRefusal | null, now: number): string | null {
  if (!refusal) return null;
  return `${controlClassLabel(refusal.class)}: ${controlReasonLabel(refusal.reason)}, ${ago(refusal.at, now)}`;
}

export type ControlHealth = { tone: "ok" | "warn"; summary: string };

/** One sentence on the channel now, from facts only. */
export function controlHealth(diagnostics: RoomControlDiagnostics, now = diagnostics.observedAt): ControlHealth {
  const pressed = diagnostics.classes.filter((entry) => entry.backpressureSince !== null);
  if (pressed.length) {
    const first = pressed[0];
    return { tone: "warn", summary: `${controlClassLabel(first.class)} are being refused for their rate budget, since ${ago(first.backpressureSince!, now)} (${first.backpressureRefused} so far). Nothing refused was acted on; it recovers on its own as the rate drops.` };
  }
  if (diagnostics.historyStale > 0) {
    return { tone: "warn", summary: `${diagnostics.historyStale} kept event(s) are past their age and were not pruned.` };
  }
  if (diagnostics.nativeReviews.pending >= diagnostics.nativeReviews.limit) {
    return { tone: "warn", summary: `${diagnostics.nativeReviews.pending} requests are waiting for Accept here, the most this Bridge may hold. New ones are refused until one is answered or expires.` };
  }
  return { tone: "ok", summary: "Accepting control messages. Nothing is backed up." };
}

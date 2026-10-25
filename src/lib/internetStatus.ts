/** What Pastey says about internet connectivity. The carrier only moves a
 * Bridge's already-encrypted traffic between networks; it never decides
 * which device is trusted or what it may do. Rust validates the relay
 * address again and is authoritative. */
import type { BridgeInternetStatus, InternetConnectivity } from "./tauri";

const MAX_RELAY_URL_LENGTH = 512;

/** The same rules Rust applies to a relay address. */
export function relayUrlProblem(input: string): string | null {
  const url = input.trim();
  if (!url) return null;
  if (url.length > MAX_RELAY_URL_LENGTH) return "That address is too long.";
  if (/\s/.test(url) || !url.startsWith("https://")) return "Use the relay's https:// address.";
  return null;
}

/** This device's setting, in one line. */
export function internetSettingLine(view: InternetConnectivity | null): { text: string; tone: "ok" | "warn" | "neutral" } {
  if (!view) return { text: "Checking…", tone: "neutral" };
  if (!view.enabled) return { text: "Off. Paired devices reach this one only on this network or a private network you set per Bridge.", tone: "neutral" };
  if (view.running) return { text: view.relayUrl ? "On, with your relay." : "On, with n0's public relays.", tone: "ok" };
  return { text: view.problem ? `On, but not running: ${view.problem}` : "On, starting…", tone: "warn" };
}

/** How one Bridge's peer is reached across the internet now. */
export function bridgeInternetLine(status: BridgeInternetStatus | null): string {
  if (!status) return "Checking…";
  if (!status.carrierRunning) return "Not running on this device (Settings → General).";
  if (!status.peerReachableByCarrier) return "The other device has not turned it on, or has not connected since.";
  if (status.path === "direct") return "In use: directly between the two devices.";
  if (status.path === "relayed") return "In use: through a relay, which sees only encrypted traffic.";
  if (status.path === "unknown") return "In use: finding the best path…";
  return "Ready. Used when the other device is not on this network.";
}

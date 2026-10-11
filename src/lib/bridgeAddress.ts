/** Reaching a Bridge's other device on another network through a
 * user-managed private or overlay network (for example Tailscale or
 * WireGuard). The address only says where to look; the Bridge code, room
 * identity and session keys decide whether a Bridge forms. Rust validates
 * again and is authoritative. */
import type { RoomInfo } from "./types";

const MAX_ADDRESS_LENGTH = 253;

/** The same rules Rust applies, checked early so typing mistakes are
 * explained before anything is sent. */
export function peerAddressProblem(input: string): string | null {
  const address = input.trim();
  if (!address) return "Enter the other device's network address or name.";
  if (address.length > MAX_ADDRESS_LENGTH) return "That address is too long.";
  if (/[\s/@]|:\/\/|:/.test(address)) {
    return "Enter only the device's address or name, without a port, path or scheme.";
  }
  if (/^[0-9.]+$/.test(address)) {
    const octets = address.split(".");
    if (octets.length !== 4 || octets.some((part) => part === "" || Number(part) > 255)) {
      return "That is not a valid IPv4 address.";
    }
    const [a, b] = octets.map(Number);
    const privateOrOverlay = a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 100 && b >= 64 && b <= 127);
    if (!privateOrOverlay) {
      return "Use an address on a private or overlay network (for example Tailscale or WireGuard). Pastey must not be exposed directly to the internet.";
    }
    return null;
  }
  const labels = address.split(".");
  if (labels.some((label) => !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(label))) {
    return "Enter only the device's address or name, without a port, path or scheme.";
  }
  // Resolvers read `0x08080808` or `0x7f.1` as addresses without a lookup.
  if (labels.every((label) => /^(?:[0-9]+|0x[0-9a-f]*)$/i.test(label))) {
    return "Enter an IPv4 address as four decimal numbers, or the device's network name.";
  }
  return null;
}

/** A join or reconnect failure in terms the user can act on. */
export function bridgeConnectionErrorMessage(error: unknown, viaAddress: boolean): string {
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  if (/must not be exposed|without a port|address or name|IPv4 address|cannot be another device|no IPv4 address|could not be resolved/i.test(message)) {
    return message;
  }
  if (/did not answer|timed out while waiting for room discovery/i.test(message)) {
    return viaAddress
      ? "The other device did not answer at that address. Check that Pastey is open there with this Bridge, that both devices are connected to the same overlay network, and that the code has not expired or been burned."
      : "No device on this network answered for that code. Check the code, that it has not expired or been burned, and that Pastey is open on the other device. If it is on another network, enter its overlay address.";
  }
  if (/identity does not match|Bridge is burned/i.test(message)) {
    return "That address answered for a different or ended Bridge. Check the address and the code.";
  }
  if (/delivery|connect|Network|unreachable/i.test(message)) {
    return viaAddress
      ? "The other device was found but could not be reached to finish joining. Check that its firewall allows Pastey on the overlay network."
      : "The other device was found but could not be reached to finish joining.";
  }
  return message || "Pastey could not join this Bridge.";
}

/** What the Bridge's connection to its other device truthfully is now. */
export function bridgeReachability(room: RoomInfo | null): "connected" | "reconnecting" | "disconnected" | "none" {
  const peers = room?.peers ?? [];
  if (peers.length === 0) return "none";
  if (peers.some((peer) => peer.liveness === "connected")) return "connected";
  if (peers.some((peer) => peer.liveness === "reconnecting")) return "reconnecting";
  return "disconnected";
}

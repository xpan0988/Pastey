export const ACTIVE_BRIDGE_POLL_INTERVAL_MS = 1_600;

export function bridgePollingIntervalMs(active: boolean): number | null {
  return active ? ACTIVE_BRIDGE_POLL_INTERVAL_MS : null;
}

export function reconcileSelectedPeerIds(
  current: readonly string[],
  routeablePeerIds: readonly string[],
): readonly string[] {
  const routeable = new Set(routeablePeerIds);
  const retained = current.filter((peerId) => routeable.has(peerId));
  const next = retained.length > 0
    ? retained
    : routeablePeerIds[0]
      ? [routeablePeerIds[0]]
      : [];
  return sameStringValues(current, next) ? current : next;
}

function sameStringValues(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

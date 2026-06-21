import {
  assertRouteAllowedForContentKind,
  bridgePeerSessionId,
  getExplicitTargetPeerIds,
  isBroadcastRoute,
  validateBridgeRoute,
  type BridgeContentKind,
  type BridgePeerSessionId,
  type BridgeRoute,
} from "./bridgeRouting";

export type { BridgePeerSessionId } from "./bridgeRouting";
export { bridgePeerSessionId } from "./bridgeRouting";

export type BridgePeerDisplayName = string & { readonly __bridgePeerDisplayName: unique symbol };
export type BridgePeerJoinMethod = "nearby_accept" | "manual_code";
export type BridgePeerLiveness = "connected" | "disconnected" | "left" | "stale";

export interface BridgePeerSession {
  readonly bridgeSessionId: string;
  readonly peerSessionId: BridgePeerSessionId;
  readonly displayName: BridgePeerDisplayName;
  readonly joinMethod: BridgePeerJoinMethod;
  readonly liveness: BridgePeerLiveness;
  readonly accepted: true;
  readonly sessionVerified: true;
  readonly currentSessionOnly: true;
  readonly isLocalSelf?: boolean;
}

export interface BridgePeerCollection {
  readonly bridgeSessionId: string;
  readonly peers: readonly BridgePeerSession[];
}

export type BridgePeerSessionNormalizationResult =
  | { ok: true; peer: BridgePeerSession; errors: [] }
  | { ok: false; errors: string[] };

export type BridgePeerCollectionValidationResult =
  | { valid: true; collection: BridgePeerCollection; errors: [] }
  | { valid: false; errors: string[] };

export type DefaultBridgeRouteResult =
  | {
      status: "selected_peer";
      route: BridgeRoute;
      peer: BridgePeerSession;
      routeablePeerIds: readonly BridgePeerSessionId[];
    }
  | {
      status: "no_route";
      reason: "no_routeable_peer";
      routeablePeerIds: readonly BridgePeerSessionId[];
      errors: string[];
    }
  | {
      status: "requires_explicit_selection";
      reason: "multiple_routeable_peers";
      routeablePeerIds: readonly BridgePeerSessionId[];
      errors: string[];
    };

export class BridgePeerRouteError extends Error {
  readonly errors: readonly string[];

  constructor(errors: readonly string[]) {
    super(errors.join(" "));
    this.name = "BridgePeerRouteError";
    this.errors = errors;
  }
}

interface RouteablePeerOptions {
  allowLocalSelf?: boolean;
}

interface RouteCompatibilityOptions extends RouteablePeerOptions {
  contentKind?: BridgeContentKind;
}

const PEER_REQUIRED_FIELDS = [
  "bridgeSessionId",
  "peerSessionId",
  "displayName",
  "joinMethod",
  "liveness",
  "accepted",
  "sessionVerified",
  "currentSessionOnly",
];
const PEER_OPTIONAL_FIELDS = ["isLocalSelf"];
const COLLECTION_REQUIRED_FIELDS = ["bridgeSessionId", "peers"];
const UNSUPPORTED_AUTHORITY_FIELDS = [
  "authority",
  "authorized",
  "automaticApproval",
  "consent",
  "consentId",
  "durableIdentity",
  "durableIdentityRef",
  "durableTrustedDevice",
  "executionAuthority",
  "history",
  "historyId",
  "persistentIdentity",
  "reusableTrust",
  "trust",
  "trustedDeviceId",
  "trustedPeerIds",
];

export function bridgePeerDisplayName(value: string): BridgePeerDisplayName {
  const normalized = normalizeIdentifier(value);
  if (normalized === null) {
    throw new Error("Bridge peer display name must be a non-empty string.");
  }
  return normalized as BridgePeerDisplayName;
}

export function normalizeBridgePeerSession(value: unknown): BridgePeerSessionNormalizationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { ok: false, errors: ["Bridge peer session must be an object."] };
  }
  rejectUnsupportedAuthorityFields(value, "Bridge peer session", errors);
  requireExactFields(value, PEER_REQUIRED_FIELDS, PEER_OPTIONAL_FIELDS, "Bridge peer session", errors);

  const bridgeSessionId = normalizeIdentifier(value.bridgeSessionId);
  const peerSessionId = normalizeIdentifier(value.peerSessionId);
  const displayName = normalizeIdentifier(value.displayName);
  if (bridgeSessionId === null) errors.push("Bridge peer session requires a non-empty bridgeSessionId.");
  if (peerSessionId === null) errors.push("Bridge peer session requires a non-empty peerSessionId.");
  if (displayName === null) errors.push("Bridge peer session requires a non-empty displayName.");
  if (value.joinMethod !== "nearby_accept" && value.joinMethod !== "manual_code") {
    errors.push("Bridge peer joinMethod must be nearby_accept or manual_code.");
  }
  if (!["connected", "disconnected", "left", "stale"].includes(String(value.liveness))) {
    errors.push("Bridge peer liveness is unsupported.");
  }
  if (value.accepted !== true) {
    errors.push("Bridge peer session requires accepted true for the current session.");
  }
  if (value.sessionVerified !== true) {
    errors.push("Bridge peer session requires sessionVerified true for the current session.");
  }
  if (value.currentSessionOnly !== true) {
    errors.push("Bridge peer session requires currentSessionOnly true.");
  }
  if ("isLocalSelf" in value && typeof value.isLocalSelf !== "boolean") {
    errors.push("Bridge peer session isLocalSelf must be boolean when present.");
  }
  const isLocalSelf = typeof value.isLocalSelf === "boolean" ? value.isLocalSelf : undefined;

  return errors.length === 0 &&
    bridgeSessionId !== null &&
    peerSessionId !== null &&
    displayName !== null &&
    (value.joinMethod === "nearby_accept" || value.joinMethod === "manual_code") &&
    ["connected", "disconnected", "left", "stale"].includes(String(value.liveness))
    ? {
        ok: true,
        peer: {
          bridgeSessionId,
          peerSessionId: bridgePeerSessionId(peerSessionId),
          displayName: bridgePeerDisplayName(displayName),
          joinMethod: value.joinMethod,
          liveness: value.liveness as BridgePeerLiveness,
          accepted: true,
          sessionVerified: true,
          currentSessionOnly: true,
          ...(isLocalSelf === undefined ? {} : { isLocalSelf }),
        },
        errors: [],
      }
    : { ok: false, errors: unique(errors) };
}

export function validateBridgePeerCollection(value: unknown): BridgePeerCollectionValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Bridge peer collection must be an object."] };
  }
  rejectUnsupportedAuthorityFields(value, "Bridge peer collection", errors);
  requireExactFields(value, COLLECTION_REQUIRED_FIELDS, [], "Bridge peer collection", errors);

  const bridgeSessionId = normalizeIdentifier(value.bridgeSessionId);
  if (bridgeSessionId === null) {
    errors.push("Bridge peer collection requires a non-empty bridgeSessionId.");
  }
  if (!Array.isArray(value.peers)) {
    errors.push("Bridge peer collection requires peers.");
    return { valid: false, errors: unique(errors) };
  }

  const peers: BridgePeerSession[] = [];
  for (const candidate of value.peers) {
    const normalized = normalizeBridgePeerSession(candidate);
    if (normalized.ok) {
      peers.push(normalized.peer);
    } else {
      errors.push(...normalized.errors);
    }
  }

  const ids = peers.map((peer) => peer.peerSessionId);
  if (new Set(ids).size !== ids.length) {
    errors.push("Bridge peer collection rejects duplicate current-session peer ids.");
  }
  if (bridgeSessionId !== null) {
    for (const peer of peers) {
      if (peer.bridgeSessionId !== bridgeSessionId) {
        errors.push("Bridge peer collection rejects peers from another bridgeSessionId.");
      }
    }
  }

  return errors.length === 0 && bridgeSessionId !== null
    ? { valid: true, collection: { bridgeSessionId, peers }, errors: [] }
    : { valid: false, errors: unique(errors) };
}

export function getRouteableBridgePeers(
  collection: BridgePeerCollection,
  options: RouteablePeerOptions = {},
): readonly BridgePeerSession[] {
  return collection.peers.filter((peer) => isPeerRouteable(peer, options));
}

export function findBridgePeerBySessionId(
  collection: BridgePeerCollection,
  peerSessionId: BridgePeerSessionId,
): BridgePeerSession | undefined {
  return collection.peers.find((peer) => peer.peerSessionId === peerSessionId);
}

export function assertPeerCanBeRouteTarget(
  peer: BridgePeerSession,
  options: RouteablePeerOptions = {},
): void {
  const errors = peerRouteabilityErrors(peer, options);
  if (errors.length > 0) {
    throw new BridgePeerRouteError(errors);
  }
}

export function assertRouteTargetsKnownPeers(route: BridgeRoute, collection: BridgePeerCollection): void {
  const errors = routeKnownPeerErrors(route, collection);
  if (errors.length > 0) {
    throw new BridgePeerRouteError(errors);
  }
}

export function assertRouteTargetsRouteablePeers(
  route: BridgeRoute,
  collection: BridgePeerCollection,
  options: RouteablePeerOptions = {},
): void {
  const errors = routeRouteabilityErrors(route, collection, options);
  if (errors.length > 0) {
    throw new BridgePeerRouteError(errors);
  }
}

export function assertRouteCompatibleWithPeerCollection(
  route: BridgeRoute,
  collection: BridgePeerCollection,
  options: RouteCompatibilityOptions = {},
): void {
  const errors: string[] = [];
  const routeShape = validateBridgeRoute(route);
  if (!routeShape.valid) {
    errors.push(...routeShape.errors);
  }
  if (route.bridgeSessionId !== collection.bridgeSessionId) {
    errors.push("Bridge route and peer collection must share bridgeSessionId.");
  }
  if (options.contentKind) {
    try {
      assertRouteAllowedForContentKind(route, options.contentKind);
    } catch (error) {
      if (error instanceof BridgePeerRouteError) {
        errors.push(...error.errors);
      } else if (isErrorWithErrors(error)) {
        errors.push(...error.errors);
      } else {
        errors.push(error instanceof Error ? error.message : String(error));
      }
    }
  }
  try {
    assertRouteTargetsKnownPeers(route, collection);
    assertRouteTargetsRouteablePeers(route, collection, options);
  } catch (error) {
    if (isErrorWithErrors(error)) {
      errors.push(...error.errors);
    } else {
      errors.push(error instanceof Error ? error.message : String(error));
    }
  }
  if (errors.length > 0) {
    throw new BridgePeerRouteError(unique(errors));
  }
}

export function resolveBridgeRoutePeerIds(
  route: BridgeRoute,
  collection: BridgePeerCollection,
  options: RouteablePeerOptions = {},
): readonly BridgePeerSessionId[] {
  assertRouteCompatibleWithPeerCollection(route, collection, options);
  return isBroadcastRoute(route)
    ? getRouteableBridgePeers(collection, options).map((peer) => peer.peerSessionId)
    : getExplicitTargetPeerIds(route);
}

export function deriveDefaultBridgeRouteForCurrentSession(collection: BridgePeerCollection): DefaultBridgeRouteResult {
  const routeablePeers = getRouteableBridgePeers(collection);
  const routeablePeerIds = routeablePeers.map((peer) => peer.peerSessionId);
  if (routeablePeers.length === 0) {
    return {
      status: "no_route",
      reason: "no_routeable_peer",
      routeablePeerIds,
      errors: ["No routeable remote Bridge peer is available for the current session."],
    };
  }
  if (routeablePeers.length > 1) {
    return {
      status: "requires_explicit_selection",
      reason: "multiple_routeable_peers",
      routeablePeerIds,
      errors: ["Multiple routeable Bridge peers require explicit target selection."],
    };
  }

  const [peer] = routeablePeers;
  const route: BridgeRoute = {
    bridgeSessionId: collection.bridgeSessionId,
    target: {
      kind: "selected_peer",
      peerSessionId: peer.peerSessionId,
    },
  };
  assertRouteCompatibleWithPeerCollection(route, collection);
  return {
    status: "selected_peer",
    route,
    peer,
    routeablePeerIds,
  };
}

export const deriveSinglePeerBridgeRoute = deriveDefaultBridgeRouteForCurrentSession;

function isPeerRouteable(peer: BridgePeerSession, options: RouteablePeerOptions): boolean {
  return peerRouteabilityErrors(peer, options).length === 0;
}

function peerRouteabilityErrors(peer: BridgePeerSession, options: RouteablePeerOptions): string[] {
  const errors: string[] = [];
  if (peer.accepted !== true || peer.sessionVerified !== true || peer.currentSessionOnly !== true) {
    errors.push("Bridge peer route target must be current-session accepted and session-verified.");
  }
  if (peer.liveness !== "connected") {
    errors.push("Bridge peer route target must be connected.");
  }
  if (peer.isLocalSelf === true && options.allowLocalSelf !== true) {
    errors.push("Bridge peer route target must be remote unless local self is explicitly allowed.");
  }
  return unique(errors);
}

function routeKnownPeerErrors(route: BridgeRoute, collection: BridgePeerCollection): string[] {
  if (isBroadcastRoute(route)) {
    return [];
  }
  const errors: string[] = [];
  for (const peerSessionId of getExplicitTargetPeerIds(route)) {
    if (!findBridgePeerBySessionId(collection, peerSessionId)) {
      errors.push("Bridge route target must be a known current-session peer.");
    }
  }
  return unique(errors);
}

function routeRouteabilityErrors(
  route: BridgeRoute,
  collection: BridgePeerCollection,
  options: RouteablePeerOptions,
): string[] {
  const errors: string[] = [];
  if (isBroadcastRoute(route)) {
    if (getRouteableBridgePeers(collection, options).length === 0) {
      errors.push("Broadcast route requires at least one routeable remote Bridge peer.");
    }
    return errors;
  }

  for (const peerSessionId of getExplicitTargetPeerIds(route)) {
    const peer = findBridgePeerBySessionId(collection, peerSessionId);
    if (!peer) {
      errors.push("Bridge route target must be a known current-session peer.");
      continue;
    }
    errors.push(...peerRouteabilityErrors(peer, options));
  }
  return unique(errors);
}

function normalizeIdentifier(value: unknown): string | null {
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isErrorWithErrors(error: unknown): error is { errors: readonly string[] } {
  return isRecord(error) && Array.isArray(error.errors) && error.errors.every((value) => typeof value === "string");
}

function requireExactFields(
  value: Record<string, unknown>,
  requiredFields: readonly string[],
  optionalFields: readonly string[],
  label: string,
  errors: string[],
): void {
  for (const field of requiredFields) {
    if (!(field in value)) {
      errors.push(`${label} is missing ${field}.`);
    }
  }
  for (const field of Object.keys(value)) {
    if (!requiredFields.includes(field) && !optionalFields.includes(field)) {
      errors.push(`${label} contains unsupported field ${field}.`);
    }
  }
}

function rejectUnsupportedAuthorityFields(value: Record<string, unknown>, label: string, errors: string[]): void {
  for (const field of UNSUPPORTED_AUTHORITY_FIELDS) {
    if (field in value) {
      errors.push(`${label} must not include ${field}; membership and routing are not consent, trust, or authority.`);
    }
  }
}

function unique(values: readonly string[]): string[] {
  return Array.from(new Set(values));
}

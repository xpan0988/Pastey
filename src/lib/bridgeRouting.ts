export type BridgePeerSessionId = string & { readonly __bridgePeerSessionId: unique symbol };

export type BridgeTarget =
  | {
      readonly kind: "selected_peer";
      readonly peerSessionId: BridgePeerSessionId;
    }
  | {
      readonly kind: "selected_peers";
      readonly peerSessionIds: readonly BridgePeerSessionId[];
    }
  | {
      readonly kind: "broadcast_bridge";
      readonly explicit: true;
    };

export interface BridgeRoute {
  readonly bridgeSessionId: string;
  readonly target: BridgeTarget;
}

export type BridgeContentKind =
  | "text"
  | "file"
  | "image"
  | "pasted_image"
  | "bridge_control_event"
  | "agent_bridge_capability_event";

export interface BridgeRoutingPolicy {
  readonly contentKind: BridgeContentKind;
  readonly allowSelectedPeer: boolean;
  readonly allowSelectedPeers: boolean;
  readonly allowBroadcast: boolean;
  readonly requireExactSelectedPeer?: boolean;
}

export type BridgeTargetNormalizationResult =
  | { ok: true; target: BridgeTarget; errors: [] }
  | { ok: false; errors: string[] };

export type BridgeRouteValidationResult =
  | { valid: true; route: BridgeRoute; errors: [] }
  | { valid: false; errors: string[] };

export type BridgeRouteErrorCode =
  | "no_routeable_peer"
  | "unknown_peer"
  | "peer_unrouteable"
  | "unsupported_selected_peers"
  | "unsupported_broadcast"
  | "malformed_route"
  | "route_mismatch"
  | "route_expired";

export class BridgeRoutingPolicyError extends Error {
  readonly errors: readonly string[];

  constructor(errors: readonly string[]) {
    super(errors.join(" "));
    this.name = "BridgeRoutingPolicyError";
    this.errors = errors;
  }
}

export class BridgeRouteCodedError extends Error {
  readonly code: BridgeRouteErrorCode;

  constructor(code: BridgeRouteErrorCode, message: string) {
    super(`[pastey:bridge-route-error code=${code}] ${message}`);
    this.name = "BridgeRouteCodedError";
    this.code = code;
  }
}

export const DEFAULT_BRIDGE_ROUTING_POLICIES: Readonly<Record<BridgeContentKind, BridgeRoutingPolicy>> = {
  text: {
    contentKind: "text",
    allowSelectedPeer: true,
    allowSelectedPeers: true,
    allowBroadcast: true,
  },
  file: {
    contentKind: "file",
    allowSelectedPeer: true,
    allowSelectedPeers: true,
    allowBroadcast: true,
  },
  image: {
    contentKind: "image",
    allowSelectedPeer: true,
    allowSelectedPeers: true,
    allowBroadcast: true,
  },
  pasted_image: {
    contentKind: "pasted_image",
    allowSelectedPeer: true,
    allowSelectedPeers: true,
    allowBroadcast: true,
  },
  bridge_control_event: {
    contentKind: "bridge_control_event",
    allowSelectedPeer: true,
    allowSelectedPeers: false,
    allowBroadcast: false,
  },
  agent_bridge_capability_event: {
    contentKind: "agent_bridge_capability_event",
    allowSelectedPeer: true,
    allowSelectedPeers: false,
    allowBroadcast: false,
    requireExactSelectedPeer: true,
  },
};

const BRIDGE_ROUTE_ERROR_CODE_PATTERN = /\[pastey:bridge-route-error code=([a-z_]+)\]/;

export function bridgeRouteError(code: BridgeRouteErrorCode, message: string): BridgeRouteCodedError {
  return new BridgeRouteCodedError(code, message);
}

export function bridgeRouteErrorCodeFromMessage(error: unknown): BridgeRouteErrorCode | null {
  if (error instanceof BridgeRouteCodedError) {
    return error.code;
  }
  const message = error instanceof Error ? error.message : String(error);
  const code = message.match(BRIDGE_ROUTE_ERROR_CODE_PATTERN)?.[1];
  return isBridgeRouteErrorCode(code) ? code : inferBridgeRouteErrorCode(message);
}

export function formatBridgeRouteErrorForUser(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  const code = bridgeRouteErrorCodeFromMessage(error);
  switch (code) {
    case "no_routeable_peer":
      return "No routeable Bridge peer is available for this send.";
    case "unknown_peer":
      return "That Bridge peer is no longer in the current session.";
    case "peer_unrouteable":
      return "That Bridge peer is not currently routeable.";
    case "unsupported_selected_peers":
      return "Selected-peers delivery is not supported for this action.";
    case "unsupported_broadcast":
      return "Broadcast delivery is not supported for this action.";
    case "malformed_route":
      return "The Bridge route payload was malformed.";
    case "route_mismatch":
      return "The Bridge route does not match the current room.";
    case "route_expired":
      return "The Bridge route expired or the room is no longer active.";
    default:
      return message.replace(BRIDGE_ROUTE_ERROR_CODE_PATTERN, "").trim();
  }
}

const ROUTE_REQUIRED_FIELDS = ["bridgeSessionId", "target"];
const SELECTED_PEER_FIELDS = ["kind", "peerSessionId"];
const SELECTED_PEERS_FIELDS = ["kind", "peerSessionIds"];
const BROADCAST_FIELDS = ["kind", "explicit"];
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

export function bridgePeerSessionId(value: string): BridgePeerSessionId {
  const normalized = normalizeIdentifier(value);
  if (normalized === null) {
    throw new Error("Bridge peer session id must be a non-empty current-session string.");
  }
  return normalized as BridgePeerSessionId;
}

export function normalizeBridgeTarget(value: unknown): BridgeTargetNormalizationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { ok: false, errors: ["Bridge target must be an explicit target object."] };
  }
  rejectUnsupportedAuthorityFields(value, "Bridge target", errors);

  switch (value.kind) {
    case "selected_peer": {
      requireExactFields(value, SELECTED_PEER_FIELDS, "selected_peer target", errors);
      const peerSessionId = normalizeIdentifier(value.peerSessionId);
      if (peerSessionId === null) {
        errors.push("selected_peer target requires one non-empty peerSessionId.");
      }
      return errors.length === 0
        ? {
            ok: true,
            target: { kind: "selected_peer", peerSessionId: peerSessionId as BridgePeerSessionId },
            errors: [],
          }
        : { ok: false, errors: unique(errors) };
    }
    case "selected_peers": {
      requireExactFields(value, SELECTED_PEERS_FIELDS, "selected_peers target", errors);
      if (!Array.isArray(value.peerSessionIds)) {
        errors.push("selected_peers target requires peerSessionIds.");
        return { ok: false, errors: unique(errors) };
      }
      const peerSessionIds = value.peerSessionIds.map((id) => normalizeIdentifier(id));
      if (peerSessionIds.some((id) => id === null)) {
        errors.push("selected_peers target requires only non-empty peerSessionIds.");
      }
      if (peerSessionIds.length < 2) {
        errors.push("selected_peers target requires two or more peerSessionIds.");
      }
      const compactPeerSessionIds = peerSessionIds.filter((id): id is string => id !== null);
      if (new Set(compactPeerSessionIds).size !== compactPeerSessionIds.length) {
        errors.push("selected_peers target rejects duplicate peerSessionIds.");
      }
      return errors.length === 0
        ? {
            ok: true,
            target: {
              kind: "selected_peers",
              peerSessionIds: compactPeerSessionIds as BridgePeerSessionId[],
            },
            errors: [],
          }
        : { ok: false, errors: unique(errors) };
    }
    case "broadcast_bridge": {
      requireExactFields(value, BROADCAST_FIELDS, "broadcast_bridge target", errors);
      if (value.explicit !== true) {
        errors.push("broadcast_bridge target requires explicit true.");
      }
      return errors.length === 0
        ? { ok: true, target: { kind: "broadcast_bridge", explicit: true }, errors: [] }
        : { ok: false, errors: unique(errors) };
    }
    default:
      return { ok: false, errors: ["Bridge target kind is unsupported or missing."] };
  }
}

export function validateBridgeRoute(
  value: unknown,
  options: { acceptedPeerSessionIds?: readonly BridgePeerSessionId[] } = {},
): BridgeRouteValidationResult {
  const errors: string[] = [];
  if (!isRecord(value)) {
    return { valid: false, errors: ["Bridge route must be an object."] };
  }
  rejectUnsupportedAuthorityFields(value, "Bridge route", errors);
  requireExactFields(value, ROUTE_REQUIRED_FIELDS, "Bridge route", errors);

  const bridgeSessionId = normalizeIdentifier(value.bridgeSessionId);
  if (bridgeSessionId === null) {
    errors.push("Bridge route requires a non-empty bridgeSessionId.");
  }

  const target = normalizeBridgeTarget(value.target);
  if (!target.ok) {
    errors.push(...target.errors);
  }

  if (target.ok && options.acceptedPeerSessionIds) {
    errors.push(...validateAcceptedPeers(target.target, options.acceptedPeerSessionIds));
  }

  return errors.length === 0 && target.ok && bridgeSessionId !== null
    ? {
        valid: true,
        route: {
          bridgeSessionId,
          target: target.target,
        },
        errors: [],
      }
    : { valid: false, errors: unique(errors) };
}

export function isBroadcastRoute(route: BridgeRoute): boolean {
  return route.target.kind === "broadcast_bridge";
}

export function getExplicitTargetPeerIds(route: BridgeRoute): readonly BridgePeerSessionId[] {
  switch (route.target.kind) {
    case "selected_peer":
      return [route.target.peerSessionId];
    case "selected_peers":
      return [...route.target.peerSessionIds];
    case "broadcast_bridge":
      return [];
  }
}

export function assertRouteAllowedForContentKind(
  route: BridgeRoute,
  contentKind: BridgeContentKind,
  policy: BridgeRoutingPolicy = DEFAULT_BRIDGE_ROUTING_POLICIES[contentKind],
): void {
  const errors: string[] = [];
  if (policy.contentKind !== contentKind) {
    errors.push("Bridge routing policy contentKind does not match the asserted content kind.");
  }

  switch (route.target.kind) {
    case "selected_peer":
      if (!policy.allowSelectedPeer) {
        errors.push(`${contentKind} does not allow selected_peer routes.`);
      }
      break;
    case "selected_peers":
      if (policy.requireExactSelectedPeer) {
        errors.push(`${contentKind} requires exactly one selected peer.`);
      }
      if (!policy.allowSelectedPeers) {
        errors.push(`${contentKind} does not allow selected_peers routes.`);
      }
      break;
    case "broadcast_bridge":
      if (policy.requireExactSelectedPeer) {
        errors.push(`${contentKind} requires exactly one selected peer.`);
      }
      if (!policy.allowBroadcast) {
        errors.push(`${contentKind} does not allow broadcast_bridge routes by default.`);
      }
      break;
  }

  if (errors.length > 0) {
    throw new BridgeRoutingPolicyError(unique(errors));
  }
}

function validateAcceptedPeers(
  target: BridgeTarget,
  acceptedPeerSessionIds: readonly BridgePeerSessionId[],
): string[] {
  const accepted = new Set(acceptedPeerSessionIds);
  const errors: string[] = [];
  const explicitPeerIds = target.kind === "broadcast_bridge" ? [] : getExplicitTargetPeerIds({ bridgeSessionId: "bridge", target });
  for (const peerSessionId of explicitPeerIds) {
    if (!accepted.has(peerSessionId)) {
      errors.push("Bridge route target must be a current-session accepted peer.");
    }
  }
  if (target.kind === "broadcast_bridge" && accepted.size === 0) {
    errors.push("broadcast_bridge route requires at least one current-session accepted peer.");
  }
  return unique(errors);
}

function normalizeIdentifier(value: unknown): string | null {
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function requireExactFields(value: Record<string, unknown>, allowedFields: readonly string[], label: string, errors: string[]): void {
  for (const field of allowedFields) {
    if (!(field in value)) {
      errors.push(`${label} is missing ${field}.`);
    }
  }
  for (const field of Object.keys(value)) {
    if (!allowedFields.includes(field)) {
      errors.push(`${label} contains unsupported field ${field}.`);
    }
  }
}

function rejectUnsupportedAuthorityFields(value: Record<string, unknown>, label: string, errors: string[]): void {
  for (const field of UNSUPPORTED_AUTHORITY_FIELDS) {
    if (field in value) {
      errors.push(`${label} must not include ${field}; routing is not consent, trust, or authority.`);
    }
  }
}

function unique(values: readonly string[]): string[] {
  return Array.from(new Set(values));
}

function isBridgeRouteErrorCode(value: unknown): value is BridgeRouteErrorCode {
  return typeof value === "string" && [
    "no_routeable_peer",
    "unknown_peer",
    "peer_unrouteable",
    "unsupported_selected_peers",
    "unsupported_broadcast",
    "malformed_route",
    "route_mismatch",
    "route_expired",
  ].includes(value);
}

function inferBridgeRouteErrorCode(message: string): BridgeRouteErrorCode | null {
  if (/No routeable remote Bridge peer|no current routeable peers/i.test(message)) {
    return "no_routeable_peer";
  }
  if (/unknown current-session peer|known current-session peer|no longer in the current session/i.test(message)) {
    return "unknown_peer";
  }
  if (/not currently routeable|must be connected|disconnected|stale|left/i.test(message)) {
    return "peer_unrouteable";
  }
  if (/selected_peers|selected peers/i.test(message) && /not enabled|per-target outcome|exactly one selected Bridge peer/i.test(message)) {
    return "unsupported_selected_peers";
  }
  if (/broadcast/i.test(message) && /not enabled|per-target outcome|exactly one selected Bridge peer/i.test(message)) {
    return "unsupported_broadcast";
  }
  if (/does not match|active Bridge session|current room/i.test(message)) {
    return "route_mismatch";
  }
  if (/expired|requires an active room|no longer active/i.test(message)) {
    return "route_expired";
  }
  if (/route .*invalid|malformed|unsupported or missing|must be an object|unsupported field|schema version/i.test(message)) {
    return "malformed_route";
  }
  return null;
}

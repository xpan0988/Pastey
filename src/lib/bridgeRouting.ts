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

export class BridgeRoutingPolicyError extends Error {
  readonly errors: readonly string[];

  constructor(errors: readonly string[]) {
    super(errors.join(" "));
    this.name = "BridgeRoutingPolicyError";
    this.errors = errors;
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
    allowBroadcast: false,
  },
  image: {
    contentKind: "image",
    allowSelectedPeer: true,
    allowSelectedPeers: true,
    allowBroadcast: false,
  },
  pasted_image: {
    contentKind: "pasted_image",
    allowSelectedPeer: true,
    allowSelectedPeers: true,
    allowBroadcast: false,
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

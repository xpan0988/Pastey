import {
  assertRouteAllowedForContentKind,
  type BridgeContentKind,
  type BridgePeerSessionId,
  type BridgeRoute,
} from "./bridgeRouting";
import {
  bridgePeerDisplayName,
  bridgePeerSessionId,
  deriveDefaultBridgeRouteForCurrentSession,
  getRouteableBridgePeers,
  validateBridgePeerCollection,
  type BridgePeerCollection,
  type BridgePeerJoinMethod,
  type BridgePeerLiveness,
  type BridgePeerSession,
  type DefaultBridgeRouteResult,
} from "./bridgePeers";

export interface LegacyRoomBridgePeerInput {
  readonly peerSessionId?: string;
  readonly displayName?: string | null;
  readonly joinMethod?: BridgePeerJoinMethod;
  readonly liveness?: BridgePeerLiveness;
  readonly connected?: boolean;
  readonly left?: boolean;
  readonly stale?: boolean;
  readonly isLocalSelf?: boolean;
}

export interface LegacyRoomBridgeInput {
  readonly id: string;
  readonly status?: "active" | "peer_left" | "burned" | "expired" | string;
  readonly local_role?: "creator" | "joined" | string;
  readonly peer_device_name?: string | null;
  readonly peer_connected?: boolean;
  readonly peer_burned_at?: number | null;
  readonly bridgeSessionId?: string;
  readonly joinMethod?: BridgePeerJoinMethod;
  readonly peers?: readonly LegacyRoomBridgePeerInput[];
}

export interface LegacyRoomRoutingStateDescription {
  readonly bridgeSessionId: string;
  readonly peerCount: number;
  readonly routeablePeerIds: readonly BridgePeerSessionId[];
  readonly defaultRouteStatus: DefaultBridgeRouteResult["status"];
  readonly currentSessionOnly: true;
  readonly legacySinglePeerRuntime: true;
  readonly enablesBroadcast: false;
}

export type LegacyRoomDefaultBridgeRouteResult = DefaultBridgeRouteResult;

const DEFAULT_LEGACY_PEER_NAME = "Legacy Room peer";

export function legacyRoomToBridgePeerCollection(room: LegacyRoomBridgeInput): BridgePeerCollection {
  const bridgeSessionId = bridgeSessionIdForLegacyRoom(room);
  const peers = legacyPeersForRoom(room, bridgeSessionId);
  const result = validateBridgePeerCollection({ bridgeSessionId, peers });
  if (!result.valid) {
    throw new Error(result.errors.join(" "));
  }
  return result.collection;
}

export function deriveLegacyRoomDefaultBridgeRoute(room: LegacyRoomBridgeInput): LegacyRoomDefaultBridgeRouteResult {
  return deriveDefaultBridgeRouteForCurrentSession(legacyRoomToBridgePeerCollection(room));
}

export function describeLegacyRoomRoutingState(room: LegacyRoomBridgeInput): LegacyRoomRoutingStateDescription {
  const collection = legacyRoomToBridgePeerCollection(room);
  const defaultRoute = deriveDefaultBridgeRouteForCurrentSession(collection);
  return {
    bridgeSessionId: collection.bridgeSessionId,
    peerCount: collection.peers.length,
    routeablePeerIds: getRouteableBridgePeers(collection).map((peer) => peer.peerSessionId),
    defaultRouteStatus: defaultRoute.status,
    currentSessionOnly: true,
    legacySinglePeerRuntime: true,
    enablesBroadcast: false,
  };
}

export function assertLegacyRoomRouteAllowedForContentKind(
  route: BridgeRoute,
  contentKind: BridgeContentKind,
): void {
  assertRouteAllowedForContentKind(route, contentKind);
}

function legacyPeersForRoom(room: LegacyRoomBridgeInput, bridgeSessionId: string): readonly BridgePeerSession[] {
  const peerInputs = room.peers ?? [singlePeerInputForRoom(room)];
  return peerInputs
    .filter((peer) => hasLegacyPeerSignal(peer))
    .map((peer, index) => legacyPeerToBridgePeer(room, bridgeSessionId, peer, index));
}

function singlePeerInputForRoom(room: LegacyRoomBridgeInput): LegacyRoomBridgePeerInput {
  return {
    peerSessionId: legacyPeerSessionIdForRoom(room, 0),
    displayName: room.peer_device_name ?? DEFAULT_LEGACY_PEER_NAME,
    joinMethod: room.joinMethod ?? joinMethodForLegacyRoom(room),
    liveness: livenessForLegacyRoom(room),
  };
}

function legacyPeerToBridgePeer(
  room: LegacyRoomBridgeInput,
  bridgeSessionId: string,
  peer: LegacyRoomBridgePeerInput,
  index: number,
): BridgePeerSession {
  return {
    bridgeSessionId,
    peerSessionId: bridgePeerSessionId(peer.peerSessionId ?? legacyPeerSessionIdForRoom(room, index)),
    displayName: bridgePeerDisplayName(peer.displayName ?? DEFAULT_LEGACY_PEER_NAME),
    joinMethod: peer.joinMethod ?? room.joinMethod ?? joinMethodForLegacyRoom(room),
    liveness: peer.liveness ?? livenessForLegacyPeer(peer),
    accepted: true,
    sessionVerified: true,
    currentSessionOnly: true,
    ...(peer.isLocalSelf === undefined ? {} : { isLocalSelf: peer.isLocalSelf }),
  };
}

function bridgeSessionIdForLegacyRoom(room: LegacyRoomBridgeInput): string {
  const explicit = normalizeIdentifier(room.bridgeSessionId);
  if (explicit !== null) return explicit;
  const roomId = normalizeIdentifier(room.id);
  if (roomId === null) {
    throw new Error("Legacy Room adapter requires a non-empty room id.");
  }
  return `legacy-room:${roomId}`;
}

function legacyPeerSessionIdForRoom(room: LegacyRoomBridgeInput, index: number): string {
  const roomId = normalizeIdentifier(room.id);
  if (roomId === null) {
    throw new Error("Legacy Room adapter requires a non-empty room id.");
  }
  return index === 0 ? `legacy-room-peer:${roomId}` : `legacy-room-peer:${roomId}:${index}`;
}

function joinMethodForLegacyRoom(room: LegacyRoomBridgeInput): BridgePeerJoinMethod {
  return room.local_role === "joined" ? "manual_code" : "nearby_accept";
}

function livenessForLegacyRoom(room: LegacyRoomBridgeInput): BridgePeerLiveness {
  if (room.status === "peer_left" || room.peer_burned_at != null) return "left";
  if (room.status === "burned" || room.status === "expired") return "stale";
  return room.peer_connected === true ? "connected" : "disconnected";
}

function livenessForLegacyPeer(peer: LegacyRoomBridgePeerInput): BridgePeerLiveness {
  if (peer.left === true) return "left";
  if (peer.stale === true) return "stale";
  return peer.connected === true ? "connected" : "disconnected";
}

function hasLegacyPeerSignal(peer: LegacyRoomBridgePeerInput): boolean {
  return peer.peerSessionId !== undefined ||
    peer.displayName !== undefined ||
    peer.joinMethod !== undefined ||
    peer.liveness !== undefined ||
    peer.connected !== undefined ||
    peer.left !== undefined ||
    peer.stale !== undefined ||
    peer.isLocalSelf !== undefined;
}

function normalizeIdentifier(value: unknown): string | null {
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : null;
}

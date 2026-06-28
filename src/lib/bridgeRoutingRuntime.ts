import {
  assertLegacyRoomRouteAllowedForContentKind,
  deriveLegacyRoomDefaultBridgeRoute,
  legacyRoomToBridgePeerCollection,
  type LegacyRoomBridgeInput,
} from "./bridgeRoomAdapter";
import {
  bridgePeerSessionId,
  bridgeRouteError,
  validateBridgeRoute,
  type BridgeContentKind,
  type BridgeRoute,
} from "./bridgeRouting";
import type { BridgePeerCollection, BridgePeerSession } from "./bridgePeers";
import {
  assertRouteCompatibleWithPeerCollection,
} from "./bridgePeers";
import type { RoomControlSessionContext, RoomInfo, RoomItem } from "./types";

export type BridgeRoutingRuntimeState =
  | {
      status: "ready_selected_peer";
      collection: BridgePeerCollection;
      route: BridgeRoute;
      peer: BridgePeerSession;
      currentSessionOnly: true;
      legacySinglePeerRuntime: true;
      enablesBroadcast: false;
    }
  | {
      status: "no_route";
      collection: BridgePeerCollection;
      reason: "no_routeable_peer";
      errors: readonly string[];
      currentSessionOnly: true;
      legacySinglePeerRuntime: true;
      enablesBroadcast: false;
    }
  | {
      status: "requires_explicit_selection";
      collection: BridgePeerCollection;
      reason: "multiple_routeable_peers";
      errors: readonly string[];
      currentSessionOnly: true;
      legacySinglePeerRuntime: true;
      enablesBroadcast: false;
    }
  | {
      status: "invalid";
      errors: readonly string[];
      currentSessionOnly: true;
      legacySinglePeerRuntime: true;
      enablesBroadcast: false;
    };

type BridgeRoutePayloadTarget =
  | {
      readonly kind: "selected_peer";
      readonly peerSessionId: string;
    }
  | {
      readonly kind: "selected_peers";
      readonly peerSessionIds: readonly string[];
    }
  | {
      readonly kind: "broadcast_bridge";
      readonly explicit: true;
    };

interface BridgeRoutePayload<TSchemaVersion extends string> {
  readonly schemaVersion: TSchemaVersion;
  readonly bridgeSessionId: string;
  readonly target: BridgeRoutePayloadTarget;
}

export type TextBridgeRoutePayload = BridgeRoutePayload<"pastey-bridge-text-route/v1">;
export type FileBridgeRoutePayload = BridgeRoutePayload<"pastey-bridge-file-route/v1">;
export type ControlBridgeRoutePayload = BridgeRoutePayload<"pastey-bridge-control-route/v1">;

export type TextRoomSender = (
  roomId: string,
  text: string,
  bridgeRoute?: TextBridgeRoutePayload,
) => Promise<RoomItem>;
export type FileRoomSender = (
  roomId: string,
  path: string,
  options?: {
    displayName?: string;
    mimeType?: string | null;
    queueItemId?: string | null;
    requestedWindow?: number | null;
    bridgeRoute?: FileBridgeRoutePayload;
  },
) => Promise<RoomItem>;
export type TransferInputEnqueuer<TInput> = (roomId: string, inputs: TInput[]) => void;
export type FilePathEnqueuer = (roomId: string, paths: string[]) => void;
export type BridgeRoutableControlEvent = {
  readonly roomRef: string;
  readonly sourceDeviceRef: string;
  readonly targetPeerRef?: string;
  readonly kind: string;
};

export function deriveBridgeRoutingStateForRoom(room: LegacyRoomBridgeInput): BridgeRoutingRuntimeState {
  try {
    const collection = legacyRoomToBridgePeerCollection(room);
    const defaultRoute = deriveLegacyRoomDefaultBridgeRoute(room);
    switch (defaultRoute.status) {
      case "selected_peer":
        return {
          status: "ready_selected_peer",
          collection,
          route: defaultRoute.route,
          peer: defaultRoute.peer,
          currentSessionOnly: true,
          legacySinglePeerRuntime: true,
          enablesBroadcast: false,
        };
      case "no_route":
        return {
          status: "no_route",
          collection,
          reason: defaultRoute.reason,
          errors: defaultRoute.errors,
          currentSessionOnly: true,
          legacySinglePeerRuntime: true,
          enablesBroadcast: false,
        };
      case "requires_explicit_selection":
        return {
          status: "requires_explicit_selection",
          collection,
          reason: defaultRoute.reason,
          errors: defaultRoute.errors,
          currentSessionOnly: true,
          legacySinglePeerRuntime: true,
          enablesBroadcast: false,
        };
    }
  } catch (error) {
    return {
      status: "invalid",
      errors: [error instanceof Error ? error.message : String(error)],
      currentSessionOnly: true,
      legacySinglePeerRuntime: true,
      enablesBroadcast: false,
    };
  }
}

export function requireReadySelectedPeerRouteForContentKind(
  room: LegacyRoomBridgeInput,
  contentKind: BridgeContentKind,
): Extract<BridgeRoutingRuntimeState, { status: "ready_selected_peer" }> {
  const state = deriveBridgeRoutingStateForRoom(room);
  if (state.status !== "ready_selected_peer") {
    throw new Error(routeStateErrorMessage(state));
  }
  assertLegacyRoomRouteAllowedForContentKind(state.route, contentKind);
  return state;
}

export function deriveAuthoritativeTextSendRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
): BridgeRoute {
  return deriveAuthoritativeDataRoute(input, "text");
}

export function deriveAuthoritativeFileSendRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
): BridgeRoute {
  return deriveAuthoritativeDataRoute(input, "file");
}

export function deriveAuthoritativeImageSendRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
): BridgeRoute {
  return deriveAuthoritativeDataRoute(input, "image");
}

export function deriveAuthoritativeControlRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
): BridgeRoute {
  return deriveAuthoritativeSelectedPeerRoute(input, "control");
}

export function deriveAuthoritativeCapabilityRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
): BridgeRoute {
  return deriveAuthoritativeSelectedPeerRoute(input, "capability");
}

export function assertControlEventHasSelectedPeerRoute(
  session: RoomControlSessionContext,
  event: BridgeRoutableControlEvent,
): BridgeRoute {
  return assertSessionBoundControlRoute(session, event, "control");
}

export function assertCapabilityEventHasSelectedPeerRoute(
  session: RoomControlSessionContext,
  event: BridgeRoutableControlEvent,
): BridgeRoute {
  return assertSessionBoundControlRoute(session, event, "capability");
}

export async function sendTextToRoomWithBridgeRoute(
  room: RoomInfo,
  text: string,
  sender: TextRoomSender,
  explicitRoute?: BridgeRoute,
): Promise<RoomItem> {
  const route = deriveAuthoritativeDataRouteForRoom(room, "text", explicitRoute);
  return sender(room.id, text, bridgeRoutePayload(route, "pastey-bridge-text-route/v1"));
}

export async function sendFileToRoomWithBridgeRoute(
  room: RoomInfo,
  path: string,
  options: Parameters<FileRoomSender>[2],
  sender: FileRoomSender,
  explicitRoute?: BridgeRoute,
  contentKind: Extract<BridgeContentKind, "file" | "image" | "pasted_image"> = "file",
): Promise<RoomItem> {
  const route = deriveAuthoritativeDataRouteForRoom(room, contentKind, explicitRoute);
  return sender(room.id, path, {
    ...options,
    bridgeRoute: bridgeRoutePayload(route, "pastey-bridge-file-route/v1"),
  });
}

export function enqueueFilePathsWithBridgeRoute(
  room: RoomInfo,
  paths: string[],
  enqueuer: FilePathEnqueuer,
  explicitRoute?: BridgeRoute,
): void {
  deriveAuthoritativeDataRouteForRoom(room, "file", explicitRoute);
  enqueuer(room.id, paths);
}

export function enqueueTransferInputsWithBridgeRoute<TInput>(
  room: RoomInfo,
  inputs: TInput[],
  contentKind: Extract<BridgeContentKind, "file" | "image" | "pasted_image">,
  enqueuer: TransferInputEnqueuer<TInput>,
  explicitRoute?: BridgeRoute,
): void {
  deriveAuthoritativeDataRouteForRoom(room, contentKind, explicitRoute);
  enqueuer(room.id, inputs);
}

export function routeStateLabel(state: BridgeRoutingRuntimeState): string {
  switch (state.status) {
    case "ready_selected_peer":
      return `Selected peer: ${state.peer.displayName}`;
    case "no_route":
      return "No routeable peer";
    case "requires_explicit_selection":
      return "Explicit peer selection required";
    case "invalid":
      return "Bridge route unavailable";
  }
}

function routeStateErrorMessage(state: BridgeRoutingRuntimeState): string {
  switch (state.status) {
    case "ready_selected_peer":
      return "";
    case "no_route":
      return bridgeRouteError(
        "no_routeable_peer",
        state.errors[0] ?? "No routeable Bridge peer is available.",
      ).message;
    case "requires_explicit_selection":
      return bridgeRouteError(
        "unsupported_selected_peers",
        state.errors[0] ?? "Multiple Bridge peers require explicit selection.",
      ).message;
    case "invalid":
      return bridgeRouteError(
        "malformed_route",
        state.errors[0] ?? "Bridge route derivation failed.",
      ).message;
  }
}

function deriveAuthoritativeSelectedPeerRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
  contentKind: "text" | "file" | "image" | "control" | "capability",
): BridgeRoute {
  const route = routeFromRuntimeInput(input);
  const validation = validateBridgeRoute(route);
  if (!validation.valid) {
    throw bridgeRouteError("malformed_route", validation.errors.join(" "));
  }
  if (validation.route.target.kind !== "selected_peer") {
    throw bridgeRouteError(
      validation.route.target.kind === "selected_peers"
        ? "unsupported_selected_peers"
        : "unsupported_broadcast",
      `Production ${contentKind} send requires exactly one selected Bridge peer.`,
    );
  }
  return validation.route;
}

function deriveAuthoritativeDataRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
  contentKind: Extract<BridgeContentKind, "text" | "file" | "image" | "pasted_image">,
): BridgeRoute {
  const route = routeFromRuntimeInput(input);
  const validation = validateBridgeRoute(route);
  if (!validation.valid) {
    throw bridgeRouteError("malformed_route", validation.errors.join(" "));
  }
  assertLegacyRoomRouteAllowedForContentKind(validation.route, contentKind);
  return validation.route;
}

function deriveAuthoritativeDataRouteForRoom(
  room: RoomInfo,
  contentKind: Extract<BridgeContentKind, "text" | "file" | "image" | "pasted_image">,
  explicitRoute?: BridgeRoute,
): BridgeRoute {
  const collection = legacyRoomToBridgePeerCollection(room);
  const route = explicitRoute ?? routeFromRuntimeInput(room);
  const validation = validateBridgeRoute(route);
  if (!validation.valid) {
    throw bridgeRouteError("malformed_route", validation.errors.join(" "));
  }
  try {
    assertRouteCompatibleWithPeerCollection(validation.route, collection, { contentKind });
  } catch (error) {
    throw bridgeRouteError(routeCompatibilityErrorCode(error), error instanceof Error ? error.message : String(error));
  }
  return validation.route;
}

function routeCompatibilityErrorCode(error: unknown): "no_routeable_peer" | "unknown_peer" | "peer_unrouteable" | "unsupported_selected_peers" | "unsupported_broadcast" | "route_mismatch" | "route_expired" {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("share bridgeSessionId")) return "route_mismatch";
  if (message.includes("expired")) return "route_expired";
  if (message.includes("unsupported for file") || message.includes("does not allow selected_peers")) return "unsupported_selected_peers";
  if (message.includes("does not allow broadcast")) return "unsupported_broadcast";
  if (message.includes("known") || message.includes("unknown")) return "unknown_peer";
  if (message.includes("connected") || message.includes("route target")) return "peer_unrouteable";
  return "no_routeable_peer";
}

export function bridgeRoutePayload<TSchemaVersion extends string>(
  route: BridgeRoute,
  schemaVersion: TSchemaVersion,
): BridgeRoutePayload<TSchemaVersion> {
  return {
    schemaVersion,
    bridgeSessionId: route.bridgeSessionId,
    target: route.target.kind === "selected_peers"
      ? {
          kind: "selected_peers",
          peerSessionIds: [...route.target.peerSessionIds],
        }
      : route.target.kind === "broadcast_bridge"
        ? { kind: "broadcast_bridge", explicit: true }
        : {
            kind: "selected_peer",
            peerSessionId: route.target.peerSessionId,
          },
  };
}

function assertSessionBoundControlRoute(
  session: RoomControlSessionContext,
  event: BridgeRoutableControlEvent,
  contentKind: "control" | "capability",
): BridgeRoute {
  if (event.roomRef !== session.roomId) {
    throw bridgeRouteError("route_mismatch", `Production ${contentKind} send requires the active Bridge session.`);
  }
  if (event.sourceDeviceRef !== session.localSessionRef) {
    throw bridgeRouteError("route_mismatch", `Production ${contentKind} send requires the active local session source.`);
  }
  if (event.targetPeerRef !== session.peerSessionRef) {
    throw bridgeRouteError("route_mismatch", `Production ${contentKind} send requires exactly one selected Bridge peer target.`);
  }
  if (!session.peerConnected) {
    throw bridgeRouteError("peer_unrouteable", `Production ${contentKind} send requires a connected selected Bridge peer.`);
  }

  const route = deriveAuthoritativeSelectedPeerRoute({
    bridgeSessionId: `legacy-room:${session.roomId}`,
    target: {
      kind: "selected_peer",
      peerSessionId: bridgePeerSessionId(session.peerRouteRef ?? session.peerSessionRef),
    },
  }, contentKind);
  return route;
}

function routeFromRuntimeInput(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
): BridgeRoute {
  if (isBridgeRoute(input)) {
    return input;
  }
  if (isBridgeRoutingRuntimeState(input)) {
    if (input.status !== "ready_selected_peer") {
      throw new Error(routeStateErrorMessage(input));
    }
    return input.route;
  }
  const state = deriveBridgeRoutingStateForRoom(input);
  if (state.status !== "ready_selected_peer") {
    throw new Error(routeStateErrorMessage(state));
  }
  return state.route;
}

function isBridgeRoute(value: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute): value is BridgeRoute {
  return "target" in value && "bridgeSessionId" in value;
}

function isBridgeRoutingRuntimeState(
  value: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
): value is BridgeRoutingRuntimeState {
  return "status" in value && "currentSessionOnly" in value && "legacySinglePeerRuntime" in value;
}

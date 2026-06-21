import {
  assertLegacyRoomRouteAllowedForContentKind,
  deriveLegacyRoomDefaultBridgeRoute,
  legacyRoomToBridgePeerCollection,
  type LegacyRoomBridgeInput,
} from "./bridgeRoomAdapter";
import {
  bridgePeerSessionId,
  validateBridgeRoute,
  type BridgeContentKind,
  type BridgeRoute,
} from "./bridgeRouting";
import type { BridgePeerCollection, BridgePeerSession } from "./bridgePeers";
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

interface SelectedPeerBridgeRoutePayload<TSchemaVersion extends string> {
  readonly schemaVersion: TSchemaVersion;
  readonly bridgeSessionId: string;
  readonly target: {
    readonly kind: "selected_peer";
    readonly peerSessionId: string;
  };
}

export type TextBridgeRoutePayload = SelectedPeerBridgeRoutePayload<"pastey-bridge-text-route/v1">;
export type FileBridgeRoutePayload = SelectedPeerBridgeRoutePayload<"pastey-bridge-file-route/v1">;

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
  return deriveAuthoritativeSelectedPeerRoute(input, "text");
}

export function deriveAuthoritativeFileSendRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
): BridgeRoute {
  return deriveAuthoritativeSelectedPeerRoute(input, "file");
}

export function deriveAuthoritativeImageSendRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
): BridgeRoute {
  return deriveAuthoritativeSelectedPeerRoute(input, "image");
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
): Promise<RoomItem> {
  const route = deriveAuthoritativeTextSendRoute(room);
  return sender(room.id, text, selectedPeerRoutePayload(route, "pastey-bridge-text-route/v1"));
}

export async function sendFileToRoomWithBridgeRoute(
  room: RoomInfo,
  path: string,
  options: Parameters<FileRoomSender>[2],
  sender: FileRoomSender,
): Promise<RoomItem> {
  const route = deriveAuthoritativeFileSendRoute(room);
  return sender(room.id, path, {
    ...options,
    bridgeRoute: selectedPeerRoutePayload(route, "pastey-bridge-file-route/v1"),
  });
}

export function enqueueFilePathsWithBridgeRoute(
  room: RoomInfo,
  paths: string[],
  enqueuer: FilePathEnqueuer,
): void {
  deriveAuthoritativeFileSendRoute(room);
  enqueuer(room.id, paths);
}

export function enqueueTransferInputsWithBridgeRoute<TInput>(
  room: RoomInfo,
  inputs: TInput[],
  contentKind: Extract<BridgeContentKind, "file" | "image" | "pasted_image">,
  enqueuer: TransferInputEnqueuer<TInput>,
): void {
  if (contentKind === "pasted_image" || contentKind === "image") {
    deriveAuthoritativeImageSendRoute(room);
  } else {
    deriveAuthoritativeFileSendRoute(room);
  }
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
      return state.errors[0] ?? "No routeable Bridge peer is available.";
    case "requires_explicit_selection":
      return state.errors[0] ?? "Multiple Bridge peers require explicit selection.";
    case "invalid":
      return state.errors[0] ?? "Bridge route derivation failed.";
  }
}

function deriveAuthoritativeSelectedPeerRoute(
  input: LegacyRoomBridgeInput | BridgeRoutingRuntimeState | BridgeRoute,
  contentKind: "text" | "file" | "image" | "control" | "capability",
): BridgeRoute {
  const route = routeFromRuntimeInput(input);
  const validation = validateBridgeRoute(route);
  if (!validation.valid) {
    throw new Error(validation.errors.join(" "));
  }
  if (validation.route.target.kind !== "selected_peer") {
    throw new Error(`Production ${contentKind} send requires exactly one selected Bridge peer.`);
  }
  return validation.route;
}

function selectedPeerRoutePayload<TSchemaVersion extends string>(
  route: BridgeRoute,
  schemaVersion: TSchemaVersion,
): SelectedPeerBridgeRoutePayload<TSchemaVersion> {
  if (route.target.kind !== "selected_peer") {
    throw new Error("Production send route payload requires exactly one selected Bridge peer.");
  }
  return {
    schemaVersion,
    bridgeSessionId: route.bridgeSessionId,
    target: {
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
    throw new Error(`Production ${contentKind} send requires the active Bridge session.`);
  }
  if (event.sourceDeviceRef !== session.localSessionRef) {
    throw new Error(`Production ${contentKind} send requires the active local session source.`);
  }
  if (event.targetPeerRef !== session.peerSessionRef) {
    throw new Error(`Production ${contentKind} send requires exactly one selected Bridge peer target.`);
  }
  if (!session.peerConnected) {
    throw new Error(`Production ${contentKind} send requires a connected selected Bridge peer.`);
  }

  const route = deriveAuthoritativeSelectedPeerRoute({
    bridgeSessionId: session.roomId,
    target: {
      kind: "selected_peer",
      peerSessionId: bridgePeerSessionId(session.peerSessionRef),
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

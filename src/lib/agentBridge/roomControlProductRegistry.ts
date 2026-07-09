import {
  validateRoomControlEvent,
  type CapabilityPreviewRoomControlEvent,
  type RoomControlEvent,
} from "./roomControlEvent";

export type RoomControlProductOwner = "hello_peer" | "request_file";

export interface RoomControlProductRegistry {
  readonly previews: ReadonlyMap<string, RoomControlProductPreviewRecord>;
  readonly processedEventIds: readonly string[];
}

export interface RoomControlProductPreviewRecord {
  readonly owner: RoomControlProductOwner;
  readonly capability: string;
  readonly envelopeId: string;
  readonly requestId: string;
  readonly sourcePreviewEventId: string;
  readonly roomRef: string;
  readonly sourceDeviceRef: string;
  readonly targetPeerRef: string;
  readonly expiresAt: string;
}

export interface RoutedRoomControlInbox {
  readonly registry: RoomControlProductRegistry;
  readonly helloPeer: readonly RoomControlEvent[];
  readonly requestFile: readonly RoomControlEvent[];
  readonly ignoredEventIds: readonly string[];
}

const MAX_PROCESSED_EVENT_IDS = 256;

export function createRoomControlProductRegistry(): RoomControlProductRegistry {
  return {
    previews: new Map(),
    processedEventIds: [],
  };
}

export function registerOutboundCapabilityPreview(
  registry: RoomControlProductRegistry,
  event: CapabilityPreviewRoomControlEvent,
  owner: RoomControlProductOwner,
  now = new Date(),
): RoomControlProductRegistry {
  const previews = pruneExpiredPreviews(registry.previews, now);
  const record: RoomControlProductPreviewRecord = {
    owner,
    capability: event.payload.request.capability,
    envelopeId: event.payload.envelopeId,
    requestId: event.payload.request.requestId,
    sourcePreviewEventId: event.eventId,
    roomRef: event.roomRef,
    sourceDeviceRef: event.sourceDeviceRef,
    targetPeerRef: event.targetPeerRef,
    expiresAt: event.expiresAt,
  };
  previews.set(previewKey(record), record);
  return {
    ...registry,
    previews,
  };
}

export function routeRoomControlInboxEvents(
  registry: RoomControlProductRegistry,
  events: readonly unknown[],
  options: {
    now?: Date;
    expectedRoomRef: string;
    expectedSourceDeviceRef: string;
    expectedTargetPeerRef: string;
  },
): RoutedRoomControlInbox {
  const now = options.now ?? new Date();
  const processed = new Set(registry.processedEventIds);
  const nextProcessed = [...registry.processedEventIds];
  const previews = pruneExpiredPreviews(registry.previews, now);
  const helloPeer: RoomControlEvent[] = [];
  const requestFile: RoomControlEvent[] = [];
  const ignoredEventIds: string[] = [];

  for (const candidate of events) {
    const validation = validateRoomControlEvent(candidate, {
      now,
      expectedRoomRef: options.expectedRoomRef,
      expectedSourceDeviceRef: options.expectedSourceDeviceRef,
      expectedTargetPeerRef: options.expectedTargetPeerRef,
    });
    if (!validation.valid) continue;
    const event = validation.value;
    if (processed.has(event.eventId)) {
      ignoredEventIds.push(event.eventId);
      continue;
    }
    processed.add(event.eventId);
    nextProcessed.push(event.eventId);
    const owner = ownerForEvent(event, previews);
    if (owner === "hello_peer") helloPeer.push(event);
    if (owner === "request_file") requestFile.push(event);
  }

  return {
    registry: {
      previews,
      processedEventIds: nextProcessed.slice(-MAX_PROCESSED_EVENT_IDS),
    },
    helloPeer,
    requestFile,
    ignoredEventIds,
  };
}

function ownerForEvent(
  event: RoomControlEvent,
  previews: ReadonlyMap<string, RoomControlProductPreviewRecord>,
): RoomControlProductOwner | null {
  if (event.kind === "capability_preview") {
    return ownerForCapability(event.payload.request.capability);
  }
  if (
    event.kind === "capability_preview_ack"
    || event.kind === "capability_preview_deny"
    || event.kind === "capability_preview_invalid"
    || event.kind === "capability_preview_expired"
  ) {
    return previews.get(previewKey({
      envelopeId: event.payload.envelopeId,
      requestId: event.payload.requestId,
      roomRef: event.roomRef,
      sourceDeviceRef: event.targetPeerRef ?? "",
      targetPeerRef: event.sourceDeviceRef,
    }))?.owner ?? null;
  }
  if (event.kind === "capability_execute_request") {
    return ownerForCapability(event.payload.capability);
  }
  if (event.kind === "capability_execution_result" && "capability" in event.payload) {
    return ownerForCapability(event.payload.capability);
  }
  return null;
}

function ownerForCapability(capability: string): RoomControlProductOwner | null {
  if (capability === "runtime.hello_stdout" || capability === "runtime.execute_hello_template") {
    return "hello_peer";
  }
  if (capability === "filesystem.find_file_candidates" || capability === "transfer.request_candidate_payload") {
    return "request_file";
  }
  return null;
}

function previewKey(input: {
  envelopeId: string;
  requestId: string;
  roomRef: string;
  sourceDeviceRef: string;
  targetPeerRef: string;
}): string {
  return [
    input.roomRef,
    input.sourceDeviceRef,
    input.targetPeerRef,
    input.envelopeId,
    input.requestId,
  ].join("\u001f");
}

function pruneExpiredPreviews(
  current: ReadonlyMap<string, RoomControlProductPreviewRecord>,
  now: Date,
): Map<string, RoomControlProductPreviewRecord> {
  return new Map(
    [...current].filter(([, record]) => Date.parse(record.expiresAt) > now.getTime()),
  );
}

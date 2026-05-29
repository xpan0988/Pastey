import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { BottomTabBar, type TabKey } from "./components/BottomTabBar";
import { DevicesPage } from "./pages/DevicesPage";
import { RoomPage } from "./pages/RoomPage";
import { RoomsPage } from "./pages/RoomsPage";
import { SettingsPage } from "./pages/SettingsPage";
import {
  acceptNearbyJoin,
  burnRoom,
  cancelTransfer,
  deleteTempFile,
  getConfig,
  getFileTransferMetadata,
  getRoom,
  listRoomItems,
  listRooms,
  markJoinPromptRendered,
  pendingJoinRequests,
  rejectNearbyJoin,
  sendFileToRoom
} from "./lib/tauri";
import { FILE_TOO_LARGE_MESSAGE, MAX_FILE_SIZE_BYTES } from "./lib/constants";
import {
  activeCancellableTransferIds,
  cancelBatchLocally,
  cancelQueueItem,
  clearQueuedItemsForRoom,
  correlateTransferProgress,
  createTransferSchedulerState,
  enqueueTransferBatch,
  fileIdentityKey,
  hasNonterminalDedupeKey,
  markQueueItemCancelled,
  markQueueItemCompleted,
  markQueueItemFailed,
  markQueueItemMetadataFailed,
  markQueueItemMetadataLoading,
  markQueueItemMetadataReady,
  markQueueItemPreparing,
  markQueueItemSending,
  planRunnableTransferLaunches,
  queuedItemsNeedingMetadata,
  selectRoomTransferQueue,
  type TransferQueueInput,
  type TransferSchedulerState
} from "./lib/transferScheduler";
import { mergeTransferEvent } from "./lib/transferState";
import type { AppConfig, FileTransferProgressEvent, JoinRequestPrompt, RoomInfo, RoomItem } from "./lib/types";

type View =
  | { screen: "tabs" }
  | { screen: "room"; roomId: string };

interface PreparedQueueMetadata {
  displayName: string;
  mimeType?: string | null;
  sizeBytes: number;
  modifiedMs: number;
  dedupeKey: string;
}

interface FocusPayload {
  target?: "home" | "settings";
}

function App() {
  const [view, setView] = useState<View>({ screen: "tabs" });
  const [activeTab, setActiveTab] = useState<TabKey>("devices");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [rooms, setRooms] = useState<RoomInfo[]>([]);
  const [currentRoom, setCurrentRoom] = useState<RoomInfo | null>(null);
  const [roomItems, setRoomItems] = useState<RoomItem[]>([]);
  const [transfers, setTransfers] = useState<Record<string, FileTransferProgressEvent>>({});
  const [scheduler, setScheduler] = useState<TransferSchedulerState>(() => createTransferSchedulerState());
  const [joinRequest, setJoinRequest] = useState<JoinRequestPrompt | null>(null);
  const [focusToken, setFocusToken] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const closedRoomIdsRef = useRef<Set<string>>(new Set());
  const schedulerRef = useRef(scheduler);
  const viewRef = useRef(view);
  const launchingQueueItemWindowsRef = useRef<Map<string, number>>(new Map());
  const metadataPreflightItemIdsRef = useRef<Set<string>>(new Set());
  const cancellingQueueTransferIdsRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    schedulerRef.current = scheduler;
  }, [scheduler]);

  useEffect(() => {
    viewRef.current = view;
  }, [view]);

  function updateSchedulerState(updater: (current: TransferSchedulerState) => TransferSchedulerState) {
    setScheduler((current) => {
      const next = updater(current);
      schedulerRef.current = next;
      return next;
    });
  }

  useEffect(() => {
    async function load() {
      try {
        const [nextConfig, nextRooms] = await Promise.all([getConfig(), listRooms()]);
        setConfig(nextConfig);
        setRooms(nextRooms);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    }

    void load();
  }, []);

  useEffect(() => {
    void pendingJoinRequests().then((requests) => {
      if (requests.length > 0) {
        setJoinRequest(requests[0]);
      }
    });
  }, []);

  useEffect(() => {
    let unlistenFocus: (() => void) | undefined;
    let unlistenTransfer: (() => void) | undefined;
    let unlistenJoinRequest: (() => void) | undefined;

    void listen<FocusPayload>("pastey://focus", (event) => {
      const target = event.payload.target ?? "home";
      setView({ screen: "tabs" });
      setActiveTab(target === "settings" ? "settings" : "devices");
      setFocusToken((value) => value + 1);
    }).then((fn) => {
      unlistenFocus = fn;
    });

    void listen<FileTransferProgressEvent>("pastey://transfer-progress", (event) => {
      if (closedRoomIdsRef.current.has(event.payload.room_id)) {
        return;
      }
      setTransfers((current) => mergeTransferEvent(current, event.payload, closedRoomIdsRef.current));
      updateSchedulerState((current) => correlateTransferProgress(current, {
        roomId: event.payload.room_id,
        queueItemId: event.payload.queue_item_id,
        direction: event.payload.direction,
        fileName: event.payload.file_name,
        fileSize: event.payload.file_size,
        transferId: event.payload.transfer_id,
        status: event.payload.status
      }));
      if (event.payload.status === "completed") {
        void refreshCurrentRoom();
      }
    }).then((fn) => {
      unlistenTransfer = fn;
    });

    void listen<JoinRequestPrompt>("pastey://join-request", (event) => {
      setJoinRequest(event.payload);
    }).then((fn) => {
      unlistenJoinRequest = fn;
    });

    return () => {
      if (unlistenFocus) unlistenFocus();
      if (unlistenTransfer) unlistenTransfer();
      if (unlistenJoinRequest) unlistenJoinRequest();
    };
  }, [view]);

  useEffect(() => {
    const metadataItems = queuedItemsNeedingMetadata(
      scheduler,
      rooms,
      closedRoomIdsRef.current,
      metadataPreflightItemIdsRef.current
    );

    for (const item of metadataItems) {
      metadataPreflightItemIdsRef.current.add(item.id);
      void prepareQueueItemMetadata(item.id)
        .catch((err) => {
          const message = err instanceof Error ? err.message : String(err);
          updateSchedulerState((current) => markQueueItemMetadataFailed(current, item.id, message));
          void refreshRoomAfterQueueItem(item.roomId);
        })
        .finally(() => {
          metadataPreflightItemIdsRef.current.delete(item.id);
          updateSchedulerState((current) => ({ ...current }));
        });
    }
  }, [scheduler, rooms]);

  useEffect(() => {
    const { runnablePlans } = planRunnableTransferLaunches(
      scheduler,
      rooms,
      closedRoomIdsRef.current,
      launchingQueueItemWindowsRef.current
    );

    for (const plan of runnablePlans) {
      if (launchingQueueItemWindowsRef.current.has(plan.itemId)) {
        continue;
      }

      launchingQueueItemWindowsRef.current.set(plan.itemId, plan.requestedWindow);
      void processTransferQueueItem(plan.itemId, plan.requestedWindow).finally(() => {
        launchingQueueItemWindowsRef.current.delete(plan.itemId);
        updateSchedulerState((current) => ({ ...current }));
      });
    }
  }, [scheduler, rooms]);

  useEffect(() => {
    for (const itemId of [...launchingQueueItemWindowsRef.current.keys()]) {
      const item = scheduler.items[itemId];
      if (!item || item.status === "completed" || item.status === "failed" || item.status === "cancelled") {
        launchingQueueItemWindowsRef.current.delete(itemId);
      }
    }

    for (const itemId of [...metadataPreflightItemIdsRef.current]) {
      const item = scheduler.items[itemId];
      if (!item || item.status !== "queued" || item.metadataStatus !== "unknown") {
        metadataPreflightItemIdsRef.current.delete(itemId);
      }
    }
  }, [scheduler]);

  useEffect(() => {
    return () => {
      launchingQueueItemWindowsRef.current.clear();
      metadataPreflightItemIdsRef.current.clear();
    };
  }, []);

  useEffect(() => {
    const transferIds = activeCancellableTransferIds(scheduler);
    for (const transferId of transferIds) {
      if (cancellingQueueTransferIdsRef.current.has(transferId)) {
        continue;
      }

      cancellingQueueTransferIdsRef.current.add(transferId);
      void cancelTransfer(transferId).catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
      });
    }
  }, [scheduler]);

  async function refreshRooms(selectedRoomId?: string): Promise<RoomInfo | null> {
    const nextRooms = await listRooms();
    setRooms(nextRooms);

    if (selectedRoomId) {
      const match = nextRooms.find((room) => room.id === selectedRoomId) ?? null;
      setCurrentRoom(match);
      return match;
    }

    return null;
  }

  async function openRoom(room: RoomInfo) {
    closedRoomIdsRef.current.delete(room.id);
    setView({ screen: "room", roomId: room.id });
    try {
      const [nextRoom, nextItems] = await Promise.all([getRoom(room.id), listRoomItems(room.id)]);
      setCurrentRoom(nextRoom);
      setRoomItems(nextItems);
      await refreshRooms(room.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function refreshCurrentRoom() {
    if (view.screen !== "room") return;
    try {
      const [nextRoom, nextItems] = await Promise.all([getRoom(view.roomId), listRoomItems(view.roomId)]);
      setCurrentRoom(nextRoom);
      setRoomItems(nextItems);
      const visibleRoom = await refreshRooms(view.roomId);
      if (!visibleRoom) {
        setView({ screen: "tabs" });
        setRoomItems([]);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (
        message === "room not found" ||
        message === "File is no longer available" ||
        message === "File is no longer available."
      ) {
        setView({ screen: "tabs" });
        setCurrentRoom(null);
        setRoomItems([]);
        return;
      }

      setError(message);
    }
  }

  async function refreshRoomAfterQueueItem(roomId: string) {
    const currentView = viewRef.current;
    if (currentView.screen !== "room" || currentView.roomId !== roomId) {
      await refreshRooms();
      return;
    }

    try {
      const [nextRoom, nextItems] = await Promise.all([getRoom(roomId), listRoomItems(roomId)]);
      setCurrentRoom(nextRoom);
      setRoomItems(nextItems);
      const visibleRoom = await refreshRooms(roomId);
      if (!visibleRoom) {
        setView({ screen: "tabs" });
        setRoomItems([]);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (
        message === "room not found" ||
        message === "File is no longer available" ||
        message === "File is no longer available."
      ) {
        setView({ screen: "tabs" });
        setCurrentRoom(null);
        setRoomItems([]);
        return;
      }

      setError(message);
    }
  }

  async function processTransferQueueItem(itemId: string, requestedWindow: number) {
    updateSchedulerState((current) => markQueueItemPreparing(current, itemId, requestedWindow));

    try {
      let item = schedulerRef.current.items[itemId];
      if (!item || item.cancelRequested) {
        updateSchedulerState((current) => markQueueItemCancelled(current, itemId));
        return;
      }

      const metadata = await prepareQueueItemMetadata(itemId);
      if (!metadata) {
        updateSchedulerState((current) => markQueueItemCancelled(current, itemId));
        return;
      }

      item = schedulerRef.current.items[itemId];
      if (!item || item.cancelRequested) {
        updateSchedulerState((current) => markQueueItemCancelled(current, itemId));
        return;
      }

      if (metadata.sizeBytes > MAX_FILE_SIZE_BYTES) {
        updateSchedulerState((current) => markQueueItemFailed(current, itemId, FILE_TOO_LARGE_MESSAGE));
        await refreshRoomAfterQueueItem(item.roomId);
        return;
      }

      if (hasNonterminalDedupeKey(schedulerRef.current, metadata.dedupeKey, itemId)) {
        updateSchedulerState((current) => markQueueItemCancelled(current, itemId));
        return;
      }

      updateSchedulerState((current) => markQueueItemSending(current, itemId, {
        displayName: metadata.displayName,
        mimeType: metadata.mimeType,
        sizeBytes: metadata.sizeBytes,
        modifiedMs: metadata.modifiedMs,
        dedupeKey: metadata.dedupeKey
      }));

      item = schedulerRef.current.items[itemId];
      const batch = item ? schedulerRef.current.batches[item.batchId] : null;
      if (!item || item.cancelRequested || batch?.cancelRequested) {
        updateSchedulerState((current) => markQueueItemCancelled(current, itemId));
        return;
      }

      await sendFileToRoom(item.roomId, item.path, {
        displayName: metadata.displayName,
        mimeType: metadata.mimeType,
        queueItemId: item.id,
        requestedWindow
      });
      updateSchedulerState((current) => markQueueItemCompleted(current, itemId));
      await refreshRoomAfterQueueItem(item.roomId);
    } catch (err) {
      const latestItem = schedulerRef.current.items[itemId];
      const message = err instanceof Error ? err.message : String(err);
      updateSchedulerState((current) => {
        if (latestItem?.cancelRequested) {
          return markQueueItemCancelled(current, itemId);
        }

        return latestItem?.metadataStatus === "loading"
          ? markQueueItemMetadataFailed(current, itemId, message)
          : markQueueItemFailed(current, itemId, message);
      });
      if (latestItem && latestItem.metadataStatus !== "loading") {
        await refreshRoomAfterQueueItem(latestItem.roomId);
      }
    } finally {
      await cleanupSchedulerTempFile(itemId);
    }
  }

  async function prepareQueueItemMetadata(itemId: string): Promise<PreparedQueueMetadata | null> {
    const cached = cachedQueueItemMetadata(schedulerRef.current.items[itemId]);
    if (cached) {
      return cached;
    }

    updateSchedulerState((current) => markQueueItemMetadataLoading(current, itemId));

    let item = schedulerRef.current.items[itemId];
    if (!item || item.cancelRequested) {
      return null;
    }

    const metadata = await getFileTransferMetadata(item.path);
    item = schedulerRef.current.items[itemId];
    if (!item || item.cancelRequested) {
      return null;
    }

    const displayName = item.displayName?.trim() ? item.displayName : metadata.display_name;
    const prepared: PreparedQueueMetadata = {
      displayName,
      mimeType: item.mimeType ?? metadata.mime_type,
      sizeBytes: metadata.size_bytes,
      modifiedMs: metadata.modified_ms,
      dedupeKey: item.dedupeKey ?? fileIdentityKey(displayName, metadata.size_bytes, metadata.modified_ms)
    };

    updateSchedulerState((current) => markQueueItemMetadataReady(current, itemId, prepared));
    return prepared;
  }

  function cachedQueueItemMetadata(item: TransferSchedulerState["items"][string] | undefined): PreparedQueueMetadata | null {
    if (
      !item ||
      item.metadataStatus !== "ready" ||
      !item.displayName ||
      typeof item.sizeBytes !== "number" ||
      typeof item.modifiedMs !== "number" ||
      !item.dedupeKey
    ) {
      return null;
    }

    return {
      displayName: item.displayName,
      mimeType: item.mimeType,
      sizeBytes: item.sizeBytes,
      modifiedMs: item.modifiedMs,
      dedupeKey: item.dedupeKey
    };
  }

  async function cleanupSchedulerTempFile(itemId: string) {
    const item = schedulerRef.current.items[itemId];
    if (!item?.deleteWhenDone) {
      return;
    }

    try {
      await deleteTempFile(item.path);
    } catch {
      // Scheduler-created temp files are best-effort cleanup; send state is authoritative.
    }
  }

  function enqueueRoomFiles(roomId: string, paths: string[]) {
    updateSchedulerState((current) => enqueueTransferBatch(
      current,
      roomId,
      paths.map((path) => ({ path }))
    ));
  }

  function enqueueRoomTransferInputs(roomId: string, inputs: TransferQueueInput[]) {
    updateSchedulerState((current) => enqueueTransferBatch(current, roomId, inputs));
  }

  async function handleCancelQueueItem(itemId: string) {
    const item = schedulerRef.current.items[itemId];
    const transferId = item?.status === "sending" ? item.activeTransferId : undefined;
    updateSchedulerState((current) => cancelQueueItem(current, itemId));

    if (transferId) {
      await cancelTransfer(transferId);
    }
  }

  async function handleCancelQueueBatch(batchId: string) {
    const transferIds = Object.values(schedulerRef.current.items)
      .filter((item) => item.batchId === batchId && item.status === "sending" && item.activeTransferId)
      .map((item) => item.activeTransferId)
      .filter((transferId): transferId is string => Boolean(transferId));

    updateSchedulerState((current) => cancelBatchLocally(current, batchId));

    for (const transferId of transferIds) {
      await cancelTransfer(transferId);
    }
  }

  async function handleBurnRoom(roomId: string) {
    updateSchedulerState((current) => clearQueuedItemsForRoom(current, roomId));
    await burnRoom(roomId);
    closedRoomIdsRef.current.add(roomId);
    setView({ screen: "tabs" });
    setActiveTab("rooms");
    setCurrentRoom(null);
    setRoomItems([]);
    setTransfers((current) => Object.fromEntries(Object.entries(current).filter(([, transfer]) => transfer.room_id !== roomId)));
    await refreshRooms();
  }

  async function handleAcceptJoinRequest(request: JoinRequestPrompt) {
    try {
      const room = await acceptNearbyJoin(request.request_id);
      setJoinRequest(null);
      await openRoom(room);
    } catch (err) {
      setJoinRequest(null);
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  useEffect(() => {
    if (joinRequest) {
      void markJoinPromptRendered();
    }
  }, [joinRequest]);

  async function handleRejectJoinRequest(request: JoinRequestPrompt) {
    try {
      await rejectNearbyJoin(request.request_id);
    } finally {
      setJoinRequest(null);
    }
  }

  if (!config) {
    return (
      <div className="app-shell center-panel">
        <div className="panel">{error ?? "Loading pastey..."}</div>
      </div>
    );
  }

  return (
    <div className="app-shell">
      {error ? <div className="error-box">{error}</div> : null}
      {joinRequest ? (
        <div className="join-request-banner panel">
          <div className="subtle-stack tight">
            <strong>{joinRequest.device_name} wants to join.</strong>
            <span className="muted">
              {joinRequest.platform} • Pastey {joinRequest.app_version}
            </span>
          </div>
          <div className="row gap">
            <button className="primary-button" onClick={() => void handleAcceptJoinRequest(joinRequest)}>
              Accept
            </button>
            <button className="ghost-button" onClick={() => void handleRejectJoinRequest(joinRequest)}>
              Reject
            </button>
          </div>
        </div>
      ) : null}

      <main>
        {view.screen === "tabs" && activeTab === "devices" ? (
          <DevicesPage rooms={rooms} onOpenRoom={(room) => void openRoom(room)} shouldFocus={focusToken > 0} />
        ) : null}

        {view.screen === "tabs" && activeTab === "rooms" ? (
          <RoomsPage
            config={config}
            rooms={rooms}
            transfers={Object.values(transfers)}
            onOpenRoom={(room) => void openRoom(room)}
            onConfigChange={setConfig}
          />
        ) : null}

        {view.screen === "room" && currentRoom ? (
          <RoomPage
            room={currentRoom}
            items={roomItems}
            transfers={Object.values(transfers).filter((transfer) => transfer.room_id === currentRoom.id)}
            queue={selectRoomTransferQueue(scheduler, currentRoom.id)}
            onBack={() => {
              setView({ screen: "tabs" });
              void refreshRooms();
            }}
            onRefresh={refreshCurrentRoom}
            onBurn={handleBurnRoom}
            onEnqueueFiles={enqueueRoomFiles}
            onEnqueueTransferInputs={enqueueRoomTransferInputs}
            onCancelQueueItem={handleCancelQueueItem}
            onCancelQueueBatch={handleCancelQueueBatch}
          />
        ) : null}

        {view.screen === "tabs" && activeTab === "settings" ? (
          <SettingsPage
            config={config}
            onConfigChange={setConfig}
            onJoinWithCode={() => {
              setActiveTab("devices");
              setFocusToken((value) => value + 1);
            }}
          />
        ) : null}
      </main>

      {view.screen === "tabs" ? <BottomTabBar activeTab={activeTab} onSelectTab={setActiveTab} /> : null}
    </div>
  );
}

export default App;

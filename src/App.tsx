import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import type { MutableRefObject } from "react";
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
  sendFileToRoom,
  updateTransferWindow
} from "./lib/tauri";
import { FILE_TOO_LARGE_MESSAGE, MAX_FILE_SIZE_BYTES } from "./lib/constants";
import {
  activeCancellableTransferRequests,
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
  markQueueItemRuntimeWindow,
  markQueueItemSending,
  planActiveTransferWindowRebalances,
  planRunnableTransferLaunches,
  queuedItemsNeedingMetadata,
  selectRoomTransferQueue,
  type TransferLaunchPlannerResult,
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
  const runtimeWindowUpdateKeysRef = useRef<Set<string>>(new Set());
  const plannerLaunchSummaryKeyRef = useRef<string>("");

  useEffect(() => {
    schedulerRef.current = scheduler;
  }, [scheduler]);

  useEffect(() => {
    viewRef.current = view;
  }, [view]);

  function updateSchedulerState(updater: (current: TransferSchedulerState) => TransferSchedulerState): TransferSchedulerState {
    const next = updater(schedulerRef.current);
    schedulerRef.current = next;
    setScheduler(next);
    return next;
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
      console.info("[pastey queue] event=metadata_preflight_start room_id=%s queue_item_id=%s path=%s", item.roomId, item.id, item.path);
      metadataPreflightItemIdsRef.current.add(item.id);
      void prepareQueueItemMetadata(item.id)
        .catch((err) => {
          const message = err instanceof Error ? err.message : String(err);
          console.info("[pastey queue] event=metadata_preflight_failed room_id=%s queue_item_id=%s error=%s", item.roomId, item.id, message);
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
    const launchPlan = planRunnableTransferLaunches(
      scheduler,
      rooms,
      closedRoomIdsRef.current,
      launchingQueueItemWindowsRef.current
    );
    logPlannerLaunchSummary(
      scheduler,
      launchPlan,
      plannerLaunchSummaryKeyRef,
      launchingQueueItemWindowsRef.current
    );
    const { runnablePlans } = launchPlan;

    if (runnablePlans.length > 0) {
      console.info("[pastey queue] event=planner_launch_plan_count count=%d", runnablePlans.length);
    }

    for (const plan of runnablePlans) {
      if (launchingQueueItemWindowsRef.current.has(plan.itemId)) {
        continue;
      }

      launchingQueueItemWindowsRef.current.set(plan.itemId, plan.requestedWindow);
      const item = scheduler.items[plan.itemId];
      console.info(
        "[pastey queue] event=planner_launch_start room_id=%s queue_item_id=%s display_name=%s size_bytes=%s requested_window=%d lane=%s",
        plan.roomId,
        plan.itemId,
        item?.displayName ?? "unknown",
        typeof item?.sizeBytes === "number" ? String(item.sizeBytes) : "unknown",
        plan.requestedWindow,
        plan.lane
      );
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

    const activeTransferIds = new Set(
      Object.values(scheduler.items)
        .filter((item) => item.status === "sending" && item.activeTransferId)
        .map((item) => item.activeTransferId)
    );
    for (const updateKey of [...runtimeWindowUpdateKeysRef.current]) {
      const transferId = updateKey.split(":")[0];
      if (!activeTransferIds.has(transferId)) {
        runtimeWindowUpdateKeysRef.current.delete(updateKey);
      }
    }
  }, [scheduler]);

  useEffect(() => {
    return () => {
      launchingQueueItemWindowsRef.current.clear();
      metadataPreflightItemIdsRef.current.clear();
      runtimeWindowUpdateKeysRef.current.clear();
    };
  }, []);

  useEffect(() => {
    const transferRequests = activeCancellableTransferRequests(scheduler);
    for (const request of transferRequests) {
      if (cancellingQueueTransferIdsRef.current.has(request.transferId)) {
        continue;
      }

      cancellingQueueTransferIdsRef.current.add(request.transferId);
      void cancelTransfer(request.transferId, {
        source: "queue-cancel-effect",
        queueItemId: request.itemId,
        batchId: request.batchId,
        roomId: request.roomId
      }).catch((err) => {
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
      void rebalanceActiveTransferWindows({ itemId, status: "completed" });
      await refreshRoomAfterQueueItem(item.roomId);
    } catch (err) {
      const latestItem = schedulerRef.current.items[itemId];
      const message = err instanceof Error ? err.message : String(err);
      let terminalStatus: "cancelled" | "failed" | null = null;
      updateSchedulerState((current) => {
        if (latestItem?.cancelRequested) {
          terminalStatus = "cancelled";
          return markQueueItemCancelled(current, itemId);
        }

        terminalStatus = latestItem?.metadataStatus === "loading" ? null : "failed";
        return latestItem?.metadataStatus === "loading"
          ? markQueueItemMetadataFailed(current, itemId, message)
          : markQueueItemFailed(current, itemId, message);
      });
      if (terminalStatus) {
        void rebalanceActiveTransferWindows({ itemId, status: terminalStatus });
      }
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

    console.info(
      "[pastey queue] event=metadata_preflight_ready room_id=%s queue_item_id=%s display_name=%s size_bytes=%d",
      item.roomId,
      item.id,
      prepared.displayName,
      prepared.sizeBytes
    );
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

  async function rebalanceActiveTransferWindows(trigger?: { itemId: string; status: "completed" | "failed" | "cancelled" }) {
    const plans = planActiveTransferWindowRebalances(
      schedulerRef.current,
      rooms,
      closedRoomIdsRef.current,
      launchingQueueItemWindowsRef.current
    );
    if (trigger) {
      const triggerItem = schedulerRef.current.items[trigger.itemId];
      console.info(
        "[pastey queue] event=rebalance_trigger room_id=%s batch_id=%s queue_item_id=%s status=%s",
        triggerItem?.roomId ?? "unknown",
        triggerItem?.batchId ?? "unknown",
        trigger.itemId,
        trigger.status
      );
    }
    console.info(
      "[pastey queue] event=rebalance_active_targets count=%d targets=%s",
      plans.length,
      plans.map((plan) => `${plan.itemId}:${plan.transferId}:${plan.previousWindow}->${plan.requestedWindow}`).join(",")
    );

    for (const plan of plans) {
      const updateKey = `${plan.transferId}:${plan.requestedWindow}`;
      if (runtimeWindowUpdateKeysRef.current.has(updateKey)) {
        continue;
      }

      runtimeWindowUpdateKeysRef.current.add(updateKey);
      const item = schedulerRef.current.items[plan.itemId];
      console.info(
        "[pastey queue] event=rebalance_update_attempt room_id=%s batch_id=%s queue_item_id=%s transfer_id=%s previous_window=%d requested_window=%d",
        item?.roomId ?? "unknown",
        item?.batchId ?? "unknown",
        plan.itemId,
        plan.transferId,
        plan.previousWindow,
        plan.requestedWindow
      );
      void updateTransferWindow(plan.transferId, plan.requestedWindow)
        .then((result) => {
          console.info(
            "[pastey queue] event=rebalance_update_result queue_item_id=%s transfer_id=%s updated=%s reason=%s previous_window=%s effective_window=%s requested_window=%d",
            plan.itemId,
            plan.transferId,
            String(result.updated),
            result.reason ?? "none",
            result.previous_window === null || typeof result.previous_window === "undefined" ? "unknown" : String(result.previous_window),
            result.effective_window === null || typeof result.effective_window === "undefined" ? "unknown" : String(result.effective_window),
            plan.requestedWindow
          );
          const effectiveWindow = result.effective_window ?? null;
          if (
            (result.updated || result.reason === "unchanged") &&
            typeof effectiveWindow === "number"
          ) {
            updateSchedulerState((current) => markQueueItemRuntimeWindow(
              current,
              plan.itemId,
              effectiveWindow
            ));
          }
        })
        .catch((err) => {
          setError(err instanceof Error ? err.message : String(err));
        });
    }
  }

  function enqueueRoomFiles(roomId: string, paths: string[]) {
    console.info("[pastey queue] event=queue_enqueue_attempt room_id=%s file_count=%d", roomId, paths.length);
    updateSchedulerState((current) => {
      const next = enqueueTransferBatch(
        current,
        roomId,
        paths.map((path) => ({ path }))
      );
      if (next === current) {
        console.info("[pastey queue] event=queue_enqueue_rejected room_id=%s reason=no_new_items", roomId);
      }
      return next;
    });
  }

  function enqueueRoomTransferInputs(roomId: string, inputs: TransferQueueInput[]) {
    console.info("[pastey queue] event=queue_enqueue_attempt room_id=%s file_count=%d", roomId, inputs.length);
    updateSchedulerState((current) => {
      const next = enqueueTransferBatch(current, roomId, inputs);
      if (next === current) {
        console.info("[pastey queue] event=queue_enqueue_rejected room_id=%s reason=no_new_items", roomId);
      }
      return next;
    });
  }

  async function handleCancelQueueItem(itemId: string) {
    const item = schedulerRef.current.items[itemId];
    const transferId = item?.status === "sending" ? item.activeTransferId : undefined;
    updateSchedulerState((current) => cancelQueueItem(current, itemId));

    if (transferId && item) {
      await cancelTransfer(transferId, {
        source: "queue-item-cancel",
        queueItemId: item.id,
        batchId: item.batchId,
        roomId: item.roomId
      });
    }
  }

  async function handleCancelQueueBatch(batchId: string) {
    const transferRequests = Object.values(schedulerRef.current.items)
      .filter((item) => item.batchId === batchId && item.status === "sending" && item.activeTransferId)
      .map((item) => ({
        transferId: item.activeTransferId as string,
        itemId: item.id,
        batchId: item.batchId,
        roomId: item.roomId
      }));

    updateSchedulerState((current) => cancelBatchLocally(current, batchId));

    for (const request of transferRequests) {
      await cancelTransfer(request.transferId, {
        source: "queue-batch-cancel",
        queueItemId: request.itemId,
        batchId: request.batchId,
        roomId: request.roomId
      });
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

function logPlannerLaunchSummary(
  scheduler: TransferSchedulerState,
  launchPlan: TransferLaunchPlannerResult,
  lastSummaryKeyRef: MutableRefObject<string>,
  launchingItemWindows: ReadonlyMap<string, number>
) {
  const candidates = Object.values(scheduler.items).filter((item) => (
    item.status === "queued" || item.status === "preparing" || item.status === "sending"
  ));
  if (candidates.length === 0 && launchPlan.runnablePlans.length === 0 && launchPlan.plannerResult.heldPlans.length === 0) {
    return;
  }

  const metadataReadyCount = candidates.filter((item) => (
    item.metadataStatus === "ready" && typeof item.sizeBytes === "number"
  )).length;
  const activeCandidateCount = candidates.filter((item) => (
    item.status === "preparing" || item.status === "sending" || launchingItemWindows.has(item.id)
  )).length;
  const heldReasonCounts = launchPlan.plannerResult.heldPlans.reduce<Record<string, number>>((counts, plan) => {
    counts[plan.reason] = (counts[plan.reason] ?? 0) + 1;
    return counts;
  }, {});
  const runnableDetails = launchPlan.runnablePlans.map((plan) => {
    const item = scheduler.items[plan.itemId];
    return {
      itemId: plan.itemId,
      roomId: plan.roomId,
      displayName: item?.displayName ?? null,
      sizeBytes: item?.sizeBytes ?? null,
      requestedWindow: plan.requestedWindow,
      lane: plan.lane
    };
  });
  const summaryKey = JSON.stringify({
    candidates: candidates.map((item) => [
      item.id,
      item.status,
      item.metadataStatus,
      item.sizeBytes ?? null,
      item.requestedWindow ?? null,
      item.activeTransferId ?? null,
      item.cancelRequested
    ]),
    launching: [...launchingItemWindows.entries()],
    runnableDetails,
    heldReasonCounts
  });
  if (summaryKey === lastSummaryKeyRef.current) {
    return;
  }
  lastSummaryKeyRef.current = summaryKey;

  console.info(
    "[pastey queue] event=planner_launch_summary total_candidates=%d metadata_ready_candidates=%d active_candidates=%d runnable_count=%d held_reasons=%s runnable=%s",
    candidates.length,
    metadataReadyCount,
    activeCandidateCount,
    launchPlan.runnablePlans.length,
    JSON.stringify(heldReasonCounts),
    JSON.stringify(runnableDetails)
  );
}

export default App;

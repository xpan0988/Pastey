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
  logFrontendDiagnostic,
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
  completeMicroFlowGroupFromChildren,
  correlateTransferProgress,
  createTransferSchedulerState,
  enqueueTransferBatch,
  finishMicroFlowGroup,
  fileIdentityKey,
  hasNonterminalDedupeKey,
  isTerminalMicroFlowGroup,
  markMicroFlowGroupQueued,
  markMicroFlowGroupRunning,
  markQueueItemCancelled,
  markQueueItemCompleted,
  markQueueItemFailed,
  markQueueItemMetadataFailed,
  markQueueItemMetadataLoading,
  markQueueItemMetadataReady,
  markQueueItemPreparing,
  markQueueItemRuntimeWindow,
  markQueueItemSending,
  microGroupPlannerDiagnosticFields,
  planActiveTransferWindowRebalances,
  planRunnableTransferLaunches,
  queuedItemsNeedingMetadata,
  recordMicroFlowGroupChildTerminal,
  selectRoomTransferQueue,
  summarizeMicroFlowGroupPlanning,
  type TransferLaunchPlannerResult,
  type TransferQueueInput,
  type TransferQueueItemStatus,
  type TransferSchedulerState
} from "./lib/transferScheduler";
import {
  DEFAULT_TRANSFER_PLANNER_POLICY,
  type MicroFlowGroupMode,
  type TransferPlannerPolicy
} from "./lib/transferPlanner";
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

interface RuntimeWindowDiagnosticStats {
  transferId?: string;
  roomId: string;
  itemId: string;
  initialWindow: number;
  finalWindow: number;
  minWindow: number;
  maxWindow: number;
  updateCount: number;
  protocol: string;
  overrideSource: string;
}

type RuntimeWindowTerminalStatus = Extract<TransferQueueItemStatus, "completed" | "failed" | "cancelled"> | "interrupted";

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
  const runtimeWindowStatsRef = useRef<Map<string, RuntimeWindowDiagnosticStats>>(new Map());
  const plannerLaunchSummaryKeyRef = useRef<string>("");
  const serialMicroGroupRunningRef = useRef(false);

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
      recordRuntimeWindowTransferId(event.payload);
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
      console.info(
        "[pastey queue] event=metadata_preflight_start room_id=%s queue_item_id=%s display_name=%s",
        item.roomId,
        item.id,
        item.displayName ?? "unknown"
      );
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
    const plannerPolicy = microGroupPlannerPolicy(config?.micro_flow_group_mode);
    const launchPlan = planRunnableTransferLaunches(
      scheduler,
      rooms,
      closedRoomIdsRef.current,
      launchingQueueItemWindowsRef.current,
      false,
      plannerPolicy
    );
    logPlannerLaunchSummary(
      scheduler,
      launchPlan,
      plannerLaunchSummaryKeyRef,
      launchingQueueItemWindowsRef.current,
      rooms,
      closedRoomIdsRef.current,
      plannerPolicy
    );
    const { runnablePlans, microGroupPlans } = launchPlan;

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

    const microGroupPlan = serialMicroGroupRunningRef.current ? undefined : microGroupPlans[0];
    if (microGroupPlan) {
      serialMicroGroupRunningRef.current = true;
      for (const childItemId of microGroupPlan.childItemIds) {
        launchingQueueItemWindowsRef.current.set(childItemId, 0);
      }
      updateSchedulerState((current) => markMicroFlowGroupQueued(current, microGroupPlan));
      emitPasteyDiagnostic("[pastey:micro-group]", {
        event: "launched",
        group_id: microGroupPlan.groupId,
        room_id: microGroupPlan.roomId,
        children: microGroupPlan.childItemIds.length,
        requested_window: microGroupPlan.requestedWindow,
        total_bytes: microGroupPlan.totalBytes,
        dispatch_mode: microGroupPlan.dispatchMode
      });
      void processMicroFlowGroup(microGroupPlan.groupId, microGroupPlan.childItemIds, microGroupPlan.requestedWindow).finally(() => {
        for (const childItemId of microGroupPlan.childItemIds) {
          launchingQueueItemWindowsRef.current.delete(childItemId);
        }
        serialMicroGroupRunningRef.current = false;
        updateSchedulerState((current) => ({ ...current }));
      });
    }
  }, [scheduler, rooms, config?.micro_flow_group_mode]);

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

    const liveItemIds = new Set(Object.keys(scheduler.items));
    for (const [itemId, stats] of [...runtimeWindowStatsRef.current.entries()]) {
      const item = scheduler.items[itemId];
      if (!item) {
        runtimeWindowStatsRef.current.delete(itemId);
        continue;
      }
      if (item.status === "completed" || item.status === "failed" || item.status === "cancelled") {
        emitRuntimeWindowDiagnosticSummary(
          itemId,
          item.status,
          runtimeWindowTerminalReason(item.status, item)
        );
        continue;
      }
      if (!liveItemIds.has(itemId) || (stats.transferId && !activeTransferIds.has(stats.transferId) && item.status !== "sending")) {
        runtimeWindowStatsRef.current.delete(itemId);
      }
    }
  }, [scheduler]);

  useEffect(() => {
    return () => {
      launchingQueueItemWindowsRef.current.clear();
      metadataPreflightItemIdsRef.current.clear();
      runtimeWindowUpdateKeysRef.current.clear();
      runtimeWindowStatsRef.current.clear();
      serialMicroGroupRunningRef.current = false;
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

  function startRuntimeWindowDiagnostic(itemId: string, requestedWindow: number) {
    const item = schedulerRef.current.items[itemId];
    if (!item) {
      return;
    }

    runtimeWindowStatsRef.current.set(itemId, {
      roomId: item.roomId,
      itemId,
      initialWindow: requestedWindow,
      finalWindow: requestedWindow,
      minWindow: requestedWindow,
      maxWindow: requestedWindow,
      updateCount: 0,
      protocol: "unknown",
      overrideSource: "planner_request"
    });
    emitPasteyDiagnostic("[pastey:runtime-window]", {
      event: "tracking_started",
      room_id: item.roomId,
      queue_item_id: itemId,
      item_id: itemId,
      transfer_id: item.activeTransferId ?? "pending",
      initial_window: requestedWindow,
      final_window: requestedWindow,
      min_window: requestedWindow,
      max_window: requestedWindow,
      update_count: 0,
      protocol: "unknown",
      transfer_protocol: "unknown",
      override_source: "planner_request"
    });
  }

  function recordRuntimeWindowTransferId(progress: FileTransferProgressEvent) {
    if (progress.direction !== "outgoing" || !progress.queue_item_id) {
      return;
    }

    const stats = runtimeWindowStatsRef.current.get(progress.queue_item_id);
    if (!stats) {
      return;
    }

    stats.transferId = progress.transfer_id;
  }

  function recordRuntimeWindowUpdateResult(
    plan: { itemId: string; transferId: string; previousWindow: number; requestedWindow: number },
    result: Awaited<ReturnType<typeof updateTransferWindow>>
  ) {
    const item = schedulerRef.current.items[plan.itemId];
    let stats = runtimeWindowStatsRef.current.get(plan.itemId);
    if (!stats) {
      stats = {
        roomId: item?.roomId ?? "unknown",
        itemId: plan.itemId,
        initialWindow: plan.previousWindow,
        finalWindow: plan.previousWindow,
        minWindow: plan.previousWindow,
        maxWindow: plan.previousWindow,
        updateCount: 0,
        protocol: "unknown",
        overrideSource: "planner_request"
      };
      runtimeWindowStatsRef.current.set(plan.itemId, stats);
    }

    const effectiveWindow = result.effective_window ?? null;
    stats.transferId = result.transfer_id || plan.transferId;
    if (result.updated) {
      stats.updateCount += 1;
    }
    if (typeof effectiveWindow === "number") {
      stats.finalWindow = effectiveWindow;
      stats.minWindow = Math.min(stats.minWindow, effectiveWindow);
      stats.maxWindow = Math.max(stats.maxWindow, effectiveWindow);
    }
    if (result.reason === "unsupported_protocol") {
      stats.protocol = "unsupported";
    } else if (result.updated || result.reason === "unchanged") {
      stats.protocol = "binary-v1";
    }

    emitPasteyDiagnostic("[pastey:runtime-window]", {
      event: "update",
      room_id: item?.roomId ?? stats.roomId,
      queue_item_id: plan.itemId,
      item_id: plan.itemId,
      transfer_id: stats.transferId,
      previous_window: plan.previousWindow,
      requested_window: plan.requestedWindow,
      effective_window: typeof effectiveWindow === "number" ? effectiveWindow : "unknown",
      updated: result.updated,
      reason: result.reason,
      update_count: stats.updateCount,
      protocol: stats.protocol,
      override_source: stats.overrideSource
    });
  }

  function emitRuntimeWindowDiagnosticSummary(
    itemId: string,
    terminalStatus: RuntimeWindowTerminalStatus,
    terminalReason: string = terminalStatus
  ) {
    const stats = runtimeWindowStatsRef.current.get(itemId);
    if (!stats) {
      return;
    }

    const item = schedulerRef.current.items[itemId];
    if (item?.activeTransferId) {
      stats.transferId = item.activeTransferId;
    }

    emitPasteyDiagnostic("[pastey:runtime-window]", {
      event: "summary",
      room_id: item?.roomId ?? stats.roomId,
      queue_item_id: itemId,
      item_id: itemId,
      transfer_id: stats.transferId ?? "none",
      initial_window: stats.initialWindow,
      final_runtime_window: stats.finalWindow,
      min_runtime_window: stats.minWindow,
      max_runtime_window: stats.maxWindow,
      runtime_window_update_count: stats.updateCount,
      final_window: stats.finalWindow,
      min_window: stats.minWindow,
      max_window: stats.maxWindow,
      update_count: stats.updateCount,
      protocol: stats.protocol,
      transfer_protocol: stats.protocol,
      override_source: stats.overrideSource,
      terminal_status: terminalStatus,
      terminal_reason: terminalReason
    });
    runtimeWindowStatsRef.current.delete(itemId);
  }

  function emitRuntimeWindowSummariesForBatch(
    batchId: string,
    terminalStatus: RuntimeWindowTerminalStatus,
    terminalReason: string
  ) {
    for (const [itemId] of [...runtimeWindowStatsRef.current.entries()]) {
      const item = schedulerRef.current.items[itemId];
      if (item?.batchId === batchId) {
        emitRuntimeWindowDiagnosticSummary(itemId, terminalStatus, terminalReason);
      }
    }
  }

  function emitRuntimeWindowSummariesForRoom(
    roomId: string,
    terminalStatus: RuntimeWindowTerminalStatus,
    terminalReason: string
  ) {
    for (const [itemId] of [...runtimeWindowStatsRef.current.entries()]) {
      const item = schedulerRef.current.items[itemId];
      if ((item?.roomId ?? runtimeWindowStatsRef.current.get(itemId)?.roomId) === roomId) {
        emitRuntimeWindowDiagnosticSummary(itemId, terminalStatus, terminalReason);
      }
    }
  }

  function runtimeWindowTerminalReason(
    terminalStatus: Extract<TransferQueueItemStatus, "completed" | "failed" | "cancelled">,
    item?: TransferSchedulerState["items"][string]
  ): string {
    if (terminalStatus === "completed") {
      return "send_result";
    }
    if (item?.errorMessage?.trim()) {
      return item.errorMessage;
    }
    if (terminalStatus === "cancelled") {
      return item?.cancelRequested ? "cancel_requested" : "cancelled";
    }
    return "send_result";
  }

  async function processTransferQueueItem(
    itemId: string,
    requestedWindow: number
  ): Promise<Extract<TransferQueueItemStatus, "completed" | "failed" | "cancelled"> | null> {
    updateSchedulerState((current) => markQueueItemPreparing(current, itemId, requestedWindow));
    let runtimeTerminalStatus: Extract<TransferQueueItemStatus, "completed" | "failed" | "cancelled"> | null = null;

    try {
      let item = schedulerRef.current.items[itemId];
      if (!item || item.cancelRequested) {
        updateSchedulerState((current) => markQueueItemCancelled(current, itemId));
        return "cancelled";
      }

      const metadata = await prepareQueueItemMetadata(itemId);
      if (!metadata) {
        updateSchedulerState((current) => markQueueItemCancelled(current, itemId));
        return "cancelled";
      }

      item = schedulerRef.current.items[itemId];
      if (!item || item.cancelRequested) {
        updateSchedulerState((current) => markQueueItemCancelled(current, itemId));
        return "cancelled";
      }

      if (metadata.sizeBytes > MAX_FILE_SIZE_BYTES) {
        updateSchedulerState((current) => markQueueItemFailed(current, itemId, FILE_TOO_LARGE_MESSAGE));
        await refreshRoomAfterQueueItem(item.roomId);
        return "failed";
      }

      if (hasNonterminalDedupeKey(schedulerRef.current, metadata.dedupeKey, itemId)) {
        updateSchedulerState((current) => markQueueItemCancelled(current, itemId));
        return "cancelled";
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
        return "cancelled";
      }

      startRuntimeWindowDiagnostic(itemId, requestedWindow);
      await sendFileToRoom(item.roomId, item.path, {
        displayName: metadata.displayName,
        mimeType: metadata.mimeType,
        queueItemId: item.id,
        requestedWindow
      });
      updateSchedulerState((current) => markQueueItemCompleted(current, itemId));
      runtimeTerminalStatus = "completed";
      void rebalanceActiveTransferWindows({ itemId, status: "completed" });
      await refreshRoomAfterQueueItem(item.roomId);
      return "completed";
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
        runtimeTerminalStatus = terminalStatus;
        void rebalanceActiveTransferWindows({ itemId, status: terminalStatus });
      }
      if (latestItem && latestItem.metadataStatus !== "loading") {
        await refreshRoomAfterQueueItem(latestItem.roomId);
      }
      return terminalStatus ?? "failed";
    } finally {
      if (runtimeTerminalStatus) {
        emitRuntimeWindowDiagnosticSummary(
          itemId,
          runtimeTerminalStatus,
          runtimeWindowTerminalReason(runtimeTerminalStatus, schedulerRef.current.items[itemId])
        );
      }
      await cleanupSchedulerTempFile(itemId);
    }
  }

  async function processMicroFlowGroup(groupId: string, childItemIds: string[], requestedWindow: number) {
    updateSchedulerState((current) => markMicroFlowGroupRunning(current, groupId));
    const runningGroup = schedulerRef.current.microGroups[groupId];
    if (runningGroup) {
      emitPasteyDiagnostic("[pastey:micro-group]", {
        event: "running",
        group_id: groupId,
        room_id: runningGroup.roomId,
        status: "running",
        children: runningGroup.childItemIds.length,
        requested_window: runningGroup.requestedWindow
      });
    }

    for (const [childIndex, childItemId] of childItemIds.entries()) {
      const group = schedulerRef.current.microGroups[groupId];
      if (!group || isTerminalMicroFlowGroup(group)) {
        break;
      }

      const item = schedulerRef.current.items[childItemId];
      if (!item) {
        updateSchedulerState((current) => recordMicroFlowGroupChildTerminal(
          current,
          groupId,
          childItemId,
          "failed"
        ));
        emitPasteyDiagnostic("[pastey:micro-group]", {
          event: "child_terminal",
          group_id: groupId,
          room_id: "unknown",
          child_item_id: childItemId,
          child_index: childIndex + 1,
          children: childItemIds.length,
          display_name: "unknown",
          size_bytes: "unknown",
          status: "failed",
          reason: "missing_queue_item"
        });
        continue;
      }

      if (item.status === "completed" || item.status === "failed" || item.status === "cancelled") {
        const terminalChildStatus: Extract<TransferQueueItemStatus, "completed" | "failed" | "cancelled"> = item.status;
        updateSchedulerState((current) => recordMicroFlowGroupChildTerminal(
          current,
          groupId,
          childItemId,
          terminalChildStatus
        ));
        emitPasteyDiagnostic("[pastey:micro-group]", {
          event: "child_terminal",
          group_id: groupId,
          room_id: item.roomId,
          child_item_id: childItemId,
          child_index: childIndex + 1,
          children: childItemIds.length,
          display_name: item.displayName ?? "unknown",
          size_bytes: typeof item.sizeBytes === "number" ? item.sizeBytes : "unknown",
          status: terminalChildStatus,
          reason: "already_terminal"
        });
        continue;
      }

      if (item.status !== "queued") {
        emitPasteyDiagnostic("[pastey:micro-group]", {
          event: "child_skipped",
          group_id: groupId,
          room_id: item.roomId,
          child_item_id: childItemId,
          child_index: childIndex + 1,
          children: childItemIds.length,
          display_name: item.displayName ?? "unknown",
          size_bytes: typeof item.sizeBytes === "number" ? item.sizeBytes : "unknown",
          status: item.status,
          reason: "not_queued"
        });
        continue;
      }
      if (closedRoomIdsRef.current.has(item.roomId)) {
        updateSchedulerState((current) => finishMicroFlowGroup(
          current,
          groupId,
          "interrupted",
          "room_closed_before_child_launch"
        ));
        emitPasteyDiagnostic("[pastey:micro-group]", {
          event: "stopped",
          group_id: groupId,
          room_id: item.roomId,
          status: "interrupted",
          terminal_reason: "room_closed_before_child_launch",
          next_child_item_id: childItemId,
          child_index: childIndex + 1,
          children: childItemIds.length
        });
        break;
      }

      const batch = schedulerRef.current.batches[item.batchId];
      if (!batch || batch.cancelRequested || batch.status !== "running") {
        updateSchedulerState((current) => finishMicroFlowGroup(
          current,
          groupId,
          "cancelled",
          "batch_cancelled_before_child_launch"
        ));
        emitPasteyDiagnostic("[pastey:micro-group]", {
          event: "stopped",
          group_id: groupId,
          room_id: item.roomId,
          status: "cancelled",
          terminal_reason: "batch_cancelled_before_child_launch",
          next_child_item_id: childItemId,
          child_index: childIndex + 1,
          children: childItemIds.length
        });
        break;
      }
      if (item.cancelRequested) {
        updateSchedulerState((current) => recordMicroFlowGroupChildTerminal(
          current,
          groupId,
          childItemId,
          "cancelled"
        ));
        emitPasteyDiagnostic("[pastey:micro-group]", {
          event: "child_terminal",
          group_id: groupId,
          room_id: item.roomId,
          child_item_id: childItemId,
          child_index: childIndex + 1,
          children: childItemIds.length,
          display_name: item.displayName ?? "unknown",
          size_bytes: typeof item.sizeBytes === "number" ? item.sizeBytes : "unknown",
          status: "cancelled",
          reason: "child_cancel_requested_before_launch"
        });
        continue;
      }

      emitPasteyDiagnostic("[pastey:micro-group]", {
        event: "child_running",
        group_id: groupId,
        room_id: item.roomId,
        child_item_id: childItemId,
        child_index: childIndex + 1,
        children: childItemIds.length,
        display_name: item.displayName ?? "unknown",
        size_bytes: typeof item.sizeBytes === "number" ? item.sizeBytes : "unknown",
        requested_window: requestedWindow
      });
      const childStatus = await processTransferQueueItem(childItemId, requestedWindow);
      if (childStatus) {
        updateSchedulerState((current) => recordMicroFlowGroupChildTerminal(
          current,
          groupId,
          childItemId,
          childStatus
        ));
        const latestChild = schedulerRef.current.items[childItemId] ?? item;
        emitPasteyDiagnostic("[pastey:micro-group]", {
          event: "child_terminal",
          group_id: groupId,
          room_id: latestChild.roomId,
          child_item_id: childItemId,
          child_index: childIndex + 1,
          children: childItemIds.length,
          display_name: latestChild.displayName ?? item.displayName ?? "unknown",
          size_bytes: typeof latestChild.sizeBytes === "number" ? latestChild.sizeBytes : "unknown",
          status: childStatus,
          reason: "send_result"
        });
      }

      const latestGroup = schedulerRef.current.microGroups[groupId];
      if (!latestGroup || isTerminalMicroFlowGroup(latestGroup)) {
        break;
      }
      if (closedRoomIdsRef.current.has(item.roomId)) {
        updateSchedulerState((current) => finishMicroFlowGroup(
          current,
          groupId,
          "interrupted",
          "room_closed_during_child_transfer"
        ));
        emitPasteyDiagnostic("[pastey:micro-group]", {
          event: "stopped",
          group_id: groupId,
          room_id: item.roomId,
          status: "interrupted",
          terminal_reason: "room_closed_during_child_transfer",
          next_child_item_id: "none",
          last_child_item_id: childItemId,
          child_index: childIndex + 1,
          children: childItemIds.length
        });
        break;
      }

      const latestItem = schedulerRef.current.items[childItemId];
      const latestBatch = latestItem ? schedulerRef.current.batches[latestItem.batchId] : undefined;
      if (!latestBatch || latestBatch.cancelRequested || latestBatch.status === "cancelled") {
        updateSchedulerState((current) => finishMicroFlowGroup(
          current,
          groupId,
          "cancelled",
          "batch_cancelled_during_child_transfer"
        ));
        emitPasteyDiagnostic("[pastey:micro-group]", {
          event: "stopped",
          group_id: groupId,
          room_id: item.roomId,
          status: "cancelled",
          terminal_reason: "batch_cancelled_during_child_transfer",
          next_child_item_id: "none",
          last_child_item_id: childItemId,
          child_index: childIndex + 1,
          children: childItemIds.length
        });
        break;
      }
    }

    updateSchedulerState((current) => completeMicroFlowGroupFromChildren(current, groupId));
    const terminalGroup = schedulerRef.current.microGroups[groupId];
    if (terminalGroup) {
      emitPasteyDiagnostic("[pastey:micro-group]", {
        event: "final",
        group_id: groupId,
        room_id: terminalGroup.roomId,
        status: terminalGroup.status,
        terminal_reason: terminalGroup.terminalReason ?? "none",
        children: terminalGroup.childItemIds.length,
        completed: terminalGroup.completedChildIds.length,
        failed: terminalGroup.failedChildIds.length,
        cancelled: terminalGroup.cancelledChildIds.length,
        requested_window: terminalGroup.requestedWindow,
        total_bytes: microGroupTotalBytesFromScheduler(schedulerRef.current, terminalGroup.childItemIds)
      });
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
          recordRuntimeWindowUpdateResult(plan, result);
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
    emitRuntimeWindowDiagnosticSummary(itemId, "cancelled", "queue_item_cancelled");
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

    emitRuntimeWindowSummariesForBatch(batchId, "cancelled", "batch_cancelled");
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
    emitRuntimeWindowSummariesForRoom(roomId, "interrupted", "room_burned");
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
  launchingItemWindows: ReadonlyMap<string, number>,
  rooms: readonly Pick<RoomInfo, "id" | "status">[],
  closedRoomIds: ReadonlySet<string>,
  plannerPolicy: TransferPlannerPolicy
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
  const fixedPolicy: TransferPlannerPolicy = {
    ...DEFAULT_TRANSFER_PLANNER_POLICY,
    microGroupMode: "fixed"
  };
  const fixedDiagnostics = summarizeMicroFlowGroupPlanning(
    scheduler,
    rooms,
    closedRoomIds,
    fixedPolicy
  );
  const dynamicPolicy: TransferPlannerPolicy = {
    ...DEFAULT_TRANSFER_PLANNER_POLICY,
    microGroupMode: "dynamic",
    microGroupMaxChildSizeBytes: fixedDiagnostics.dynamicChildCapBytes,
    microGroupMaxGroupBytes: fixedDiagnostics.dynamicGroupCapBytes
  };
  const microGroupDiagnostics = summarizeMicroFlowGroupPlanning(
    scheduler,
    rooms,
    closedRoomIds,
    plannerPolicy.microGroupMode === "dynamic" ? dynamicPolicy : fixedPolicy
  );
  const dynamicDiagnostics = summarizeMicroFlowGroupPlanning(
    scheduler,
    rooms,
    closedRoomIds,
    dynamicPolicy
  );
  const fixedCandidateResult = planRunnableTransferLaunches(
    scheduler,
    rooms,
    closedRoomIds,
    launchingItemWindows,
    false,
    fixedPolicy
  ).plannerResult;
  const dynamicCandidateResult = planRunnableTransferLaunches(
    scheduler,
    rooms,
    closedRoomIds,
    launchingItemWindows,
    false,
    dynamicPolicy
  ).plannerResult;
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
  const microGroupDetails = launchPlan.microGroupPlans.map((plan) => ({
    groupId: plan.groupId,
    roomId: plan.roomId,
    childCount: plan.childItemIds.length,
    requestedWindow: plan.requestedWindow,
    lane: plan.lane,
    totalBytes: plan.totalBytes,
    dispatchMode: plan.dispatchMode
  }));
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
    microGroupDetails,
    heldReasonCounts,
    microGroupDiagnostics,
    fixedDiagnostics,
    dynamicDiagnostics,
    fixedCandidateResult,
    dynamicCandidateResult
  });
  if (summaryKey === lastSummaryKeyRef.current) {
    return;
  }
  lastSummaryKeyRef.current = summaryKey;

  console.info(
    "[pastey queue] event=planner_launch_summary total_candidates=%d metadata_ready_candidates=%d active_candidates=%d runnable_count=%d micro_group_count=%d held_reasons=%s runnable=%s micro_groups=%s",
    candidates.length,
    metadataReadyCount,
    activeCandidateCount,
    launchPlan.runnablePlans.length,
    launchPlan.microGroupPlans.length,
    JSON.stringify(heldReasonCounts),
    JSON.stringify(runnableDetails),
    JSON.stringify(microGroupDetails)
  );
  emitPasteyDiagnostic("[pastey:planner]", {
    event: "launch_summary",
    room_id: plannerSummaryRoomId(launchPlan),
    runnable_plans: launchPlan.runnablePlans.length,
    ...microGroupPlannerDiagnosticFields(
      plannerPolicy.microGroupMode,
      launchPlan.microGroupPlans.length,
      launchPlan.microGroupPlans.reduce((total, plan) => total + plan.childItemIds.length, 0),
      microGroupDiagnostics,
      fixedCandidateResult.microGroupPlans.reduce((total, plan) => total + plan.childTaskIds.length, 0),
      dynamicCandidateResult.microGroupPlans.reduce((total, plan) => total + plan.childTaskIds.length, 0)
    ),
    active_plans: launchPlan.plannerResult.activePlans.length,
    held_plans: launchPlan.plannerResult.heldPlans.length,
    live_requested_window_total: launchPlan.plannerResult.requestedWindowTotal,
    global_window_budget: launchPlan.plannerResult.globalWindowBudget,
    tiny_grouped_children: launchPlan.microGroupPlans.reduce((total, plan) => total + plan.childItemIds.length, 0),
    tiny_individual_runnable: launchPlan.runnablePlans.filter((plan) => plan.sizeClass === "tiny").length,
    tiny_candidates: microGroupDiagnostics.tinyCandidates,
    largest_eligible_micro_group_bucket: microGroupDiagnostics.largestEligibleBucket,
    over_child_size_limit: microGroupDiagnostics.overChildSizeLimit,
    metadata_missing: microGroupDiagnostics.metadataMissing,
    room_unavailable: microGroupDiagnostics.roomUnavailable,
    cancelled_or_terminal: microGroupDiagnostics.cancelledOrTerminal,
    live_held_reasons: formatHeldReasonCounts(heldReasonCounts)
  });
  for (const plan of launchPlan.microGroupPlans) {
    emitPasteyDiagnostic("[pastey:micro-group]", {
      event: "planned",
      group_id: plan.groupId,
      room_id: plan.roomId,
      children: plan.childItemIds.length,
      requested_window: plan.requestedWindow,
      total_bytes: plan.totalBytes,
      dispatch_mode: plan.dispatchMode,
      micro_group_mode: plan.microGroupMode,
      max_child_size_bytes: plannerPolicy.microGroupMode === "dynamic"
        ? microGroupDiagnostics.dynamicChildCapBytes
        : plannerPolicy.microGroupMaxChildSizeBytes,
      max_group_bytes: plannerPolicy.microGroupMode === "dynamic"
        ? microGroupDiagnostics.dynamicGroupCapBytes
        : plannerPolicy.microGroupMaxGroupBytes
    });
  }
}

function microGroupPlannerPolicy(mode?: MicroFlowGroupMode): TransferPlannerPolicy {
  return {
    ...DEFAULT_TRANSFER_PLANNER_POLICY,
    microGroupMode: mode === "fixed" ? "fixed" : "dynamic"
  };
}

function emitPasteyDiagnostic(
  prefix: "[pastey:planner]" | "[pastey:micro-group]" | "[pastey:runtime-window]",
  fields: Record<string, string | number | boolean | null | undefined>
) {
  emitDiagnosticLine(`${prefix} ${formatDiagnosticFields(fields)}`);
}

function emitDiagnosticLine(line: string) {
  console.info(line);
  void logFrontendDiagnostic(line).catch((err) => {
    console.warn(
      "[pastey diagnostics] event=frontend_log_failed error=%s",
      err instanceof Error ? err.message : String(err)
    );
  });
}

function formatDiagnosticFields(fields: Record<string, string | number | boolean | null | undefined>): string {
  return Object.entries(fields)
    .map(([key, value]) => `${key}=${diagnosticValue(value)}`)
    .join(" ");
}

function diagnosticValue(value: string | number | boolean | null | undefined): string {
  if (value === null || typeof value === "undefined") {
    return "none";
  }
  const raw = String(value).trim() || "none";
  const sanitized = raw
    .replace(/[\s=]+/g, "_")
    .replace(/[\\/]+/g, "_")
    .replace(/[^A-Za-z0-9._:,-]/g, "_")
    .slice(0, 160);
  return sanitized || "none";
}

function formatHeldReasonCounts(counts: Record<string, number>): string {
  const entries = Object.entries(counts).sort(([left], [right]) => left.localeCompare(right));
  if (entries.length === 0) {
    return "none";
  }

  return entries.map(([reason, count]) => `${diagnosticValue(reason)}:${count}`).join(",");
}

function plannerSummaryRoomId(launchPlan: TransferLaunchPlannerResult): string {
  const roomIds = new Set<string>();
  launchPlan.runnablePlans.forEach((plan) => roomIds.add(plan.roomId));
  launchPlan.microGroupPlans.forEach((plan) => roomIds.add(plan.roomId));
  launchPlan.plannerResult.activePlans.forEach((plan) => roomIds.add(plan.roomId));
  launchPlan.plannerResult.heldPlans.forEach((plan) => roomIds.add(plan.roomId));
  if (roomIds.size === 0) {
    return "none";
  }
  if (roomIds.size === 1) {
    return [...roomIds][0];
  }
  return "mixed";
}

function microGroupTotalBytesFromScheduler(
  scheduler: TransferSchedulerState,
  childItemIds: readonly string[]
): number {
  return childItemIds.reduce((total, childItemId) => {
    const sizeBytes = scheduler.items[childItemId]?.sizeBytes;
    return total + (typeof sizeBytes === "number" ? sizeBytes : 0);
  }, 0);
}

export default App;

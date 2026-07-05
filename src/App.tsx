import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { MutableRefObject, ReactNode } from "react";
import { AppShell } from "./components/AppShell";
import type { PrimaryView } from "./components/PrimarySidebar";
import { RoomPage } from "./pages/RoomPage";
import { SettingsPage } from "./pages/SettingsPage";
import { formatBytes, formatCode, formatTimestamp } from "./lib/format";
import {
  acceptNearbyJoin,
  burnRoom,
  cancelTransfer,
  deleteTempFile,
  getConfig,
  getFileTransferMetadata,
  getRoom,
  joinRoom,
  listNearbyDevices,
  listRoomItems,
  listRooms,
  logFrontendDiagnostic,
  markJoinPromptRendered,
  pendingJoinRequests,
  rejectNearbyJoin,
  requestNearbyJoin,
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
  type TransferQueueItem,
  type TransferQueueItemStatus,
  type TransferSchedulerState
} from "./lib/transferScheduler";
import {
  DEFAULT_TRANSFER_PLANNER_POLICY,
  type MicroFlowGroupMode,
  type TransferPlannerPolicy
} from "./lib/transferPlanner";
import {
  deriveBridgeRoutingStateForRoom,
  routeStateLabel,
  sendFileToRoomWithBridgeRoute
} from "./lib/bridgeRoutingRuntime";
import { bridgePeerSessionId, formatBridgeRouteErrorForUser, type BridgeRoute } from "./lib/bridgeRouting";
import { legacyRoomToBridgePeerCollection } from "./lib/bridgeRoomAdapter";
import { findBridgePeerBySessionId, getRouteableBridgePeers, type BridgePeerSession } from "./lib/bridgePeers";
import { mergeTransferEvent } from "./lib/transferState";
import {
  buildCandidatePayloadWorkflowPayloadPreview,
  confirmCandidatePayloadWorkflowSearch,
  createRuntimeDataWindowTargetState,
  createCandidatePayloadWorkflow,
  getControlWindowSessionRevision,
  getOutgoingControlWindowDemand,
  getRuntimeControlWindowStatus,
  logAgentBridgeLifecycle,
  markCandidatePayloadWorkflowPayloadPendingConsent,
  publishRuntimeControlWindowStatus,
  reduceRuntimeDataWindowTarget,
  startCandidatePayloadWorkflowFromSearchAdvisory,
  subscribeOutgoingControlWindowDemand,
  subscribeControlWindowSessionRevision,
  type CandidatePayloadWorkflow,
  type CandidatePayloadWorkflowCandidate,
  type RuntimeDataWindowTarget,
} from "./lib/agentBridge";
import {
  buildMockAiContextSnapshot,
  buildMockFileCandidatePlan,
} from "./lib/ai";
import type { AppConfig, FileTransferProgressEvent, JoinRequestPrompt, NearbyDevice, RoomInfo, RoomItem } from "./lib/types";

type View =
  | { screen: "primary" }
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
  const [view, setView] = useState<View>({ screen: "primary" });
  const [activePrimaryView, setActivePrimaryView] = useState<PrimaryView>("bridge");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [rooms, setRooms] = useState<RoomInfo[]>([]);
  const [currentRoom, setCurrentRoom] = useState<RoomInfo | null>(null);
  const [activeBridgeRoomId, setActiveBridgeRoomId] = useState("");
  const [roomItems, setRoomItems] = useState<RoomItem[]>([]);
  const [transfers, setTransfers] = useState<Record<string, FileTransferProgressEvent>>({});
  const [scheduler, setScheduler] = useState<TransferSchedulerState>(() => createTransferSchedulerState());
  const [joinRequest, setJoinRequest] = useState<JoinRequestPrompt | null>(null);
  const [focusToken, setFocusToken] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const outgoingControlDemand = useSyncExternalStore(
    subscribeOutgoingControlWindowDemand,
    getOutgoingControlWindowDemand,
  );
  const controlWindowSessionRevision = useSyncExternalStore(
    subscribeControlWindowSessionRevision,
    getControlWindowSessionRevision,
  );
  const [runtimeDataWindowState, setRuntimeDataWindowState] = useState(
    createRuntimeDataWindowTargetState,
  );
  const closedRoomIdsRef = useRef<Set<string>>(new Set());
  const schedulerRef = useRef(scheduler);
  const roomsRef = useRef(rooms);
  const viewRef = useRef(view);
  const launchingQueueItemWindowsRef = useRef<Map<string, number>>(new Map());
  const metadataPreflightItemIdsRef = useRef<Set<string>>(new Set());
  const cancellingQueueTransferIdsRef = useRef<Set<string>>(new Set());
  const runtimeWindowUpdateKeysRef = useRef<Set<string>>(new Set());
  const runtimeWindowStatsRef = useRef<Map<string, RuntimeWindowDiagnosticStats>>(new Map());
  const plannerLaunchSummaryKeyRef = useRef<string>("");
  const serialMicroGroupRunningRef = useRef(false);
  const runtimeDataWindowTargetRef = useRef<RuntimeDataWindowTarget>(8);
  const runtimeWindowRebalanceChainRef = useRef<Promise<unknown>>(Promise.resolve());

  useEffect(() => {
    viewRef.current = view;
  }, [view]);

  useEffect(() => {
    roomsRef.current = rooms;
  }, [rooms]);

  useEffect(() => {
    setRuntimeDataWindowState((current) =>
      reduceRuntimeDataWindowTarget(current, {
        type: "demand_changed",
        outgoingControlDemand,
        nowMs: Date.now(),
      })
    );
  }, [outgoingControlDemand]);

  useEffect(() => {
    setRuntimeDataWindowState(
      outgoingControlDemand
        ? reduceRuntimeDataWindowTarget(createRuntimeDataWindowTargetState(), {
            type: "demand_changed",
            outgoingControlDemand: true,
            nowMs: Date.now(),
          })
        : createRuntimeDataWindowTargetState()
    );
  }, [controlWindowSessionRevision]);

  useEffect(() => {
    if (runtimeDataWindowState.restoreAfterMs === null) {
      return;
    }
    const delayMs = Math.max(0, runtimeDataWindowState.restoreAfterMs - Date.now());
    const timeout = window.setTimeout(() => {
      setRuntimeDataWindowState((current) =>
        reduceRuntimeDataWindowTarget(current, {
          type: "restore_quiet_period_elapsed",
          nowMs: Date.now(),
        })
      );
    }, delayMs);
    return () => window.clearTimeout(timeout);
  }, [runtimeDataWindowState.restoreAfterMs]);

  useEffect(() => {
    runtimeDataWindowTargetRef.current = runtimeDataWindowState.targetDataWindows;
    if (runtimeDataWindowState.targetDataWindows === 7) {
      logAgentBridgeLifecycle({
        eventKind: "control_demand_started",
        roomRefShort: currentRoom?.id,
        runtimeDataWindowTarget: 7,
      });
    }
    logAgentBridgeLifecycle({
      eventKind: runtimeDataWindowState.targetDataWindows === 7
        ? "runtime_window_target_7"
        : "runtime_window_target_8",
      roomRefShort: currentRoom?.id,
      runtimeDataWindowTarget: runtimeDataWindowState.targetDataWindows,
    });
    void scheduleActiveTransferWindowRebalance(undefined, runtimeDataWindowState.targetDataWindows);
  }, [runtimeDataWindowState.targetDataWindows]);

  useEffect(() => {
    const current = getRuntimeControlWindowStatus();
    if (current.targetDataWindows !== runtimeDataWindowState.targetDataWindows) {
      return;
    }
    const reason = runtimeDataWindowState.targetDataWindows === 8
      ? "idle"
      : runtimeDataWindowState.outgoingControlDemand
        ? "outgoing_control_demand"
        : "restore_quiet_period";
    publishRuntimeControlWindowStatus({ ...current, reason });
  }, [
    runtimeDataWindowState.outgoingControlDemand,
    runtimeDataWindowState.restoreAfterMs,
    runtimeDataWindowState.targetDataWindows,
  ]);

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
        const connected = nextRooms.filter((room) => room.peer_connected);
        if (connected.length === 1) {
          setActiveBridgeRoomId((current) => current || connected[0].id);
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    }

    void load();
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function loadActiveBridge() {
      if (!activeBridgeRoomId) {
        setCurrentRoom(null);
        setRoomItems([]);
        return;
      }

      try {
        const [nextRoom, nextItems, nextRooms] = await Promise.all([
          getRoom(activeBridgeRoomId),
          listRoomItems(activeBridgeRoomId),
          listRooms(),
        ]);
        if (cancelled) return;
        setCurrentRoom(nextRoom);
        setRoomItems(nextItems);
        setRooms(nextRooms);
      } catch (err) {
        if (cancelled) return;
        const message = err instanceof Error ? err.message : String(err);
        if (
          message === "room not found" ||
          message === "File is no longer available" ||
          message === "File is no longer available."
        ) {
          setActiveBridgeRoomId("");
          setCurrentRoom(null);
          setRoomItems([]);
          await refreshRooms();
          return;
        }
        setError(message);
      }
    }

    void loadActiveBridge();
    if (!activeBridgeRoomId) return () => {
      cancelled = true;
    };

    const interval = window.setInterval(() => {
      void loadActiveBridge();
    }, 2000);

    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [activeBridgeRoomId]);

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
      setView({ screen: "primary" });
      setActivePrimaryView(target === "settings" ? "settings" : "bridge");
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
    const plannerPolicy = microGroupPlannerPolicy(
      config?.micro_flow_group_mode,
      runtimeDataWindowState.targetDataWindows
    );
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
  }, [scheduler, rooms, config?.micro_flow_group_mode, runtimeDataWindowState.targetDataWindows]);

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
    const targetRoomId = view.screen === "room" ? view.roomId : activeBridgeRoomId;
    if (!targetRoomId) return;
    try {
      const [nextRoom, nextItems] = await Promise.all([getRoom(targetRoomId), listRoomItems(targetRoomId)]);
      setCurrentRoom(nextRoom);
      setRoomItems(nextItems);
      const visibleRoom = await refreshRooms(targetRoomId);
      if (!visibleRoom) {
        setView({ screen: "primary" });
        if (activeBridgeRoomId === targetRoomId) {
          setActiveBridgeRoomId("");
        }
        setRoomItems([]);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (
        message === "room not found" ||
        message === "File is no longer available" ||
        message === "File is no longer available."
      ) {
        setView({ screen: "primary" });
        if (activeBridgeRoomId === targetRoomId) {
          setActiveBridgeRoomId("");
        }
        setCurrentRoom(null);
        setRoomItems([]);
        return;
      }

      setError(message);
    }
  }

  async function refreshRoomAfterQueueItem(roomId: string) {
    const currentView = viewRef.current;
    if ((currentView.screen !== "room" || currentView.roomId !== roomId) && activeBridgeRoomId !== roomId) {
      await refreshRooms();
      return;
    }

    try {
      const [nextRoom, nextItems] = await Promise.all([getRoom(roomId), listRoomItems(roomId)]);
      setCurrentRoom(nextRoom);
      setRoomItems(nextItems);
      const visibleRoom = await refreshRooms(roomId);
      if (!visibleRoom) {
        setView({ screen: "primary" });
        setRoomItems([]);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (
        message === "room not found" ||
        message === "File is no longer available" ||
        message === "File is no longer available."
      ) {
        setView({ screen: "primary" });
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
      const sendOptions = {
        displayName: metadata.displayName,
        mimeType: metadata.mimeType,
        queueItemId: item.id,
        requestedWindow
      };
      const roomForRoute = rooms.find((room) => room.id === item.roomId);
      if (!roomForRoute) {
        throw new Error("No current Room state is available for Bridge file route derivation.");
      }
      await sendFileToRoomWithBridgeRoute(
        roomForRoute,
        item.path,
        sendOptions,
        sendFileToRoom,
        item.bridgeRoute,
        item.bridgeContentKind ?? "file",
      );
      updateSchedulerState((current) => markQueueItemCompleted(current, itemId));
      runtimeTerminalStatus = "completed";
      void scheduleActiveTransferWindowRebalance({ itemId, status: "completed" });
      await refreshRoomAfterQueueItem(item.roomId);
      return "completed";
    } catch (err) {
      const latestItem = schedulerRef.current.items[itemId];
      const message = formatBridgeRouteErrorForUser(err);
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
        void scheduleActiveTransferWindowRebalance({ itemId, status: terminalStatus });
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
    const baseDedupeKey = item.dedupeKey ?? fileIdentityKey(displayName, metadata.size_bytes, metadata.modified_ms);
    const prepared: PreparedQueueMetadata = {
      displayName,
      mimeType: item.mimeType ?? metadata.mime_type,
      sizeBytes: metadata.size_bytes,
      modifiedMs: metadata.modified_ms,
      dedupeKey: item.targetPeerSessionId
        ? targetQueueDedupeKey(baseDedupeKey, item.targetPeerSessionId)
        : baseDedupeKey
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

  function targetQueueDedupeKey(baseDedupeKey: string, targetPeerSessionId: string): string {
    const suffix = `:bridge-target:${targetPeerSessionId}`;
    return baseDedupeKey.endsWith(suffix) ? baseDedupeKey : `${baseDedupeKey}${suffix}`;
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

  function scheduleActiveTransferWindowRebalance(
    trigger?: { itemId: string; status: "completed" | "failed" | "cancelled" },
    targetDataWindows = runtimeDataWindowTargetRef.current,
  ): Promise<unknown> {
    const next = runtimeWindowRebalanceChainRef.current.then(() =>
      rebalanceActiveTransferWindows(trigger, targetDataWindows)
    );
    runtimeWindowRebalanceChainRef.current = next.catch(() => undefined);
    return next;
  }

  async function rebalanceActiveTransferWindows(
    trigger?: { itemId: string; status: "completed" | "failed" | "cancelled" },
    targetDataWindows = runtimeDataWindowTargetRef.current,
  ): Promise<{ summary: string; error?: string }> {
    const policy = microGroupPlannerPolicy(config?.micro_flow_group_mode, targetDataWindows);
    const plans = planActiveTransferWindowRebalances(
      schedulerRef.current,
      roomsRef.current,
      closedRoomIdsRef.current,
      launchingQueueItemWindowsRef.current,
      policy
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

    const results = await Promise.all(plans.map(async (plan) => {
      const updateKey = `${plan.transferId}:${plan.requestedWindow}`;
      if (runtimeWindowUpdateKeysRef.current.has(updateKey)) {
        return { description: `${plan.transferId}:coalesced`, failed: false };
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
      try {
        const result = await updateTransferWindow(plan.transferId, plan.requestedWindow);
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
        const failed = !(
          result.reason === "not_active" ||
          ((result.updated || result.reason === "unchanged") &&
            effectiveWindow === plan.requestedWindow)
        );
        return {
          description: `${plan.transferId}:${plan.previousWindow}->${effectiveWindow ?? plan.requestedWindow} (${result.reason})`,
          failed,
        };
      } catch (err) {
        console.warn(
          "[pastey queue] event=rebalance_update_failed transfer_id=%s requested_window=%d error=%s",
          plan.transferId,
          plan.requestedWindow,
          err instanceof Error ? err.message.slice(0, 256) : String(err).slice(0, 256)
        );
        return { description: `${plan.transferId}:update_failed`, failed: true };
      } finally {
        runtimeWindowUpdateKeysRef.current.delete(updateKey);
      }
    }));
    const failed = results.some((result) => result.failed);
    const outcome = {
      summary: results.length > 0
        ? results.map((result) => result.description).join(", ").slice(0, 512)
        : `No active data allocations required adjustment for target ${targetDataWindows}.`,
      ...(failed ? { error: "One or more active data-window updates failed." } : {}),
    };
    publishRuntimeControlWindowStatus({
      targetDataWindows,
      reason: targetDataWindows === 8
        ? "idle"
        : getOutgoingControlWindowDemand()
          ? "outgoing_control_demand"
          : "restore_quiet_period",
      reservationReady: !failed,
      activeAllocationUpdates: outcome.summary,
      ...(outcome.error ? { lastError: outcome.error } : {}),
    });
    return outcome;
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

  function enqueueAgentBridgeCandidatePayloadHandoff(roomId: string, input: TransferQueueInput): boolean {
    console.info(
      "[pastey queue] event=agent_bridge_candidate_payload_handoff_enqueue_attempt room_id=%s candidate_id=%s",
      roomId,
      input.agentBridgeMetadata?.candidateId ?? "unknown"
    );
    const next = enqueueTransferBatch(schedulerRef.current, roomId, [input]);
    if (next === schedulerRef.current) {
      console.info(
        "[pastey queue] event=agent_bridge_candidate_payload_handoff_enqueue_rejected room_id=%s reason=no_new_items",
        roomId
      );
      return false;
    }
    updateSchedulerState(() => next);
    console.info(
      "[pastey queue] event=agent_bridge_candidate_payload_handoff_queued room_id=%s candidate_id=%s label=%s",
      roomId,
      input.agentBridgeMetadata?.candidateId ?? "unknown",
      input.agentBridgeMetadata?.label ?? "Candidate payload request"
    );
    return true;
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
    setView({ screen: "primary" });
    setActivePrimaryView("transfers");
    if (activeBridgeRoomId === roomId) {
      setActiveBridgeRoomId("");
    }
    setCurrentRoom(null);
    setRoomItems([]);
    setTransfers((current) => Object.fromEntries(Object.entries(current).filter(([, transfer]) => transfer.room_id !== roomId)));
    await refreshRooms();
  }

  async function handleAcceptJoinRequest(request: JoinRequestPrompt) {
    try {
      const room = await acceptNearbyJoin(request.request_id);
      setJoinRequest(null);
      await handleConnectionJoined(room);
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

  const transferEvents = Object.values(transfers);
  const schedulerItems = Object.values(scheduler.items);
  const activeTransfers = transferEvents.filter(isActiveTransfer);
  const activeQueueItems = schedulerItems.filter(isActiveQueueItem);
  const approvalCount = joinRequest ? 1 : 0;
  const connectedRooms = rooms.filter((room) => room.peer_connected);
  const activeBridgeRoom = activeBridgeRoomId
    ? currentRoom?.id === activeBridgeRoomId
      ? currentRoom
      : rooms.find((room) => room.id === activeBridgeRoomId) ?? null
    : null;

  function selectPrimaryView(nextView: PrimaryView) {
    setActivePrimaryView(nextView);
    setView({ screen: "primary" });
  }

  async function handleConnectionJoined(room: RoomInfo) {
    closedRoomIdsRef.current.delete(room.id);
    setActiveBridgeRoomId(room.id);
    setActivePrimaryView("bridge");
    setView({ screen: "primary" });
    await refreshRooms();
  }

  if (!config) {
    return (
      <div className="app-shell center-panel">
        <div className="panel">{error ?? "Loading pastey..."}</div>
      </div>
    );
  }

  const shellContent = view.screen === "room" && currentRoom ? (
    <RoomPage
      room={currentRoom}
      items={roomItems}
      transfers={transferEvents.filter((transfer) => transfer.room_id === currentRoom.id)}
      queue={selectRoomTransferQueue(scheduler, currentRoom.id)}
      onBack={() => {
        setView({ screen: "primary" });
        void refreshRooms();
      }}
      onRefresh={refreshCurrentRoom}
      onBurn={handleBurnRoom}
      onEnqueueFiles={enqueueRoomFiles}
      onEnqueueTransferInputs={enqueueRoomTransferInputs}
      onEnqueueCandidatePayloadHandoff={enqueueAgentBridgeCandidatePayloadHandoff}
      onCancelQueueItem={handleCancelQueueItem}
      onCancelQueueBatch={handleCancelQueueBatch}
      agentBridgeEnabled={config.dev_tools_enabled}
    />
  ) : (
    <>
      {activePrimaryView === "bridge" ? (
        <BridgeView
          rooms={rooms}
          activeBridgeRoom={activeBridgeRoom}
          activeBridgeRoomId={activeBridgeRoomId}
          roomItems={activeBridgeRoom ? roomItems : []}
          transfers={transferEvents}
          activeTransfers={activeTransfers}
          queueItems={schedulerItems}
          activeQueueItems={activeQueueItems}
          joinRequest={joinRequest}
          approvalCount={approvalCount}
          onSelectView={selectPrimaryView}
          onEnqueueTransferInputs={enqueueRoomTransferInputs}
          onSetActiveBridge={setActiveBridgeRoomId}
        />
      ) : null}
      {activePrimaryView === "devices" ? (
        <DevicesWorkbenchView
          rooms={rooms}
          activeBridgeRoomId={activeBridgeRoomId}
          shouldFocus={focusToken > 0}
          onSelectConnection={setActiveBridgeRoomId}
          onConnectionJoined={(room) => void handleConnectionJoined(room)}
          onSelectView={selectPrimaryView}
        />
      ) : null}
      {activePrimaryView === "transfers" ? (
        <TransfersView rooms={rooms} transfers={transferEvents} queueItems={schedulerItems} />
      ) : null}
      {activePrimaryView === "inbox" ? (
        <InboxView
          joinRequest={joinRequest}
          onAccept={(request) => void handleAcceptJoinRequest(request)}
          onReject={(request) => void handleRejectJoinRequest(request)}
        />
      ) : null}
      {activePrimaryView === "settings" ? (
        <SettingsPage
          config={config}
          onConfigChange={setConfig}
          onJoinWithCode={() => {
            setActivePrimaryView("devices");
            setFocusToken((value) => value + 1);
          }}
        />
      ) : null}
    </>
  );

  return (
    <div className="app-shell workstation-app">
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

      <AppShell
        activeView={activePrimaryView}
        topStatus={{
          thisDevice: "This device",
          thisDeviceStatus: `Pastey ${config.app_version}`,
          peerDiscovery: `${connectedRooms.length} connected`,
          peerDiscoveryStatus: rooms.length > 0 ? `${rooms.length} connection${rooms.length === 1 ? "" : "s"} known` : "Discovery ready",
          approvalsCount: approvalCount,
          queueCount: activeQueueItems.length,
        }}
        onSelectView={selectPrimaryView}
      >
        {shellContent}
      </AppShell>
    </div>
  );
}

type BridgeTargetSelectionMode = "selected_peer" | "selected_peers" | "broadcast_bridge";

function BridgeView({
  rooms,
  activeBridgeRoom,
  activeBridgeRoomId,
  roomItems,
  transfers,
  activeTransfers,
  queueItems,
  activeQueueItems,
  joinRequest,
  approvalCount,
  onSelectView,
  onEnqueueTransferInputs,
  onSetActiveBridge,
}: {
  rooms: RoomInfo[];
  activeBridgeRoom: RoomInfo | null;
  activeBridgeRoomId: string;
  roomItems: RoomItem[];
  transfers: FileTransferProgressEvent[];
  activeTransfers: FileTransferProgressEvent[];
  queueItems: TransferQueueItem[];
  activeQueueItems: TransferQueueItem[];
  joinRequest: JoinRequestPrompt | null;
  approvalCount: number;
  onSelectView: (view: PrimaryView) => void;
  onEnqueueTransferInputs: (roomId: string, inputs: TransferQueueInput[]) => void;
  onSetActiveBridge: (roomId: string) => void;
}) {
  const connectedBridges = rooms.filter((room) => room.peer_connected);
  const bridgeRoom = activeBridgeRoom ?? (!activeBridgeRoomId && connectedBridges.length === 1 ? connectedBridges[0] : null);
  const bridgeQueueItems = bridgeRoom ? queueItems.filter((item) => item.roomId === bridgeRoom.id) : [];
  const bridgeTransfers = bridgeRoom ? transfers.filter((transfer) => transfer.room_id === bridgeRoom.id) : [];
  const bridgeActiveTransfers = bridgeRoom ? activeTransfers.filter((transfer) => transfer.room_id === bridgeRoom.id) : activeTransfers;
  const completedTransfers = transfers.filter((transfer) => transfer.status === "completed");
  const issueTransfers = transfers.filter((transfer) => (
    transfer.status === "failed" ||
    transfer.status === "cancelled" ||
    transfer.status === "burned" ||
    transfer.status === "interrupted"
  ));
  const bridgePeerCollection = useMemo(() => {
    if (!bridgeRoom) return null;
    try {
      return legacyRoomToBridgePeerCollection(bridgeRoom);
    } catch {
      return null;
    }
  }, [bridgeRoom]);
  const routeablePeers = useMemo(
    () => bridgePeerCollection ? [...getRouteableBridgePeers(bridgePeerCollection)] : [],
    [bridgePeerCollection],
  );
  const bridgeRouteState = useMemo(
    () => bridgeRoom ? deriveBridgeRoutingStateForRoom(bridgeRoom) : null,
    [bridgeRoom],
  );
  const [targetMode, setTargetMode] = useState<BridgeTargetSelectionMode>("selected_peer");
  const [selectedPeerIds, setSelectedPeerIds] = useState<string[]>([]);
  const [dropActive, setDropActive] = useState(false);
  const [bridgeMessage, setBridgeMessage] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [safeScopes, setSafeScopes] = useState<SafeSearchScope[]>(["downloads", "desktop", "documents", "pastey_shared"]);
  const [workflow, setWorkflow] = useState<CandidatePayloadWorkflow>(() => createCandidatePayloadWorkflow());
  const [selectedCandidateId, setSelectedCandidateId] = useState("");
  const [requestMessage, setRequestMessage] = useState<string | null>(null);

  useEffect(() => {
    if (routeablePeers.length === 0) {
      setSelectedPeerIds([]);
      return;
    }
    setSelectedPeerIds((current) => {
      const routeableIds = new Set(routeablePeers.map((peer) => peer.peerSessionId));
      const next = current.filter((peerId) => routeableIds.has(bridgePeerSessionId(peerId)));
      return next.length > 0 ? next : [routeablePeers[0].peerSessionId];
    });
  }, [routeablePeers]);

  const selectedBridgeRoute = useMemo(
    () => buildSelectedBridgeRoute(bridgePeerCollection?.bridgeSessionId ?? `legacy-room:${bridgeRoom?.id ?? "none"}`, routeablePeers, targetMode, selectedPeerIds),
    [bridgePeerCollection?.bridgeSessionId, bridgeRoom?.id, routeablePeers, selectedPeerIds, targetMode],
  );
  const selectedRoutePeers = selectedBridgeRoute ? resolvedPeersForRoute(selectedBridgeRoute, routeablePeers) : [];
  const selectedSinglePeer = selectedBridgeRoute?.target.kind === "selected_peer" ? selectedRoutePeers[0] ?? null : null;
  const canSendFiles = Boolean(bridgeRoom?.peer_connected && selectedBridgeRoute && selectedRoutePeers.length > 0);
  const canRequestFile = Boolean(bridgeRoom?.peer_connected && selectedSinglePeer && searchQuery.trim().length > 0 && safeScopes.length > 0);
  const candidates = workflow.snapshot.candidates ?? [];
  const canRequestPayload = Boolean(selectedSinglePeer && workflow.snapshot.state === "candidate_selection_required" && selectedCandidateId.length > 0);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (cancelled) return;
        if (event.payload.type === "over") {
          setDropActive(canSendFiles);
          return;
        }
        if (event.payload.type === "drop") {
          setDropActive(false);
          if (event.payload.paths.length > 0) {
            enqueueBridgeFiles(event.payload.paths);
          }
          return;
        }
        setDropActive(false);
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [canSendFiles, bridgeRoom, selectedBridgeRoute, routeablePeers]);

  function toggleScope(scope: SafeSearchScope) {
    setSafeScopes((current) =>
      current.includes(scope)
        ? current.filter((candidate) => candidate !== scope)
        : [...current, scope]
    );
  }

  function enqueueBridgeFiles(paths: string[]) {
    if (!bridgeRoom || !selectedBridgeRoute || selectedRoutePeers.length === 0 || !canSendFiles) {
      setBridgeMessage("Select a connected Bridge member before adding files.");
      return;
    }
    const inputs = bridgeTransferInputsForSelectedRoute(
      paths.map((path) => ({ path })),
      selectedBridgeRoute,
      selectedRoutePeers,
      bridgeRoom.id,
      "file",
    );
    onEnqueueTransferInputs(bridgeRoom.id, inputs);
    setBridgeMessage(`${paths.length} file${paths.length === 1 ? "" : "s"} added to Transfers.`);
  }

  async function handleChooseFiles() {
    if (!bridgeRoom || !canSendFiles) {
      setBridgeMessage("Select a connected Bridge member before adding files.");
      return;
    }
    const selected = await open({ multiple: true, directory: false });
    const paths = typeof selected === "string" ? [selected] : Array.isArray(selected) ? selected : [];
    if (paths.length > 0) enqueueBridgeFiles(paths);
  }

  function handleMetadataRequest() {
    if (!bridgeRoom || !selectedSinglePeer || !canRequestFile) return;
    const prepared = prepareCandidateSearchWorkflow(searchQuery, safeScopes, selectedSinglePeer.peerSessionId);
    const started = startCandidatePayloadWorkflowFromSearchAdvisory(
      createCandidatePayloadWorkflow(),
      prepared.plan,
      prepared.context,
    );
    const nextWorkflow = started.ok ? started.workflow : started.workflow;
    const confirmed = started.ok
      ? confirmCandidatePayloadWorkflowSearch(started.workflow)
      : null;
    const workflowAfterRequest = confirmed?.ok ? confirmed.workflow : nextWorkflow;
    setWorkflow(workflowAfterRequest);
    setSelectedCandidateId("");
    setRequestMessage(
      confirmed?.ok
        ? "Metadata-only request prepared. Receiver Allow once is required."
        : started.ok
          ? "Request preview prepared, but it could not be confirmed locally."
          : started.errors.join(" ")
    );
  }

  function handlePayloadRequest() {
    if (!selectedSinglePeer || !selectedCandidateId) return;
    const prepared = prepareCandidateSearchWorkflow(searchQuery || "selected candidate", safeScopes, selectedSinglePeer.peerSessionId);
    const preview = buildCandidatePayloadWorkflowPayloadPreview(
      workflow,
      { candidateId: selectedCandidateId, selectedByUser: true },
      prepared.context,
    );
    if (!preview.ok) {
      setWorkflow(preview.workflow);
      setRequestMessage(preview.errors.join(" "));
      return;
    }
    const pending = markCandidatePayloadWorkflowPayloadPendingConsent(preview.workflow);
    setWorkflow(pending.ok ? pending.workflow : preview.workflow);
    setRequestMessage("Candidate payload request prepared. Receiver Allow once is required before anything is queued.");
  }

  if (!bridgeRoom) {
    return (
      <section className="workstation-view bridge-workstation" aria-label="Bridge">
        <Card className="bridge-empty-card">
          <span className="meta-label">Bridge</span>
          <h2>No active Bridge</h2>
          <p className="muted">Add or open a device connection to start sending files, requesting files, and reviewing queue state.</p>
          <ActionRow>
            <button type="button" className="primary-button" onClick={() => onSelectView("devices")}>Open Devices</button>
            <button type="button" className="secondary-button" onClick={() => onSelectView("devices")}>Join with code</button>
          </ActionRow>
        </Card>
        <div className="workbench-grid three-column">
          <Card>
            <span className="meta-label">Devices</span>
            <h2>{connectedBridges.length}</h2>
            <p className="muted">Connected peers ready for a Bridge.</p>
          </Card>
          <Card>
            <span className="meta-label">Inbox</span>
            <h2>{approvalCount}</h2>
            <p className="muted">{joinRequest ? `${joinRequest.device_name} is waiting.` : "No requests waiting."}</p>
          </Card>
          <Card>
            <span className="meta-label">Transfers</span>
            <h2>{activeQueueItems.length}</h2>
            <p className="muted">Current-session queued or active items.</p>
          </Card>
        </div>
      </section>
    );
  }

  return (
    <section className="workstation-view bridge-workstation" aria-label="Bridge">
      <Card className="bridge-overview-card">
        <div className="task-card-header">
          <div>
            <span className="meta-label">Bridge</span>
            <h2>{bridgeRoom.peer_device_name ?? "Waiting peer"}</h2>
            <p className="muted">
              {bridgeRouteState ? routeStateLabel(bridgeRouteState) : "No routeable peer"}.
              {" "}
              {routeablePeers.length} member{routeablePeers.length === 1 ? "" : "s"} routeable.
            </p>
          </div>
          <StatusChip tone={bridgeRoom.peer_connected ? "success" : "neutral"}>
            {bridgeRoom.peer_connected ? "Connected" : connectionStatusLabel(bridgeRoom)}
          </StatusChip>
        </div>
        <div className="bridge-selector-row">
          <label className="field-label">
            <span>Active Bridge</span>
            <select value={bridgeRoom.id} onChange={(event) => onSetActiveBridge(event.target.value)}>
              {rooms.map((room) => (
                <option key={room.id} value={room.id}>
                  {room.peer_device_name ?? "Waiting peer"} - {connectionStatusLabel(room)}
                </option>
              ))}
            </select>
          </label>
          <button type="button" className="secondary-button" onClick={() => onSelectView("devices")}>Manage devices</button>
        </div>
      </Card>

      <div className="workbench-grid two-column">
        <Card className="bridge-members-card">
          <div className="section-row">
            <h2>Members</h2>
            <span className="muted">{selectedRoutePeers.length} selected</span>
          </div>
          <div className="segmented-control" aria-label="Bridge target">
            <button type="button" className={targetMode === "selected_peer" ? "active" : ""} disabled={routeablePeers.length === 0} onClick={() => setTargetMode("selected_peer")}>Peer</button>
            <button type="button" className={targetMode === "selected_peers" ? "active" : ""} disabled={routeablePeers.length < 2} onClick={() => setTargetMode("selected_peers")}>Peers</button>
            <button type="button" className={targetMode === "broadcast_bridge" ? "active" : ""} disabled={routeablePeers.length === 0} onClick={() => setTargetMode("broadcast_bridge")}>All members</button>
          </div>
          <div className="trusted-peer-grid">
            {routeablePeers.length === 0 ? <span className="empty-inline">No connected members yet.</span> : null}
            {routeablePeers.map((peer) => {
              const checked = selectedPeerIds.includes(peer.peerSessionId) || targetMode === "broadcast_bridge";
              return (
                <button
                  key={peer.peerSessionId}
                  type="button"
                  className={`trusted-peer-pill ${checked ? "selected" : ""}`}
                  disabled={targetMode === "broadcast_bridge"}
                  onClick={() => {
                    if (targetMode === "selected_peer") {
                      setSelectedPeerIds([peer.peerSessionId]);
                    } else {
                      setSelectedPeerIds((current) => checked
                        ? current.filter((peerId) => peerId !== peer.peerSessionId)
                        : [...current, peer.peerSessionId]);
                    }
                  }}
                >
                  <span>{peer.displayName}</span>
                  <small>{peer.liveness}</small>
                </button>
              );
            })}
          </div>
          <p className="muted">Request file requires one selected peer. Send files can target selected peers or all current members when routeable.</p>
        </Card>

        <Card className="bridge-action-card">
          <div className="section-row">
            <h2>Send files</h2>
            <StatusChip tone={canSendFiles ? "success" : "neutral"}>{canSendFiles ? "Ready" : "Select member"}</StatusChip>
          </div>
          <div className={`send-drop-zone ${dropActive ? "drop-active" : ""}`}>
            <strong>Drop files for this Bridge</strong>
            <span className="muted">Files are queued through the existing transfer queue.</span>
            <button type="button" className="primary-button" disabled={!canSendFiles} onClick={() => void handleChooseFiles()}>
              Choose files
            </button>
            {bridgeMessage ? <p className="muted">{bridgeMessage}</p> : null}
          </div>
        </Card>
      </div>

      <Card className="find-search-card">
        <div className="section-row">
          <h2>Request file</h2>
          <StatusChip tone={selectedSinglePeer ? "success" : "neutral"}>{selectedSinglePeer ? "Single peer" : "Peer required"}</StatusChip>
        </div>
        <div className="find-search-grid">
          <label className="field-label">
            <span>Search query</span>
            <input value={searchQuery} onChange={(event) => setSearchQuery(event.target.value)} placeholder="report, invoice, photo name..." />
          </label>
          <div className="safe-scope-card">
            <span className="field-label-text">Safe locations</span>
            <div className="scope-chip-grid" aria-label="Safe locations">
              {SAFE_SEARCH_SCOPES.map((scope) => (
                <ScopeChip
                  key={scope.value}
                  label={scope.label}
                  checked={safeScopes.includes(scope.value)}
                  onChange={() => toggleScope(scope.value)}
                />
              ))}
            </div>
          </div>
        </div>
        <div className="find-request-row">
          <button type="button" className="primary-button" disabled={!canRequestFile} onClick={handleMetadataRequest}>
            Request metadata-only search
          </button>
          <p className="muted">
            {selectedSinglePeer
              ? "Receiver Allow once is required. Results contain metadata only, never file contents or full local paths."
              : "Select exactly one peer before requesting file metadata."}
          </p>
        </div>
        <div className="candidate-card-list">
          {candidates.length === 0 ? (
            <EmptyState title="No candidates yet" detail="Candidate cards appear only after the receiver approves metadata-only search and returns metadata." />
          ) : null}
          {candidates.map((candidate) => (
            <CandidateMetadataCard
              key={candidate.candidateId}
              candidate={candidate}
              selected={selectedCandidateId === candidate.candidateId}
              onSelect={() => setSelectedCandidateId(candidate.candidateId)}
            />
          ))}
        </div>
        <ActionRow>
          <button type="button" className="secondary-button" disabled={!canRequestPayload} onClick={handlePayloadRequest}>
            Request this candidate payload
          </button>
          <span className="muted">Queued from approved candidate payload request means queued only, not completed.</span>
        </ActionRow>
        {requestMessage ? <p className="muted">{requestMessage}</p> : null}
      </Card>

      <div className="workbench-grid three-column">
        <SummaryPanel title="Transfers" emptyLabel="No current transfer state.">
          {bridgeQueueItems.slice(0, 3).map((item) => (
            <div key={item.id} className="summary-row">
              <span>{queueItemLabel(item)}</span>
              <small>{queueItemStatusLabel(item)}</small>
            </div>
          ))}
          {bridgeActiveTransfers.slice(0, 3).map((transfer) => (
            <div key={transfer.transfer_id} className="summary-row">
              <span>{transfer.file_name}</span>
              <small>{transfer.status}</small>
            </div>
          ))}
        </SummaryPanel>
        <SummaryPanel title="Inbox" emptyLabel="No requests waiting.">
          {joinRequest ? (
            <div className="summary-row">
              <span>{joinRequest.device_name}</span>
              <small>Connection request</small>
            </div>
          ) : null}
        </SummaryPanel>
        <SummaryPanel title="Bridge messages" emptyLabel="No current-session messages.">
          {roomItems.slice(0, 4).map((item) => (
            <div key={item.id} className="summary-row">
              <span>{item.display_name ?? item.text ?? "Message"}</span>
              <small>{item.direction}</small>
            </div>
          ))}
        </SummaryPanel>
      </div>

      <div className="workbench-grid three-column">
        <Card>
          <span className="meta-label">Queue</span>
          <h2>{bridgeQueueItems.length}</h2>
          <p className="muted">Items queued for this Bridge.</p>
        </Card>
        <Card>
          <span className="meta-label">Completed</span>
          <h2>{completedTransfers.length}</h2>
          <p className="muted">Current-session transfer events only.</p>
        </Card>
        <Card>
          <span className="meta-label">Needs review</span>
          <h2>{issueTransfers.length}</h2>
          <p className="muted">Failed, cancelled, burned, or interrupted current-session events.</p>
        </Card>
      </div>
    </section>
  );
}

function buildSelectedBridgeRoute(
  bridgeSessionId: string,
  routeablePeers: readonly BridgePeerSession[],
  targetMode: BridgeTargetSelectionMode,
  selectedPeerIds: readonly string[],
): BridgeRoute | null {
  if (targetMode === "broadcast_bridge") {
    return routeablePeers.length > 0
      ? { bridgeSessionId, target: { kind: "broadcast_bridge", explicit: true } }
      : null;
  }
  const routeableIds = new Set(routeablePeers.map((peer) => peer.peerSessionId));
  const selectedIds = selectedPeerIds
    .map((peerId) => bridgePeerSessionId(peerId))
    .filter((peerId) => routeableIds.has(peerId));
  if (targetMode === "selected_peer") {
    const peerSessionId = selectedIds[0] ?? routeablePeers[0]?.peerSessionId;
    return peerSessionId
      ? { bridgeSessionId, target: { kind: "selected_peer", peerSessionId } }
      : null;
  }
  return selectedIds.length >= 2
    ? { bridgeSessionId, target: { kind: "selected_peers", peerSessionIds: selectedIds } }
    : null;
}

function resolvedPeersForRoute(route: BridgeRoute, routeablePeers: readonly BridgePeerSession[]): BridgePeerSession[] {
  if (route.target.kind === "broadcast_bridge") {
    return [...routeablePeers];
  }
  if (route.target.kind === "selected_peer") {
    const peer = findBridgePeerBySessionId({
      bridgeSessionId: route.bridgeSessionId,
      peers: routeablePeers,
    }, route.target.peerSessionId);
    return peer ? [peer] : [];
  }
  return route.target.peerSessionIds
    .map((peerSessionId) => findBridgePeerBySessionId({
      bridgeSessionId: route.bridgeSessionId,
      peers: routeablePeers,
    }, peerSessionId))
    .filter((peer): peer is BridgePeerSession => Boolean(peer));
}

function bridgeTransferInputsForSelectedRoute(
  inputs: TransferQueueInput[],
  selectedBridgeRoute: BridgeRoute,
  selectedRoutePeers: readonly BridgePeerSession[],
  bridgeId: string,
  contentKind: "file" | "image" | "pasted_image",
): TransferQueueInput[] {
  const operationId = `bridge-queue:${bridgeId}:${Date.now()}:${Math.random().toString(36).slice(2)}`;
  return inputs.flatMap((input) => selectedRoutePeers.map((peer) => ({
    ...input,
    bridgeRoute: {
      bridgeSessionId: selectedBridgeRoute.bridgeSessionId,
      target: {
        kind: "selected_peer",
        peerSessionId: peer.peerSessionId,
      },
    },
    bridgeOperationId: operationId,
    bridgeTargetKind: selectedBridgeRoute.target.kind,
    bridgeContentKind: contentKind,
    targetPeerSessionId: peer.peerSessionId,
    targetPeerDisplayName: peer.displayName,
    targetCount: selectedRoutePeers.length,
  })));
}

function DevicesWorkbenchView({
  rooms,
  activeBridgeRoomId,
  shouldFocus,
  onSelectConnection,
  onConnectionJoined,
  onSelectView,
}: {
  rooms: RoomInfo[];
  activeBridgeRoomId: string;
  shouldFocus: boolean;
  onSelectConnection: (roomId: string) => void;
  onConnectionJoined: (room: RoomInfo) => void;
  onSelectView: (view: PrimaryView) => void;
}) {
  const [nearbyDevices, setNearbyDevices] = useState<NearbyDevice[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState("");
  const [joinCode, setJoinCode] = useState("");
  const [busy, setBusy] = useState<"join" | "nearby" | "refresh" | null>(null);
  const [joiningDeviceId, setJoiningDeviceId] = useState<string | null>(null);
  const [deviceMessage, setDeviceMessage] = useState<string | null>(null);
  const joinInputRef = useRef<HTMLInputElement | null>(null);
  const roomRows = rooms.map((room) => ({
    kind: "room" as const,
    id: `room:${room.id}`,
    room,
    name: room.peer_device_name ?? "Waiting peer",
    status: connectionStatusLabel(room),
    latency: room.peer_connected ? "Current session" : formatTimestamp(room.created_at),
    capabilities: "Send, request, transfer queue",
    osType: room.local_role === "creator" ? "Created here" : "Joined here",
    action: "Select",
  }));
  const nearbyRows = nearbyDevices.map((device) => ({
    kind: "nearby" as const,
    id: `nearby:${device.device_id}`,
    device,
    name: device.display_name,
    status: nearbyDeviceStatus(device),
    latency: device.last_seen_seconds_ago <= 2 ? "Ready now" : `Seen ${device.last_seen_seconds_ago}s ago`,
    capabilities: nearbyCapabilitySummary(device),
    osType: `${device.platform} / Pastey ${device.app_version}`,
    action: device.compatible && device.availability === "Available" ? "Add to Bridge" : "Unavailable",
  }));
  const deviceRows = [...nearbyRows, ...roomRows];
  const selectedConnectionRoom = activeBridgeRoomId
    ? rooms.find((room) => room.id === activeBridgeRoomId) ?? null
    : null;
  const selectedDevice = deviceRows.find((device) => device.id === selectedDeviceId)
    ?? (selectedConnectionRoom ? deviceRows.find((device) => device.id === `room:${selectedConnectionRoom.id}`) : null)
    ?? deviceRows[0]
    ?? null;
  const selectedRoom = selectedDevice?.kind === "room"
    ? selectedDevice.room
    : selectedConnectionRoom;
  const connectedCount = rooms.filter((room) => room.peer_connected).length;
  const trustedRooms = rooms.filter((room) => room.peer_device_name);

  useEffect(() => {
    if (shouldFocus) {
      joinInputRef.current?.focus();
      joinInputRef.current?.select();
    }
  }, [shouldFocus]);

  useEffect(() => {
    let cancelled = false;

    async function loadNearby(reason: "initial" | "poll" | "refresh" = "poll") {
      if (reason === "refresh") setBusy("refresh");
      try {
        const devices = await listNearbyDevices();
        if (cancelled) return;
        setNearbyDevices(devices);
        setDeviceMessage(devices.length === 0 ? "No nearby devices found." : null);
      } catch {
        if (!cancelled) {
          setNearbyDevices([]);
          setDeviceMessage("Pastey cannot see nearby devices on this network.");
        }
      } finally {
        if (!cancelled && reason === "refresh") setBusy(null);
      }
    }

    void loadNearby("initial");
    const interval = window.setInterval(() => {
      void loadNearby("poll");
    }, 2000);

    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, []);

  useEffect(() => {
    if (!selectedDeviceId && activeBridgeRoomId && deviceRows.some((device) => device.id === `room:${activeBridgeRoomId}`)) {
      setSelectedDeviceId(`room:${activeBridgeRoomId}`);
      return;
    }
    if (!selectedDeviceId && deviceRows[0]) {
      setSelectedDeviceId(deviceRows[0].id);
    }
  }, [deviceRows, activeBridgeRoomId, selectedDeviceId]);

  async function handleJoinRoom() {
    setBusy("join");
    setDeviceMessage(null);

    try {
      const room = await joinRoom(joinCode);
      setJoinCode("");
      setSelectedDeviceId(`room:${room.id}`);
      onConnectionJoined(room);
    } catch (err) {
      setDeviceMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleNearbyJoin(device: NearbyDevice) {
    setBusy("nearby");
    setJoiningDeviceId(device.device_id);
    setDeviceMessage(`Waiting for ${device.display_name} to approve...`);

    try {
      const room = await requestNearbyJoin(device.device_id);
      setDeviceMessage(null);
      setSelectedDeviceId(`room:${room.id}`);
      onConnectionJoined(room);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setDeviceMessage(networkHelpMessage(message));
    } finally {
      setJoiningDeviceId(null);
      setBusy(null);
    }
  }

  return (
    <section className="workstation-view devices-workstation" aria-label="Devices">
      <section className="summary-card devices-table-card">
        <div className="section-row">
          <h2>Discovered devices</h2>
          <button
            type="button"
            className="secondary-button compact-button"
            disabled={busy !== null}
            onClick={() => {
              setBusy("refresh");
              void listNearbyDevices()
                .then((devices) => {
                  setNearbyDevices(devices);
                  setDeviceMessage(devices.length === 0 ? "No nearby devices found." : null);
                })
                .catch(() => setDeviceMessage("Pastey cannot see nearby devices on this network."))
                .finally(() => setBusy(null));
            }}
          >
            {busy === "refresh" ? "Scanning..." : "Rescan network"}
          </button>
        </div>
        <div className="device-table" role="table" aria-label="Discovered devices">
          <div className="device-table-header" role="row">
            <span>Device</span>
            <span>Status</span>
            <span>Last seen / Latency</span>
            <span>Capabilities</span>
            <span>OS / Type</span>
            <span>Actions</span>
          </div>
          {deviceRows.length === 0 ? (
            <div className="empty-state">
              <strong>No devices discovered yet</strong>
              <p className="muted">Use nearby discovery or join with code to connect a device.</p>
            </div>
          ) : null}
          {deviceRows.map((device) => (
            <DeviceRow
              key={device.id}
              selected={selectedDevice?.id === device.id}
              name={device.name}
              subtitle={device.kind === "nearby" ? "Nearby device" : "Connected peer"}
              status={device.status}
              latency={device.latency}
              capabilities={device.capabilities}
              osType={device.osType}
              action={device.action}
              onClick={() => {
                setSelectedDeviceId(device.id);
                if (device.kind === "room") onSelectConnection(device.room.id);
              }}
            />
          ))}
        </div>
        {deviceMessage ? <p className="muted">{deviceMessage}</p> : null}
      </section>

      <section className="summary-card selected-device-card">
        <div className="section-row">
          <h2>Selected device</h2>
          {selectedDevice ? <span className={`status-chip ${selectedDevice.status === "Available" || selectedDevice.status === "Connected" ? "success" : "neutral"}`}>{selectedDevice.status}</span> : null}
        </div>
        {selectedDevice ? (
          <>
            <div className="selected-device-heading">
              <strong>{selectedDevice.name}</strong>
              <span className="muted">{selectedDevice.kind === "nearby" ? selectedDevice.osType : selectedDevice.latency}</span>
            </div>
            <div className="device-detail-grid">
              <section>
                <h3>Capabilities summary</h3>
                <div className="summary-list">
                  <div className="summary-row"><span>Capabilities</span><small>{selectedDevice.capabilities}</small></div>
                  <div className="summary-row"><span>Request file</span><small>{selectedDevice.kind === "room" ? "Safe locations" : "Available after connection"}</small></div>
                  <div className="summary-row"><span>Max transfer size</span><small>{selectedDevice.kind === "nearby" && selectedDevice.device.capabilities.includes("large_file") ? "Large files ready" : "10 GB"}</small></div>
                  <div className="summary-row"><span>Encryption</span><small>AES-256 (E2EE)</small></div>
                </div>
              </section>
              <section>
                <h3>Connection details</h3>
                <div className="summary-list">
                  <div className="summary-row"><span>Connection</span><small>{selectedDevice.status}</small></div>
                  <div className="summary-row"><span>Discovery</span><small>{selectedDevice.kind === "nearby" ? "Nearby broadcast" : "Connected session"}</small></div>
                  <div className="summary-row"><span>Last seen</span><small>{selectedDevice.latency}</small></div>
                </div>
              </section>
              <section>
                <h3>Bridge actions</h3>
                <div className="device-action-list">
                  <button
                    type="button"
                    className="secondary-button"
                    disabled={selectedDevice.kind === "nearby" || !selectedRoom}
                    onClick={() => {
                      if (selectedRoom) {
                        onSelectConnection(selectedRoom.id);
                        onSelectView("bridge");
                      }
                    }}
                  >
                    Open in Bridge
                  </button>
                  <button type="button" className="secondary-button" onClick={() => onSelectView("transfers")}>View transfers</button>
                  {selectedDevice.kind === "nearby" ? (
                    <button
                      type="button"
                      className="primary-button"
                      disabled={busy !== null || selectedDevice.device.availability !== "Available" || !selectedDevice.device.compatible}
                      onClick={() => void handleNearbyJoin(selectedDevice.device)}
                    >
                      {joiningDeviceId === selectedDevice.device.device_id ? "Waiting..." : "Add to Bridge"}
                    </button>
                  ) : null}
                </div>
              </section>
            </div>
          </>
        ) : (
          <p className="muted">No selected device yet. Nearby devices appear when discovery sees them.</p>
        )}
      </section>

      <div className="workbench-grid two-column">
        <section className="summary-card">
          <h2>Paired / trusted peers</h2>
          <div className="trusted-peer-grid">
            {trustedRooms.length === 0 ? <span className="empty-inline">Trusted peer details are not available yet.</span> : null}
            {trustedRooms.slice(0, 4).map((room) => (
              <div key={room.id} className="trusted-peer-pill">
                <span>{room.peer_device_name}</span>
                <small>{room.peer_connected ? "Available" : "Recent"}</small>
              </div>
            ))}
          </div>
        </section>
        <section className="summary-card">
          <h2>Discovery / security summary</h2>
          <div className="summary-list">
            <div className="summary-row"><span>Nearby devices</span><small>{nearbyDevices.length}</small></div>
            <div className="summary-row"><span>Connected peers</span><small>{connectedCount}</small></div>
            <div className="summary-row"><span>Known connections</span><small>{rooms.length}</small></div>
            <div className="summary-row"><span>Approval required</span><small>On</small></div>
            <div className="summary-row"><span>Encryption</span><small>Enabled</small></div>
          </div>
        </section>
      </div>

      <section className="summary-card manual-join-card">
        <div>
          <h2>Join with code</h2>
          <p className="muted">Connect manually with an existing connection code.</p>
        </div>
        <div className="join-code-controls compact">
          <input
            ref={joinInputRef}
            inputMode="numeric"
            aria-label="Connection code"
            placeholder="4829-1736"
            value={formatCode(joinCode)}
            onChange={(event) => setJoinCode(event.target.value.replace(/[^\d]/g, "").slice(0, 8))}
            onKeyDown={(event) => {
              if (event.key === "Enter" && joinCode.length === 8 && busy === null) {
                event.preventDefault();
                void handleJoinRoom();
              }
            }}
          />
          <button className="secondary-button" onClick={handleJoinRoom} disabled={busy !== null || joinCode.length !== 8}>
            {busy === "join" ? "Joining..." : "Join"}
          </button>
        </div>
      </section>

      <Card className="device-diagnostics-card">
        <div className="section-row">
          <h2>Advanced diagnostics</h2>
          <StatusChip tone={busy === "refresh" ? "warning" : "neutral"}>{busy === "refresh" ? "Scanning" : "Ready"}</StatusChip>
        </div>
        <div className="summary-list">
          <div className="summary-row"><span>Discovery polling</span><small>Every 2 seconds</small></div>
          <div className="summary-row"><span>Nearby command</span><small>Existing discovery wrapper</small></div>
          <div className="summary-row"><span>Last message</span><small>{deviceMessage ?? "None"}</small></div>
        </div>
      </Card>
    </section>
  );
}

type SafeSearchScope = "downloads" | "desktop" | "documents" | "pastey_shared";

const SAFE_SEARCH_SCOPES: Array<{ value: SafeSearchScope; label: string }> = [
  { value: "downloads", label: "Downloads" },
  { value: "documents", label: "Documents" },
  { value: "desktop", label: "Desktop" },
  { value: "pastey_shared", label: "Pastey Shared" },
];

function CandidateMetadataCard({
  candidate,
  selected,
  onSelect,
}: {
  candidate: CandidatePayloadWorkflowCandidate;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      className={`candidate-metadata-card ${selected ? "selected" : ""}`}
      onClick={onSelect}
    >
      <span className="meta-label">Candidate metadata</span>
      <strong>{candidate.candidateDisplayName}</strong>
      <span>{formatBytes(candidate.sizeBytes)} - {candidate.extension || candidate.mimeFamily}</span>
      <span>{candidate.redactedLocation}</span>
      <span>Modified {formatTimestamp(Date.parse(candidate.modifiedAt))}</span>
      <small>{candidate.matchReason} - {candidate.confidence}</small>
    </button>
  );
}

function prepareCandidateSearchWorkflow(
  searchQuery: string,
  safeScopes: SafeSearchScope[],
  targetPeerRef: string,
) {
  const plan = buildMockFileCandidatePlan();
  const proposedInput = plan.proposedInput ?? {};
  const query = typeof proposedInput.query === "object" && proposedInput.query !== null ? proposedInput.query : {};
  const context = buildMockAiContextSnapshot();
  return {
    plan: {
      ...plan,
      title: "Request metadata-only search",
      explanation: "Ask the selected peer for metadata-only candidates. Receiver Allow once is required.",
      references: [{ kind: "peer" as const, ref: targetPeerRef }],
      proposedInput: {
        ...proposedInput,
        targetPeerRef,
        query: {
          ...query,
          rawUserRequest: searchQuery,
          filenameHint: searchQuery.trim(),
          searchMode: "filename_metadata_only",
        },
        scopePolicy: {
          allowedScopes: safeScopes,
          allowFullDisk: false,
          includeFileContents: false,
          includeAbsolutePaths: false,
          includeHiddenFiles: false,
        },
        safety: {
          returnRedactedPaths: true,
          noAutoTransfer: true,
          requireReceiverConsent: true,
          selectedPeerOnly: true,
        },
      },
    },
    context: {
      ...context,
      peers: [{
        peerRef: targetPeerRef,
        visible: true,
        trusted: true,
        capabilities: [
          "filesystem.find_file_candidates",
          "transfer.request_candidate_payload",
        ],
      }],
    },
  };
}

function InboxView({
  joinRequest,
  onAccept,
  onReject,
}: {
  joinRequest: JoinRequestPrompt | null;
  onAccept: (request: JoinRequestPrompt) => void;
  onReject: (request: JoinRequestPrompt) => void;
}) {
  return (
    <section className="workstation-view inbox-workstation" aria-label="Inbox">
      <Card className="approval-card">
        {joinRequest ? (
          <>
            <div>
              <span className="meta-label">Inbox</span>
              <h2>{joinRequest.device_name}</h2>
              <p className="muted">
                {joinRequest.platform} - Pastey {joinRequest.app_version}. Review before allowing this connection once.
              </p>
            </div>
            <div className="row gap">
              <button className="ghost-button" onClick={() => onReject(joinRequest)}>Deny</button>
              <button className="primary-button" onClick={() => onAccept(joinRequest)}>Allow once</button>
            </div>
          </>
        ) : (
          <>
            <h2>No requests waiting</h2>
            <p className="muted">
              Metadata-only searches and payload requests still require an explicit receiver decision when they arrive.
            </p>
          </>
        )}
      </Card>
      <div className="workbench-grid two-column">
        <Card className="approval-card">
          <span className="meta-label">Metadata search request</span>
          <h2>No live request</h2>
          <p className="muted">
            Search approvals are reviewed when a live peer request is available. Results contain metadata only, never file contents or full local paths.
          </p>
          <StatusChip tone="neutral">Non-actionable</StatusChip>
        </Card>
        <Card className="approval-card">
          <span className="meta-label">Candidate payload request</span>
          <h2>No live request</h2>
          <p className="muted">
            Candidate payload approval appears only for a concrete selected candidate. Allow once queues the approved request, not transfer completion.
          </p>
          <StatusChip tone="neutral">Non-actionable</StatusChip>
        </Card>
        <Card className="approval-card">
          <span className="meta-label">Security facts</span>
          <h2>Explicit consent</h2>
          <div className="summary-list">
            <div className="summary-row"><span>Allow once</span><small>One request only</small></div>
            <div className="summary-row"><span>Metadata search</span><small>No contents</small></div>
            <div className="summary-row"><span>Payload request</span><small>Selected candidate only</small></div>
          </div>
        </Card>
      </div>
    </section>
  );
}

function TransfersView({
  rooms,
  transfers,
  queueItems,
}: {
  rooms: RoomInfo[];
  transfers: FileTransferProgressEvent[];
  queueItems: TransferQueueItem[];
}) {
  const [filter, setFilter] = useState<TransferViewFilter>("all");
  const events = buildTransferEvents(rooms, transfers, queueItems);
  const filteredEvents = events.filter((event) => {
    if (filter === "all") return true;
    if (filter === "transfers") return event.kind === "transfer";
    if (filter === "agent") return event.kind === "agent";
    return event.tone === "danger";
  });

  return (
    <section className="workstation-view transfers-workstation" aria-label="Transfers">
      <div className="segmented-control" aria-label="Transfer filters">
        {TRANSFER_VIEW_FILTERS.map((item) => (
          <button
            key={item.value}
            type="button"
            className={filter === item.value ? "active" : ""}
            onClick={() => setFilter(item.value)}
          >
            {item.label}
          </button>
        ))}
      </div>

      <div className="activity-event-list">
        {filteredEvents.length === 0 ? (
          <EmptyState title="No transfers for this filter" detail="Current-session transfer and queue items appear here." />
        ) : null}
        {filteredEvents.map((event) => (
          <article key={event.id} className="activity-event-row">
            <div>
              <span className={`status-chip ${event.tone}`}>{event.label}</span>
              <h2>{event.title}</h2>
              <p className="muted">{event.detail}</p>
            </div>
            <small>{event.timeLabel}</small>
          </article>
        ))}
      </div>
      <p className="muted activity-note">Full history is not stored yet.</p>
    </section>
  );
}

type TransferViewFilter = "all" | "transfers" | "agent" | "errors";

interface TransferEventView {
  id: string;
  kind: "transfer" | "agent" | "approval";
  label: string;
  title: string;
  detail: string;
  tone: "neutral" | "success" | "warning" | "danger";
  timeLabel: string;
}

const TRANSFER_VIEW_FILTERS: Array<{ value: TransferViewFilter; label: string }> = [
  { value: "all", label: "All" },
  { value: "transfers", label: "Transfers" },
  { value: "agent", label: "Requests" },
  { value: "errors", label: "Errors" },
];

function buildTransferEvents(
  rooms: RoomInfo[],
  transfers: FileTransferProgressEvent[],
  queueItems: TransferQueueItem[],
): TransferEventView[] {
  const transferEvents = transfers.map((transfer) => ({
    id: `transfer-${transfer.transfer_id}`,
    kind: "transfer" as const,
    label: transferEventLabel(transfer),
    title: transfer.file_name,
    detail: `${transfer.direction === "incoming" ? "Incoming" : "Outgoing"} - ${formatBytes(transfer.transferred_bytes)} of ${formatBytes(transfer.file_size)}`,
    tone: transferEventTone(transfer),
    timeLabel: transfer.status,
  }));
  const queueEvents = queueItems.map((item) => ({
    id: `queue-${item.id}`,
    kind: item.agentBridgeMetadata ? "agent" as const : "transfer" as const,
    label: item.agentBridgeMetadata ? "Candidate payload request" : "Transfer queue",
    title: queueItemLabel(item),
    detail: item.agentBridgeMetadata
      ? `${roomLabelById(rooms, item.roomId)} - Queued from approved request`
      : `${roomLabelById(rooms, item.roomId)} - ${queueItemStatusLabel(item)}`,
    tone: item.status === "failed" ? "danger" as const : item.status === "completed" ? "success" as const : "neutral" as const,
    timeLabel: formatTimestamp(item.updatedAt),
  }));
  return [...queueEvents, ...transferEvents].slice(0, 40);
}

function transferEventLabel(transfer: FileTransferProgressEvent): string {
  if (transfer.status === "completed") return "Transfer completed";
  if (transfer.status === "cancelled") return "Transfer cancelled";
  if (transfer.status === "burned" || transfer.status === "interrupted") return "Burned";
  if (transfer.status === "failed") return "Failed";
  return "Transfer queue";
}

function transferEventTone(transfer: FileTransferProgressEvent): TransferEventView["tone"] {
  if (transfer.status === "completed") return "success";
  if (transfer.status === "failed" || transfer.status === "burned" || transfer.status === "interrupted") return "danger";
  if (transfer.status === "cancelled") return "warning";
  return "neutral";
}

function roomLabelById(rooms: RoomInfo[], roomId: string): string {
  const room = rooms.find((candidate) => candidate.id === roomId);
  return room?.peer_device_name ?? "Current connection";
}

function connectionStatusLabel(room: RoomInfo): string {
  if (room.peer_connected) return "Connected";
  if (room.peer_burned_at) return "Peer done";
  if (room.status === "peer_left") return "Peer disconnected";
  if (room.status === "burned") return "Burned";
  return "Waiting";
}

function nearbyDeviceStatus(device: NearbyDevice): string {
  if (!device.compatible) return "Update needed";
  return device.availability;
}

function nearbyCapabilitySummary(device: NearbyDevice): string {
  const capabilities = [];
  if (device.capabilities.includes("large_file")) capabilities.push("Large files");
  if (device.capabilities.includes("nearby_join")) capabilities.push("Nearby join");
  if (device.compatible) capabilities.push("Send & receive");
  return capabilities.length > 0 ? capabilities.join(", ") : "Discovery only";
}

function networkHelpMessage(message: string): string {
  if (message.includes("rejected")) return "Join request rejected.";
  if (message.includes("timed out")) return "Join request timed out.";
  if (message.includes("No nearby")) return "No nearby devices found.";
  if (message.includes("could not connect")) return "Device found, but Pastey could not connect to it.";
  if (message.includes("block") || message.includes("Firewall")) return "This network may block local device connections.";
  return message;
}

function Card({ className, children }: { className?: string; children: ReactNode }) {
  return <section className={`summary-card${className ? ` ${className}` : ""}`}>{children}</section>;
}

function Section({
  title,
  trailing,
  className,
  children,
}: {
  title: string;
  trailing?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section className={className}>
      <div className="section-row">
        <h2>{title}</h2>
        {trailing ? <span className="muted">{trailing}</span> : null}
      </div>
      {children}
    </section>
  );
}

function StatusChip({
  tone = "neutral",
  children,
}: {
  tone?: "neutral" | "success" | "warning" | "danger";
  children: ReactNode;
}) {
  return <span className={`status-chip ${tone}`}>{children}</span>;
}

function OptionRow({
  label,
  detail,
  control,
  disabled,
}: {
  label: string;
  detail?: string;
  control?: ReactNode;
  disabled?: boolean;
}) {
  return (
    <label className={`option-row-card ${disabled ? "disabled-option" : ""}`}>
      <span>
        <strong>{label}</strong>
        {detail ? <small>{detail}</small> : null}
      </span>
      {control ? <span className="option-row-control">{control}</span> : null}
    </label>
  );
}

function ScopeChip({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: () => void;
}) {
  return (
    <button
      type="button"
      className={`scope-chip ${checked ? "checked" : ""}`}
      aria-pressed={checked}
      onClick={onChange}
    >
      <span className="scope-chip-check" aria-hidden="true" />
      <span>{label}</span>
    </button>
  );
}

function EmptyState({ title, detail }: { title: string; detail: string }) {
  return (
    <div className="empty-state compact-empty-card">
      <strong>{title}</strong>
      <p className="muted">{detail}</p>
    </div>
  );
}

function ActionRow({ children }: { children: ReactNode }) {
  return <div className="action-row">{children}</div>;
}

function DeviceRow({
  selected,
  name,
  subtitle,
  status,
  latency,
  capabilities,
  osType,
  action,
  onClick,
}: {
  selected: boolean;
  name: string;
  subtitle: string;
  status: string;
  latency: string;
  capabilities: string;
  osType: string;
  action: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`device-table-row ${selected ? "selected" : ""}`}
      role="row"
      onClick={onClick}
    >
      <span>
        <strong>{name}</strong>
        <small>{subtitle}</small>
      </span>
      <span>
        <StatusChip tone={status === "Available" || status === "Connected" ? "success" : "neutral"}>{status}</StatusChip>
      </span>
      <span>{latency}</span>
      <span>{capabilities}</span>
      <span>{osType}</span>
      <span>{action}</span>
    </button>
  );
}

function TransferRow({
  title,
  detail,
  meta,
  status,
  tone,
  actionLabel,
  onAction,
}: {
  title: string;
  detail: string;
  meta: string;
  status: string;
  tone: "neutral" | "success" | "warning" | "danger";
  actionLabel?: string;
  onAction?: () => void;
}) {
  return (
    <article className="transfer-row">
      <div>
        <strong>{title}</strong>
        <span className="muted">{detail}</span>
      </div>
      <span className="muted">{meta}</span>
      <StatusChip tone={tone}>{status}</StatusChip>
      {onAction ? (
        <button type="button" className="ghost-button compact-button" onClick={onAction}>
          {actionLabel ?? "Open"}
        </button>
      ) : null}
    </article>
  );
}

function SummaryPanel({
  title,
  emptyLabel,
  children,
}: {
  title: string;
  emptyLabel: string;
  children: ReactNode;
}) {
  const hasContent = Array.isArray(children) ? children.length > 0 : Boolean(children);

  return (
    <section className="summary-card">
      <h2>{title}</h2>
      <div className="summary-list">
        {hasContent ? children : <span className="empty-inline">{emptyLabel}</span>}
      </div>
    </section>
  );
}

function isActiveTransfer(transfer: FileTransferProgressEvent): boolean {
  return transfer.status === "pending" || transfer.status === "transferring";
}

function isActiveQueueItem(item: TransferQueueItem): boolean {
  return item.status === "queued" || item.status === "preparing" || item.status === "sending";
}

function queueItemLabel(item: TransferQueueItem): string {
  if (item.agentBridgeMetadata) return "Queued from approved candidate payload request";
  return item.displayName ?? "Queued transfer";
}

function queueItemStatusLabel(item: TransferQueueItem): string {
  if (item.status === "queued" && item.agentBridgeMetadata) {
    return "Queued only";
  }
  return item.status;
}

function findWorkflowStatusLabel(state: CandidatePayloadWorkflow["snapshot"]["state"]): string {
  switch (state) {
    case "idle":
      return "Waiting for peer, query, and safe locations.";
    case "search_preview_ready":
      return "Metadata search preview ready.";
    case "search_pending_receiver_consent":
      return "Waiting for receiver Allow once for metadata search.";
    case "search_completed_candidates_ready":
    case "candidate_selection_required":
      return "Candidate selection required.";
    case "payload_preview_ready":
      return "Candidate payload request preview ready.";
    case "payload_pending_receiver_consent":
      return "Waiting for receiver Allow once for candidate payload.";
    case "handoff_queued":
      return "Queued from approved candidate payload request.";
    case "failed":
      return "Needs review.";
  }
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

function microGroupPlannerPolicy(
  mode?: MicroFlowGroupMode,
  globalWindowBudget = DEFAULT_TRANSFER_PLANNER_POLICY.globalWindowBudget,
): TransferPlannerPolicy {
  return {
    ...DEFAULT_TRANSFER_PLANNER_POLICY,
    globalWindowBudget,
    maxRequestedWindow: Math.min(
      DEFAULT_TRANSFER_PLANNER_POLICY.maxRequestedWindow,
      globalWindowBudget
    ),
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

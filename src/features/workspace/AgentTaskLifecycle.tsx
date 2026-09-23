import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  approveNativeV2Plan,
  approveRemoteNativeCodexWorkspaceMovement,
  cancelNativeV2PlanAttempt,
  cancelNativeAgentTask,
  discardNativeAgentConflictResult,
  cancelRemoteNativeAgentTask,
  getNativeAgentRecoveryProjection,
  getNativeAgentTaskStatus,
  getNativeAgentWorkspaceMovementStatus,
  getNativeV2PlanStatus,
  listNativeAgentCapabilities,
  proposeRemoteNativeCodexWorkspaceMovement,
  revealNativeAgentConflictResult,
  reconcileRemoteNativeAgentTask,
  retryNativeAgentWorkspaceResultReturn,
  startNativeCodexTask,
  startRemoteNativeCodexTask,
  stopBridgeNativeAgentTask,
  type NativeAgentCapability,
  type NativeAgentTaskStatus,
  type NativeAgentRecoveryProjection,
  type NativeAgentWorkspaceMovement,
  startNativeV2PlanAttempt,
  type NativeV2PlanStatus,
  type NativeV2ProductState,
} from "../../lib/tauri";
import { ownAsyncDisposer } from "../../lib/subscriptionLifecycle";

const ONE_DAY_SECONDS = 24 * 60 * 60;

export type LifecycleTone = "neutral" | "pending" | "live" | "danger" | "complete";

/** Durable movement history stays visible. Any movement whose result or
 * execution authority remains unresolved owns the canonical workspace. */
export function nativeAgentMovementBlocksNewRun(
  movement: NativeAgentWorkspaceMovement | null,
): boolean {
  return !!movement && (
    !["completed", "failed", "cancelled", "interrupted"].includes(movement.state)
    || (movement.state === "interrupted" && [
      "native_agent_reconciliation_required",
      "native_agent_outcome_unknown",
      "result_apply_interrupted",
    ].includes(movement.code ?? ""))
  );
}

export function nativeAgentRecoveryRequiresAction(
  status: NativeAgentTaskStatus | null,
  movement: NativeAgentWorkspaceMovement | null,
): boolean {
  return nativeAgentInterruptedRecoveryRequiresAction(status, movement)
    || movement?.state === "conflict_recovery_required"
    || (movement?.code === "result_return_retry_required"
      && ["returning_result", "interrupted"].includes(movement.state));
}

export function nativeAgentInterruptedRecoveryRequiresAction(
  status: NativeAgentTaskStatus | null,
  movement: NativeAgentWorkspaceMovement | null,
): boolean {
  return (status?.state === "interrupted"
    && ["native_agent_reconciliation_required", "native_agent_outcome_unknown"].includes(status.code ?? ""))
    || (movement?.state === "interrupted" && [
      "native_agent_reconciliation_required",
      "conflict_result_retention_required",
      "result_apply_interrupted",
    ].includes(movement.code ?? ""));
}

export function nativeAgentRemoteReconciliationRequired(
  status: NativeAgentTaskStatus | null,
  movement: NativeAgentWorkspaceMovement | null,
): boolean {
  return (status?.state === "interrupted"
    && ["native_agent_reconciliation_required", "native_agent_outcome_unknown"].includes(status.code ?? ""))
    || (movement?.state === "interrupted" && movement.code === "native_agent_reconciliation_required");
}

export function nativeAgentConsequenceAbandonmentRequired(
  movement: NativeAgentWorkspaceMovement | null,
): boolean {
  return movement?.state === "interrupted"
    && movement.code === "conflict_result_retention_required";
}

export function nativeAgentTaskNeedsObservation(status: NativeAgentTaskStatus | null): boolean {
  return !!status && (["queued", "running"].includes(status.state)
    || (status.state === "interrupted"
      && ["native_agent_reconciliation_required", "native_agent_outcome_unknown"].includes(status.code ?? "")));
}

export const STATE_COPY: Record<NativeV2ProductState, { label: string; detail: string; tone: LifecycleTone }> = {
  draft: { label: "Awaiting review", detail: "The PM proposal is an immutable Draft. Nothing can execute yet.", tone: "pending" },
  approved: { label: "Awaiting Host admission", detail: "Requester approval is recorded. Participating Hosts must still admit the Plan.", tone: "pending" },
  checking_readiness: { label: "Awaiting Host readiness", detail: "Pastey is checking the whole Plan scope. Approval does not imply execution.", tone: "pending" },
  preparing: { label: "Preparing", detail: "Participating Hosts are deriving bounded local execution state.", tone: "pending" },
  running: { label: "Running", detail: "Managed execution is active under the approved Plan.", tone: "live" },
  completed: { label: "Completed", detail: "The Host reported the Plan terminal state.", tone: "complete" },
  failed: { label: "Failed", detail: "The Host reported a terminal failure.", tone: "danger" },
  interrupted: { label: "Interrupted / indeterminate", detail: "Pastey cannot safely infer a successful result.", tone: "danger" },
  cancelled: { label: "Cancelled", detail: "The managed attempt was cancelled. The Bridge was not burned.", tone: "neutral" },
};

function nowSeconds(): number {
  return Math.floor(Date.now() / 1_000);
}

function hasTauriRuntime(): boolean {
  return typeof window !== "undefined" && ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);
}

export function useAgentTaskLifecycle() {
  const [revisionInput, setRevisionInput] = useState("");
  const [revisionId, setRevisionId] = useState<string | null>(null);
  const [status, setStatus] = useState<NativeV2PlanStatus | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState<"open" | "approve" | "start" | "cancel" | null>(null);
  const revisionIdRef = useRef(revisionId);

  useEffect(() => {
    revisionIdRef.current = revisionId;
  }, [revisionId]);

  const refresh = useCallback(async (id = revisionId) => {
    if (!id) return;
    try {
      setStatus(await getNativeV2PlanStatus(id));
      setMessage(null);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not read this Plan status.");
    }
  }, [revisionId]);

  useEffect(() => {
    if (!hasTauriRuntime()) return;
    if (revisionId) void refresh(revisionId);
    const interval = revisionId ? window.setInterval(() => void refresh(revisionId), 2_000) : null;
    return () => {
      if (interval !== null) window.clearInterval(interval);
    };
  }, [refresh, revisionId]);

  useEffect(() => {
    if (!hasTauriRuntime()) return;
    const dispose = ownAsyncDisposer(listen<NativeV2PlanStatus>("pastey://native-v2-plan-status", (event) => {
      if (revisionIdRef.current === null || event.payload.revisionId === revisionIdRef.current) {
        setRevisionId(event.payload.revisionId);
        setStatus(event.payload);
      }
    }));
    return dispose;
  }, []);

  const openRevision = useCallback(async () => {
    const nextId = revisionInput.trim();
    if (!nextId) return;
    setBusy("open");
    setRevisionId(nextId);
    try {
      setStatus(await getNativeV2PlanStatus(nextId));
      setMessage(null);
    } catch (error) {
      setStatus(null);
      setMessage(error instanceof Error ? error.message : "Pastey could not open this Draft.");
    } finally { setBusy(null); }
  }, [revisionInput]);

  const approve = useCallback(async () => {
    if (!status || status.state !== "draft") return;
    setBusy("approve");
    try {
      setStatus(await approveNativeV2Plan(status.revisionId, `native-v2-approval-${crypto.randomUUID()}`, nowSeconds() + ONE_DAY_SECONDS));
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not approve this Draft.");
    } finally { setBusy(null); }
  }, [status]);

  const beginReadiness = useCallback(async () => {
    if (status?.state !== "approved" || !status.approvalId) return;
    setBusy("start");
    try {
      setStatus(await startNativeV2PlanAttempt(status.approvalId, `native-v2-attempt-${crypto.randomUUID()}`, nowSeconds() + ONE_DAY_SECONDS));
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not begin Host readiness.");
    } finally { setBusy(null); }
  }, [status]);

  const cancel = useCallback(async () => {
    if (!status?.attemptId || !["checking_readiness", "preparing", "running"].includes(status.state)) return;
    setBusy("cancel");
    try {
      setStatus(await cancelNativeV2PlanAttempt(status.attemptId));
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not cancel this managed attempt.");
    } finally { setBusy(null); }
  }, [status]);

  const closeRevision = useCallback(() => {
    setRevisionId(null);
    setStatus(null);
    setMessage(null);
  }, []);

  const progress = useMemo(() => status && status.totalSteps > 0
    ? `${status.completedSteps} of ${status.totalSteps} steps reported`
    : "Step projection unavailable", [status]);

  return { status, presentation: status ? STATE_COPY[status.state] : null, progress, revisionInput, setRevisionInput, message, busy, openRevision, approve, beginReadiness, cancel, closeRevision, refresh };
}

export type AgentTaskController = ReturnType<typeof useAgentTaskLifecycle>;

/** Product-facing lifecycle for a Host-native mature Agent. It does not use
 * the managed Worker/Plan controller above. */
export function useNativeAgentTask(roomId: string) {
  const [capabilities, setCapabilities] = useState<NativeAgentCapability[]>([]);
  const [workspace, setWorkspace] = useState("");
  const [taskText, setTaskText] = useState("");
  const [status, setStatus] = useState<NativeAgentTaskStatus | null>(null);
  const [movement, setMovement] = useState<NativeAgentWorkspaceMovement | null>(null);
  const [recoveryProjection, setRecoveryProjection] = useState<NativeAgentRecoveryProjection | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const recoveryCorrelation = movement
    ? { taskId: movement.taskId, movementId: movement.movementId, targetHostRef: movement.targetHostRef }
    : recoveryProjection
      ? {
        taskId: recoveryProjection.task.taskId,
        movementId: recoveryProjection.movement?.movementId ?? null,
        targetHostRef: recoveryProjection.targetHostRef ?? null,
      }
      : null;
  const cancellableTaskId = status && (
    ["queued", "running"].includes(status.state)
    || (status.state === "interrupted" && ["native_agent_outcome_unknown", "native_agent_reconciliation_required"].includes(status.code ?? ""))
  )
    ? status.taskId
    : movement?.state === "agent_running"
      ? movement.taskId
      : null;

  const refreshCapabilities = useCallback(async () => {
    try { setCapabilities(await listNativeAgentCapabilities()); }
    catch (error) { setMessage(error instanceof Error ? error.message : "Pastey could not inspect native Agents."); }
  }, []);

  const loadRecoveryProjection = useCallback(async (isCurrent: () => boolean = () => true) => {
    const projection = await getNativeAgentRecoveryProjection(roomId);
    if (!isCurrent()) return null;
    setRecoveryProjection(projection);
    if (projection) {
      setStatus(projection.task);
      setMovement(projection.movement ?? null);
    } else {
      setStatus(null);
      setMovement(null);
    }
    return projection;
  }, [roomId]);

  useEffect(() => { void refreshCapabilities(); }, [refreshCapabilities]);
  useEffect(() => {
    if (!hasTauriRuntime() || !roomId) return;
    let cancelled = false;
    void loadRecoveryProjection(() => !cancelled).catch((error) => {
      if (!cancelled) setMessage(error instanceof Error ? error.message : "Pastey could not recover unresolved native Agent work.");
    });
    return () => { cancelled = true; };
  }, [loadRecoveryProjection, roomId]);
  useEffect(() => {
    const recoveryTaskId = recoveryCorrelation?.taskId;
    if (!recoveryTaskId || status?.taskId !== recoveryTaskId
      || nativeAgentRecoveryRequiresAction(status, movement)) return;
    // The card remains single-item. Once its durable fact becomes terminal,
    // immediately ask for the next unresolved Bridge-bound item until the
    // backend reports that recovery is drained.
    setRecoveryProjection(null);
    void loadRecoveryProjection().catch((error) => {
      setMessage(error instanceof Error ? error.message : "Pastey could not load the next unresolved native Agent task.");
    });
  }, [loadRecoveryProjection, movement, recoveryCorrelation?.taskId, status]);
  useEffect(() => {
    const observedStatus = status;
    if (!observedStatus || !nativeAgentTaskNeedsObservation(observedStatus)) return;
    let cancelled = false;
    const poll = async () => {
      try { setStatus(await getNativeAgentTaskStatus(observedStatus.taskId)); }
      catch (error) { setMessage(error instanceof Error ? error.message : "Pastey lost the native Agent task outcome."); }
      if (!cancelled) window.setTimeout(() => void poll(), 1_000);
    };
    const timer = window.setTimeout(() => void poll(), 1_000);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [status]);
  useEffect(() => {
    if (!movement || ["completed", "conflict_recovery_required", "failed", "cancelled"].includes(movement.state)
      || (movement.state === "interrupted" && movement.code !== "native_agent_reconciliation_required")) return;
    const timer = window.setTimeout(() => {
      void getNativeAgentWorkspaceMovementStatus(movement.movementId).then(setMovement).catch((error) => {
        setMessage(error instanceof Error ? error.message : "Pastey lost the workspace movement outcome.");
      });
    }, 1_000);
    return () => window.clearTimeout(timer);
  }, [movement]);

  const start = useCallback(async () => {
    if (!workspace.trim() || !taskText.trim() || busy) return;
    setBusy(true); setMessage(null);
    try { setStatus(await startNativeCodexTask(workspace.trim(), taskText.trim())); }
    catch (error) { setMessage(error instanceof Error ? error.message : "Pastey could not start Codex."); }
    finally { setBusy(false); }
  }, [busy, taskText, workspace]);

  const startRemote = useCallback(async (roomId: string, peerSessionId: string, hostRef: string) => {
    if (!roomId || !peerSessionId || !hostRef || !workspace.trim() || !taskText.trim() || busy) return;
    setBusy(true); setMessage(null);
    try {
      const started = await startRemoteNativeCodexTask(roomId, peerSessionId, hostRef, workspace.trim(), taskText.trim(), true);
      try {
        const projection = await loadRecoveryProjection();
        if (!projection) setStatus(started);
      } catch (error) {
        setStatus(started);
        setMessage(error instanceof Error ? error.message : "Pastey could not load the remote task correlation.");
      }
    }
    catch (error) { setMessage(error instanceof Error ? error.message : "Pastey could not start remote Codex."); }
    finally { setBusy(false); }
  }, [busy, loadRecoveryProjection, taskText, workspace]);

  const proposeRemoteMovement = useCallback(async (roomId: string, hostRef: string) => {
    if (!roomId || !hostRef || !workspace.trim() || !taskText.trim() || busy) return;
    setBusy(true); setMessage(null);
    try { setMovement(await proposeRemoteNativeCodexWorkspaceMovement(hostRef, workspace.trim(), taskText.trim(), roomId)); }
    catch (error) { setMessage(error instanceof Error ? error.message : "Pastey could not prepare the workspace review."); }
    finally { setBusy(false); }
  }, [busy, taskText, workspace]);

  const approveRemoteMovement = useCallback(async (roomId: string, peerSessionId: string) => {
    if (!movement || movement.state !== "awaiting_approval" || busy) return;
    setBusy(true); setMessage(null);
    try { setMovement(await approveRemoteNativeCodexWorkspaceMovement(movement.movementId, roomId, peerSessionId)); }
    catch (error) {
      try { setMovement(await getNativeAgentWorkspaceMovementStatus(movement.movementId)); } catch { /* retain the original actionable error */ }
      setMessage(error instanceof Error ? error.message : "Pastey could not start the approved workspace movement.");
    }
    finally { setBusy(false); }
  }, [busy, movement]);

  const cancel = useCallback(async () => {
    if (!cancellableTaskId) return;
    setBusy(true);
    try { setStatus(await cancelNativeAgentTask(cancellableTaskId)); }
    catch (error) { setMessage(error instanceof Error ? error.message : "Pastey could not cancel Codex."); }
    finally { setBusy(false); }
  }, [cancellableTaskId]);

  const cancelRemote = useCallback(async (roomId: string, peerSessionId: string, hostRef: string) => {
    if (!cancellableTaskId) return;
    setBusy(true);
    try {
      setStatus(await cancelRemoteNativeAgentTask(roomId, peerSessionId, hostRef, cancellableTaskId));
      if (movement) setMovement(await getNativeAgentWorkspaceMovementStatus(movement.movementId));
    }
    catch (error) { setMessage(error instanceof Error ? error.message : "Pastey could not cancel remote Codex."); }
    finally { setBusy(false); }
  }, [cancellableTaskId, movement]);

  const reconcileRemote = useCallback(async () => {
    if (!recoveryCorrelation?.taskId || !recoveryCorrelation.targetHostRef || busy) return;
    setBusy(true); setMessage(null);
    try {
      await reconcileRemoteNativeAgentTask(
        roomId,
        recoveryCorrelation.targetHostRef,
        recoveryCorrelation.taskId,
        recoveryCorrelation.movementId,
      );
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not reconcile the remote task.");
    } finally { setBusy(false); }
  }, [busy, recoveryCorrelation, roomId]);

  const retryResultReturn = useCallback(async () => {
    if (!movement || ![
      "result_return_retry_required",
      "result_apply_interrupted",
    ].includes(movement.code ?? "") || busy) return;
    setBusy(true); setMessage(null);
    try { setMovement(await retryNativeAgentWorkspaceResultReturn(movement.movementId, roomId)); }
    catch (error) {
      try { setMovement(await getNativeAgentWorkspaceMovementStatus(movement.movementId)); } catch { /* preserve delivery error */ }
      setMessage(error instanceof Error ? error.message : "Pastey could not retry the durable result Return.");
    } finally { setBusy(false); }
  }, [busy, movement, roomId]);

  const stopRecovery = useCallback(async () => {
    if (!recoveryCorrelation?.taskId || busy) return;
    setBusy(true); setMessage(null);
    try {
      setStatus(await stopBridgeNativeAgentTask(roomId, recoveryCorrelation.taskId));
      if (movement) setMovement(await getNativeAgentWorkspaceMovementStatus(movement.movementId));
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not abandon the unresolved recovery.");
    } finally { setBusy(false); }
  }, [busy, movement, recoveryCorrelation, roomId]);

  const refreshRecoveryProjection = useCallback(async () => {
    try { await loadRecoveryProjection(); }
    catch (error) {
      setMessage(error instanceof Error ? error.message : "Pastey could not load durable native Agent recovery details.");
    }
  }, [loadRecoveryProjection]);

  const revealConflictResult = useCallback(async () => {
    if (!movement || movement.state !== "conflict_recovery_required" || busy) return;
    setBusy(true); setMessage(null);
    try { await revealNativeAgentConflictResult(movement.movementId); }
    catch (error) { setMessage(error instanceof Error ? error.message : "Pastey could not reveal the retained result."); }
    finally { setBusy(false); }
  }, [busy, movement]);

  const discardConflictResult = useCallback(async () => {
    if (!movement || movement.state !== "conflict_recovery_required" || busy) return;
    setBusy(true); setMessage(null);
    try { setMovement(await discardNativeAgentConflictResult(movement.movementId)); }
    catch (error) { setMessage(error instanceof Error ? error.message : "Pastey could not discard the retained result."); }
    finally { setBusy(false); }
  }, [busy, movement]);

  return { capabilities, workspace, setWorkspace, taskText, setTaskText, status, movement, recoveryTargetHostRef: recoveryCorrelation?.targetHostRef ?? null, recoveryTaskId: recoveryCorrelation?.taskId ?? null, message, busy, canCancel: !!cancellableTaskId, start, startRemote, proposeRemoteMovement, approveRemoteMovement, cancel, cancelRemote, stopRecovery, reconcileRemote, retryResultReturn, revealConflictResult, discardConflictResult, refreshCapabilities, refreshRecoveryProjection };
}

export function StatusBadge({ tone, children }: { tone: LifecycleTone; children: React.ReactNode }) {
  return <span className={`v2-status ${tone}`}>{children}</span>;
}

import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  approveNativeV2Plan,
  cancelNativeV2PlanAttempt,
  getNativeV2PlanStatus,
  startNativeV2PlanAttempt,
  type NativeV2PlanStatus,
  type NativeV2ProductState,
} from "../../lib/tauri";

const ONE_DAY_SECONDS = 24 * 60 * 60;

export type LifecycleTone = "neutral" | "pending" | "live" | "danger" | "complete";

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
    let dispose: (() => void) | undefined;
    void listen<NativeV2PlanStatus>("pastey://native-v2-plan-status", (event) => {
      if (revisionId === null || event.payload.revisionId === revisionId) {
        setRevisionId(event.payload.revisionId);
        setStatus(event.payload);
      }
    }).then((unlisten) => { dispose = unlisten; });
    return () => {
      if (interval !== null) window.clearInterval(interval);
      dispose?.();
    };
  }, [refresh, revisionId]);

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

export function StatusBadge({ tone, children }: { tone: LifecycleTone; children: React.ReactNode }) {
  return <span className={`v2-status ${tone}`}>{children}</span>;
}

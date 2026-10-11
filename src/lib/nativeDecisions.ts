/** Presentation of native requests (Codex approvals) awaiting the user who
 * started a task. Rust owns every rule: which requests can be approved, by
 * whom, and when. These helpers only explain them. */
import type { NativeAgentTaskStatus, NativeDecision, NativeDecisionReview } from "./tauri";

/** This device's own progress sending one decision; never a native fact. */
export type NativeDecisionDelivery = "sending" | "sent";

export interface NativeDecisionPresentation {
  title: string;
  stateLabel: string;
  canAccept: boolean;
  canDecline: boolean;
  note: string | null;
}

const UNAVAILABLE_COPY: Record<string, string> = {
  native_decision_content_incomplete: "Codex did not include everything needed to review this (the command or its folder), so it cannot be approved here.",
  native_decision_widens_permissions: "This command also asks for wider sandbox permissions, which Pastey cannot show in full, so it cannot be approved here.",
  native_decision_accept_not_offered: "Codex does not offer a one-time approval for this request.",
  native_decision_content_unrenderable: "This request contains characters that could disguise what it does, so it is not shown and cannot be approved here.",
  native_decision_content_too_large: "This request is too large to show in full, so it cannot be approved here.",
  native_decision_content_unavailable: "Pastey did not receive the exact changes, so they cannot be approved here.",
  native_decision_content_changed: "The changes were updated after they were shown, so they cannot be approved.",
  native_decision_session_scope: "Codex asks to allow writes for the rest of its session; Pastey only gives one-time approvals.",
  native_decision_outside_task: "This request does not belong to this task's turn.",
  native_decision_unsupported: "Pastey cannot answer this kind of request (for example a permission grant, a question or a form).",
  native_decision_malformed: "Codex sent this request in a form Pastey cannot read safely.",
  native_decision_requester_unverified: "Only the device that started this task, after proving its identity, can answer Codex here, and this task has no such device.",
};

export function nativeDecisionUnavailableCopy(reason: string | null | undefined): string {
  return (reason && UNAVAILABLE_COPY[reason]) || "This request cannot be answered here.";
}

const DECISION_LABEL: Record<NativeDecision, string> = { accept: "approved once", decline: "denied" };

export function nativeDecisionTitle(review: NativeDecisionReview): string {
  switch (review.kind) {
    case "command_execution": return review.content?.networkHost ? "Codex asks for network access" : "Codex asks to run a command";
    case "file_change": return "Codex asks to change files";
    case "unsupported": return "Codex is waiting for something Pastey cannot answer";
  }
}

export function nativeDecisionExpired(review: NativeDecisionReview, now: number): boolean {
  return review.state === "pending" && review.expiresAt <= now;
}

/** What one review shows and offers. `remote` is whether the task runs on
 * another device: only the device that started a remote task decides its
 * requests, and never for a task running here. */
export function nativeDecisionPresentation(
  review: NativeDecisionReview,
  options: { now: number; remote: boolean; hostName: string; delivery?: NativeDecisionDelivery },
): NativeDecisionPresentation {
  const title = nativeDecisionTitle(review);
  const closed = { canAccept: false, canDecline: false };
  if (review.state === "resolved") {
    return {
      title, ...closed,
      stateLabel: review.submitted
        ? `Codex received your answer: ${DECISION_LABEL[review.submitted]}.`
        : "Codex no longer waits for this request.",
      note: review.submitted === "accept" ? "This confirms the answer only, not what the operation did." : null,
    };
  }
  if (review.state === "submitted") {
    return {
      title, ...closed,
      stateLabel: `Your answer (${review.submitted ? DECISION_LABEL[review.submitted] : "sent"}) was given to Codex on ${options.hostName}. Waiting for Codex to confirm.`,
      note: null,
    };
  }
  if (nativeDecisionExpired(review, options.now)) {
    return { title, ...closed, stateLabel: "This request expired. Stop the task to interrupt it.", note: null };
  }
  if (options.delivery) {
    return {
      title, ...closed,
      stateLabel: options.delivery === "sending"
        ? `Sending your answer to ${options.hostName}…`
        : `Sent. Waiting for ${options.hostName} to give it to Codex.`,
      note: null,
    };
  }
  const note = review.canAccept ? null : nativeDecisionUnavailableCopy(review.unavailableReason);
  if (!options.remote) {
    return {
      title, ...closed,
      stateLabel: "Waiting. Pastey answers Codex's requests only on the device that started a task from another device.",
      note,
    };
  }
  return {
    title,
    canAccept: review.canAccept,
    canDecline: review.canDecline,
    stateLabel: review.canAccept || review.canDecline ? "Waiting for your decision." : "Waiting. Stop the task to interrupt it.",
    note,
  };
}

/** Whether the user who started this task has a native request to decide
 * right now, so the generic "cannot continue from here" note is not shown. */
export function nativeDecisionsAwaitUser(status: NativeAgentTaskStatus | null, remote: boolean, now: number): boolean {
  return remote && status?.state === "running" && (status.nativeDecisions ?? []).some((review) =>
    review.state === "pending" && !nativeDecisionExpired(review, now) && (review.canAccept || review.canDecline));
}

export function nativeFileChangeLabel(change: { change: string; path: string; moveTo?: string }): string {
  switch (change.change) {
    case "add": return `Add ${change.path}`;
    case "delete": return `Delete ${change.path}`;
    case "move": return `Move ${change.path} → ${change.moveTo ?? ""}`;
    default: return `Edit ${change.path}`;
  }
}

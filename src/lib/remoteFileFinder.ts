/** Find one file on another Bridge device and receive it in this device's
 * Inbox, through the existing Bridge Plan Search → Select → Transfer flow.
 * Renderer state only: Rust owns the immutable revision, the receiving
 * device owns where it searches, and nothing names a path. */
import type { ComposerBlock, SafeSearchScope } from "./bridgePlanComposer";
import { parseBridgePlanSearchCandidates, parseBridgePlanSearchTerminalResult, type BridgePlanSearchCandidate } from "./bridgePlanSearchResults";
import type { BridgePlanWorkspace } from "./tauri";
import type { ReceivedRoomControlEvent } from "./types";

/** Another device searches only folders its own user shared with this
 * Bridge; it refuses its Downloads, Desktop, Documents and other built-in
 * places, so they are not offered. */
export const FINDER_SCOPES: ReadonlyArray<{ value: SafeSearchScope; label: string; detail: string }> = [
  { value: "authorized_folders", label: "Shared search folders", detail: "Only folders that device's user shared with this Bridge in its Settings." },
];

export interface RemoteFindInput {
  filenameHint: string;
  extension: string;
  scopes: SafeSearchScope[];
}

/** Search on the selected device, then Transfer only the chosen file to
 * this device's Inbox. */
export function remoteFindBlocks(input: RemoteFindInput): { blocks: ComposerBlock[] | null; error: string | null } {
  const filenameHint = input.filenameHint.trim();
  if (!filenameHint || filenameHint.length > 128 || !/[a-z0-9]/i.test(filenameHint)) {
    return { blocks: null, error: "Enter part of the file name to look for." };
  }
  if (/[/\\]/.test(filenameHint)) {
    return { blocks: null, error: "Enter a file name, not a folder path. Pastey searches only the locations you choose below." };
  }
  const extension = input.extension.trim().replace(/^\./, "").toLowerCase();
  if (extension && !/^[a-z0-9]{1,16}$/.test(extension)) {
    return { blocks: null, error: "The file type must be a short extension such as mp4." };
  }
  const scopes = FINDER_SCOPES.map((scope) => scope.value).filter((scope) => input.scopes.includes(scope));
  if (scopes.length === 0) return { blocks: null, error: "Choose at least one place to search." };
  return {
    error: null,
    blocks: [
      { primitive: "Search", executionDevice: "selected_device", filenameHint, extension, safeScopes: scopes },
      { primitive: "Transfer", source: "selected_device", destination: "requesting_device", landingMode: "final_delivery" },
    ],
  };
}

export function remoteFindGoal(input: RemoteFindInput, deviceName: string): string {
  const extension = input.extension.trim().replace(/^\./, "").toLowerCase();
  return `Find “${input.filenameHint.trim()}”${extension ? ` (.${extension})` : ""} on ${deviceName} and send only the file I choose to this device's Inbox.`;
}

/** The newest reviewable revision the Host returned. */
export function availableRevisionId(workspace: BridgePlanWorkspace): string | null {
  const revisions = workspace.revisions.flatMap((value) => {
    if (!isRecord(value) || !isRecord(value.revision) || typeof value.revision.revision_id !== "string" || value.state !== "available") return [];
    return [value.revision.revision_id];
  });
  return revisions[revisions.length - 1] ?? null;
}

export type RemoteFindProgress =
  | { phase: "searching" }
  | { phase: "candidates"; candidates: BridgePlanSearchCandidate[] }
  | { phase: "no_matches" }
  | { phase: "search_failed"; message: string }
  | { phase: "sending" }
  | { phase: "delivered"; summary: string }
  | { phase: "transfer_failed"; message: string };

/** One attempt's progress from the Host's durable records and the events
 * the selected device sent back for exactly that attempt. */
export function remoteFindProgress(
  attemptId: string,
  workspace: BridgePlanWorkspace | null,
  events: ReceivedRoomControlEvent[],
  selectedCandidateId: string | null,
): RemoteFindProgress {
  const attempt = workspace?.attempts.find((value) =>
    isRecord(value) && isRecord(value.attempt) && value.attempt.attempt_id === attemptId);
  const attemptState = isRecord(attempt) && typeof attempt.state === "string" ? attempt.state : null;
  const searchFailure = events.flatMap((event) => {
    const payload = isRecord(event.event) && isRecord(event.event.payload) ? event.event.payload : null;
    return event.kind === "bridge_plan.step_failed" && payload?.attemptId === attemptId && payload?.stepId === "search"
      ? [typeof payload.failureCode === "string" ? payload.failureCode : "search_failed"]
      : [];
  })[0];
  if (searchFailure) return { phase: "search_failed", message: searchFailureMessage(searchFailure) };
  if (selectedCandidateId) {
    const result = workspace?.results.find((value) =>
      isRecord(value) && value.attempt_id === attemptId && value.step_id !== "search" && typeof value.summary === "string");
    if (isRecord(result) && attemptState === "completed") return { phase: "delivered", summary: String(result.summary) };
    if (attemptState && ["failed", "interrupted", "cancelled", "burned"].includes(attemptState)) {
      return { phase: "transfer_failed", message: "The file was not delivered. Nothing was changed on either device; search again to retry." };
    }
    return { phase: "sending" };
  }
  const candidates = events.flatMap(parseBridgePlanSearchCandidates).filter((candidate) => candidate.attemptId === attemptId);
  if (candidates.length > 0) return { phase: "candidates", candidates };
  const terminal = events.flatMap((event) => {
    const result = parseBridgePlanSearchTerminalResult(event);
    return result && result.attemptId === attemptId ? [result] : [];
  })[0];
  if (terminal && terminal.candidateCount === 0) return { phase: "no_matches" };
  if (attemptState && ["failed", "interrupted", "cancelled", "burned"].includes(attemptState)) {
    return { phase: "search_failed", message: "The search did not finish. Check that the device is connected and try again." };
  }
  return { phase: "searching" };
}

export function searchFailureMessage(code: string): string {
  switch (code) {
    case "no_searchable_scopes":
      return "That device has not shared any folder with this Bridge, so there is nowhere to search. On that device, its user must add the folder (for example a project's output folder) under Settings → Shared search folders and choose this Bridge.";
    case "search_timeout":
      return "The search took too long and stopped. Use a more specific file name.";
    default:
      return "The search could not run on that device.";
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

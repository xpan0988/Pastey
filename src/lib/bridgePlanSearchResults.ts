import { formatBytes, formatTimestamp } from "./format";
import type { ReceivedRoomControlEvent } from "./types";

export type BridgePlanSearchCandidate = {
  attemptId: string;
  candidateId: string;
  displayName: string;
  extension: string;
  mimeFamily: string;
  sizeBytes: number;
  redactedLocation: string | null;
  modifiedAt: string | null;
  matchReason: string;
  confidence: string | null;
};

export type BridgePlanSearchTerminalResult = {
  attemptId: string;
  summary: string;
  candidateCount: number;
};

export type BridgePlanSearchCandidateMode = "selectable" | "result";

export type BridgePlanSearchTerminalPresentation = {
  status: "completed";
  summary: string;
  isEmpty: boolean;
};

const MAX_CANDIDATE_TEXT_LENGTH = 256;

export function parseBridgePlanSearchCandidates(event: ReceivedRoomControlEvent): BridgePlanSearchCandidate[] {
  const result = parseBridgePlanSearchResult(event);
  if (!result) return [];
  const candidates = Array.isArray(result.safeResult.candidates) ? result.safeResult.candidates : [];
  return candidates.flatMap((candidate) => parseCandidate(result.attemptId, candidate));
}

export function parseBridgePlanSearchTerminalResult(event: ReceivedRoomControlEvent): BridgePlanSearchTerminalResult | null {
  const result = parseBridgePlanSearchResult(event);
  if (!result) return null;
  const candidates = Array.isArray(result.safeResult.candidates) ? result.safeResult.candidates : [];
  const summary = boundedString(result.safeResult.summary)
    ?? (candidates.length === 0
      ? "Search completed with no matching files."
      : `Search finished with ${candidates.length} matching file result(s).`);
  return {
    attemptId: result.attemptId,
    summary: candidates.length === 0 ? "Search completed with no matching files." : summary,
    candidateCount: candidates.length,
  };
}

export function bridgePlanSearchCandidateMode(primitives: readonly string[]): BridgePlanSearchCandidateMode {
  return primitives.includes("Transfer") || primitives.includes("Transform") ? "selectable" : "result";
}

export function terminalSearchPresentation(result: BridgePlanSearchTerminalResult): BridgePlanSearchTerminalPresentation {
  return {
    status: "completed",
    summary: result.summary,
    isEmpty: result.candidateCount === 0,
  };
}

export function selectedBridgePlanCandidateId(candidate: Pick<BridgePlanSearchCandidate, "candidateId">): string {
  return candidate.candidateId;
}

export function candidateFileType(candidate: Pick<BridgePlanSearchCandidate, "extension" | "mimeFamily">): string {
  if (candidate.extension) return `${candidate.extension.toUpperCase()} file`;
  return candidate.mimeFamily || "File";
}

export function candidateMetadata(candidate: BridgePlanSearchCandidate): {
  size: string;
  fileType: string;
  redactedLocation: string | null;
  modifiedAt: string | null;
  matchReason: string;
  confidence: string | null;
} {
  return {
    size: formatBytes(candidate.sizeBytes),
    fileType: candidateFileType(candidate),
    redactedLocation: candidate.redactedLocation,
    modifiedAt: candidate.modifiedAt ? formatTimestamp(Date.parse(candidate.modifiedAt) / 1000) : null,
    matchReason: candidate.matchReason,
    confidence: candidate.confidence,
  };
}

function parseBridgePlanSearchResult(event: ReceivedRoomControlEvent): { attemptId: string; safeResult: Record<string, unknown> } | null {
  if (event.kind !== "bridge_plan.step_result") return null;
  const payload = roomControlEventPayload(event.event);
  const attemptId = boundedString(payload?.attemptId, 128);
  const safeResult = isRecord(payload?.safeResult) ? payload.safeResult : null;
  return payload?.stepId === "search" && attemptId && safeResult ? { attemptId, safeResult } : null;
}

function parseCandidate(attemptId: string, value: unknown): BridgePlanSearchCandidate[] {
  if (!isRecord(value)) return [];
  const candidateId = boundedString(value.candidateId, 128);
  const displayName = boundedString(value.displayName);
  const extension = boundedString(value.extension);
  const mimeFamily = boundedString(value.mimeFamily);
  const matchReason = boundedString(value.matchReason);
  const sizeBytes = typeof value.sizeBytes === "number" ? value.sizeBytes : null;
  if (!candidateId || !displayName || !extension || !mimeFamily || !matchReason
    || sizeBytes === null || !Number.isSafeInteger(sizeBytes) || sizeBytes < 0
    || hasPathSyntax(candidateId) || hasPathSyntax(displayName)) return [];
  return [{
    attemptId,
    candidateId,
    displayName,
    extension,
    mimeFamily,
    sizeBytes,
    redactedLocation: safeRedactedLocation(value.redactedLocation),
    modifiedAt: safeTimestamp(value.modifiedAt),
    matchReason,
    confidence: safeConfidence(value.confidence),
  }];
}

function safeRedactedLocation(value: unknown): string | null {
  const location = boundedString(value);
  return location && !hasAbsolutePathSyntax(location) ? location : null;
}

function safeTimestamp(value: unknown): string | null {
  const timestamp = boundedString(value);
  return timestamp && Number.isFinite(Date.parse(timestamp)) ? timestamp : null;
}

function safeConfidence(value: unknown): string | null {
  const confidence = boundedString(value, 32);
  return confidence && /^[a-z][a-z_-]*$/i.test(confidence) ? confidence : null;
}

function boundedString(value: unknown, maximum = MAX_CANDIDATE_TEXT_LENGTH): string | null {
  return typeof value === "string" && value.length > 0 && value.length <= maximum && !/[\u0000-\u001f\u007f]/.test(value)
    ? value
    : null;
}

function hasPathSyntax(value: string): boolean {
  return value.includes("/") || value.includes("\\");
}

function hasAbsolutePathSyntax(value: string): boolean {
  return value.startsWith("/") || value.includes("\\") || /^[a-z]:\//i.test(value);
}

function roomControlEventPayload(event: unknown): Record<string, unknown> | null {
  return isRecord(event) && isRecord(event.payload) ? event.payload : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

import { logFrontendDiagnostic } from "../tauri";
import { getAgentBridgeRuntimeConfig, type AgentBridgeLogLevel } from "./config";

export interface AgentBridgeLogEvent {
  timestamp?: string;
  category?: "agent_bridge";
  roomRefShort?: string;
  sessionRefShort?: string;
  peerRefShort?: string;
  eventKind: AgentBridgeLogEventKind;
  stateFrom?: string;
  stateTo?: string;
  eventIdShort?: string;
  requestIdShort?: string;
  executionIdShort?: string;
  policyResult?: string;
  consentResult?: string;
  transportResult?: string;
  runtimeDataWindowTarget?: 7 | 8;
  errorCode?: string;
  executionResult?: "hello_peer_template_succeeded" | "hello_stdout_succeeded";
}

export type AgentBridgeLogEventKind =
  | "advisory_generated"
  | "policy_accepted"
  | "policy_rejected"
  | "local_confirmation_requested"
  | "local_confirmation_confirmed"
  | "preview_built"
  | "preview_queued"
  | "control_demand_started"
  | "runtime_window_target_7"
  | "transport_sending"
  | "transport_delivered"
  | "transport_rejected"
  | "peer_review_started"
  | "peer_allowed_once"
  | "peer_denied"
  | "consent_expired"
  | "execution_request_queued"
  | "execution_request_sent"
  | "consent_consumed"
  | "hello_peer_execution_started"
  | "hello_peer_execution_succeeded"
  | "hello_peer_execution_rejected"
  | "execution_result_delivered"
  | "runtime_window_target_8"
  | "session_cleared";

const MAX_FIELD_LENGTH = 64;
const EVENT_KINDS = new Set<AgentBridgeLogEventKind>([
  "advisory_generated",
  "policy_accepted",
  "policy_rejected",
  "local_confirmation_requested",
  "local_confirmation_confirmed",
  "preview_built",
  "preview_queued",
  "control_demand_started",
  "runtime_window_target_7",
  "transport_sending",
  "transport_delivered",
  "transport_rejected",
  "peer_review_started",
  "peer_allowed_once",
  "peer_denied",
  "consent_expired",
  "execution_request_queued",
  "execution_request_sent",
  "consent_consumed",
  "hello_peer_execution_started",
  "hello_peer_execution_succeeded",
  "hello_peer_execution_rejected",
  "execution_result_delivered",
  "runtime_window_target_8",
  "session_cleared",
]);
const ERROR_EVENTS = new Set<AgentBridgeLogEventKind>([
  "policy_rejected",
  "transport_rejected",
  "hello_peer_execution_rejected",
  "consent_expired",
]);
const VERBOSE_EVENTS = new Set<AgentBridgeLogEventKind>([
  "control_demand_started",
  "runtime_window_target_7",
  "runtime_window_target_8",
]);
const ERROR_CODES = new Set([
  "replay",
  "duplicate",
  "expired",
  "invalid_event",
  "room_mismatch",
  "source_mismatch",
  "target_mismatch",
  "session_mismatch",
  "session_unavailable",
  "peer_unavailable",
  "rate_limited",
  "inbox_full",
  "already_consumed",
  "consent_missing",
  "missing_consent",
  "consent_mismatch",
  "consent_expired",
  "invalid_consent",
  "consent_not_allowed_once",
  "consent_binding_mismatch",
  "invalid_transition",
  "malformed_request",
  "execution_timeout",
  "invalid_bounded_output",
  "runtime_unavailable",
  "nonzero_exit",
  "stdout_mismatch",
  "output_truncated",
  "policy_rejected",
  "oversized",
  "malformed_receipt",
  "transport_error",
  "unknown",
  "cloud_context_not_allowed",
  "provider_http_error",
  "provider_response_invalid",
  "provider_request_failed",
  "provider_json_parse_failed",
]);

export function shortAgentBridgeRef(value: string | undefined): string | undefined {
  if (!value) return undefined;
  return value.length <= 16 ? value : `${value.slice(0, 7)}..${value.slice(-7)}`;
}

export function buildAgentBridgeLogLine(
  event: AgentBridgeLogEvent,
  level: AgentBridgeLogLevel = getAgentBridgeRuntimeConfig().logLevel,
): string | null {
  if (!EVENT_KINDS.has(event.eventKind)) return null;
  if (!shouldLog(event.eventKind, level)) return null;
  const value: AgentBridgeLogEvent = {
    timestamp: safeTimestamp(event.timestamp),
    category: "agent_bridge",
    eventKind: event.eventKind,
    ...(event.roomRefShort ? { roomRefShort: shortAgentBridgeRef(event.roomRefShort) } : {}),
    ...(event.sessionRefShort ? { sessionRefShort: shortAgentBridgeRef(event.sessionRefShort) } : {}),
    ...(event.peerRefShort ? { peerRefShort: shortAgentBridgeRef(event.peerRefShort) } : {}),
    ...boundedOptional(event),
  };
  return `[pastey:agent-bridge] ${JSON.stringify(value)}`;
}

export function logAgentBridgeLifecycle(event: AgentBridgeLogEvent): void {
  const line = buildAgentBridgeLogLine(event);
  if (!line) return;
  void logFrontendDiagnostic(line).catch(() => {
    // Logging is an audit mirror only and never changes workflow behavior.
  });
}

function shouldLog(eventKind: AgentBridgeLogEventKind, level: AgentBridgeLogLevel): boolean {
  if (level === "off") return false;
  if (level === "errors") return ERROR_EVENTS.has(eventKind);
  if (level === "standard") return !VERBOSE_EVENTS.has(eventKind);
  return true;
}

function boundedOptional(event: AgentBridgeLogEvent): Partial<AgentBridgeLogEvent> {
  const result: Partial<AgentBridgeLogEvent> = {};
  for (const key of [
    "stateFrom",
    "stateTo",
    "eventIdShort",
    "requestIdShort",
    "executionIdShort",
    "policyResult",
    "consentResult",
    "transportResult",
    "errorCode",
    "executionResult",
  ] as const) {
    const value = event[key];
    if (typeof value === "string") {
      (result as Record<string, unknown>)[key] = key.endsWith("IdShort")
        ? shortAgentBridgeRef(value)
        : key === "errorCode"
          ? safeErrorCode(value)
          : bounded(value);
    }
  }
  if (event.runtimeDataWindowTarget === 7 || event.runtimeDataWindowTarget === 8) {
    result.runtimeDataWindowTarget = event.runtimeDataWindowTarget;
  }
  return result;
}

function safeErrorCode(value: string): string {
  return ERROR_CODES.has(value) ? value : "invalid_event";
}

function safeTimestamp(value: string | undefined): string {
  return value && value.length <= 32 && Number.isFinite(Date.parse(value))
    ? value
    : new Date().toISOString();
}

function bounded(value: string): string {
  return value.replace(/[\r\n]/g, " ").slice(0, MAX_FIELD_LENGTH);
}

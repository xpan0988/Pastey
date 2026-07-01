import type { AiActionKind } from "./types";

export type AgentBridgeCapabilityId =
  | "runtime.execute_hello_template"
  | "runtime.hello_stdout"
  | "filesystem.find_file_candidates"
  | "transfer.request_candidate_payload";

export type AgentBridgeCapabilityVersion = "legacy" | "v1";
export type AgentBridgeRoutePolicy = "selected-peer";
export type AgentBridgeConsentPolicy = "exact-allow-once";
export type AgentBridgeExecutorKind =
  | "ts_in_process_fixed_template"
  | "rust_host_helper"
  | "filesystem_find_candidates_host"
  | "transfer_candidate_payload_host";
export type AgentBridgeAuditRedactionPolicy = "metadata_only";
export type AgentBridgeCapabilityLifecycle = "implemented";
export type AgentBridgeProviderInputShape = "fixed_message" | "file_candidate_advisory" | "candidate_payload_request";

export interface AgentBridgeCapabilityContract {
  readonly capability: AgentBridgeCapabilityId;
  readonly version: AgentBridgeCapabilityVersion;
  readonly lifecycle: AgentBridgeCapabilityLifecycle;
  readonly providerActionKind: Extract<AiActionKind, "request_peer_hello_demo" | "request_peer_hello_stdout_demo" | "request_peer_file_candidates" | "request_peer_candidate_payload">;
  readonly providerInputShape: AgentBridgeProviderInputShape;
  readonly previewRequestSchema?: string;
  readonly consentGrantSchema?: string;
  readonly executionRequestSchema?: string;
  readonly executionResultSchema?: string;
  readonly routePolicy: AgentBridgeRoutePolicy;
  readonly consentPolicy: AgentBridgeConsentPolicy;
  readonly executorKind: AgentBridgeExecutorKind;
  readonly auditRedactionPolicy: AgentBridgeAuditRedactionPolicy;
  readonly requiresPeerCapabilityAdvertisement: boolean;
  readonly providerInputField?: "message";
  readonly providerInputValue?: string;
  readonly typedBindingField?: "exactMessage" | "expectedStdout";
  readonly typedBindingValue?: string;
  readonly constraintFields: readonly string[];
  readonly allowTempOnlyFilesystem: boolean;
  readonly forbiddenProviderFields: readonly string[];
  readonly resultOnlyFields: readonly string[];
  readonly ui: {
    readonly label: string;
    readonly shortLabel: string;
    readonly resultLabel: string;
  };
}

export interface AgentBridgeCapabilityEnvelope<TPayload = unknown> {
  readonly schemaVersion: "pastey-agent-bridge-capability-envelope-v1";
  readonly capability: AgentBridgeCapabilityId;
  readonly capabilityVersion: AgentBridgeCapabilityVersion;
  readonly requestId: string;
  readonly roomRef: string;
  readonly sourceDeviceRef: string;
  readonly targetPeerRef: string;
  readonly routePolicy: AgentBridgeRoutePolicy;
  readonly consentPolicy: AgentBridgeConsentPolicy;
  readonly createdAt: string;
  readonly expiresAt: string;
  readonly payloadHash: string;
  readonly typedPayload: TPayload;
  readonly transport: {
    readonly kind: "room-control";
    readonly route: AgentBridgeRoutePolicy;
    readonly previewOnly: boolean;
    readonly maxPayloadBytes: number;
  };
}

export const HELLO_TEMPLATE_CAPABILITY = "runtime.execute_hello_template" as const;
export const HELLO_STDOUT_CAPABILITY = "runtime.hello_stdout" as const;
export const FILE_CANDIDATES_CAPABILITY = "filesystem.find_file_candidates" as const;
export const CANDIDATE_PAYLOAD_CAPABILITY = "transfer.request_candidate_payload" as const;
export const HELLO_TEMPLATE_MESSAGE = "hello peer!";
export const HELLO_STDOUT_EXPECTED_STDOUT = "hello peer";
export const HELLO_STDOUT_RUNTIME_KIND = "rust_host_helper";
export const FILE_CANDIDATES_EXECUTOR_KIND = "filesystem_find_candidates_host" as const;
export const CANDIDATE_PAYLOAD_EXECUTOR_KIND = "transfer_candidate_payload_host" as const;
export const SHARED_CAPABILITY_ENVELOPE_SCHEMA = "pastey-agent-bridge-capability-envelope-v1";
export const SELECTED_PEER_ROUTE_POLICY: AgentBridgeRoutePolicy = "selected-peer";
export const EXACT_ALLOW_ONCE_CONSENT_POLICY: AgentBridgeConsentPolicy = "exact-allow-once";

const BASE_FORBIDDEN_PROVIDER_FIELDS = [
  "command",
  "cmd",
  "shell",
  "script",
  "code",
  "path",
  "absolutePath",
  "filePath",
  "cwd",
  "env",
  "environment",
  "args",
  "arguments",
  "networkTarget",
  "url",
  "filesystemTree",
    "rawLogs",
    "contents",
    "fileContents",
    "secret",
  "token",
  "apiKey",
  "roomKey",
  "roomCode",
  "transportKey",
  "hiddenTransfer",
  "peerFilesystemSearch",
  "selectedPeers",
  "targetPeerRefs",
  "broadcast",
  "broadcastBridge",
  "durableTrust",
  "trustedExecutor",
    "autoTransfer",
    "automaticTransfer",
    "autoSend",
    "sendFile",
    "transferQueueId",
    "transferQueueItemId",
    "handoffId",
    "executed",
  "execution",
  "completed",
  "succeeded",
  "process",
  "spawn",
] as const;

const RESULT_ONLY_FIELDS = ["stdout", "stderr", "exitCode", "durationMs", "timedOut"] as const;

export const AGENT_BRIDGE_CAPABILITY_REGISTRY: readonly AgentBridgeCapabilityContract[] = Object.freeze([
  Object.freeze({
    capability: HELLO_TEMPLATE_CAPABILITY,
    version: "legacy",
    lifecycle: "implemented",
    providerActionKind: "request_peer_hello_demo",
    providerInputShape: "fixed_message",
    previewRequestSchema: "pastey-hello-peer-request-v1",
    consentGrantSchema: "pastey-hello-peer-consent-grant-v1",
    executionRequestSchema: "pastey-hello-peer-execution-request-v1",
    executionResultSchema: "pastey-hello-peer-execution-result-v1",
    routePolicy: SELECTED_PEER_ROUTE_POLICY,
    consentPolicy: EXACT_ALLOW_ONCE_CONSENT_POLICY,
    executorKind: "ts_in_process_fixed_template",
    auditRedactionPolicy: "metadata_only",
    requiresPeerCapabilityAdvertisement: true,
    providerInputField: "message",
    providerInputValue: HELLO_TEMPLATE_MESSAGE,
    typedBindingField: "exactMessage",
    typedBindingValue: HELLO_TEMPLATE_MESSAGE,
    constraintFields: ["templateOnly", "noRawShell", "filesystem", "network", "timeoutMs", "maxStdoutBytes"],
    allowTempOnlyFilesystem: true,
    forbiddenProviderFields: BASE_FORBIDDEN_PROVIDER_FIELDS,
    resultOnlyFields: RESULT_ONLY_FIELDS,
    ui: {
      label: "Hello Peer",
      shortLabel: "Hello Peer",
      resultLabel: "Hello Peer result",
    },
  }),
  Object.freeze({
    capability: HELLO_STDOUT_CAPABILITY,
    version: "v1",
    lifecycle: "implemented",
    providerActionKind: "request_peer_hello_stdout_demo",
    providerInputShape: "fixed_message",
    previewRequestSchema: "pastey-runtime-hello-stdout-request-v1",
    consentGrantSchema: "pastey-runtime-hello-stdout-consent-grant-v1",
    executionRequestSchema: "pastey-runtime-hello-stdout-execution-request-v1",
    executionResultSchema: "pastey-runtime-hello-stdout-execution-result-v1",
    routePolicy: SELECTED_PEER_ROUTE_POLICY,
    consentPolicy: EXACT_ALLOW_ONCE_CONSENT_POLICY,
    executorKind: "rust_host_helper",
    auditRedactionPolicy: "metadata_only",
    requiresPeerCapabilityAdvertisement: true,
    providerInputField: "message",
    providerInputValue: HELLO_STDOUT_EXPECTED_STDOUT,
    typedBindingField: "expectedStdout",
    typedBindingValue: HELLO_STDOUT_EXPECTED_STDOUT,
    constraintFields: [
      "templateOnly",
      "noRawShell",
      "filesystem",
      "network",
      "timeoutMs",
      "maxStdoutBytes",
      "maxStderrBytes",
    ],
    allowTempOnlyFilesystem: false,
    forbiddenProviderFields: BASE_FORBIDDEN_PROVIDER_FIELDS,
    resultOnlyFields: RESULT_ONLY_FIELDS,
    ui: {
      label: "Hello Stdout",
      shortLabel: "Hello Stdout",
      resultLabel: "Hello Stdout result",
    },
  }),
  Object.freeze({
    capability: FILE_CANDIDATES_CAPABILITY,
    version: "v1",
    lifecycle: "implemented",
    providerActionKind: "request_peer_file_candidates",
    providerInputShape: "file_candidate_advisory",
    previewRequestSchema: "filesystem-find-file-candidates-request-v1",
    consentGrantSchema: "filesystem-find-file-candidates-consent-grant-v1",
    executionRequestSchema: "filesystem-find-file-candidates-execution-request-v1",
    executionResultSchema: "filesystem-find-file-candidates-result-v1",
    routePolicy: SELECTED_PEER_ROUTE_POLICY,
    consentPolicy: EXACT_ALLOW_ONCE_CONSENT_POLICY,
    executorKind: FILE_CANDIDATES_EXECUTOR_KIND,
    auditRedactionPolicy: "metadata_only",
    requiresPeerCapabilityAdvertisement: true,
    constraintFields: [],
    allowTempOnlyFilesystem: false,
    forbiddenProviderFields: BASE_FORBIDDEN_PROVIDER_FIELDS,
    resultOnlyFields: RESULT_ONLY_FIELDS,
    ui: {
      label: "Find File Candidates",
      shortLabel: "File Candidates",
      resultLabel: "File candidate result",
    },
  }),
  Object.freeze({
    capability: CANDIDATE_PAYLOAD_CAPABILITY,
    version: "v1",
    lifecycle: "implemented",
    providerActionKind: "request_peer_candidate_payload",
    providerInputShape: "candidate_payload_request",
    previewRequestSchema: "transfer-request-candidate-payload-request-v1",
    consentGrantSchema: "transfer-request-candidate-payload-consent-grant-v1",
    executionRequestSchema: "transfer-request-candidate-payload-execution-request-v1",
    executionResultSchema: "transfer-request-candidate-payload-result-v1",
    routePolicy: SELECTED_PEER_ROUTE_POLICY,
    consentPolicy: EXACT_ALLOW_ONCE_CONSENT_POLICY,
    executorKind: CANDIDATE_PAYLOAD_EXECUTOR_KIND,
    auditRedactionPolicy: "metadata_only",
    requiresPeerCapabilityAdvertisement: true,
    constraintFields: [],
    allowTempOnlyFilesystem: false,
    forbiddenProviderFields: BASE_FORBIDDEN_PROVIDER_FIELDS,
    resultOnlyFields: RESULT_ONLY_FIELDS,
    ui: {
      label: "Request Candidate Payload",
      shortLabel: "Candidate Payload",
      resultLabel: "Candidate payload request result",
    },
  }),
]);

export const AGENT_BRIDGE_CAPABILITY_ACTION_KINDS = AGENT_BRIDGE_CAPABILITY_REGISTRY
  .map((contract) => contract.providerActionKind);

export function listAgentBridgeCapabilityContracts(): readonly AgentBridgeCapabilityContract[] {
  return AGENT_BRIDGE_CAPABILITY_REGISTRY;
}

export function getAgentBridgeCapabilityContract(
  capability: unknown,
): AgentBridgeCapabilityContract | undefined {
  return typeof capability === "string"
    ? AGENT_BRIDGE_CAPABILITY_REGISTRY.find((contract) => contract.capability === capability)
    : undefined;
}

export function getAgentBridgeCapabilityContractByVersion(
  capability: unknown,
  version: unknown,
): AgentBridgeCapabilityContract | undefined {
  return typeof capability === "string" && typeof version === "string"
    ? AGENT_BRIDGE_CAPABILITY_REGISTRY.find((contract) =>
        contract.capability === capability && contract.version === version
      )
    : undefined;
}

export function getAgentBridgeCapabilityContractByActionKind(
  kind: unknown,
): AgentBridgeCapabilityContract | undefined {
  return typeof kind === "string"
    ? AGENT_BRIDGE_CAPABILITY_REGISTRY.find((contract) => contract.providerActionKind === kind)
    : undefined;
}

export function getAgentBridgeCapabilityContractByPreviewSchema(
  schemaVersion: unknown,
): AgentBridgeCapabilityContract | undefined {
  return getContractBySchema("previewRequestSchema", schemaVersion);
}

export function getAgentBridgeCapabilityContractByExecutionRequestSchema(
  schemaVersion: unknown,
): AgentBridgeCapabilityContract | undefined {
  return getContractBySchema("executionRequestSchema", schemaVersion);
}

export function getAgentBridgeCapabilityContractByExecutionResultSchema(
  schemaVersion: unknown,
): AgentBridgeCapabilityContract | undefined {
  return getContractBySchema("executionResultSchema", schemaVersion);
}

export function getAgentBridgeCapabilityContractByConsentGrantSchema(
  schemaVersion: unknown,
): AgentBridgeCapabilityContract | undefined {
  return getContractBySchema("consentGrantSchema", schemaVersion);
}

export function findForbiddenProviderFieldPaths(
  value: unknown,
  contract?: AgentBridgeCapabilityContract,
): string[] {
  const forbidden = new Set([
    ...(contract?.forbiddenProviderFields ?? BASE_FORBIDDEN_PROVIDER_FIELDS),
    ...(contract?.resultOnlyFields ?? RESULT_ONLY_FIELDS),
  ].map(normalizeCapabilityFieldName));
  return findFieldPaths(value, forbidden);
}

export function findCapabilityFieldPaths(value: unknown, fieldNames: readonly string[]): string[] {
  return findFieldPaths(value, new Set(fieldNames.map(normalizeCapabilityFieldName)));
}

export function normalizeCapabilityFieldName(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, "").toLowerCase();
}

function getContractBySchema(
  key: "previewRequestSchema" | "consentGrantSchema" | "executionRequestSchema" | "executionResultSchema",
  schemaVersion: unknown,
): AgentBridgeCapabilityContract | undefined {
  return typeof schemaVersion === "string"
    ? AGENT_BRIDGE_CAPABILITY_REGISTRY.find((contract) => contract[key] !== undefined && contract[key] === schemaVersion)
    : undefined;
}

function findFieldPaths(value: unknown, forbidden: Set<string>): string[] {
  const found: string[] = [];
  visitValue(value, "$", forbidden, found);
  return found;
}

function visitValue(value: unknown, path: string, forbidden: Set<string>, found: string[]) {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => visitValue(entry, `${path}[${index}]`, forbidden, found));
    return;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return;
  }
  for (const [key, entry] of Object.entries(value as Record<string, unknown>)) {
    const entryPath = `${path}.${key}`;
    if (forbidden.has(normalizeCapabilityFieldName(key))) {
      found.push(entryPath);
    }
    visitValue(entry, entryPath, forbidden, found);
  }
}

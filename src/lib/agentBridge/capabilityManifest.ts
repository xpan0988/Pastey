import {
  CANDIDATE_PAYLOAD_CAPABILITY,
  CANDIDATE_PAYLOAD_EXECUTOR_KIND,
  EXACT_ALLOW_ONCE_CONSENT_POLICY,
  FILE_CANDIDATES_CAPABILITY,
  FILE_CANDIDATES_EXECUTOR_KIND,
  HELLO_STDOUT_CAPABILITY,
  HELLO_STDOUT_RUNTIME_KIND,
  HELLO_TEMPLATE_CAPABILITY,
  SELECTED_PEER_ROUTE_POLICY,
  type AgentBridgeAuditRedactionPolicy,
  type AgentBridgeCapabilityId,
  type AgentBridgeCapabilityVersion,
  type AgentBridgeConsentPolicy,
  type AgentBridgeExecutorKind,
  type AgentBridgeRoutePolicy,
} from "../ai/capabilityRegistry";
import type { AiActionKind } from "../ai/types";

export type CapabilityTemplateKind =
  | "bounded_runtime_action"
  | "metadata_discovery"
  | "candidate_payload_handoff"
  | "future_receiver_local_operation";

export type AgentBridgeAutonomyProfile =
  | "manual"
  | "assisted"
  | "trusted_session";

export type AgentBridgeApprovalPolicy =
  | "always_ask"
  | "ask_on_sensitive"
  | "session_bound"
  | "never_auto_approve";

export type AgentBridgeApprovalReviewer =
  | "user"
  | "policy_gate"
  | "auto_review";

export type AgentBridgeDataExposurePolicy =
  | "metadata_only"
  | "local_only_source"
  | "payload_queue_internal";

export type AgentBridgeManifestAuditRedactionPolicy =
  | AgentBridgeAuditRedactionPolicy
  | "local_only"
  | "queue_internal";

export interface CapabilityManifest {
  readonly capability: AgentBridgeCapabilityId;
  readonly version: AgentBridgeCapabilityVersion;
  readonly templateKind: CapabilityTemplateKind;
  readonly providerActionKind: Extract<AiActionKind,
    | "request_peer_hello_demo"
    | "request_peer_hello_stdout_demo"
    | "request_peer_file_candidates"
    | "request_peer_candidate_payload">;
  readonly executorKind: AgentBridgeExecutorKind;
  readonly routePolicy: AgentBridgeRoutePolicy | "local-only";
  readonly consentPolicy: AgentBridgeConsentPolicy | "none" | "session-bound-policy";
  readonly dataExposurePolicy: AgentBridgeDataExposurePolicy;
  readonly auditRedactionPolicy: AgentBridgeManifestAuditRedactionPolicy;
  readonly schemaVersions: {
    readonly advisory?: string;
    readonly request: string;
    readonly consentGrant?: string;
    readonly executionRequest?: string;
    readonly result: string;
  };
  readonly autonomySupport: {
    readonly manual: boolean;
    readonly assisted: boolean;
    readonly trustedSession: boolean;
  };
  readonly approvalRequirements: {
    readonly localUserConfirm: boolean;
    readonly receiverAllowOnce: boolean;
    readonly allowSessionPolicy: boolean;
    readonly allowAutoReview: boolean;
  };
  readonly safety: {
    readonly selectedPeerOnly: boolean;
    readonly rejectsBroadcast: boolean;
    readonly rejectsSelectedPeers: boolean;
    readonly forbidsAbsolutePathExposure: boolean;
    readonly forbidsContentExposure: boolean;
    readonly forbidsGenericExecution: boolean;
  };
}

const DEFAULT_AUTONOMY_SUPPORT = Object.freeze({
  manual: true,
  assisted: true,
  trustedSession: false,
});

const DEFAULT_APPROVAL_REQUIREMENTS = Object.freeze({
  localUserConfirm: true,
  receiverAllowOnce: true,
  allowSessionPolicy: false,
  allowAutoReview: false,
});

const DEFAULT_SELECTED_PEER_SAFETY = Object.freeze({
  selectedPeerOnly: true,
  rejectsBroadcast: true,
  rejectsSelectedPeers: true,
  forbidsAbsolutePathExposure: true,
  forbidsContentExposure: true,
  forbidsGenericExecution: true,
});

export const AGENT_BRIDGE_CAPABILITY_MANIFESTS: readonly CapabilityManifest[] = Object.freeze([
  Object.freeze({
    capability: HELLO_TEMPLATE_CAPABILITY,
    version: "legacy",
    templateKind: "bounded_runtime_action",
    providerActionKind: "request_peer_hello_demo",
    executorKind: "ts_in_process_fixed_template",
    routePolicy: SELECTED_PEER_ROUTE_POLICY,
    consentPolicy: EXACT_ALLOW_ONCE_CONSENT_POLICY,
    dataExposurePolicy: "metadata_only",
    auditRedactionPolicy: "metadata_only",
    schemaVersions: {
      advisory: "ai-action-plan-v1",
      request: "pastey-hello-peer-request-v1",
      consentGrant: "pastey-hello-peer-consent-grant-v1",
      executionRequest: "pastey-hello-peer-execution-request-v1",
      result: "pastey-hello-peer-execution-result-v1",
    },
    autonomySupport: DEFAULT_AUTONOMY_SUPPORT,
    approvalRequirements: DEFAULT_APPROVAL_REQUIREMENTS,
    safety: DEFAULT_SELECTED_PEER_SAFETY,
  }),
  Object.freeze({
    capability: HELLO_STDOUT_CAPABILITY,
    version: "v1",
    templateKind: "bounded_runtime_action",
    providerActionKind: "request_peer_hello_stdout_demo",
    executorKind: HELLO_STDOUT_RUNTIME_KIND,
    routePolicy: SELECTED_PEER_ROUTE_POLICY,
    consentPolicy: EXACT_ALLOW_ONCE_CONSENT_POLICY,
    dataExposurePolicy: "metadata_only",
    auditRedactionPolicy: "metadata_only",
    schemaVersions: {
      advisory: "ai-action-plan-v1",
      request: "pastey-runtime-hello-stdout-request-v1",
      consentGrant: "pastey-runtime-hello-stdout-consent-grant-v1",
      executionRequest: "pastey-runtime-hello-stdout-execution-request-v1",
      result: "pastey-runtime-hello-stdout-execution-result-v1",
    },
    autonomySupport: DEFAULT_AUTONOMY_SUPPORT,
    approvalRequirements: DEFAULT_APPROVAL_REQUIREMENTS,
    safety: DEFAULT_SELECTED_PEER_SAFETY,
  }),
  Object.freeze({
    capability: FILE_CANDIDATES_CAPABILITY,
    version: "v1",
    templateKind: "metadata_discovery",
    providerActionKind: "request_peer_file_candidates",
    executorKind: FILE_CANDIDATES_EXECUTOR_KIND,
    routePolicy: SELECTED_PEER_ROUTE_POLICY,
    consentPolicy: EXACT_ALLOW_ONCE_CONSENT_POLICY,
    dataExposurePolicy: "metadata_only",
    auditRedactionPolicy: "metadata_only",
    schemaVersions: {
      advisory: "ai-action-plan-v1",
      request: "filesystem-find-file-candidates-request-v1",
      consentGrant: "filesystem-find-file-candidates-consent-grant-v1",
      executionRequest: "filesystem-find-file-candidates-execution-request-v1",
      result: "filesystem-find-file-candidates-result-v1",
    },
    autonomySupport: DEFAULT_AUTONOMY_SUPPORT,
    approvalRequirements: DEFAULT_APPROVAL_REQUIREMENTS,
    safety: DEFAULT_SELECTED_PEER_SAFETY,
  }),
  Object.freeze({
    capability: CANDIDATE_PAYLOAD_CAPABILITY,
    version: "v1",
    templateKind: "candidate_payload_handoff",
    providerActionKind: "request_peer_candidate_payload",
    executorKind: CANDIDATE_PAYLOAD_EXECUTOR_KIND,
    routePolicy: SELECTED_PEER_ROUTE_POLICY,
    consentPolicy: EXACT_ALLOW_ONCE_CONSENT_POLICY,
    dataExposurePolicy: "payload_queue_internal",
    auditRedactionPolicy: "metadata_only",
    schemaVersions: {
      advisory: "ai-action-plan-v1",
      request: "transfer-request-candidate-payload-request-v1",
      consentGrant: "transfer-request-candidate-payload-consent-grant-v1",
      executionRequest: "transfer-request-candidate-payload-execution-request-v1",
      result: "transfer-request-candidate-payload-result-v1",
    },
    autonomySupport: DEFAULT_AUTONOMY_SUPPORT,
    approvalRequirements: DEFAULT_APPROVAL_REQUIREMENTS,
    safety: DEFAULT_SELECTED_PEER_SAFETY,
  }),
]);

export const HELLO_STDOUT_CAPABILITY_MANIFEST = requireCapabilityManifest(HELLO_STDOUT_CAPABILITY);

export function listCapabilityManifests(): readonly CapabilityManifest[] {
  return AGENT_BRIDGE_CAPABILITY_MANIFESTS;
}

export function getCapabilityManifest(capability: string): CapabilityManifest | null {
  return AGENT_BRIDGE_CAPABILITY_MANIFESTS.find((manifest) => manifest.capability === capability) ?? null;
}

export function requireCapabilityManifest(capability: string): CapabilityManifest {
  const manifest = getCapabilityManifest(capability);
  if (!manifest) {
    throw new Error(`[pastey:capability-manifest code=unknown_capability] Unknown capability manifest: ${capability}.`);
  }
  return manifest;
}

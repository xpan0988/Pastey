import { useEffect, useState } from "react";
import {
  CloudOpenAICompatibleProvider,
  CLOUD_STRICT_AI_CONTEXT_POLICY,
  MOCK_AI_CONTEXT_POLICY,
  acknowledgeCapabilityPreview,
  buildCapabilityRequestPreviewEnvelope,
  buildHelloPeerRequestFromPendingAction,
  buildHelloStdoutRequestFromPendingAction,
  buildMockAiContextSnapshot,
  cancelPendingAiAction,
  checkAndRecordCapabilityPreview,
  confirmPendingAiAction,
  createCapabilityPreviewSessionState,
  createPendingAiAction,
  denyCapabilityPreview,
  evaluateAiPolicy,
  expirePendingAiAction,
  markCapabilityPreviewReceived,
  mockProvider,
  validateAiActionPlan,
  validateCapabilityRequestPreviewEnvelope,
  validateHelloPeerRequest,
  validateHelloStdoutRequest,
  type AiActionPlan,
  type AiGenerateResult,
  type AiPolicyResult,
  type CapabilityPreviewSessionState,
  type CapabilityRequestPreviewEnvelope,
  type CapabilityRequest,
  type PendingAiAction
} from "../lib/ai";
import { AgentBridgeOverview } from "./agentBridge/AgentBridgeOverview";
import { AgentBridgeAdvancedDiagnostics } from "./agentBridge/AgentBridgeAdvancedDiagnostics";
import { RoomControlPanel } from "./agentBridge/RoomControlPanel";
import {
  logAgentBridgeLifecycle,
  useAgentBridgeRuntimeConfig,
} from "../lib/agentBridge";
import type { RoomInfo } from "../lib/types";

interface AiSlotPreviewResult {
  provider: string;
  model: string;
  plan?: AiActionPlan;
  validationStatus: "accepted" | "rejected";
  validationErrors: string[];
  policy?: AiPolicyResult;
  providerError?: string;
}

interface HelloPeerOutboundPreviewState {
  request?: CapabilityRequest;
  validationStatus: "accepted" | "rejected";
  errors: string[];
}

interface CapabilityEnvelopePreviewState {
  envelope?: CapabilityRequestPreviewEnvelope;
  validationStatus: "accepted" | "rejected";
  errors: string[];
}

export function AiSlotPreview({ room }: { room: RoomInfo }) {
  const [result, setResult] = useState<AiSlotPreviewResult | null>(null);
  const [pendingAction, setPendingAction] = useState<PendingAiAction | null>(null);
  const [outboundPreview, setOutboundPreview] = useState<HelloPeerOutboundPreviewState | null>(null);
  const [envelopePreview, setEnvelopePreview] = useState<CapabilityEnvelopePreviewState | null>(null);
  const [inboundPreview, setInboundPreview] = useState<CapabilityEnvelopePreviewState | null>(null);
  const [previewSession, setPreviewSession] = useState<CapabilityPreviewSessionState>(createCapabilityPreviewSessionState);
  const [generating, setGenerating] = useState(false);
  const bridgeConfig = useAgentBridgeRuntimeConfig();
  const { providerKind, cloudBaseUrl, cloudModel, cloudApiKey } = bridgeConfig;

  useEffect(() => {
    if (!pendingAction || pendingAction.status !== "pending") return;
    const delayMs = Math.max(0, new Date(pendingAction.expiresAt).getTime() - Date.now());
    const timeout = window.setTimeout(() => {
      setPendingAction((current) => current ? expirePendingAiAction(current) : current);
    }, delayMs + 25);
    return () => window.clearTimeout(timeout);
  }, [pendingAction]);

  async function generateMockAdvisoryPlan() {
    setGenerating(true);
    try {
      const context = buildMockAiContextSnapshot();
      const generated = await mockProvider.generate({
        requestId: `ai-slot-preview-${Date.now()}`,
        providerId: mockProvider.config.providerId,
        context,
        contextPolicy: MOCK_AI_CONTEXT_POLICY,
        allowedActionKinds: context.allowedActions,
        outputSchema: "ai-action-plan/v1",
        userRequest: "Ask the visible trusted peer to run the restricted Hello Stdout demo."
      });
      showGeneratedResult(generated, mockProvider.config.displayName, context);
      logAgentBridgeLifecycle({ eventKind: "advisory_generated", roomRefShort: room.id });
    } finally {
      setGenerating(false);
    }
  }

  async function generateCloudAdvisoryPlan() {
    setGenerating(true);
    try {
      const context = buildMockAiContextSnapshot();
      const provider = new CloudOpenAICompatibleProvider({
        providerId: "pastey-cloud-openai-compatible-preview",
        displayName: "CloudOpenAICompatibleProvider",
        kind: "cloud_openai_compatible",
        apiShape: "openai_compatible_chat",
        baseUrl: cloudBaseUrl,
        model: cloudModel,
        apiKeyRef: cloudApiKey ? "runtime-memory-only" : undefined,
        timeoutMs: 30_000,
        maxOutputTokens: 512,
        enabled: true
      }, {
        apiKey: cloudApiKey
      });
      const generated = await provider.generate({
        requestId: `ai-slot-cloud-preview-${Date.now()}`,
        providerId: provider.config.providerId,
        context,
        contextPolicy: CLOUD_STRICT_AI_CONTEXT_POLICY,
        allowedActionKinds: context.allowedActions,
        outputSchema: "ai-action-plan/v1",
        userRequest: "Propose the restricted Hello Stdout advisory for the visible trusted mock peer."
      });
      showGeneratedResult(generated, provider.config.displayName, context);
      logAgentBridgeLifecycle({ eventKind: "advisory_generated", roomRefShort: room.id });
    } finally {
      setGenerating(false);
    }
  }

  function showGeneratedResult(
    generated: AiGenerateResult,
    providerDisplayName: string,
    context: ReturnType<typeof buildMockAiContextSnapshot>
  ) {
    const validation = validateAiActionPlan(generated.parsedPlan);
    const plan = validation.valid ? validation.value : undefined;
    const policy = plan ? evaluateAiPolicy(plan, context) : undefined;
    let nextPendingAction: PendingAiAction | null = null;
    let pendingError: string | undefined;
    if (plan && policy?.status === "accepted") {
      try {
        nextPendingAction = createPendingAiAction(plan, policy);
      } catch (error) {
        pendingError = error instanceof Error ? error.message : "Pending action creation failed closed.";
      }
    }
    if (nextPendingAction) {
      logAgentBridgeLifecycle({
        eventKind: "local_confirmation_requested",
        roomRefShort: room.id,
        requestIdShort: nextPendingAction.pendingId,
      });
    }
    setPendingAction(nextPendingAction);
    setOutboundPreview(null);
    setEnvelopePreview(null);
    setInboundPreview(null);
    setResult({
      provider: providerDisplayName,
      model: generated.model,
      plan,
      validationStatus: validation.valid ? "accepted" : "rejected",
      validationErrors: validation.errors,
      policy,
      providerError: generated.error
        ? `${generated.error.code}: ${generated.error.message}`
        : pendingError
    });
    logAgentBridgeLifecycle({
      eventKind: policy?.status === "accepted" ? "policy_accepted" : "policy_rejected",
      roomRefShort: room.id,
      policyResult: policy?.status ?? "not_evaluated",
      errorCode: generated.error?.code,
    });
  }

  function confirmPendingLocally() {
    setOutboundPreview(null);
    setEnvelopePreview(null);
    setInboundPreview(null);
    setPendingAction((current) => current ? confirmPendingAiAction(current) : current);
    logAgentBridgeLifecycle({ eventKind: "local_confirmation_confirmed", roomRefShort: room.id });
  }

  function cancelPendingLocally() {
    setOutboundPreview(null);
    setEnvelopePreview(null);
    setInboundPreview(null);
    setPendingAction((current) => current ? cancelPendingAiAction(current) : current);
  }

  function buildOutboundRequestPreview() {
    if (!pendingAction) return;
    const buildResult = pendingAction.actionPlan.kind === "request_peer_hello_stdout_demo"
      ? buildHelloStdoutRequestFromPendingAction(pendingAction)
      : buildHelloPeerRequestFromPendingAction(pendingAction);
    if (!buildResult.ok) {
      setOutboundPreview({
        validationStatus: "rejected",
        errors: buildResult.errors
      });
      return;
    }
    const validation = buildResult.request.capability === "runtime.hello_stdout/v1"
      ? validateHelloStdoutRequest(buildResult.request)
      : validateHelloPeerRequest(buildResult.request);
    setEnvelopePreview(null);
    setInboundPreview(null);
    setOutboundPreview({
      request: validation.valid ? validation.value : buildResult.request,
      validationStatus: validation.valid ? "accepted" : "rejected",
      errors: validation.errors
    });
    logAgentBridgeLifecycle({ eventKind: "preview_built", roomRefShort: room.id, requestIdShort: buildResult.request.requestId });
  }

  function buildPreviewEnvelope() {
    const request = outboundPreview?.request;
    if (!request) return;
    const buildResult = buildCapabilityRequestPreviewEnvelope(request, {
      roomRef: "mock-room-preview"
    });
    if (!buildResult.ok) {
      setEnvelopePreview({
        validationStatus: "rejected",
        errors: buildResult.errors
      });
      return;
    }
    const validation = validateCapabilityRequestPreviewEnvelope(buildResult.envelope, {
      expectedRoomRef: "mock-room-preview",
      expectedTargetPeerRef: request.targetPeerRef
    });
    setEnvelopePreview({
      envelope: validation.valid ? validation.value : buildResult.envelope,
      validationStatus: validation.valid ? "accepted" : "rejected",
      errors: validation.errors
    });
    logAgentBridgeLifecycle({ eventKind: "preview_built", roomRefShort: room.id, eventIdShort: buildResult.envelope.envelopeId });
    setInboundPreview(null);
  }

  function previewInboundLocally() {
    const envelope = envelopePreview?.envelope;
    if (!envelope) return;
    const validation = validateCapabilityRequestPreviewEnvelope(envelope, {
      expectedRoomRef: envelope.roomRef,
      expectedTargetPeerRef: envelope.targetPeerRef
    });
    if (!validation.valid) {
      setInboundPreview({
        validationStatus: "rejected",
        errors: validation.errors
      });
      return;
    }
    const replay = checkAndRecordCapabilityPreview(validation.value, previewSession);
    if (!replay.ok) {
      setInboundPreview({
        envelope: { ...validation.value, status: replay.reason === "expired" ? "expired" : "invalid" },
        validationStatus: "rejected",
        errors: replay.errors
      });
      return;
    }
    setPreviewSession(replay.state);
    setInboundPreview({
      envelope: markCapabilityPreviewReceived(validation.value),
      validationStatus: "accepted",
      errors: []
    });
  }

  function acknowledgeInboundPreview() {
    setInboundPreview((current) => current?.envelope
      ? { ...current, envelope: acknowledgeCapabilityPreview(current.envelope) }
      : current);
  }

  function denyInboundPreview() {
    setInboundPreview((current) => current?.envelope
      ? { ...current, envelope: denyCapabilityPreview(current.envelope) }
      : current);
  }

  const cloudReady = cloudBaseUrl.trim().length > 0 && cloudModel.trim().length > 0;
  const workflowStatus = getWorkflowStatus({
    result,
    pendingAction,
    outboundPreview,
    envelopePreview
  });
  const workflowError = result?.providerError
    ?? result?.validationErrors[0]
    ?? outboundPreview?.errors[0]
    ?? envelopePreview?.errors[0];
  const nextAction = getNextWorkflowAction({
    result,
    pendingAction,
    outboundPreview,
    envelopePreview,
    generating,
    providerKind,
    cloudReady,
    generateMockAdvisoryPlan,
    generateCloudAdvisoryPlan,
    confirmPendingLocally,
    cancelPendingLocally,
    buildOutboundRequestPreview,
    buildPreviewEnvelope,
    queuePreview: () => {
      document.getElementById("agent-bridge-room-control-title")?.scrollIntoView({
        behavior: "smooth",
        block: "center"
      });
    }
  });

  if (!bridgeConfig.enabled) return null;
  return (
    <section className="panel ai-slot-preview room-agent-bridge" data-testid="room-agent-bridge-panel">
        <div className="diagnostics-panel-header">
          <div>
            <strong>Agent Bridge</strong>
            <p className="muted">Peer: {room.peer_device_name ?? "Waiting for peer"} · current-session bounded Agent Bridge workflow.</p>
          </div>
        </div>
        <div className="agent-bridge-safety-summary">
          <span>Preview-only room control. Delivery is not consent.</span>
          <span>Allow once and execution request remain explicit. No generic runtime or reusable trust.</span>
        </div>
        <AgentBridgeOverview
          workflowStatus={workflowStatus}
          summary={nextAction.summary}
          error={workflowError}
          actionLabel={nextAction.label}
          actionDisabled={nextAction.disabled}
          onAction={nextAction.onAction}
          secondaryActionLabel={nextAction.secondaryLabel}
          onSecondaryAction={nextAction.onSecondaryAction}
        />
        <RoomControlPanel room={room} envelope={envelopePreview?.envelope} />
        <AgentBridgeAdvancedDiagnostics>
          {result ? (
            <>
            <div className="diagnostic-grid">
              <PreviewBlock title="Provider" value={`${result.provider} / ${result.model}`} />
              <PreviewBlock title="Action kind" value={result.plan?.kind ?? "Rejected before parsing"} />
              <PreviewBlock title="Validation" value={result.validationStatus} />
              <PreviewBlock title="Policy" value={result.policy?.status ?? "Not evaluated"} />
            </div>
            {result.plan ? (
              <div className="ai-slot-plan-copy">
                <strong>{result.plan.title}</strong>
                <p className="muted">{result.plan.explanation}</p>
              </div>
            ) : null}
            {result.providerError ? (
              <PreviewMessages title="Provider error" messages={[result.providerError]} emptyMessage="No provider error." />
            ) : null}
            <PreviewMessages title="Validation details" messages={result.validationErrors} emptyMessage="Plan shape accepted." />
            <PreviewMessages title="Policy reasons" messages={result.policy?.reasons ?? []} emptyMessage="No rejection reasons." />
            <PreviewMessages title="Policy warnings" messages={result.policy?.warnings ?? []} emptyMessage="No warnings." />
            {pendingAction ? (
              <PendingActionCard
                pending={pendingAction}
                onConfirm={confirmPendingLocally}
                onCancel={cancelPendingLocally}
              />
            ) : null}
            {pendingAction?.status === "confirmed_local_only" ? (
              <HelloPeerOutboundPreview
                preview={outboundPreview}
                onBuild={buildOutboundRequestPreview}
              />
            ) : null}
            {outboundPreview?.request ? (
              <CapabilityEnvelopePreview
                preview={envelopePreview}
                onBuild={buildPreviewEnvelope}
                onPreviewInbound={previewInboundLocally}
              />
            ) : null}
            {inboundPreview ? (
              <InboundCapabilityPreview
                preview={inboundPreview}
                onAcknowledge={acknowledgeInboundPreview}
                onDeny={denyInboundPreview}
              />
            ) : null}
            </>
          ) : (
            <p className="muted">Generate an advisory plan to populate planning diagnostics.</p>
          )}
        </AgentBridgeAdvancedDiagnostics>
    </section>
  );
}

interface WorkflowState {
  result: AiSlotPreviewResult | null;
  pendingAction: PendingAiAction | null;
  outboundPreview: HelloPeerOutboundPreviewState | null;
  envelopePreview: CapabilityEnvelopePreviewState | null;
}

interface WorkflowActionOptions extends WorkflowState {
  generating: boolean;
  providerKind: "mock" | "cloud";
  cloudReady: boolean;
  generateMockAdvisoryPlan: () => Promise<void>;
  generateCloudAdvisoryPlan: () => Promise<void>;
  confirmPendingLocally: () => void;
  cancelPendingLocally: () => void;
  buildOutboundRequestPreview: () => void;
  buildPreviewEnvelope: () => void;
  queuePreview: () => void;
}

function getWorkflowStatus({
  result,
  pendingAction,
  outboundPreview,
  envelopePreview
}: WorkflowState): string {
  if (envelopePreview?.envelope) return "Preview ready";
  if (outboundPreview?.request) return "Confirmed locally";
  if (pendingAction?.status === "confirmed_local_only") return "Confirmed locally";
  if (pendingAction?.status === "pending") return "Awaiting local confirmation";
  if (pendingAction?.status === "cancelled") return "Rejected";
  if (pendingAction?.status === "expired") return "Rejected";
  if (result?.validationStatus === "rejected" || result?.policy?.status === "rejected") {
    return "Rejected";
  }
  if (result) return "Plan accepted";
  return "No plan";
}

function getNextWorkflowAction({
  result,
  pendingAction,
  outboundPreview,
  envelopePreview,
  generating,
  providerKind,
  cloudReady,
  generateMockAdvisoryPlan,
  generateCloudAdvisoryPlan,
  confirmPendingLocally,
  cancelPendingLocally,
  buildOutboundRequestPreview,
  buildPreviewEnvelope,
  queuePreview
}: WorkflowActionOptions) {
  if (!result || result.validationStatus === "rejected" || result.policy?.status === "rejected") {
    return {
      label: generating ? "Generating..." : "Generate advisory",
      disabled: generating || (providerKind === "cloud" && !cloudReady),
      onAction: () => void (providerKind === "mock" ? generateMockAdvisoryPlan() : generateCloudAdvisoryPlan()),
      summary: "Generate a bounded advisory plan."
    };
  }
  if (pendingAction?.status === "pending") {
    return {
      label: "Confirm locally",
      disabled: false,
      onAction: confirmPendingLocally,
      secondaryLabel: "Cancel",
      onSecondaryAction: cancelPendingLocally,
      summary: "Local confirmation is required before building a preview."
    };
  }
  if (pendingAction?.status === "confirmed_local_only" && !outboundPreview?.request) {
    return {
      label: "Build request preview",
      disabled: false,
      onAction: buildOutboundRequestPreview,
      summary: "Build the validated capability request preview."
    };
  }
  if (outboundPreview?.request && !envelopePreview?.envelope) {
    return {
      label: "Build preview",
      disabled: false,
      onAction: buildPreviewEnvelope,
      summary: "Wrap the request in a preview-only capability envelope."
    };
  }
  if (envelopePreview?.envelope) {
    return {
      label: "Continue to Room control",
      disabled: false,
      onAction: queuePreview,
      summary: "Queue the current preview from the active room-control session."
    };
  }
  return {
    label: "Generate advisory",
    disabled: generating,
    onAction: () => void generateMockAdvisoryPlan(),
    summary: "Generate a new advisory plan."
  };
}

function CapabilityEnvelopePreview({
  preview,
  onBuild,
  onPreviewInbound
}: {
  preview: CapabilityEnvelopePreviewState | null;
  onBuild: () => void;
  onPreviewInbound: () => void;
}) {
  const envelope = preview?.envelope;
  return (
    <div className="ai-slot-pending-card">
      <div className="ai-slot-pending-header">
        <div>
          <strong>Capability request transport preview</strong>
          <p className="muted">Builds an envelope locally. Existing room text transport is not used.</p>
        </div>
        <span className="ai-slot-pending-status">{envelope?.status ?? "not_built"}</span>
      </div>
      <div className="ai-slot-advisory-notice">
        <strong>Preview-only capability request.</strong>
        <span>Sending this preview does not allow execution.</span>
        <span>The peer can only view, acknowledge, or deny this preview.</span>
        <span>Only fixed bounded host-owned executors exist; this preview does not execute.</span>
        <span>No stdout, exit code, or runtime output can be produced in this phase.</span>
        <span>Preview-only room-control transport is available. The ordinary room text path is not a capability-preview channel.</span>
      </div>
      <div className="benchmark-controls">
        <button className="secondary-button" onClick={onBuild}>Build Preview Envelope</button>
        {envelope ? <button className="secondary-button" onClick={onPreviewInbound}>Preview inbound locally</button> : null}
      </div>
      {preview ? (
        <PreviewMessages title="Envelope validation" messages={preview.errors} emptyMessage="Envelope preview accepted." />
      ) : null}
      {envelope ? <CapabilityEnvelopeDetails envelope={envelope} /> : null}
    </div>
  );
}

function InboundCapabilityPreview({
  preview,
  onAcknowledge,
  onDeny
}: {
  preview: CapabilityEnvelopePreviewState;
  onAcknowledge: () => void;
  onDeny: () => void;
}) {
  const envelope = preview.envelope;
  return (
    <div className="ai-slot-pending-card">
      <div className="ai-slot-pending-header">
        <div>
          <strong>Local inbound capability preview simulation</strong>
          <p className="muted">No peer received this envelope. This card exercises inbound validation and state only.</p>
        </div>
        <span className={`ai-slot-pending-status ${envelope?.status ?? "invalid"}`}>{envelope?.status ?? "invalid"}</span>
      </div>
      <div className="ai-slot-advisory-notice">
        <strong>Inbound preview only — this cannot execute.</strong>
        <span>Acknowledging this preview is not permission to run code.</span>
        <span>A real execution still requires exact receiver Allow once and a separate explicit request.</span>
        <span>Trusted room is not execution authorization.</span>
      </div>
      <PreviewMessages title="Inbound validation" messages={preview.errors} emptyMessage="Inbound preview accepted." />
      {envelope ? <CapabilityEnvelopeDetails envelope={envelope} /> : null}
      {envelope?.status === "received_preview" ? (
        <div className="benchmark-controls">
          <button className="secondary-button" onClick={onAcknowledge}>Acknowledge preview</button>
          <button className="secondary-button" onClick={onDeny}>Deny preview</button>
        </div>
      ) : null}
    </div>
  );
}

function CapabilityEnvelopeDetails({ envelope }: { envelope: CapabilityRequestPreviewEnvelope }) {
  return (
    <>
      <div className="diagnostic-grid">
        <PreviewBlock title="Envelope ID" value={envelope.envelopeId} />
        <PreviewBlock title="Room" value={envelope.roomRef} />
        <PreviewBlock title="Source device" value={envelope.sourceDeviceRef} />
        <PreviewBlock title="Target peer" value={envelope.targetPeerRef} />
        <PreviewBlock title="Capability" value={envelope.request.capability} />
        {"message" in envelope.request.input ? <PreviewBlock title="Message" value={envelope.request.input.message} /> : null}
        {"expectedStdout" in envelope.request.input ? <PreviewBlock title="Expected stdout" value={envelope.request.input.expectedStdout} /> : null}
        <PreviewBlock title="Request ID" value={envelope.request.requestId} />
        <PreviewBlock title="Request payload hash" value={envelope.request.requestPayloadHash} />
        <PreviewBlock title="Expires" value={new Date(envelope.expiresAt).toLocaleString()} />
        <PreviewBlock title="Preview status" value={envelope.status} />
      </div>
      <div className="ai-slot-canonical-payload">
        <strong>Preview constraints</strong>
        <pre>{JSON.stringify(envelope.request.constraints, null, 2)}</pre>
      </div>
    </>
  );
}

function HelloPeerOutboundPreview({
  preview,
  onBuild
}: {
  preview: HelloPeerOutboundPreviewState | null;
  onBuild: () => void;
}) {
  const request = preview?.request;

  return (
    <div className="ai-slot-pending-card">
      <div className="ai-slot-pending-header">
        <div>
          <strong>{request?.capability === "runtime.hello_stdout/v1" ? "Hello Stdout outbound request preview" : "Hello Peer outbound request preview"}</strong>
          <p className="muted">Builds and validates a local request object only.</p>
        </div>
        <span className="ai-slot-pending-status">preview_only</span>
      </div>
      <div className="ai-slot-advisory-notice">
        <strong>Outbound preview only — no request was sent.</strong>
        <span>No peer received this request.</span>
        <span>Receiver one-time consent requires explicit PolicyGate review through the Room control queue. No execution exists.</span>
        <span>Request ID, nonce, expiry, and hash prepare future replay defenses but do not provide transport security yet.</span>
      </div>
      <div className="benchmark-controls">
        <button className="secondary-button" onClick={onBuild}>Build Capability Request Preview</button>
      </div>
      {preview ? (
        <>
          <PreviewBlock title="Request validation" value={preview.validationStatus} />
          <PreviewMessages title="Request validation details" messages={preview.errors} emptyMessage="Request preview accepted." />
        </>
      ) : null}
      {request ? (
        <>
          <div className="diagnostic-grid">
            <PreviewBlock title="Request ID" value={request.requestId} />
            <PreviewBlock title="Nonce" value={request.nonce} />
            <PreviewBlock title="Created" value={new Date(request.createdAt).toLocaleString()} />
            <PreviewBlock title="Expires" value={new Date(request.expiresAt).toLocaleString()} />
            <PreviewBlock title="Source device" value={request.sourceDeviceRef} />
            <PreviewBlock title="Target peer" value={request.targetPeerRef} />
            <PreviewBlock title="Capability" value={request.capability} />
            {"runtimePreference" in request ? <PreviewBlock title="Runtime preference" value={request.runtimePreference.join(", ")} /> : null}
            {"runtimeKind" in request ? <PreviewBlock title="Runtime kind" value={request.runtimeKind} /> : null}
            {"message" in request.input ? <PreviewBlock title="Message" value={request.input.message} /> : null}
            {"expectedStdout" in request.input ? <PreviewBlock title="Expected stdout" value={request.input.expectedStdout} /> : null}
            <PreviewBlock title="Pending payload hash" value={request.pendingPayloadHash} />
            <PreviewBlock title="Request payload hash" value={request.requestPayloadHash} />
            <PreviewBlock title="Transport status" value={request.transportStatus} />
          </div>
          <div className="ai-slot-canonical-payload">
            <strong>Outbound preview constraints</strong>
            <pre>{JSON.stringify(request.constraints, null, 2)}</pre>
          </div>
        </>
      ) : null}
    </div>
  );
}

function PendingActionCard({
  pending,
  onConfirm,
  onCancel
}: {
  pending: PendingAiAction;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const payload = pending.canonicalPayload;

  return (
    <div className="ai-slot-pending-card">
      <div className="ai-slot-pending-header">
        <div>
          <strong>Pending local confirmation</strong>
          <p className="muted">{pending.actionPlan.title}</p>
        </div>
        <span className={`ai-slot-pending-status ${pending.status}`}>{pending.status}</span>
      </div>
      <div className="diagnostic-grid">
        <PreviewBlock title="Pending ID" value={pending.pendingId} />
        <PreviewBlock title="Schema" value={payload.schemaVersion} />
        <PreviewBlock title="Action kind" value={payload.kind} />
        <PreviewBlock title="Target peer" value={payload.targetPeerRef} />
        <PreviewBlock title="Capability" value={payload.capability} />
        <PreviewBlock title="Message" value={payload.message} />
        <PreviewBlock
          title="References"
          value={payload.references.map((reference) => `${reference.kind}:${reference.ref}`).join(", ") || "None"}
        />
        <PreviewBlock title="Payload hash" value={pending.payloadHash} />
        <PreviewBlock title="Expires" value={new Date(pending.expiresAt).toLocaleString()} />
      </div>
      <div className="ai-slot-canonical-payload">
        <strong>Visible canonical constraints</strong>
        <pre>{JSON.stringify(payload.constraints, null, 2)}</pre>
      </div>
      <PreviewMessages title="Policy reasons" messages={pending.policyResult.reasons} emptyMessage="Policy accepted the bounded local proposal." />
      <PreviewMessages title="Policy warnings" messages={pending.policyResult.warnings} emptyMessage="No warnings." />
      <div className="ai-slot-advisory-notice">
        <strong>Local confirmation only - no action is executed.</strong>
        <span>Local confirmation does not send a peer request.</span>
        <span>Peer consent is still required before any future execution.</span>
        <span>Trusted room is not execution authorization.</span>
      </div>
      {pending.status === "pending" ? (
        <div className="benchmark-controls">
          <button className="secondary-button" onClick={onConfirm}>Confirm locally</button>
          <button className="secondary-button" onClick={onCancel}>Cancel</button>
        </div>
      ) : null}
      {pending.status === "confirmed_local_only" ? (
        <div className="ai-slot-local-result">
          <strong>Confirmed locally only - no peer request was sent.</strong>
          <span>Build and queue a preview explicitly; any receiver allow-once decision remains separate and non-executing.</span>
        </div>
      ) : null}
      {pending.status === "cancelled" ? (
        <div className="ai-slot-local-result"><strong>Pending action cancelled locally.</strong></div>
      ) : null}
      {pending.status === "expired" ? (
        <div className="ai-slot-local-result"><strong>Pending action expired. Generate a new advisory plan to continue.</strong></div>
      ) : null}
    </div>
  );
}

function PreviewBlock({ title, value }: { title: string; value: string }) {
  return (
    <div className="diagnostic-block">
      <strong>{title}</strong>
      <span>{value}</span>
    </div>
  );
}

function PreviewMessages({ title, messages, emptyMessage }: { title: string; messages: string[]; emptyMessage: string }) {
  return (
    <div className="ai-slot-preview-messages">
      <strong>{title}</strong>
      {messages.length > 0 ? (
        <ul>
          {messages.map((message) => <li key={message}>{message}</li>)}
        </ul>
      ) : (
        <p className="muted">{emptyMessage}</p>
      )}
    </div>
  );
}

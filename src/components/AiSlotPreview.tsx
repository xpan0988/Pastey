import { useEffect, useState } from "react";
import {
  CloudOpenAICompatibleProvider,
  CLOUD_STRICT_AI_CONTEXT_POLICY,
  MOCK_AI_CONTEXT_POLICY,
  acknowledgeCapabilityPreview,
  buildCapabilityRequestPreviewEnvelope,
  buildHelloPeerRequestFromPendingAction,
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
  type AiActionPlan,
  type AiGenerateResult,
  type AiPolicyResult,
  type CapabilityPreviewSessionState,
  type CapabilityRequestPreviewEnvelope,
  type HelloPeerRequest,
  type PendingAiAction
} from "../lib/ai";
import {
  buildCapabilityPreviewControlEvent,
  buildCapabilityPreviewStatusControlEvent,
  buildSessionBoundCapabilityPreviewControlEvent,
  createIdleRoomControlSendState,
  createControlQueueState,
  enqueueRoomControlEvent,
  getControlQueueBudget,
  markControlQueueItemStatus,
  preserveRoomControlSendStateForSession,
  sendCurrentRoomControlEvent,
  selectNextControlQueueItem,
  type CapabilityPreviewControlStatus,
  type CapabilityPreviewRoomControlEvent,
  type ControlQueueItem,
  type ControlQueueState,
  type RoomControlEvent,
  type RoomControlSendState
} from "../lib/agentBridge";
import {
  getRoomControlSessionContext,
  listReceivedRoomControlEvents,
  listRooms,
  sendRoomControlEvent
} from "../lib/tauri";
import type {
  ReceivedRoomControlEvent,
  RoomControlSessionContext,
  RoomInfo
} from "../lib/types";

type PreviewProvider = "mock" | "cloud";

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
  request?: HelloPeerRequest;
  validationStatus: "accepted" | "rejected";
  errors: string[];
}

interface CapabilityEnvelopePreviewState {
  envelope?: CapabilityRequestPreviewEnvelope;
  validationStatus: "accepted" | "rejected";
  errors: string[];
}

export function AiSlotPreview() {
  const [result, setResult] = useState<AiSlotPreviewResult | null>(null);
  const [pendingAction, setPendingAction] = useState<PendingAiAction | null>(null);
  const [outboundPreview, setOutboundPreview] = useState<HelloPeerOutboundPreviewState | null>(null);
  const [envelopePreview, setEnvelopePreview] = useState<CapabilityEnvelopePreviewState | null>(null);
  const [inboundPreview, setInboundPreview] = useState<CapabilityEnvelopePreviewState | null>(null);
  const [previewSession, setPreviewSession] = useState<CapabilityPreviewSessionState>(createCapabilityPreviewSessionState);
  const [generating, setGenerating] = useState(false);
  const [providerKind, setProviderKind] = useState<PreviewProvider>("mock");
  const [cloudBaseUrl, setCloudBaseUrl] = useState("https://api.openai.com/v1");
  const [cloudModel, setCloudModel] = useState("");
  const [cloudApiKey, setCloudApiKey] = useState("");

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
        userRequest: "Ask the visible trusted peer to run the restricted Hello Peer demo."
      });
      showGeneratedResult(generated, mockProvider.config.displayName, context);
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
        userRequest: "Propose the restricted Hello Peer advisory for the visible trusted mock peer."
      });
      showGeneratedResult(generated, provider.config.displayName, context);
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
  }

  function confirmPendingLocally() {
    setOutboundPreview(null);
    setEnvelopePreview(null);
    setInboundPreview(null);
    setPendingAction((current) => current ? confirmPendingAiAction(current) : current);
  }

  function cancelPendingLocally() {
    setOutboundPreview(null);
    setEnvelopePreview(null);
    setInboundPreview(null);
    setPendingAction((current) => current ? cancelPendingAiAction(current) : current);
  }

  function buildOutboundRequestPreview() {
    if (!pendingAction) return;
    const buildResult = buildHelloPeerRequestFromPendingAction(pendingAction);
    if (!buildResult.ok) {
      setOutboundPreview({
        validationStatus: "rejected",
        errors: buildResult.errors
      });
      return;
    }
    const validation = validateHelloPeerRequest(buildResult.request);
    setEnvelopePreview(null);
    setInboundPreview(null);
    setOutboundPreview({
      request: validation.valid ? validation.value : buildResult.request,
      validationStatus: validation.valid ? "accepted" : "rejected",
      errors: validation.errors
    });
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

  return (
    <div className="settings-row diagnostics-panel-row">
      <span className="settings-icon wrench" aria-hidden="true" />
      <div className="diagnostics-panel ai-slot-preview">
        <div className="diagnostics-panel-header">
          <div>
            <strong>AI Slot Phase E1 Preview</strong>
            <p className="muted">Capability-envelope and inbound-preview simulation. Actual room transport is unavailable.</p>
          </div>
        </div>
        <div className="ai-slot-advisory-notice">
          <strong>Advisory only - no action is executed.</strong>
          <span>Local confirmation only - no action is executed.</span>
          <span>No peer request is sent in this build.</span>
          <span>Peer consent is still required before any future execution.</span>
          <span>Trusted room is not execution authorization.</span>
          <span>Cloud context is redacted and current-session only.</span>
          <span>Provider output is untrusted and must pass validation and PolicyGate.</span>
          <span>No raw shell, file access, or hidden transfer is available.</span>
        </div>
        <div className="ai-slot-provider-controls">
          <label>
            Preview provider
            <select value={providerKind} onChange={(event) => setProviderKind(event.target.value as PreviewProvider)}>
              <option value="mock">MockProvider</option>
              <option value="cloud">CloudOpenAICompatibleProvider</option>
            </select>
          </label>
          {providerKind === "cloud" ? (
            <>
              <label>
                Base URL
                <input value={cloudBaseUrl} onChange={(event) => setCloudBaseUrl(event.target.value)} />
              </label>
              <label>
                Model
                <input
                  value={cloudModel}
                  placeholder="Provider model ID"
                  onChange={(event) => setCloudModel(event.target.value)}
                />
              </label>
              <label>
                API key (runtime memory only)
                <input
                  type="password"
                  autoComplete="off"
                  value={cloudApiKey}
                  placeholder="Optional for compatible endpoints"
                  onChange={(event) => setCloudApiKey(event.target.value)}
                />
              </label>
            </>
          ) : null}
        </div>
        <div className="benchmark-controls">
          {providerKind === "mock" ? (
            <button className="secondary-button" disabled={generating} onClick={() => void generateMockAdvisoryPlan()}>
              {generating ? "Generating..." : "Generate Mock Advisory Plan"}
            </button>
          ) : (
            <button
              className="secondary-button"
              disabled={generating || !cloudReady}
              onClick={() => void generateCloudAdvisoryPlan()}
            >
              {generating ? "Generating..." : "Generate Cloud Advisory Plan"}
            </button>
          )}
          {providerKind === "cloud" ? (
            <span className="muted">Experimental preview. Provider configuration and API key are not persisted.</span>
          ) : null}
        </div>
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
            {envelopePreview?.envelope ? (
              <LocalControlQueueSimulation
                key={envelopePreview.envelope.envelopeId}
                envelope={envelopePreview.envelope}
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
        ) : null}
      </div>
    </div>
  );
}

function LocalControlQueueSimulation({
  envelope
}: {
  envelope: CapabilityRequestPreviewEnvelope;
}) {
  const [queue, setQueue] = useState<ControlQueueState>(createControlQueueState);
  const [messages, setMessages] = useState<string[]>([]);
  const [activeRooms, setActiveRooms] = useState<RoomInfo[]>([]);
  const [selectedRoomId, setSelectedRoomId] = useState("");
  const [session, setSession] = useState<RoomControlSessionContext | null>(null);
  const [transportEvent, setTransportEvent] = useState<RoomControlEvent | null>(null);
  const [sendState, setSendState] = useState<RoomControlSendState>(createIdleRoomControlSendState);
  const [receivedEvents, setReceivedEvents] = useState<ReceivedRoomControlEvent[]>([]);
  const [transportBusy, setTransportBusy] = useState(false);
  const budget = getControlQueueBudget(queue);
  const selected = [...queue.inbound, ...queue.outbound].find((item) => item.status === "selected");

  useEffect(() => {
    void refreshTransportRooms();
  }, []);

  useEffect(() => {
    if (!selectedRoomId) {
      applySession(null);
      setReceivedEvents([]);
      return;
    }
    void getRoomControlSessionContext(selectedRoomId)
      .then(applySession)
      .catch((error) => {
        applySession(null);
        setMessages([error instanceof Error ? error.message : String(error)]);
      });
  }, [selectedRoomId]);

  useEffect(() => {
    if (!session) {
      setTransportEvent(null);
      return;
    }
    const buildResult = buildSessionBoundCapabilityPreviewControlEvent(envelope, session);
    if (!buildResult.ok) {
      setTransportEvent(null);
      setMessages(buildResult.errors);
      return;
    }
    setTransportEvent(buildResult.event);
  }, [envelope, session?.roomId, session?.localSessionRef, session?.peerSessionRef]);

  function applySession(nextSession: RoomControlSessionContext | null) {
    setSession((currentSession) => {
      setSendState((currentState) =>
        preserveRoomControlSendStateForSession(currentState, currentSession, nextSession)
      );
      return nextSession;
    });
  }

  async function refreshTransportRooms() {
    try {
      const rooms = (await listRooms()).filter(
        (room) => room.status === "active" && room.peer_connected
      );
      setActiveRooms(rooms);
      const nextRoomId = rooms.some((room) => room.id === selectedRoomId)
        ? selectedRoomId
        : rooms[0]?.id ?? "";
      setSelectedRoomId(nextRoomId);
      if (nextRoomId && nextRoomId === selectedRoomId) {
        applySession(await getRoomControlSessionContext(nextRoomId));
      }
    } catch (error) {
      setMessages([error instanceof Error ? error.message : String(error)]);
    }
  }

  function enqueueOutboundPreview() {
    const buildResult = buildCapabilityPreviewControlEvent(envelope, {
      roomRef: envelope.roomRef
    });
    if (!buildResult.ok || buildResult.event.kind !== "capability_preview") {
      setMessages(buildResult.ok ? ["Expected a capability preview control event."] : buildResult.errors);
      return;
    }
    const enqueueResult = enqueueRoomControlEvent(queue, buildResult.event, "outbound");
    if (!enqueueResult.ok) {
      setMessages(enqueueResult.errors);
      return;
    }
    setQueue(enqueueResult.state);
    setMessages(["Outbound capability preview enqueued locally. No room event was sent."]);
  }

  function simulateInboundStatus(status: CapabilityPreviewControlStatus) {
    const outbound = [...queue.outbound]
      .reverse()
      .find((item): item is ControlQueueItem & { event: CapabilityPreviewRoomControlEvent } =>
        item.event.kind === "capability_preview"
      );
    if (!outbound) {
      setMessages(["Enqueue an outbound capability preview before simulating an inbound status."]);
      return;
    }
    const buildResult = buildCapabilityPreviewStatusControlEvent(outbound.event, status);
    if (!buildResult.ok) {
      setMessages(buildResult.errors);
      return;
    }
    const enqueueResult = enqueueRoomControlEvent(queue, buildResult.event, "inbound");
    if (!enqueueResult.ok) {
      setMessages(enqueueResult.errors);
      return;
    }
    const transitionResult = markControlQueueItemStatus(
      enqueueResult.state,
      outbound.queueId,
      status,
      { reason: `Local simulation received ${buildResult.event.kind}.` }
    );
    if (!transitionResult.ok) {
      setMessages(transitionResult.errors);
      return;
    }
    setQueue(transitionResult.state);
    setMessages([
      `${buildResult.event.kind} enqueued inbound locally.`,
      status === "acknowledged_preview_only"
        ? "Acknowledgement is preview-only and is not execution consent."
        : "No retry or escalation was created."
    ]);
  }

  function selectNextLocally() {
    const result = selectNextControlQueueItem(queue);
    setQueue(result.state);
    setMessages(result.ok
      ? [`Selected ${result.item.event.kind} locally. Nothing was dispatched.`]
      : [result.reason]);
  }

  async function sendPreviewOverTransport() {
    if (!session || !transportEvent) {
      setMessages(["Select an active room session before transport delivery."]);
      return;
    }
    const result = await sendCurrentRoomControlEvent(
      transportEvent,
      (event) => sendRoomControlEvent(session.roomId, event),
      setSendState
    );
    if (result.status === "accepted") {
      setMessages([
        `Transport accepted ${result.eventId} for the peer's bounded local inbox.`,
        "Transport delivery is not peer consent."
      ]);
    } else if (result.status === "rejected") {
      setMessages([
        result.message,
        "Failed delivery was not converted into acknowledgement or denial."
      ]);
    }
  }

  async function refreshReceivedInbox() {
    if (!session) {
      setMessages(["Select an active room session before refreshing the control inbox."]);
      return;
    }
    setTransportBusy(true);
    try {
      const events = await listReceivedRoomControlEvents(session.roomId);
      setReceivedEvents(events);
      setMessages([`Loaded ${events.length} current-session control inbox event(s).`]);
    } catch (error) {
      setMessages([error instanceof Error ? error.message : String(error)]);
    } finally {
      setTransportBusy(false);
    }
  }

  return (
    <div className="ai-slot-pending-card">
      <div className="ai-slot-pending-header">
        <div>
          <strong>CL-2 local control queue simulation</strong>
          <p className="muted">Outbound/inbound priority and hypothetical budget only.</p>
        </div>
        <span className="ai-slot-pending-status">local_only</span>
      </div>
      <div className="ai-slot-advisory-notice">
        <strong>Local control queue simulation only — no room event is sent.</strong>
        <span>Control backlog would reserve one logical control lane in a future scheduler phase.</span>
        <span>Current scheduler behavior is unchanged.</span>
        <span>Acknowledging preview is not execution consent.</span>
      </div>
      <div className="ai-slot-advisory-notice">
        <strong>CL-3B preview-only transport delivery.</strong>
        <span>Transport delivery is not peer consent.</span>
        <span>Preview acknowledgement is not execution consent.</span>
        <span>No capability is executed.</span>
        <span>Scheduler reservation is not active.</span>
      </div>
      <div className="benchmark-controls">
        <select
          value={selectedRoomId}
          onChange={(event) => setSelectedRoomId(event.target.value)}
          aria-label="Active room-control transport room"
        >
          <option value="">No active room selected</option>
          {activeRooms.map((room) => (
            <option key={room.id} value={room.id}>
              {room.peer_device_name ?? room.room_code_display ?? room.id}
            </option>
          ))}
        </select>
        <button className="secondary-button" onClick={() => void refreshTransportRooms()}>
          Refresh active rooms
        </button>
        {session ? (
          <>
            <button
              className="secondary-button"
              disabled={transportBusy || sendState.status === "sending" || !transportEvent}
              onClick={() => void sendPreviewOverTransport()}
            >
              Send preview over room-control transport
            </button>
            <button
              className="secondary-button"
              disabled={transportBusy}
              onClick={() => void refreshReceivedInbox()}
            >
              Refresh received control inbox
            </button>
            <button
              className="secondary-button"
              disabled={sendState.status === "idle" || sendState.status === "sending"}
              onClick={() => setSendState(createIdleRoomControlSendState())}
            >
              Clear latest send result
            </button>
          </>
        ) : null}
      </div>
      {session ? (
        <div className="diagnostic-grid">
          <PreviewBlock title="Transport room" value={session.roomId} />
          <PreviewBlock title="Local session ref" value={session.localSessionRef} />
          <PreviewBlock title="Peer session ref" value={session.peerSessionRef} />
          <PreviewBlock title="Current transport event ID" value={transportEvent?.eventId ?? "Not built"} />
        </div>
      ) : null}
      <LatestRoomControlSend state={sendState} />
      <ReceivedControlInbox events={receivedEvents} />
      <div className="benchmark-controls">
        <button className="secondary-button" onClick={enqueueOutboundPreview}>Enqueue outbound preview locally</button>
        <button className="secondary-button" onClick={selectNextLocally}>Select next locally</button>
        <button className="secondary-button" onClick={() => simulateInboundStatus("acknowledged_preview_only")}>Simulate inbound ack</button>
        <button className="secondary-button" onClick={() => simulateInboundStatus("denied")}>Simulate inbound deny</button>
        <button className="secondary-button" onClick={() => simulateInboundStatus("invalid")}>Simulate inbound invalid</button>
        <button className="secondary-button" onClick={() => simulateInboundStatus("expired")}>Simulate inbound expired</button>
      </div>
      <div className="diagnostic-grid">
        <PreviewBlock title="Data windows (hypothetical)" value={String(budget.dataWindows)} />
        <PreviewBlock title="Control windows (hypothetical)" value={String(budget.controlWindows)} />
        <PreviewBlock title="Selected next event" value={selected?.event.kind ?? "None"} />
        <PreviewBlock title="Selected queue ID" value={selected?.queueId ?? "None"} />
      </div>
      <ControlQueueList title="Outbound queue" items={queue.outbound} />
      <ControlQueueList title="Inbound queue" items={queue.inbound} />
      <PreviewMessages title="Queue messages" messages={messages} emptyMessage="No duplicate, replay, or expiry messages." />
    </div>
  );
}

function LatestRoomControlSend({ state }: { state: RoomControlSendState }) {
  const timestamp =
    state.status === "sending"
      ? state.startedAt
      : state.status === "accepted"
        ? state.receivedAt
        : state.status === "rejected"
          ? state.occurredAt
          : null;
  const summary =
    state.status === "idle"
      ? "No send attempted."
      : state.status === "sending"
        ? "Sending…"
        : state.status === "accepted"
          ? "Accepted for peer local inbox."
          : state.message;
  return (
    <div className="ai-slot-preview-messages">
      <strong>Latest room-control send</strong>
      <div className="diagnostic-grid">
        <PreviewBlock title="Status" value={state.status} />
        <PreviewBlock title="Event ID" value={state.status === "idle" ? "None" : state.eventId} />
        <PreviewBlock title="Timestamp" value={timestamp ?? "None"} />
        <PreviewBlock
          title="Result code"
          value={state.status === "accepted" ? "accepted_for_local_inbox" : state.status === "rejected" ? state.errorCode : "None"}
        />
      </div>
      <p className="muted">{summary}</p>
      <p className="muted">Transport delivery is not peer consent.</p>
    </div>
  );
}

function ReceivedControlInbox({ events }: { events: ReceivedRoomControlEvent[] }) {
  return (
    <div className="ai-slot-preview-messages">
      <strong>Received preview-only control inbox</strong>
      {events.length > 0 ? (
        <ul>
          {events.map((event) => (
            <li key={event.eventId}>
              {event.kind} / {event.eventId} / {event.sourceDeviceRef} → {event.targetPeerRef} /
              expires {new Date(event.expiresAt).toLocaleString()}
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">No received current-session control events.</p>
      )}
    </div>
  );
}

function ControlQueueList({ title, items }: { title: string; items: ControlQueueItem[] }) {
  return (
    <div className="ai-slot-preview-messages">
      <strong>{title}</strong>
      {items.length > 0 ? (
        <ul>
          {items.map((item) => (
            <li key={item.queueId}>
              {item.event.kind} / {item.status} / priority {item.priority} / {item.queueId}
              {item.reason ? ` / ${item.reason}` : ""}
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">Empty.</p>
      )}
    </div>
  );
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
        <span>No peer executor exists in Phase E1.</span>
        <span>No stdout, exit code, or runtime output can be produced in this phase.</span>
        <span>Transport unavailable in this build. The ordinary room text path is not a capability-preview channel.</span>
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
        <span>Peer execution is not implemented in Phase E1.</span>
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
        <PreviewBlock title="Message" value={envelope.request.input.message} />
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
          <strong>Hello Peer outbound request preview</strong>
          <p className="muted">Builds and validates a local request object only.</p>
        </div>
        <span className="ai-slot-pending-status">preview_only</span>
      </div>
      <div className="ai-slot-advisory-notice">
        <strong>Outbound preview only — no request was sent.</strong>
        <span>No peer received this request.</span>
        <span>Peer consent and transport are not implemented in Phase E0.</span>
        <span>Request ID, nonce, expiry, and hash prepare future replay defenses but do not provide transport security yet.</span>
      </div>
      <div className="benchmark-controls">
        <button className="secondary-button" onClick={onBuild}>Build Hello Peer Request Preview</button>
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
            <PreviewBlock title="Runtime preference" value={request.runtimePreference.join(", ")} />
            <PreviewBlock title="Message" value={request.input.message} />
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
        <span>No peer request is sent in this build.</span>
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
          <span>Peer consent and transport are not implemented in this phase.</span>
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

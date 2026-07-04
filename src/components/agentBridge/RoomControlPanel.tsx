import { useEffect, useRef, useState, useSyncExternalStore } from "react";

import type { CandidatePayloadExecutionRequest, CapabilityRequestPreviewEnvelope } from "../../lib/ai";
import {
  buildCapabilityPreviewControlEvent,
  buildCapabilityPreviewStatusControlEvent,
  buildCandidatePayloadExecutionRequest,
  buildFileCandidateExecutionRequest,
  buildHelloPeerExecutionRequest,
  buildHelloStdoutExecutionRequest,
  buildSessionBoundCapabilityPreviewControlEvent,
  buildPeerConsentStatusEvent,
  allowPeerCapabilityOnce,
  applyInboundPeerStatusToOutboundQueue,
  createControlQueueState,
  createIdleRoomControlSendState,
  createPeerConsentSessionState,
  createPeerConsentConsumptionState,
  denyPeerCapability,
  enqueueInboundRoomControlEvents,
  enqueueRoomControlEvent,
  evaluatePeerCapabilityPreview,
  executeInboundCandidatePayloadRequest,
  executeInboundFileCandidateRequest,
  executeInboundHelloPeerRequest,
  executeInboundHelloStdoutRequest,
  getRuntimeControlWindowStatus,
  hasOutgoingControlWindowDemand,
  logAgentBridgeLifecycle,
  markControlQueueItemStatus,
  matchExecutionResultToRequest,
  preserveControlQueueForSession,
  preserveRoomControlSendStateForSession,
  processNextControlQueueItem,
  resetOutgoingControlWindowDemandForSession,
  roomControlSessionIdentity,
  selectNextControlQueueItem,
  sendCurrentRoomControlEvent,
  setOutgoingControlWindowDemand,
  subscribeRuntimeControlWindowStatus,
  waitForRuntimeDataWindowTarget,
  type CandidatePayloadHandoffResult,
  type CandidatePayloadLocalResolution,
  type CapabilityPreviewControlStatus,
  type CapabilityPreviewRoomControlEvent,
  type CapabilityPreviewAckRoomControlEvent,
  type CapabilityExecuteRequestRoomControlEvent,
  type CapabilityExecutionResultRoomControlEvent,
  type ControlQueueItem,
  type ControlQueueState,
  type PeerConsentBinding,
  type PeerConsentRecord,
  type PeerConsentSessionState,
  type PeerConsentConsumptionState,
  type RoomControlEvent,
  type RoomControlSendState,
} from "../../lib/agentBridge";
import { bridgePeerSessionId } from "../../lib/bridgeRouting";
import {
  executeFileCandidateSearchCapability,
  executeHelloStdoutCapability,
  getRoomControlSessionContext,
  listReceivedRoomControlEvents,
  resolveCandidatePayloadCapability,
  sendRoomControlEvent,
} from "../../lib/tauri";
import {
  assertCapabilityEventHasSelectedPeerRoute,
  bridgeRoutePayload,
} from "../../lib/bridgeRoutingRuntime";
import type { TransferQueueInput } from "../../lib/transferScheduler";
import type {
  ReceivedRoomControlEvent,
  RoomControlSessionContext,
  RoomInfo,
} from "../../lib/types";

interface RoomControlPanelProps {
  room: RoomInfo;
  envelope?: CapabilityRequestPreviewEnvelope;
  onEnqueueCandidatePayloadHandoff?: (input: TransferQueueInput) => boolean;
}

interface PeerReviewState {
  queueId: string;
  event: CapabilityPreviewRoomControlEvent;
  binding: PeerConsentBinding;
  status: "awaiting_peer_decision" | "allowed_once" | "consumed" | "denied" | "expired" | "invalid";
  record?: PeerConsentRecord;
}

const ROOM_CONTROL_QUEUE_DEMAND_SOURCE = "agent-bridge-room-control-queue";
const ROOM_CONTROL_ACTIVE_SEND_SOURCE = "agent-bridge-room-control-active-send";

export function RoomControlPanel({ room, envelope, onEnqueueCandidatePayloadHandoff }: RoomControlPanelProps) {
  const [queue, setQueue] = useState<ControlQueueState>(createControlQueueState);
  const [messages, setMessages] = useState<string[]>([]);
  const [session, setSession] = useState<RoomControlSessionContext | null>(null);
  const sessionRef = useRef<RoomControlSessionContext | null>(null);
  const [transportEvent, setTransportEvent] = useState<RoomControlEvent | null>(null);
  const [sendState, setSendState] = useState<RoomControlSendState>(createIdleRoomControlSendState);
  const [receivedEvents, setReceivedEvents] = useState<ReceivedRoomControlEvent[]>([]);
  const [transportBusy, setTransportBusy] = useState(false);
  const [controlDemandNowMs, setControlDemandNowMs] = useState(Date.now);
  const [peerConsentSession, setPeerConsentSession] =
    useState<PeerConsentSessionState>(createPeerConsentSessionState);
  const [peerReview, setPeerReview] = useState<PeerReviewState | null>(null);
  const [peerConsentRecords, setPeerConsentRecords] = useState<PeerConsentRecord[]>([]);
  const [consumptionState, setConsumptionState] =
    useState<PeerConsentConsumptionState>(createPeerConsentConsumptionState);
  const [senderExecutionAck, setSenderExecutionAck] =
    useState<CapabilityPreviewAckRoomControlEvent | null>(null);
  const [latestExecutionResult, setLatestExecutionResult] =
    useState<CapabilityExecutionResultRoomControlEvent | null>(null);
  const runtimeWindowStatus = useSyncExternalStore(
    subscribeRuntimeControlWindowStatus,
    getRuntimeControlWindowStatus,
  );
  const selected = [...queue.inbound, ...queue.outbound].find((item) => item.status === "selected");
  const nextActionable = selected ?? [...queue.inbound, ...queue.outbound]
    .filter((item) => item.status === "queued")
    .sort((a, b) => a.priority - b.priority || Date.parse(a.enqueuedAt) - Date.parse(b.enqueuedAt))[0];

  useEffect(() => {
    void refreshRoomSession();
  }, [room.id, room.peer_connected]);

  useEffect(() => {
    setOutgoingControlWindowDemand(
      ROOM_CONTROL_QUEUE_DEMAND_SOURCE,
      hasOutgoingControlWindowDemand(queue, sendState, { now: new Date(controlDemandNowMs) }),
    );
  }, [queue, sendState, controlDemandNowMs]);

  useEffect(() => {
    const nextExpiryMs = queue.outbound
      .filter((item) =>
        item.status === "queued" ||
        item.status === "selected" ||
        item.status === "transport_sending"
      )
      .map((item) => Date.parse(item.event.expiresAt))
      .filter((expiresAt) => expiresAt > controlDemandNowMs)
      .sort((left, right) => left - right)[0];
    if (nextExpiryMs === undefined) {
      return;
    }
    const timeout = window.setTimeout(
      () => setControlDemandNowMs(Date.now()),
      Math.max(0, nextExpiryMs - Date.now() + 1),
    );
    return () => window.clearTimeout(timeout);
  }, [queue, controlDemandNowMs]);

  useEffect(() => {
    if (!peerReview || peerReview.status !== "awaiting_peer_decision") {
      return;
    }
    const expiresAt = Date.parse(peerReview.binding.expiresAt);
    if (expiresAt <= controlDemandNowMs) {
      const expired = markControlQueueItemStatus(queue, peerReview.queueId, "expired", {
        now: new Date(controlDemandNowMs),
        reason: "Peer decision window expired. No approval was created.",
      });
      if (expired.ok) {
        setQueue(expired.state);
      }
      setPeerReview((current) => current ? { ...current, status: "expired" } : current);
      setMessages(["Peer preview expired before a decision. No approval was created."]);
      logAgentBridgeLifecycle({ eventKind: "consent_expired", roomRefShort: session?.roomId ?? room.id, errorCode: "expired" });
      return;
    }
    const timeout = window.setTimeout(
      () => setControlDemandNowMs(Date.now()),
      Math.max(0, expiresAt - Date.now() + 1),
    );
    return () => window.clearTimeout(timeout);
  }, [peerReview, queue, controlDemandNowMs]);

  useEffect(() => {
    const expiresAt = senderExecutionAck?.payload.consent
      ? Date.parse(senderExecutionAck.payload.consent.expiresAt)
      : null;
    if (expiresAt === null) {
      return;
    }
    if (expiresAt <= controlDemandNowMs) {
      setSenderExecutionAck(null);
      return;
    }
    const timeout = window.setTimeout(
      () => setControlDemandNowMs(Date.now()),
      Math.max(0, expiresAt - Date.now() + 1),
    );
    return () => window.clearTimeout(timeout);
  }, [senderExecutionAck, controlDemandNowMs]);

  useEffect(() => () => {
    setOutgoingControlWindowDemand(ROOM_CONTROL_QUEUE_DEMAND_SOURCE, false);
  }, []);

  useEffect(() => {
    if (!room.peer_connected || room.status !== "active") {
      applySession(null);
      setReceivedEvents([]);
      return;
    }
    void getRoomControlSessionContext(room.id)
      .then(applySession)
      .catch((error) => {
        applySession(null);
        setMessages([error instanceof Error ? error.message : String(error)]);
      });
  }, [room.id, room.peer_connected, room.status]);

  useEffect(() => {
    if (!session || !envelope) {
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
    const currentSession = sessionRef.current;
    const changed =
      roomControlSessionIdentity(currentSession) !== roomControlSessionIdentity(nextSession);
    setSendState((currentState) =>
      preserveRoomControlSendStateForSession(currentState, currentSession, nextSession)
    );
    setQueue((currentQueue) =>
      preserveControlQueueForSession(currentQueue, currentSession, nextSession)
    );
    if (changed) {
      resetOutgoingControlWindowDemandForSession();
      setPeerConsentSession(createPeerConsentSessionState());
      setConsumptionState(createPeerConsentConsumptionState());
      setPeerConsentRecords([]);
      setPeerReview(null);
      setSenderExecutionAck(null);
      setLatestExecutionResult(null);
      setReceivedEvents([]);
      setMessages([
        nextSession
          ? "Room-control queue bound to the selected current room session."
          : "Room-control queue cleared because no active room session is selected.",
      ]);
      logAgentBridgeLifecycle({
        eventKind: "session_cleared",
        roomRefShort: currentSession?.roomId ?? nextSession?.roomId ?? room.id,
        sessionRefShort: currentSession?.localSessionRef,
        peerRefShort: currentSession?.peerSessionRef,
      });
    }
    sessionRef.current = nextSession;
    setSession(nextSession);
  }

  async function refreshRoomSession() {
    try {
      applySession(room.peer_connected && room.status === "active"
        ? await getRoomControlSessionContext(room.id)
        : null);
    } catch (error) {
      setMessages([error instanceof Error ? error.message : String(error)]);
    }
  }

  function enqueueOutboundPreview() {
    if (!transportEvent) {
      setMessages(["Build an outbound capability preview and select an active room session first."]);
      return;
    }
    const enqueueResult = enqueueRoomControlEvent(queue, transportEvent, "outbound");
    if (!enqueueResult.ok) {
      setMessages(enqueueResult.errors);
      return;
    }
    setQueue(enqueueResult.state);
    logAgentBridgeLifecycle({ eventKind: "preview_queued", roomRefShort: session?.roomId ?? room.id, eventIdShort: enqueueResult.item.event.eventId });
    setMessages([`Outbound control event queued: ${enqueueResult.item.event.eventId}. No room event was sent yet.`]);
  }

  function enqueueOutboundPreviewLocally() {
    if (!envelope) {
      setMessages(["Build an outbound capability preview before using local simulation tools."]);
      return;
    }
    const buildResult = buildCapabilityPreviewControlEvent(envelope, {
      roomRef: envelope.roomRef,
    });
    if (!buildResult.ok) {
      setMessages(buildResult.errors);
      return;
    }
    const enqueueResult = enqueueRoomControlEvent(queue, buildResult.event, "outbound");
    if (!enqueueResult.ok) {
      setMessages(enqueueResult.errors);
      return;
    }
    setQueue(enqueueResult.state);
    setMessages(["Local simulation only: outbound preview queued without room transport."]);
  }

  function selectNextLocally() {
    const result = selectNextControlQueueItem(queue);
    setQueue(result.state);
    setMessages(result.ok
      ? [`Local simulation only: selected ${result.item.event.kind}; nothing was sent.`]
      : [result.reason]);
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
        : "No retry or escalation was created.",
    ]);
  }

  async function processNextQueueItem() {
    if (!session) {
      setMessages(["Select an active room session before processing the control queue."]);
      return;
    }
    setTransportBusy(true);
    const result = await processNextControlQueueItem(
      queue,
      (event) => sendWithRuntimeReservation(session.roomId, event),
      {
        onState: setQueue,
        onSendState: (next) => {
          setSendState(next);
          if (next.status === "sending") {
            logAgentBridgeLifecycle({
              eventKind: "transport_sending",
              roomRefShort: session.roomId,
              sessionRefShort: session.localSessionRef,
              peerRefShort: session.peerSessionRef,
              eventIdShort: next.eventId,
            });
          }
        },
      }
    );
    setQueue(result.state);
    setTransportBusy(false);
    if (result.ok && result.action === "selected_inbound") {
      await handleSelectedInbound(result.state, result.item);
    } else if (result.ok) {
      logAgentBridgeLifecycle({
        eventKind: result.item.event.kind === "capability_execute_request"
          ? "execution_request_sent"
          : result.item.event.kind === "capability_execution_result"
            ? "execution_result_delivered"
            : "transport_delivered",
        roomRefShort: session.roomId,
        sessionRefShort: session.localSessionRef,
        peerRefShort: session.peerSessionRef,
        eventIdShort: result.item.event.eventId,
        transportResult: "delivered",
      });
      setMessages([
        `Transport delivered queued event ${result.item.event.eventId} to the peer's bounded local inbox.`,
        "Transport delivery is not peer consent.",
      ]);
    } else {
      logAgentBridgeLifecycle({ eventKind: "transport_rejected", roomRefShort: session.roomId, errorCode: result.sendState?.status === "rejected" ? result.sendState.errorCode : "invalid_transition" });
      setMessages([result.message, "No retry, acknowledgement, denial, or execution was created."]);
    }
  }

  async function handleSelectedInbound(state: ControlQueueState, item: ControlQueueItem) {
    if (!session) {
      setMessages(["Peer PolicyGate requires an active current room session."]);
      return;
    }
    if (item.event.kind === "capability_preview") {
      const policy = evaluatePeerCapabilityPreview(item.event, {
        roomRef: session.roomId,
        sourceDeviceRef: session.peerSessionRef,
        targetPeerRef: session.localSessionRef,
        session: peerConsentSession,
      });
      if (policy.status === "rejected") {
        const invalid = markControlQueueItemStatus(state, item.queueId, "invalid", {
          reason: policy.errors.join(" ").slice(0, 512),
        });
        setQueue(invalid.state);
        setPeerReview(null);
        setMessages([`Peer PolicyGate rejected preview: ${policy.errors.join(" ").slice(0, 512)}`]);
        return;
      }
      const awaiting = markControlQueueItemStatus(state, item.queueId, "awaiting_peer_decision", {
        reason: "Receiver PolicyGate accepted this exact preview for explicit one-time review.",
      });
      if (!awaiting.ok) {
        setMessages(awaiting.errors);
        return;
      }
      setQueue(awaiting.state);
      const previewLabel = capabilityLabel(item.event.payload.request.capability);
      setPeerReview({
        queueId: item.queueId,
        event: item.event,
        binding: policy.binding,
        status: "awaiting_peer_decision",
      });
      setMessages([
        `Peer PolicyGate accepted this exact ${previewLabel} preview for review.`,
        "Allow once or Deny requires an explicit receiver action. No capability was executed.",
      ]);
      logAgentBridgeLifecycle({ eventKind: "peer_review_started", roomRefShort: session.roomId, eventIdShort: item.event.eventId, requestIdShort: item.event.payload.request.requestId });
      return;
    }

    if (item.event.kind === "capability_execute_request") {
      const executionRequestEvent = item.event;
      const consent = peerConsentRecords.find(
        (record) => record.binding.consentId === executionRequestEvent.payload.consentId
      );
      const isHelloStdout = executionRequestEvent.payload.capability === "runtime.hello_stdout";
      const isFileCandidate = executionRequestEvent.payload.capability === "filesystem.find_file_candidates";
      const isCandidatePayload = executionRequestEvent.payload.capability === "transfer.request_candidate_payload";
      logAgentBridgeLifecycle({
        eventKind: "hello_peer_execution_started",
        roomRefShort: session.roomId,
        requestIdShort: executionRequestEvent.payload.requestId,
        executionIdShort: executionRequestEvent.payload.executionId,
      });
      const execution = isCandidatePayload
        ? await executeInboundCandidatePayloadRequest(
            executionRequestEvent,
            consent,
            consumptionState,
            resolveCandidatePayloadCapability,
            enqueueCandidatePayloadHandoff,
            {
              roomRef: session.roomId,
              sourceDeviceRef: session.peerSessionRef,
              targetPeerRef: session.localSessionRef,
            },
          )
        : isFileCandidate
        ? await executeInboundFileCandidateRequest(
            executionRequestEvent,
            consent,
            consumptionState,
            executeFileCandidateSearchCapability,
            {
              roomRef: session.roomId,
              sourceDeviceRef: session.peerSessionRef,
              targetPeerRef: session.localSessionRef,
            },
          )
        : isHelloStdout
          ? await executeInboundHelloStdoutRequest(
            executionRequestEvent,
            consent,
            consumptionState,
            executeHelloStdoutCapability,
            {
              roomRef: session.roomId,
              sourceDeviceRef: session.peerSessionRef,
              targetPeerRef: session.localSessionRef,
            },
          )
          : executeInboundHelloPeerRequest(
            executionRequestEvent,
            consent,
            consumptionState,
            {
              roomRef: session.roomId,
              sourceDeviceRef: session.peerSessionRef,
              targetPeerRef: session.localSessionRef,
            },
          );
      const succeeded = execution.result.status === "succeeded"
        || execution.result.status === "completed"
        || execution.result.status === "handoff_queued";
      const requestStatus = succeeded
        ? "execution_consumed"
        : execution.result.status === "already_consumed"
          ? "already_consumed"
          : execution.result.status === "failed"
            ? "execution_failed"
            : "execution_rejected";
      const completed = markControlQueueItemStatus(state, item.queueId, requestStatus, {
        reason: execution.result.status === "succeeded"
          || execution.result.status === "completed"
          ? isCandidatePayload
            ? "Exact one-time consent consumed. Candidate payload handoff queued."
            : isFileCandidate
            ? "Exact one-time consent consumed. File candidate search executed once."
            : isHelloStdout
            ? "Exact one-time consent consumed. Hello Stdout demo executed once."
            : "Exact one-time consent consumed. Hello Peer demo executed once."
          : `Execution request rejected: ${execution.result.errorCode ?? execution.result.status}.`,
      });
      if (!completed.ok) {
        setMessages(completed.errors);
        return;
      }
      const outboundResult = enqueueRoomControlEvent(
        completed.state,
        execution.resultEvent,
        "outbound",
      );
      if (!outboundResult.ok) {
        setMessages(outboundResult.errors);
        return;
      }
      setConsumptionState(execution.state);
      if (execution.state.consumedConsentIds.includes(item.event.payload.consentId)) {
        logAgentBridgeLifecycle({
          eventKind: "consent_consumed",
          roomRefShort: session.roomId,
          requestIdShort: item.event.payload.requestId,
          executionIdShort: item.event.payload.executionId,
          consentResult: "consumed",
        });
      }
      logAgentBridgeLifecycle({
        eventKind: succeeded ? "hello_peer_execution_succeeded" : "hello_peer_execution_rejected",
        roomRefShort: session.roomId,
        requestIdShort: item.event.payload.requestId,
        executionIdShort: item.event.payload.executionId,
        consentResult: execution.state.consumedConsentIds.includes(item.event.payload.consentId) ? "consumed" : "not_consumed",
        errorCode: execution.result.errorCode ?? undefined,
        executionResult: succeeded
          ? isCandidatePayload
            ? "candidate_payload_handoff_queued"
            : isFileCandidate ? undefined : isHelloStdout ? "hello_stdout_succeeded" : "hello_peer_template_succeeded"
          : undefined,
      });
      if (consent && execution.state.consumedConsentIds.includes(consent.binding.consentId)) {
        setPeerReview((current) =>
          current?.record?.binding.consentId === consent.binding.consentId
            ? { ...current, status: "consumed" }
            : current
        );
      }
      setQueue(outboundResult.state);
      setMessages([
        succeeded
          ? isFileCandidate
            ? `File candidate search completed with ${"candidates" in execution.result ? execution.result.candidates.length : 0} redacted candidate(s). Exact one-time consent was consumed.`
            : isCandidatePayload
            ? "Candidate payload handoff queued. Existing transfer pipeline owns progress and completion."
            : isHelloStdout
            ? "Hello Stdout demo executed once. Exact one-time consent was consumed."
            : "Hello Peer demo executed once. Exact one-time consent was consumed."
          : `Execution request rejected: ${execution.result.errorCode ?? execution.result.status}.`,
        "Bounded execution result queued for one explicit Process next action.",
      ]);
      return;
    }

    if (item.event.kind === "capability_execution_result") {
      const executionResultEvent = item.event;
      const requestItem = state.outbound.find(
        (candidate): candidate is ControlQueueItem & { event: CapabilityExecuteRequestRoomControlEvent } =>
          candidate.event.kind === "capability_execute_request"
          && matchExecutionResultToRequest(executionResultEvent, candidate.event)
      );
      const outboundStatus = executionResultEvent.payload.status === "succeeded"
        || executionResultEvent.payload.status === "handoff_queued"
        ? "execution_succeeded"
        : executionResultEvent.payload.status === "already_consumed"
          ? "already_consumed"
          : executionResultEvent.payload.status === "failed"
            ? "execution_failed"
            : "execution_rejected";
      const matched = requestItem
        ? markControlQueueItemStatus(state, requestItem.queueId, outboundStatus, {
            reason: executionResultEvent.payload.status === "succeeded"
              || executionResultEvent.payload.status === "handoff_queued"
              ? "Peer returned the bounded execution result."
              : `Peer returned ${executionResultEvent.payload.errorCode ?? executionResultEvent.payload.status}.`,
          })
        : { ok: false as const, state, errors: ["No matching outbound execution request was found."] };
      const inbound = markControlQueueItemStatus(
        matched.ok ? matched.state : state,
        item.queueId,
        matched.ok ? outboundStatus : "invalid",
        { reason: matched.ok ? "Matched bounded execution result." : matched.errors.join(" ") },
      );
      setQueue(inbound.state);
      if (matched.ok) {
        setLatestExecutionResult(executionResultEvent);
      }
      setMessages(matched.ok
        ? [executionResultEvent.payload.status === "succeeded"
            ? `Peer returned the fixed bounded result: ${"stdout" in executionResultEvent.payload ? executionResultEvent.payload.stdout : executionResultEvent.payload.output}`
            : executionResultEvent.payload.status === "handoff_queued"
            ? "Peer queued the approved candidate payload handoff. Transfer completion remains in the existing pipeline."
            : `Peer execution result: ${executionResultEvent.payload.errorCode ?? executionResultEvent.payload.status}.`]
        : matched.errors);
      return;
    }

    const applied = applyInboundPeerStatusToOutboundQueue(state, item.event);
    const inboundStatus = item.event.payload.status;
    const inboundTransition = markControlQueueItemStatus(
      applied.ok ? applied.state : state,
      item.queueId,
      applied.ok ? inboundStatus : "invalid",
      {
        reason: applied.ok
          ? item.event.kind === "capability_preview_ack"
            ? "Peer allowed this exact preview once. No execution has occurred."
            : item.event.kind === "capability_preview_deny"
              ? "Peer denied the preview. No retry will be attempted."
              : `Peer returned ${inboundStatus}. No execution occurred.`
          : applied.errors.join(" ").slice(0, 512),
      },
    );
    setQueue(inboundTransition.state);
    if (applied.ok && item.event.kind === "capability_preview_ack" && item.event.payload.consent) {
      setSenderExecutionAck(item.event);
    }
    setMessages(applied.ok
      ? [
          item.event.kind === "capability_preview_ack"
            ? "Peer allowed this exact preview once. No execution has occurred."
            : item.event.kind === "capability_preview_deny"
              ? "Peer denied the preview. No retry will be attempted."
              : `Peer returned ${inboundStatus}. No execution occurred.`,
        ]
      : applied.errors);
  }

  async function enqueueCandidatePayloadHandoff(
    request: CandidatePayloadExecutionRequest,
    resolution: CandidatePayloadLocalResolution,
  ): Promise<CandidatePayloadHandoffResult> {
    if (!session || request.capability !== "transfer.request_candidate_payload") {
      return { queued: false, errorCode: "unsupported_route" };
    }
    if (!onEnqueueCandidatePayloadHandoff || !resolution.receiverLocalSource) {
      return { queued: false, errorCode: "handoff_failed" };
    }
    const modifiedMs = typeof resolution.modifiedAt === "string"
      ? Date.parse(resolution.modifiedAt)
      : Number.NaN;
    if (
      resolution.candidateKind !== "filesystem_file" ||
      typeof resolution.sizeBytes !== "number" ||
      !Number.isFinite(modifiedMs)
    ) {
      return { queued: false, errorCode: "handoff_failed" };
    }
    try {
      const targetPeerSessionId = session.peerRouteRef ?? session.peerSessionRef;
      const queued = onEnqueueCandidatePayloadHandoff({
        path: resolution.receiverLocalSource,
        displayName: resolution.displayName ?? request.candidateDisplayName,
        mimeType: "application/octet-stream",
        sizeBytes: resolution.sizeBytes,
        modifiedMs,
        dedupeKey: [
          "agent-bridge-candidate-payload",
          request.sourceRequestId,
          request.candidateId,
          request.candidateKind,
          resolution.sizeBytes,
          modifiedMs,
        ].join(":"),
        bridgeRoute: {
          bridgeSessionId: `legacy-room:${session.roomId}`,
          target: {
            kind: "selected_peer",
            peerSessionId: bridgePeerSessionId(targetPeerSessionId),
          },
        },
        bridgeOperationId: `agent-bridge-candidate:${request.requestId}:${request.executionId}`,
        bridgeTargetKind: "selected_peer",
        bridgeContentKind: "file",
        targetPeerSessionId,
        targetPeerDisplayName: room.peer_device_name ?? "selected peer",
        targetCount: 1,
        agentBridgeMetadata: {
          origin: "agent_bridge_candidate_payload",
          label: "Agent Bridge candidate payload request",
          note: "Queued from approved candidate payload request.",
          sourceCapability: "filesystem.find_file_candidates",
          requestCapability: "transfer.request_candidate_payload",
          sourceRequestId: request.sourceRequestId,
          candidateId: request.candidateId,
          candidateKind: "filesystem_file",
          candidateDisplayName: resolution.displayName ?? request.candidateDisplayName,
          requestedByPeerRef: request.sourceDeviceRef,
          approvedByPeerRef: request.targetPeerRef,
          consentId: request.consentId,
          agentBridgeRequestId: request.requestId,
          handoffCreatedAt: new Date().toISOString(),
          sizeBytes: resolution.sizeBytes,
          extension: resolution.extension,
          mimeFamily: resolution.mimeFamily,
        },
      });
      return queued ? { queued: true } : { queued: false, errorCode: "handoff_failed" };
    } catch {
      return { queued: false, errorCode: "unsupported_route" };
    }
  }

  function decidePeerPreview(decision: "allow_once" | "deny") {
    if (!peerReview || peerReview.status !== "awaiting_peer_decision") {
      setMessages(["No reviewable peer preview is awaiting a decision."]);
      return;
    }
    const now = new Date();
    if (Date.parse(peerReview.binding.expiresAt) <= now.getTime()) {
      const expired = markControlQueueItemStatus(queue, peerReview.queueId, "expired", {
        now,
        reason: "Peer decision window expired. No approval was created.",
      });
      setQueue(expired.state);
      setPeerReview({ ...peerReview, status: "expired" });
      setMessages(["Peer preview expired before a decision. No approval was created."]);
      return;
    }
    const decisionResult = decision === "allow_once"
      ? allowPeerCapabilityOnce(peerReview.binding, peerConsentSession, { now })
      : denyPeerCapability(peerReview.binding, peerConsentSession, { now });
    if (!decisionResult.ok) {
      setMessages(decisionResult.errors);
      return;
    }
    const statusEvent = buildPeerConsentStatusEvent(peerReview.event, decisionResult.record, { now });
    if (!statusEvent.ok) {
      setMessages(statusEvent.errors);
      return;
    }
    const reviewed = markControlQueueItemStatus(
      queue,
      peerReview.queueId,
      decision === "allow_once" ? "allowed_once" : "denied",
      {
        now,
        reason: decision === "allow_once"
          ? "Allowed once for this exact request. Waiting for an explicit execution request."
          : "Denied by receiver. No retry or escalation was created.",
      },
    );
    if (!reviewed.ok) {
      setMessages(reviewed.errors);
      return;
    }
    const outbound = enqueueRoomControlEvent(reviewed.state, statusEvent.event, "outbound", { now });
    if (!outbound.ok) {
      setMessages(outbound.errors);
      return;
    }
    setPeerConsentSession(decisionResult.state);
    setPeerConsentRecords((records) => [...records, decisionResult.record]);
    setPeerReview({
      ...peerReview,
      status: decision === "allow_once" ? "allowed_once" : "denied",
      record: decisionResult.record,
    });
    setQueue(outbound.state);
    logAgentBridgeLifecycle({
      eventKind: decision === "allow_once" ? "peer_allowed_once" : "peer_denied",
      roomRefShort: session?.roomId ?? room.id,
      eventIdShort: peerReview.event.eventId,
      requestIdShort: peerReview.binding.requestId,
      consentResult: decision,
    });
    setMessages([
      decision === "allow_once"
        ? "Allowed once. Waiting for the peer's explicit execution request."
        : "Denied.",
      `${statusEvent.event.kind} queued for one explicit Process next action. No execution occurred.`,
    ]);
  }

  function requestHelloPeerExecution() {
    if (!senderExecutionAck) {
      setMessages(["A matched unexpired Allow once acknowledgement is required."]);
      return;
    }
    const preview = queue.outbound.find(
      (item): item is ControlQueueItem & { event: CapabilityPreviewRoomControlEvent } =>
        item.event.kind === "capability_preview"
        && item.event.eventId === senderExecutionAck.payload.consent?.sourcePreviewEventId
    );
    if (!preview) {
      setMessages(["The exact source preview for this Allow once acknowledgement is unavailable."]);
      return;
    }
    const built = preview.event.payload.request.capability === "transfer.request_candidate_payload"
      ? buildCandidatePayloadExecutionRequest(preview.event, senderExecutionAck)
      : preview.event.payload.request.capability === "filesystem.find_file_candidates"
        ? buildFileCandidateExecutionRequest(preview.event, senderExecutionAck)
        : preview.event.payload.request.capability === "runtime.hello_stdout"
          ? buildHelloStdoutExecutionRequest(preview.event, senderExecutionAck)
          : buildHelloPeerExecutionRequest(preview.event, senderExecutionAck);
    if (!built.ok) {
      setMessages(built.errors);
      return;
    }
    const queued = enqueueRoomControlEvent(queue, built.event, "outbound");
    if (!queued.ok) {
      setMessages(queued.errors);
      return;
    }
    setQueue(queued.state);
    setSenderExecutionAck(null);
    setMessages([
      `${capabilityLabel(preview.event.payload.request.capability)} execution request queued. Nothing executes until Process next sends it and the receiver PolicyGate accepts it.`,
    ]);
    logAgentBridgeLifecycle({
      eventKind: "execution_request_queued",
      roomRefShort: session?.roomId ?? room.id,
      requestIdShort: built.request.requestId,
      executionIdShort: built.request.executionId,
    });
  }

  async function resendCurrentEventForReplayTest() {
    if (!session || !transportEvent) {
      setMessages(["Build an outbound capability preview and select an active room session first."]);
      return;
    }
    await sendCurrentRoomControlEvent(
      transportEvent,
      (event) => sendWithRuntimeReservation(session.roomId, event),
      setSendState
    );
    setMessages([
      `Developer replay test resent current event ${transportEvent.eventId}.`,
      "This direct replay-test action does not create or update a queue item.",
    ]);
  }

  async function sendWithRuntimeReservation(roomId: string, event: RoomControlEvent) {
    if (!session) {
      throw new Error("Agent Bridge capability send requires an active selected-peer Bridge session.");
    }
    if (roomId !== session.roomId) {
      throw new Error("Agent Bridge capability send requires the active Bridge session.");
    }
    const route = assertCapabilityEventHasSelectedPeerRoute(session, event);
    setOutgoingControlWindowDemand(ROOM_CONTROL_ACTIVE_SEND_SOURCE, true);
    try {
      const applied = await waitForRuntimeDataWindowTarget(7);
      if (!applied) {
        throw new Error("Runtime data window target 7 was not applied before control delivery.");
      }
      return await sendRoomControlEvent(
        roomId,
        event,
        bridgeRoutePayload(route, "pastey-bridge-control-route-v1"),
      );
    } finally {
      setOutgoingControlWindowDemand(ROOM_CONTROL_ACTIVE_SEND_SOURCE, false);
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
      const integrated = enqueueInboundRoomControlEvents(
        queue,
        events.map((event) => event.event),
        {
          expectedRoomRef: session.roomId,
          expectedSourceDeviceRef: session.peerSessionRef,
          expectedTargetPeerRef: session.localSessionRef,
        }
      );
      setQueue(integrated.state);
      setMessages([
        `Loaded ${events.length} current-session control inbox event(s); queued ${integrated.added.length} new inbound event(s).`,
        ...integrated.diagnostics,
      ]);
    } catch (error) {
      setMessages([error instanceof Error ? error.message : String(error)]);
    } finally {
      setTransportBusy(false);
    }
  }

  return (
    <section className="agent-bridge-section" aria-labelledby="agent-bridge-room-control-title">
      <div className="agent-bridge-section-header">
        <div>
          <strong id="agent-bridge-room-control-title">Room control</strong>
          <p className="muted">Preview-only transport and current-session queue.</p>
        </div>
        <span className="ai-slot-pending-status">{session ? "active session" : "no session"}</span>
      </div>
      <div className="agent-bridge-room-toolbar">
        <span className="agent-bridge-compact-ref" title={room.id}>
          Room: {shortRef(room.id)}
        </span>
        <span className="agent-bridge-compact-ref" title={room.peer_device_name ?? "No peer"}>
          Peer: {room.peer_device_name ?? "None"}
        </span>
        <button className="secondary-button" onClick={() => void refreshRoomSession()}>Refresh</button>
        {session ? (
          <>
            <button
              className="secondary-button"
              data-testid="agent-bridge-refresh-inbox"
              disabled={transportBusy}
              onClick={() => void refreshReceivedInbox()}
            >
              Refresh inbox
            </button>
            {transportEvent ? (
              <button
                className="secondary-button"
                data-testid="agent-bridge-queue-preview"
                disabled={transportBusy}
                onClick={enqueueOutboundPreview}
              >
                Queue preview
              </button>
            ) : null}
            <button
              className="secondary-button"
              data-testid="agent-bridge-process-next"
              disabled={transportBusy}
              onClick={() => void processNextQueueItem()}
            >
              Process next
            </button>
          </>
        ) : null}
      </div>
      <div className="agent-bridge-status-row" data-testid="agent-bridge-queue-summary">
        <strong>Inbound {queue.inbound.length} · Outbound {queue.outbound.length}</strong>
        <span>
          Runtime data window target: {runtimeWindowStatus.targetDataWindows} / 8
        </span>
        <span className="muted">
          Reason: {
            runtimeWindowStatus.reason === "outgoing_control_demand"
              ? "outgoing control demand"
              : runtimeWindowStatus.reason === "restore_quiet_period"
                ? "restore quiet period"
                : "idle"
          }
        </span>
      </div>
      <div className="agent-bridge-status-row" data-testid="agent-bridge-runtime-window-status">
        <span className="agent-bridge-status-label">Runtime scheduler reservation</span>
        <strong>
          {runtimeWindowStatus.reservationReady
            ? runtimeWindowStatus.targetDataWindows === 7 ? "Active" : "Idle"
            : "Unavailable"}
        </strong>
        <span>{runtimeWindowStatus.activeAllocationUpdates}</span>
        {runtimeWindowStatus.lastError ? <span>{runtimeWindowStatus.lastError}</span> : null}
      </div>
      <LatestRoomControlSend state={sendState} />
      {messages[0] ? (
        <div className="agent-bridge-status-row" role="status">
          <span className="agent-bridge-status-label">Queue status</span>
          <span>{messages[0]}</span>
        </div>
      ) : null}
      <div className="agent-bridge-next-item">
        <span className="agent-bridge-status-label">Next actionable</span>
        <strong>{nextActionable?.event.kind ?? "None"}</strong>
        <span className="muted">
          {nextActionable
            ? `${nextActionable.direction} · ${nextActionable.status} · event ${shortRef(nextActionable.event.eventId)}`
            : "Refresh the inbox or queue a preview."}
        </span>
      </div>
      {peerReview ? (
        <PeerConsentReviewCard
          review={peerReview}
          nowMs={controlDemandNowMs}
          onAllowOnce={() => decidePeerPreview("allow_once")}
          onDeny={() => decidePeerPreview("deny")}
        />
      ) : null}
      {senderExecutionAck?.payload.consent
        && Date.parse(senderExecutionAck.payload.consent.expiresAt) > controlDemandNowMs ? (
        <div className="agent-bridge-next-item" data-testid="agent-bridge-execution-request-card">
          <span className="agent-bridge-status-label">Peer allowed once</span>
          <strong>Exact {capabilityLabel(senderExecutionAck.payload.consent.capability)} request approved</strong>
          <span className="muted">No execution has occurred. This action is explicit and can be used once.</span>
          <button
            className="primary-button"
            data-testid="agent-bridge-request-hello-execution"
            onClick={requestHelloPeerExecution}
          >
            {senderExecutionAck.payload.consent.capability === "filesystem.find_file_candidates"
              ? "Request file candidate search"
              : senderExecutionAck.payload.consent.capability === "transfer.request_candidate_payload"
                ? "Request candidate payload scaffold"
                : senderExecutionAck.payload.consent.capability === "runtime.hello_stdout"
                  ? "Request Hello Stdout execution"
                  : "Request Hello Peer execution"}
          </button>
        </div>
      ) : null}
      {latestExecutionResult ? (
        <div className="agent-bridge-next-item" data-testid="agent-bridge-execution-result-card">
          <span className="agent-bridge-status-label">Bounded execution result</span>
          <strong>{latestExecutionResult.payload.status}</strong>
          {"stdout" in latestExecutionResult.payload ? <span>stdout: {latestExecutionResult.payload.stdout}</span> : null}
          {"stderr" in latestExecutionResult.payload && latestExecutionResult.payload.stderr ? <span>stderr: {latestExecutionResult.payload.stderr}</span> : null}
          {"exitCode" in latestExecutionResult.payload ? <span>exit {latestExecutionResult.payload.exitCode}</span> : null}
          {"durationMs" in latestExecutionResult.payload ? <span>{latestExecutionResult.payload.durationMs}ms</span> : null}
          {"output" in latestExecutionResult.payload && latestExecutionResult.payload.output ? <span>{latestExecutionResult.payload.output}</span> : null}
          {"candidates" in latestExecutionResult.payload ? (
            <ul>
              {latestExecutionResult.payload.candidates.map((candidate) => (
                <li key={candidate.candidateId}>
                  {candidate.displayName} / {candidate.redactedLocation} / {candidate.sizeBytes} bytes / {candidate.matchReason} / {candidate.confidence}
                </li>
              ))}
            </ul>
          ) : null}
          {latestExecutionResult.payload.errorCode ? <span>{latestExecutionResult.payload.errorCode}</span> : null}
        </div>
      ) : null}
      <details className="agent-bridge-room-details">
        <summary>Room control diagnostics</summary>
        <div className="agent-bridge-advanced-content">
          <section>
            <strong>Real room-control transport</strong>
            {session ? (
              <div className="agent-bridge-definition-list">
                <FullValue label="Room" value={session.roomId} />
                <FullValue label="Local session" value={session.localSessionRef} />
                <FullValue label="Peer session" value={session.peerSessionRef} />
                <FullValue label="Current event" value={transportEvent?.eventId ?? "Not built"} />
              </div>
            ) : <p className="muted">No active room-control session.</p>}
            <div className="benchmark-controls">
              <button
                className="secondary-button"
                disabled={transportBusy || sendState.status === "sending" || !transportEvent}
                onClick={() => void resendCurrentEventForReplayTest()}
              >
                Developer resend current event for replay test
              </button>
              <button
                className="secondary-button"
                disabled={sendState.status === "idle" || sendState.status === "sending"}
                onClick={() => setSendState(createIdleRoomControlSendState())}
              >
                Clear latest send result
              </button>
            </div>
            <ReceivedControlInbox events={receivedEvents} />
          </section>
          <section>
            <strong>Local simulation only</strong>
            <div className="benchmark-controls">
              <button className="secondary-button" onClick={enqueueOutboundPreviewLocally}>Enqueue outbound preview locally</button>
              <button className="secondary-button" onClick={selectNextLocally}>Select next locally</button>
              <button className="secondary-button" onClick={() => simulateInboundStatus("acknowledged_preview_only")}>Simulate inbound ack</button>
              <button className="secondary-button" onClick={() => simulateInboundStatus("denied")}>Simulate inbound deny</button>
              <button className="secondary-button" onClick={() => simulateInboundStatus("invalid")}>Simulate inbound invalid</button>
              <button className="secondary-button" onClick={() => simulateInboundStatus("expired")}>Simulate inbound expired</button>
            </div>
          </section>
          <ControlQueueList title="Outbound queue" items={queue.outbound} />
          <ControlQueueList title="Inbound queue" items={queue.inbound} />
          <Messages title="Queue messages" messages={messages} emptyMessage="No duplicate, replay, or expiry messages." />
        </div>
      </details>
    </section>
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
    <div className="agent-bridge-status-row" data-testid="agent-bridge-latest-send">
      <span className="agent-bridge-status-label">Latest send</span>
      <strong>{state.status === "accepted" ? "Delivered" : capitalize(state.status)}</strong>
      {state.status !== "idle" ? (
        <span className="agent-bridge-compact-ref" title={state.eventId}>
          event {shortRef(state.eventId)}
        </span>
      ) : null}
      {state.status === "rejected" ? <span>{state.errorCode}</span> : null}
      {timestamp ? <time dateTime={timestamp}>{new Date(timestamp).toLocaleTimeString()}</time> : null}
      <span className="muted">{summary}</span>
    </div>
  );
}

function PeerConsentReviewCard({
  review,
  nowMs,
  onAllowOnce,
  onDeny,
}: {
  review: PeerReviewState;
  nowMs: number;
  onAllowOnce: () => void;
  onDeny: () => void;
}) {
  const expired = review.status === "expired" || Date.parse(review.binding.expiresAt) <= nowMs;
  const isHelloStdout = review.binding.capability === "runtime.hello_stdout";
  const isFileCandidate = review.binding.capability === "filesystem.find_file_candidates";
  const isCandidatePayload = review.binding.capability === "transfer.request_candidate_payload";
  const capabilityDetail = "filenameHint" in review.binding
    ? `Filename hint: ${review.binding.filenameHint}; search mode: ${review.binding.searchMode}`
    : "candidateId" in review.binding
    ? `Candidate: ${review.binding.candidateDisplayName}; source request: ${shortRef(review.binding.sourceRequestId)}`
    : "expectedStdout" in review.binding
    ? `Expected stdout: ${review.binding.expectedStdout}`
    : `Exact message: ${review.binding.exactMessage}`;
  return (
    <div className="agent-bridge-next-item" data-testid="agent-bridge-peer-consent-review">
      <span className="agent-bridge-status-label">Peer PolicyGate review</span>
      <strong>{capabilityLabel(review.binding.capability)}</strong>
      <span className="agent-bridge-compact-ref" title={review.binding.sourceDeviceRef}>
        Source: {shortRef(review.binding.sourceDeviceRef)}
      </span>
      <span>{capabilityDetail}</span>
      <time dateTime={review.binding.expiresAt}>
        Expires {new Date(review.binding.expiresAt).toLocaleTimeString()}
      </time>
      <span className="muted">
        {isFileCandidate
          ? "Allow once applies only to this exact bounded filename/metadata search. No file contents are read and no file is sent automatically."
          : isCandidatePayload
            ? "Allow once applies only to this exact candidate payload request. No bytes are sent unless the approved request enters the transfer queue."
          : "Allow once applies only to this exact request and does not execute it yet."}
      </span>
      {review.status === "awaiting_peer_decision" && !expired ? (
        <div className="benchmark-controls">
          <button
            className="primary-button"
            data-testid="agent-bridge-allow-once"
            onClick={onAllowOnce}
          >
            Allow once
          </button>
          <button
            className="secondary-button"
            data-testid="agent-bridge-deny-peer-preview"
            onClick={onDeny}
          >
            Deny
          </button>
        </div>
      ) : (
        <strong>
          {review.status === "allowed_once"
            ? "Allowed once. Waiting for the peer's explicit execution request."
            : review.status === "consumed"
              ? isHelloStdout
                ? "One-time consent consumed. Hello Stdout demo executed once."
                : isFileCandidate
                  ? "One-time consent consumed. File candidate search executed once."
                  : isCandidatePayload
                    ? "One-time consent consumed for this candidate payload request."
                : "One-time consent consumed. Hello Peer demo executed once."
            : review.status === "denied"
              ? "Denied."
              : review.status === "expired"
                ? "Expired. No approval exists."
                : "Invalid. No approval exists."}
        </strong>
      )}
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
      ) : <p className="muted">No received current-session control events.</p>}
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
              {item.event.kind} / {item.status} / event {item.event.eventId} / priority {item.priority} / {item.queueId}
              {item.reason ? ` / ${item.reason}` : ""}
              {item.transportResultCode ? ` / transport ${item.transportResultCode}` : ""}
              {item.transportReceivedAt ? ` / received ${item.transportReceivedAt}` : ""}
              {item.direction === "inbound" && item.event.kind === "capability_preview"
                ? ` / request ${item.event.payload.request.requestId} / source ${item.event.sourceDeviceRef} / target ${item.event.targetPeerRef} / created ${item.event.createdAt} / expires ${item.event.expiresAt} / capability ${item.event.payload.request.capability} / previewOnly ${String(item.event.previewOnly)}`
                : ""}
            </li>
          ))}
        </ul>
      ) : <p className="muted">Empty.</p>}
    </div>
  );
}

function FullValue({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <strong>{label}</strong>
      <span title={value}>{value}</span>
    </div>
  );
}

function Messages({
  title,
  messages,
  emptyMessage,
}: {
  title: string;
  messages: string[];
  emptyMessage: string;
}) {
  return (
    <div className="ai-slot-preview-messages">
      <strong>{title}</strong>
      {messages.length > 0
        ? <ul>{messages.map((message) => <li key={message}>{message}</li>)}</ul>
        : <p className="muted">{emptyMessage}</p>}
    </div>
  );
}

function shortRef(value: string): string {
  if (!value) return "None";
  if (value.length <= 16) return value;
  return `${value.slice(0, 7)}…${value.slice(-7)}`;
}

function capabilityLabel(capability: string): string {
  if (capability === "filesystem.find_file_candidates") {
    return "File candidate search";
  }
  if (capability === "transfer.request_candidate_payload") {
    return "Candidate payload request";
  }
  if (capability === "runtime.hello_stdout") {
    return "Hello Stdout";
  }
  return "Hello Peer";
}

function capitalize(value: string): string {
  return value.length > 0 ? `${value[0]?.toUpperCase()}${value.slice(1)}` : value;
}

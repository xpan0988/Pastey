use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, ConnectInfo, Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{
    crypto,
    error::{AppError, AppResult},
    host_runtime::HostRuntime as AppState,
    logging,
    models::{BridgePeerLiveness, RoomStatus, StoredBridgePeerEndpoint},
    storage,
    transfer::RoomServerContext,
};

pub const MAX_CONTROL_REQUEST_BYTES: usize = 96 * 1024;
const MAX_CONTROL_EVENT_BYTES: usize = 64 * 1024;
const MAX_CONTROL_RESPONSE_BYTES: usize = 4 * 1024;
const MAX_EVENT_LIFETIME_SECONDS: i64 = 120;
/// Received Bridge Plan (v1) events kept for display: Find File reads its
/// progress from them. Observation only, since the Bridge Plan store holds
/// their state of record: the oldest leave first, and none is ever refused
/// for lack of room.
const MAX_HISTORY_ITEMS: usize = 64;
const MAX_HISTORY_AGE_SECONDS: i64 = 30 * 60;
const MAX_REPLAY_ITEMS: usize = 256;
/// Further attempts to deliver one unchanged native task status.
const MAX_STATUS_RESENDS: u32 = 5;
/// How long one unchanged native task status is offered again while the
/// requester refuses it for its rate budget (which proves it never arrived).
const MAX_STATUS_BACKPRESSURE: Duration = Duration::from_secs(120);

/// Whether one native task status needs no further delivery attempt: it was
/// delivered, or it failed `MAX_STATUS_RESENDS` more times (the requester
/// may already hold it, which its replay protection then reports as a
/// failure, or the session is gone and reconciliation takes over).
fn native_status_settled(delivered: bool, failed_sends: u32) -> bool {
    delivered || failed_sends >= MAX_STATUS_RESENDS
}

/// What a received Room Control event is for. Each class has its own rate
/// budget, so one kind of traffic (a polling UI's capability checks, say)
/// can never use up what another (a running task's statuses) needs.
/// Developer Terminal traffic keeps its own separate flow control.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlClassV1 {
    /// `peer_capability.query`: answered at once, never kept.
    CapabilityQuery,
    /// `peer_capability.response`: recorded at once, never kept. Its own
    /// budget, so a burst of this Host's own checks cannot make it refuse
    /// their answers.
    CapabilityAnswer,
    /// `native_agent.*`: invocations, statuses, decisions, session queries,
    /// identity. Handled at once; their state of record is Core's.
    NativeAgent,
    /// `bridge_plan.*`, including protocol v2. Their state of record is the
    /// Bridge Plan stores; v1 events are also kept as display history.
    BridgePlan,
    /// `bridge_membership.*`.
    Membership,
}

impl ControlClassV1 {
    pub(crate) const ALL: [Self; 5] = [
        Self::CapabilityQuery,
        Self::CapabilityAnswer,
        Self::NativeAgent,
        Self::BridgePlan,
        Self::Membership,
    ];

    pub(crate) fn of(kind: &str) -> Self {
        if kind == "peer_capability.response" {
            Self::CapabilityAnswer
        } else if kind.starts_with("peer_capability.") {
            Self::CapabilityQuery
        } else if kind.starts_with("native_agent.") {
            Self::NativeAgent
        } else if kind.starts_with("bridge_membership.") {
            Self::Membership
        } else {
            Self::BridgePlan
        }
    }

    /// Events accepted per rolling minute, and within any 2 seconds.
    fn budget(self) -> (usize, usize) {
        match self {
            // A UI checks every few seconds, and every run checks first.
            Self::CapabilityQuery => (60, 12),
            // Answers to this Host's own queries: no more than the other
            // Host admitted at its query budget, but they arrive with
            // jitter, so twice that, and this Host never refuses answers
            // to its own checks.
            Self::CapabilityAnswer => (120, 24),
            // One continuation is identity, invocation, a status per change
            // (Queued, Running, each native request, the outcome) and the
            // decisions; session queries come in pairs with identity.
            Self::NativeAgent => (90, 20),
            Self::BridgePlan | Self::Membership => (30, 8),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::CapabilityQuery => "capability_query",
            Self::CapabilityAnswer => "capability_answer",
            Self::NativeAgent => "native_agent",
            Self::BridgePlan => "bridge_plan",
            Self::Membership => "membership",
        }
    }
}
const CONTROL_CONTENT_TYPE: &str = "application/vnd.pastey.room-control-envelope+json";
const CONTROL_RECEIPT_CONTENT_TYPE: &str = "application/vnd.pastey.room-control-receipt+json";
const CONTROL_ERROR_CONTENT_TYPE: &str = "application/vnd.pastey.room-control-error+json";
const CONTROL_TRANSPORT_SCHEMA: &str = "pastey-room-control-transport-v1";
const CONTROL_RECEIPT_ENVELOPE_SCHEMA: &str = "pastey-room-control-receipt-envelope-v1";
const CONTROL_DELIVERY_SCHEMA: &str = "pastey-room-control-delivery-v1";
const ROOM_CONTROL_SCHEMA: &str = "pastey-room-control-event-v1";
const CONTROL_BRIDGE_ROUTE_SCHEMA_VERSION: &str = "pastey-bridge-control-route-v1";
const ALLOWED_EVENT_KINDS: &[&str] = &[
    "peer_capability.query",
    "peer_capability.response",
    "bridge_plan.review_request",
    "bridge_plan.attempt_start",
    "bridge_plan.transfer_start",
    "bridge_plan.search_selection",
    "bridge_plan.attempt_ack",
    "bridge_plan.step_progress",
    "bridge_plan.step_result",
    "bridge_plan.step_failed",
    "bridge_plan.cancel",
    "bridge_plan.v2.review_request",
    "bridge_plan.v2.attempt_start",
    "bridge_plan.v2.readiness_request",
    "bridge_plan.v2.readiness_result",
    "bridge_plan.v2.attempt_prepared",
    "bridge_plan.v2.attempt_commit",
    "bridge_plan.v2.step_result",
    "bridge_plan.v2.step_commit",
    "bridge_plan.v2.attempt_cancel",
    "developer_terminal.open_request",
    "developer_terminal.open_accepted",
    "developer_terminal.open_denied",
    "developer_terminal.input",
    "developer_terminal.output",
    "developer_terminal.resize",
    "developer_terminal.exit",
    "developer_terminal.close",
    "bridge_membership.departure",
    "bridge_membership.carrier_card",
    "native_agent.invoke",
    "native_agent.status",
    "native_agent.cancel",
    "native_agent.workspace_prepare",
    "native_agent.reconcile",
    "native_agent.reconciliation",
    "native_agent.retry_result_return",
    "native_agent.device_identity",
    "native_agent.decision",
    "native_agent.session_query",
    "native_agent.session_response",
];
const BRIDGE_PLAN_PROTOCOL_FAMILY: &str = "bridge_plan";
const PEER_CAPABILITY_PROTOCOL_FAMILY: &str = "peer_capability";
const DEVELOPER_TERMINAL_PROTOCOL_FAMILY: &str = "developer_terminal";
const BRIDGE_MEMBERSHIP_PROTOCOL_FAMILY: &str = "bridge_membership";
const NATIVE_AGENT_PROTOCOL_FAMILY: &str = "native_agent";
const BRIDGE_MEMBERSHIP_SCHEMA: &str = "pastey-bridge-membership-v1";
const MAX_TERMINAL_EVENTS_PER_MINUTE: usize = 3_000;
const MAX_TERMINAL_BURST_EVENTS: usize = 256;

/// Writes only fixed capability metadata and reason codes. In particular, it
/// never includes a transport key, endpoint, route binding, path, or grant.
pub(crate) fn log_peer_capability(stage: &str, available: Option<bool>, code: Option<&str>) {
    let mut line = format!("[pastey peer-capability] stage={stage}");
    if available.is_some() {
        line.push_str(" capability_fact=true");
        line.push_str(if available == Some(true) {
            " available=true"
        } else {
            " available=false"
        });
    }
    if let Some(code) = code {
        line.push_str(if available.is_some() {
            " reason="
        } else {
            " code="
        });
        line.push_str(code);
    }
    logging::write_transfer_line(&line);
}

fn peer_capability_rejection_stage(event: &Value) -> Option<&'static str> {
    match event.get("kind").and_then(Value::as_str) {
        Some("peer_capability.query") => Some("query_rejected"),
        Some("peer_capability.response") => Some("response_rejected"),
        _ => None,
    }
}

fn peer_capability_rejection_code(message: &str) -> &'static str {
    if message.contains("expired") {
        "expired"
    } else if message.contains("session mismatch") {
        "session_mismatch"
    } else if message.contains("too large") {
        "payload_too_large"
    } else if message.contains("Unsupported peer capability") {
        "unsupported_capability"
    } else {
        "invalid_schema"
    }
}

fn peer_capability_fact_code(fact: &crate::peer_capabilities::HostCapabilityFact) -> &'static str {
    match (fact.available, fact.unavailable_reason.as_deref()) {
        (true, None) => "available",
        (false, Some("platform_unsupported")) => "platform_unsupported",
        (false, Some("backend_unavailable")) => "backend_unavailable",
        (false, Some("capability_unavailable")) => "capability_unavailable",
        (false, Some("runtime_unavailable")) => "runtime_unavailable",
        (false, Some("not_configured")) => "not_configured",
        (false, Some("unknown")) => "unknown",
        (false, Some("plan_process_binding_required")) => "plan_process_binding_required",
        (false, Some("provider_unavailable")) => "provider_unavailable",
        (false, Some("execution_world_unavailable")) => "execution_world_unavailable",
        _ => "invalid_capability",
    }
}

fn log_peer_capability_projection(
    stage: &str,
    projection: &crate::peer_capabilities::PeerCapabilityProjection,
) {
    if projection.capabilities.is_empty() {
        log_peer_capability(stage, None, Some("empty_projection"));
        return;
    }
    for fact in &projection.capabilities {
        log_peer_capability(
            stage,
            Some(fact.available),
            Some(peer_capability_fact_code(fact)),
        );
    }
}

fn local_peer_capability_response_projection(
    state: &AppState,
    peer_session_id: &str,
    observed_at: i64,
    capability_ids: &[String],
) -> AppResult<crate::peer_capabilities::PeerCapabilityProjection> {
    let projection = crate::peer_capabilities::local_diagnostic_projection_with_system_probes(
        state,
        peer_session_id.into(),
        observed_at,
        capability_ids,
    );
    projection
}

fn requested_peer_capability_ids(
    payload: &serde_json::Map<String, Value>,
) -> AppResult<Vec<String>> {
    let capability_ids = serde_json::from_value::<Vec<String>>(
        payload.get("capabilityIds").cloned().unwrap_or(Value::Null),
    )
    .map_err(|_| AppError::InvalidInput("Invalid peer capability query.".into()))?;
    crate::peer_capabilities::normalize_system_probe_request(&capability_ids)
}

pub(crate) fn peer_capability_event(
    kind: &str,
    payload: Value,
    context: &RoomControlSessionContext,
) -> AppResult<Value> {
    let now = OffsetDateTime::now_utc();
    Ok(serde_json::json!({
        "schemaVersion": ROOM_CONTROL_SCHEMA,
        "eventId": format!("peer-capability-event-{}", uuid::Uuid::new_v4()),
        "kind": kind,
        "protocolFamily": PEER_CAPABILITY_PROTOCOL_FAMILY,
        "roomRef": context.room_id,
        "sourceDeviceRef": context.local_session_ref,
        "targetPeerRef": context.peer_session_ref,
        "createdAt": now.format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid capability event time.".into()))?,
        "expiresAt": (now + time::Duration::seconds(MAX_EVENT_LIFETIME_SECONDS)).format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid capability event time.".into()))?,
        "previewOnly": false,
        "payload": payload,
    }))
}

/// Native Agent messages use the authenticated Room Control transport. The
/// payload is lifecycle-only: it never carries native credentials, executable
/// paths, model choice, session IDs, tool calls, or sandbox configuration.
pub(crate) fn native_agent_event(
    kind: &str,
    payload: Value,
    context: &RoomControlSessionContext,
) -> AppResult<Value> {
    let now = OffsetDateTime::now_utc();
    Ok(serde_json::json!({
        "schemaVersion": ROOM_CONTROL_SCHEMA,
        "eventId": format!("native-agent-event-{}", uuid::Uuid::new_v4()),
        "kind": kind,
        "protocolFamily": NATIVE_AGENT_PROTOCOL_FAMILY,
        "roomRef": context.room_id,
        "sourceDeviceRef": context.local_session_ref,
        "targetPeerRef": context.peer_session_ref,
        "createdAt": now.format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid native Agent event time.".into()))?,
        "expiresAt": (now + time::Duration::seconds(MAX_EVENT_LIFETIME_SECONDS)).format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid native Agent event time.".into()))?,
        "previewOnly": false,
        "payload": payload,
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BridgeDepartureKind {
    Burn,
}

impl BridgeDepartureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Burn => "burn",
        }
    }
}

pub(crate) fn bridge_departure_event(
    kind: BridgeDepartureKind,
    context: &RoomControlSessionContext,
) -> AppResult<Value> {
    let now = OffsetDateTime::now_utc();
    Ok(serde_json::json!({
        "schemaVersion": ROOM_CONTROL_SCHEMA,
        "eventId": format!("bridge-membership-event-{}", uuid::Uuid::new_v4()),
        "kind": "bridge_membership.departure",
        "protocolFamily": BRIDGE_MEMBERSHIP_PROTOCOL_FAMILY,
        "roomRef": context.room_id,
        "sourceDeviceRef": context.local_session_ref,
        "targetPeerRef": context.peer_session_ref,
        "createdAt": now.format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid Bridge departure event time.".into()))?,
        "expiresAt": (now + time::Duration::seconds(MAX_EVENT_LIFETIME_SECONDS)).format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid Bridge departure event time.".into()))?,
        "previewOnly": false,
        "payload": {
            "schemaVersion": BRIDGE_MEMBERSHIP_SCHEMA,
            "departureKind": kind.as_str(),
        },
    }))
}

/// This device's internet carrier card, for one Bridge peer: how to reach
/// this device across networks. It grants nothing; the peer pins it only
/// because it arrived over this authenticated session (`connectivity`).
pub(crate) fn bridge_carrier_card_event(
    card: &crate::connectivity::CarrierCardV1,
    context: &RoomControlSessionContext,
) -> AppResult<Value> {
    let now = OffsetDateTime::now_utc();
    let invalid = || AppError::InvalidInput("Invalid carrier card event time.".into());
    Ok(serde_json::json!({
        "schemaVersion": ROOM_CONTROL_SCHEMA,
        "eventId": format!("bridge-membership-event-{}", uuid::Uuid::new_v4()),
        "kind": "bridge_membership.carrier_card",
        "protocolFamily": BRIDGE_MEMBERSHIP_PROTOCOL_FAMILY,
        "roomRef": context.room_id,
        "sourceDeviceRef": context.local_session_ref,
        "targetPeerRef": context.peer_session_ref,
        "createdAt": now.format(&Rfc3339).map_err(|_| invalid())?,
        "expiresAt": (now + time::Duration::seconds(MAX_EVENT_LIFETIME_SECONDS)).format(&Rfc3339).map_err(|_| invalid())?,
        "previewOnly": false,
        "payload": {
            "schemaVersion": BRIDGE_MEMBERSHIP_SCHEMA,
            "card": card,
        },
    }))
}

fn selected_peer_control_route(room_id: &str, peer_session_id: &str) -> Value {
    serde_json::json!({
        "schemaVersion": CONTROL_BRIDGE_ROUTE_SCHEMA_VERSION,
        "bridgeSessionId": bridge_session_ref(room_id),
        "target": {
            "kind": "selected_peer",
            "peerSessionId": peer_session_id,
        },
    })
}

fn log_bridge_plan_control_event(event: &ValidatedControlEvent, stage: &str) {
    let field = |value: &Value, name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 256)
            .unwrap_or("unknown")
            .to_string()
    };
    let payload = event.event.get("payload").unwrap_or(&event.event);
    logging::write_transfer_line(&format!(
        "[pastey bridge-plan-control] stage={stage} kind={} event_id={} bridge_id={} plan_id={} revision_id={} approval_id={} source_session={} target_session={}",
        event.kind,
        event.event_id,
        field(payload, "bridgeId"),
        field(payload, "planId"),
        field(payload, "revisionId"),
        field(payload, "approvalId"),
        event.source_device_ref,
        event.target_peer_ref,
    ));
}

/// Records only identifiers and a bounded reason code for an event rejected
/// before it can enter the Bridge Plan inbox.  The decrypted event body can
/// contain user-visible plan text, so it must never be logged here.
fn log_bridge_plan_validation_rejected(event: &Value, reason: &str) {
    let field = |object: &Value, name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 256)
            .unwrap_or("unknown")
            .to_string()
    };
    let payload = event.get("payload").unwrap_or(event);
    logging::write_transfer_line(&format!(
        "[pastey bridge-plan-control] stage=inbound_protocol_rejected reason={reason} kind={} event_id={} bridge_id={} plan_id={} revision_id={} approval_id={} revision_hash={} source_session={} target_session={}",
        field(event, "kind"),
        field(event, "eventId"),
        field(payload, "bridgeId"),
        field(payload, "planId"),
        field(payload, "revisionId"),
        field(payload, "approvalId"),
        field(payload, "revisionHash"),
        field(event, "sourceDeviceRef"),
        field(event, "targetPeerRef"),
    ));
}

fn bridge_plan_validation_reason(message: &str) -> &'static str {
    if message.contains("review revision mismatch") {
        "review_revision_hash_mismatch"
    } else if message.contains("review step digest mismatch")
        || message.contains("review step mismatch")
    {
        "review_step_digest_mismatch"
    } else if message.contains("review expiry") {
        "review_expiry_invalid"
    } else if message.contains("expired") {
        "review_expired"
    } else if message.contains("session mismatch")
        || message.contains("sender or receiver mismatch")
    {
        "review_session_mismatch"
    } else {
        "review_payload_invalid"
    }
}

fn bridge_plan_protocol_rejection_reason(error: &AppError) -> &'static str {
    let message = error.message();
    if message.contains("attempt missing") {
        "attempt_missing"
    } else if message.contains("update correlation mismatch") {
        "attempt_correlation_mismatch"
    } else if message.contains("review not found")
        || message.contains("remote plan binding missing")
    {
        "review_unknown_approval"
    } else if message.contains("revision mismatch") {
        "review_revision_hash_mismatch"
    } else if message.contains("step digest") || message.contains("step mismatch") {
        "review_step_digest_mismatch"
    } else if message.contains("expired") {
        "review_expired"
    } else if message.contains("session")
        || message.contains("sender")
        || message.contains("receiver")
    {
        "review_session_mismatch"
    } else {
        "review_payload_invalid"
    }
}

const UNSAFE_FIELDS: &[&str] = &[
    "command",
    "cmd",
    "shell",
    "script",
    "code",
    "args",
    "arguments",
    "argv",
    "stdin",
    "workingdirectory",
    "runtime",
    "interpreter",
    "compiler",
    "env",
    "environment",
    "proxy",
    "path",
    "absolutepath",
    "filepath",
    "localpath",
    "realpath",
    "filesystemtree",
    "rawlogs",
    "contents",
    "filecontents",
    "secret",
    "token",
    "apikey",
    "roomkey",
    "roomcode",
    "transportkey",
    "hiddentransfer",
    "peerfilesystemsearch",
    "transferqueueid",
    "transferqueueitemid",
    "handoffid",
    "autosend",
    "sendfile",
    "stdout",
    "stderr",
    "exitcode",
    "process",
    "spawn",
    "receiverlocalsource",
    "sourcepath",
    "resolvedpath",
    "stagingpath",
    "scoperoot",
    "candidatelease",
    "sourceidentity",
    "privatedigest",
    "workerpath",
    "workeridentityinternals",
    "cgrouppath",
    "pid",
    "seccompdescriptor",
    "resolvedimplementationauthority",
    "rawexecutorresult",
];

#[derive(Default)]
pub struct RoomControlRuntimeState {
    rooms: HashMap<String, RoomControlRoomState>,
    /// Tests: moves the rate limiter's clock (only) forward.
    #[cfg(test)]
    pub(crate) limiter_skew_seconds: i64,
}

impl RoomControlRuntimeState {
    /// The rate limiter's clock, in seconds.
    fn limiter_now(&self) -> i64 {
        #[cfg(test)]
        {
            OffsetDateTime::now_utc().unix_timestamp() + self.limiter_skew_seconds
        }
        #[cfg(not(test))]
        {
            OffsetDateTime::now_utc().unix_timestamp()
        }
    }

    /// Burn is terminal for a Bridge: retain no inbox, replay cache, or rate
    /// state that could replay or display an old control action.
    pub fn purge_room(&mut self, room_id: &str) -> bool {
        self.rooms.remove(room_id).is_some()
    }
}

#[derive(Default)]
struct RoomControlRoomState {
    /// Display history of received Bridge Plan (v1) events only.
    history: VecDeque<ReceivedRoomControlEvent>,
    seen_event_ids: VecDeque<String>,
    seen_event_id_set: HashSet<String>,
    seen_envelope_ids: VecDeque<String>,
    seen_envelope_id_set: HashSet<String>,
    seen_request_ids: VecDeque<String>,
    seen_request_id_set: HashSet<String>,
    received_at_seconds: HashMap<ControlClassV1, VecDeque<i64>>,
    stats: RoomControlStatsV1,
    terminal_seen_event_ids: VecDeque<String>,
    terminal_seen_event_id_set: HashSet<String>,
    terminal_received_at_seconds: VecDeque<i64>,
}

/// What one Bridge's Room Control did, for diagnostics. Counts, classes,
/// reasons and times only: never an event, a payload or an identifier.
#[derive(Default)]
struct RoomControlStatsV1 {
    accepted: HashMap<ControlClassV1, u64>,
    refused: HashMap<(ControlClassV1, &'static str), u64>,
    history_dropped: u64,
    last_refusal: Option<ControlRefusalV1>,
    /// When each class's current run of rate refusals began, and its length.
    backpressure: HashMap<ControlClassV1, (i64, u64)>,
    outbound_failures: HashMap<(ControlClassV1, &'static str), u64>,
    last_outbound_failure: Option<ControlRefusalV1>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlRefusalV1 {
    class: ControlClassV1,
    reason: &'static str,
    at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncryptedRoomControlEnvelope {
    schema_version: String,
    sender_public_key: String,
    wrapped_event_key: String,
    key_wrap_nonce: String,
    event_nonce: String,
    ciphertext: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncryptedRoomControlReceipt {
    schema_version: String,
    receipt_nonce: String,
    ciphertext: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoomControlDeliveryReceipt {
    pub schema_version: String,
    pub event_id: String,
    pub accepted_for_local_inbox: bool,
    pub received_at: String,
}

/// A fully authenticated and encrypted current-session delivery captured
/// before local Burn removes route/key material. It contains no reusable
/// authority and can deliver only the already-validated event to one endpoint.
pub(crate) struct PreparedRoomControlDelivery {
    endpoint_url: String,
    body: Vec<u8>,
    event_key: [u8; 32],
    event_id: String,
}

#[cfg(test)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomControlSendError {
    pub code: &'static str,
    pub message: &'static str,
}

#[cfg(test)]
impl RoomControlSendError {
    pub fn from_app_error(error: AppError) -> Self {
        let message = error.message();
        let (code, message) = if message.contains("expired") {
            ("expired", "Room control event expired before delivery.")
        } else if message.contains("already received") {
            ("replay", "Room control event was already received.")
        } else if message.contains("session mismatch") || message.contains("not active") {
            ("session_mismatch", "Room control room or session mismatch.")
        } else if message.contains("Room session is unavailable") {
            (
                "session_unavailable",
                "Room control session is unavailable.",
            )
        } else if message.contains("Peer is unavailable") {
            ("peer_unavailable", "Peer is unavailable.")
        } else if message.contains("inbox is full") {
            ("inbox_full", "Peer room control inbox is full.")
        } else if message.contains("rate") {
            ("rate_limited", "Peer room control rate limit was reached.")
        } else if message.contains("too large") {
            ("oversized", "Room control event is too large.")
        } else if message.contains("receipt is invalid") {
            (
                "malformed_receipt",
                "Room control delivery receipt was invalid.",
            )
        } else if matches!(error, AppError::Timeout(_) | AppError::Network(_)) {
            ("transport_error", "Room control transport failed.")
        } else if matches!(error, AppError::InvalidInput(_)) {
            ("invalid_event", "Room control event validation failed.")
        } else {
            ("unknown", "Room control send failed.")
        };
        Self { code, message }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedRoomControlEvent {
    pub event_id: String,
    pub kind: String,
    pub room_ref: String,
    pub source_device_ref: String,
    pub target_peer_ref: String,
    pub created_at: String,
    pub expires_at: String,
    pub received_at: String,
    pub event: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomControlSessionContext {
    pub room_id: String,
    pub local_session_ref: String,
    pub peer_session_ref: String,
    pub peer_route_ref: String,
    pub peer_observation_ref: String,
    pub peer_connected: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlError {
    code: String,
    message: String,
}

#[derive(Clone)]
struct ValidatedControlEvent {
    event_id: String,
    kind: String,
    room_ref: String,
    source_device_ref: String,
    target_peer_ref: String,
    created_at: String,
    expires_at: String,
    envelope_id: Option<String>,
    request_id: Option<String>,
    event: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RoomControlRouteEndpoint {
    peer_session_id: String,
    host: String,
    port: u16,
    transport_public_key: String,
}

fn bridge_session_ref(room_id: &str) -> String {
    format!("legacy-room:{room_id}")
}

fn room_control_route_error(code: &str, message: impl Into<String>) -> AppError {
    AppError::InvalidInput(format!(
        "[pastey:bridge-route-error code={code}] {}",
        message.into()
    ))
}

fn resolve_default_room_control_peer(
    peers: &[StoredBridgePeerEndpoint],
) -> AppResult<RoomControlRouteEndpoint> {
    let routeable = peers
        .iter()
        .filter_map(|peer| routeable_room_control_peer(peer).ok())
        .collect::<Vec<_>>();
    match routeable.as_slice() {
        [peer] => Ok(peer.clone()),
        [] => Err(AppError::InvalidInput("Peer is unavailable.".into())),
        _ => Err(room_control_route_error(
            "unsupported_selected_peers",
            "Room control requires one selected Bridge peer route.",
        )),
    }
}

fn resolve_room_control_route(
    bridge_route: Option<&Value>,
    room_id: &str,
    room: &crate::models::StoredRoom,
    peers: &[StoredBridgePeerEndpoint],
) -> AppResult<RoomControlRouteEndpoint> {
    if room.status != RoomStatus::Active {
        return Err(room_control_route_error(
            "route_expired",
            "Room control route requires an active room.",
        ));
    }
    let Some(route) = bridge_route else {
        return Err(room_control_route_error(
            "malformed_route",
            "Room control selected-peer route is required.",
        ));
    };
    let route = route.as_object().ok_or_else(|| {
        room_control_route_error("malformed_route", "Room control route must be an object.")
    })?;
    require_exact_control_route_fields(route, &["schemaVersion", "bridgeSessionId", "target"])?;
    if control_route_string_field(route, "schemaVersion")? != CONTROL_BRIDGE_ROUTE_SCHEMA_VERSION {
        return Err(room_control_route_error(
            "malformed_route",
            "Room control route schema version is unsupported.",
        ));
    }
    if control_route_string_field(route, "bridgeSessionId")? != bridge_session_ref(room_id) {
        return Err(room_control_route_error(
            "route_mismatch",
            "Room control route session does not match the current room.",
        ));
    }
    let target = route
        .get("target")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            room_control_route_error("malformed_route", "Room control route target is invalid.")
        })?;
    match control_route_string_field(target, "kind")? {
        "selected_peer" => {
            require_exact_control_route_fields(target, &["kind", "peerSessionId"])?;
            let peer_session_id = control_route_string_field(target, "peerSessionId")?;
            let Some(peer) = peers
                .iter()
                .find(|peer| peer.peer_session_id == peer_session_id)
            else {
                return Err(room_control_route_error(
                    "unknown_peer",
                    "Room control route target is not in the current session.",
                ));
            };
            routeable_room_control_peer(peer).map_err(|_| {
                room_control_route_error(
                    room_control_route_error_code_for_peer(peer),
                    "Room control route target is not currently routeable.",
                )
            })
        }
        "selected_peers" => Err(room_control_route_error(
            "unsupported_selected_peers",
            "Room control selected-peers delivery is not supported.",
        )),
        "broadcast_bridge" => Err(room_control_route_error(
            "unsupported_broadcast",
            "Room control broadcast delivery is not supported.",
        )),
        _ => Err(room_control_route_error(
            "malformed_route",
            "Room control route target kind is unsupported.",
        )),
    }
}

fn resolve_inbound_room_control_peer(
    peers: &[StoredBridgePeerEndpoint],
    sender_public_key: &str,
) -> AppResult<RoomControlRouteEndpoint> {
    let mut matches = peers
        .iter()
        .filter(|peer| peer.transport_public_key.as_deref() == Some(sender_public_key))
        .filter_map(|peer| routeable_room_control_peer(peer).ok())
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        Ok(matches.remove(0))
    } else {
        Err(room_control_route_error(
            "route_mismatch",
            "Room control sender is not an exact current-session Bridge peer.",
        ))
    }
}

fn routeable_room_control_peer(
    peer: &StoredBridgePeerEndpoint,
) -> AppResult<RoomControlRouteEndpoint> {
    if peer.liveness != BridgePeerLiveness::Connected {
        return Err(AppError::InvalidInput(
            "Room control peer is not connected.".into(),
        ));
    }
    let host = peer
        .endpoint_host
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("Room control peer endpoint is missing.".into()))?;
    let port = peer
        .endpoint_port
        .ok_or_else(|| AppError::InvalidInput("Room control peer endpoint is missing.".into()))?;
    let transport_public_key = peer
        .transport_public_key
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("Room control peer key is missing.".into()))?;
    Ok(RoomControlRouteEndpoint {
        peer_session_id: peer.peer_session_id.clone(),
        host: host.to_string(),
        port,
        transport_public_key: transport_public_key.to_string(),
    })
}

fn room_control_route_error_code_for_peer(peer: &StoredBridgePeerEndpoint) -> &'static str {
    match peer.liveness {
        BridgePeerLiveness::Left | BridgePeerLiveness::Stale | BridgePeerLiveness::Expired => {
            "route_expired"
        }
        BridgePeerLiveness::Connected => "peer_unrouteable",
        BridgePeerLiveness::Reconnecting | BridgePeerLiveness::Disconnected => "peer_unrouteable",
    }
}

fn require_exact_control_route_fields(
    object: &Map<String, Value>,
    expected: &[&str],
) -> AppResult<()> {
    if object.len() != expected.len() || expected.iter().any(|field| !object.contains_key(*field)) {
        return Err(room_control_route_error(
            "malformed_route",
            "Room control route contains unsupported or missing fields.",
        ));
    }
    Ok(())
}

fn control_route_string_field<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> AppResult<&'a str> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            room_control_route_error(
                "malformed_route",
                format!("Room control route {field} is invalid."),
            )
        })
}

pub fn room_control_session_context(
    state: &Arc<AppState>,
    room_id: &str,
) -> AppResult<RoomControlSessionContext> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    if room.status != RoomStatus::Active {
        return Err(AppError::InvalidInput("Room is not active.".into()));
    }
    let _ = storage::sync_legacy_bridge_peer_endpoint(&state.paths, &room)?;
    let peers = storage::list_bridge_peer_endpoints(&state.paths, room_id)?;
    let peer = resolve_default_room_control_peer(&peers)?;
    let local_key = state
        .active_servers
        .lock()
        .get(room_id)
        .map(|server| server.transport_public_key())
        .ok_or_else(|| AppError::InvalidInput("Room session is unavailable.".into()))?;
    Ok(RoomControlSessionContext {
        room_id: room_id.to_string(),
        local_session_ref: session_ref(&local_key),
        peer_session_ref: session_ref(&peer.transport_public_key),
        peer_route_ref: peer.peer_session_id.clone(),
        peer_observation_ref: peer_observation_ref(&peer),
        peer_connected: true,
    })
}

pub(crate) fn room_control_session_context_for_peer(
    state: &AppState,
    room_id: &str,
    peer_session_id: &str,
) -> AppResult<RoomControlSessionContext> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    if room.status != RoomStatus::Active {
        return Err(AppError::InvalidInput("Room is not active.".into()));
    }
    let _ = storage::sync_legacy_bridge_peer_endpoint(&state.paths, &room)?;
    let peers = storage::list_bridge_peer_endpoints(&state.paths, room_id)?;
    let route = selected_peer_control_route(room_id, peer_session_id);
    let peer = resolve_room_control_route(Some(&route), room_id, &room, &peers)?;
    let local_key = state
        .active_servers
        .lock()
        .get(room_id)
        .map(|server| server.transport_public_key())
        .ok_or_else(|| AppError::InvalidInput("Room session is unavailable.".into()))?;
    Ok(RoomControlSessionContext {
        room_id: room_id.to_string(),
        local_session_ref: session_ref(&local_key),
        peer_session_ref: session_ref(&peer.transport_public_key),
        peer_route_ref: peer.peer_session_id.clone(),
        peer_observation_ref: peer_observation_ref(&peer),
        peer_connected: true,
    })
}

/// Resolves the exact authenticated file-transfer sender to its current
/// logical peer session. This is used only to validate native-v2 Transfer
/// landing metadata; the transport key is never Plan or effect authority.
pub(crate) fn room_control_session_context_for_transport_key(
    state: &AppState,
    room_id: &str,
    transport_public_key: &str,
) -> AppResult<RoomControlSessionContext> {
    let peers = storage::list_bridge_peer_endpoints(&state.paths, room_id)?;
    let mut matches = peers.iter().filter(|peer| {
        peer.transport_public_key.as_deref() == Some(transport_public_key)
            && peer.liveness == crate::models::BridgePeerLiveness::Connected
    });
    let peer = matches
        .next()
        .ok_or_else(|| AppError::InvalidInput("Transfer sender session is unavailable.".into()))?;
    if matches.next().is_some() {
        return Err(AppError::InvalidInput(
            "Transfer sender session is ambiguous.".into(),
        ));
    }
    room_control_session_context_for_peer(state, room_id, &peer.peer_session_id)
}

pub(crate) fn selected_peer_route(room_id: &str, peer_session_id: &str) -> Value {
    selected_peer_control_route(room_id, peer_session_id)
}

/// Opaque current-route binding for non-authorizing peer observations. It is
/// derived from the route endpoint as well as session/key, so an endpoint
/// change cannot reuse a fact observed for the old route.
fn peer_observation_ref(peer: &RoomControlRouteEndpoint) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(peer.peer_session_id.as_bytes());
    hasher.update(canonical_endpoint_host(&peer.host).as_bytes());
    hasher.update(&peer.port.to_be_bytes());
    hasher.update(peer.transport_public_key.as_bytes());
    format!("peer-observation:{}", hasher.finalize().to_hex())
}

/// Endpoint spellings are transport metadata, not product data. Normalize
/// their semantic route identity before hashing so IPv4-mapped IPv6 and DNS
/// case differences do not spuriously invalidate a current fact.
fn canonical_endpoint_host(host: &str) -> String {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => address.to_string(),
        Ok(IpAddr::V6(address)) => address
            .to_ipv4_mapped()
            .map(|mapped| mapped.to_string())
            .unwrap_or_else(|| address.to_string()),
        Err(_) => host.trim_end_matches('.').to_ascii_lowercase(),
    }
}

pub async fn send_room_control_event(
    state: Arc<AppState>,
    room_id: &str,
    event: Value,
    bridge_route: Option<Value>,
) -> AppResult<RoomControlDeliveryReceipt> {
    let class = ControlClassV1::of(
        event
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let sent = send_room_control_event_internal(state.clone(), room_id, event, bridge_route).await;
    if let Err(error) = &sent {
        note_outbound_failure(&state, room_id, class, delivery_failure(error));
    }
    sent
}

fn note_outbound_failure(
    state: &AppState,
    room_id: &str,
    class: ControlClassV1,
    failure: ControlDeliveryFailureV1,
) {
    let mut runtime = state.room_control.lock();
    let stats = &mut runtime.rooms.entry(room_id.to_owned()).or_default().stats;
    *stats
        .outbound_failures
        .entry((class, failure.label()))
        .or_default() += 1;
    stats.last_outbound_failure = Some(ControlRefusalV1 {
        class,
        reason: failure.label(),
        at: storage::now_ts(),
    });
}

/// Sends one event built afresh by `build` for each attempt, and tries
/// again only while the receiver answers `rate_limited`: that refusal
/// comes before it acts on anything, so a new attempt cannot do anything
/// twice. Every other outcome, and running out of `patience`, is returned
/// as it is. Never retries on an unknown outcome.
pub(crate) async fn send_room_control_event_with_backpressure(
    state: Arc<AppState>,
    room_id: &str,
    mut build: impl FnMut() -> AppResult<Value>,
    bridge_route: Option<Value>,
    patience: Duration,
) -> AppResult<RoomControlDeliveryReceipt> {
    let deadline = tokio::time::Instant::now() + patience;
    let mut pause = Duration::from_millis(400);
    loop {
        let sent =
            send_room_control_event(state.clone(), room_id, build()?, bridge_route.clone()).await;
        match &sent {
            Err(error)
                if delivery_failure(error) == ControlDeliveryFailureV1::RateLimited
                    && tokio::time::Instant::now() + pause < deadline =>
            {
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(Duration::from_secs(2));
            }
            _ => return sent,
        }
    }
}

async fn send_room_control_event_internal(
    state: Arc<AppState>,
    room_id: &str,
    event: Value,
    bridge_route: Option<Value>,
) -> AppResult<RoomControlDeliveryReceipt> {
    let delivery = prepare_room_control_event(&state, room_id, event, bridge_route)?;
    deliver_prepared_room_control_event(delivery).await
}

fn prepare_room_control_event(
    state: &Arc<AppState>,
    room_id: &str,
    event: Value,
    bridge_route: Option<Value>,
) -> AppResult<PreparedRoomControlDelivery> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    if room.status != RoomStatus::Active {
        return Err(AppError::InvalidInput("Room is not active.".into()));
    }
    let _ = storage::sync_legacy_bridge_peer_endpoint(&state.paths, &room)?;
    let peers = storage::list_bridge_peer_endpoints(&state.paths, room_id)?;
    let peer = resolve_room_control_route(bridge_route.as_ref(), room_id, &room, &peers)?;
    let (local_secret, local_key) = {
        let servers = state.active_servers.lock();
        let server = servers
            .get(room_id)
            .ok_or_else(|| AppError::InvalidInput("Room session is unavailable.".into()))?;
        (server.transport_secret, server.transport_public_key())
    };
    let validated = validate_control_event(
        event,
        room_id,
        &session_ref(&local_key),
        &session_ref(&peer.transport_public_key),
        OffsetDateTime::now_utc(),
    )?;
    let plaintext = serde_json::to_vec(&validated.event)?;
    if plaintext.len() > MAX_CONTROL_EVENT_BYTES {
        return Err(AppError::InvalidInput(
            "Room control event is too large.".into(),
        ));
    }
    let event_key = crypto::random_key();
    let (ciphertext, event_nonce) = crypto::encrypt_bytes(&plaintext, &event_key)?;
    let receiver_key = crypto::decode_key(&peer.transport_public_key)?;
    let (wrapped_event_key, key_wrap_nonce, sender_public_key) =
        crypto::wrap_control_key_for_receiver(&event_key, &local_secret, &receiver_key)?;
    let envelope = EncryptedRoomControlEnvelope {
        schema_version: CONTROL_TRANSPORT_SCHEMA.into(),
        sender_public_key,
        wrapped_event_key,
        key_wrap_nonce,
        event_nonce: crypto::encode_nonce(&event_nonce),
        ciphertext: STANDARD.encode(ciphertext),
    };
    let body = serde_json::to_vec(&envelope)?;
    if body.len() > MAX_CONTROL_REQUEST_BYTES {
        return Err(AppError::InvalidInput(
            "Room control request is too large.".into(),
        ));
    }

    Ok(PreparedRoomControlDelivery {
        endpoint_url: format!(
            "http://{}:{}/rooms/{room_id}/control-events",
            peer.host, peer.port
        ),
        body,
        event_key,
        event_id: validated.event_id,
    })
}

pub(crate) fn prepare_bridge_departure_delivery(
    state: &Arc<AppState>,
    room_id: &str,
    kind: BridgeDepartureKind,
) -> AppResult<PreparedRoomControlDelivery> {
    let context = room_control_session_context(state, room_id)?;
    let route = selected_peer_control_route(room_id, &context.peer_route_ref);
    let event = bridge_departure_event(kind, &context)?;
    prepare_room_control_event(state, room_id, event, Some(route))
}

pub(crate) async fn deliver_prepared_room_control_event(
    delivery: PreparedRoomControlDelivery,
) -> AppResult<RoomControlDeliveryReceipt> {
    // Bounds sized for an internet path between Hosts. A delivery that does
    // time out is still receipt-ambiguous and handled as such by callers.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| AppError::Network("Room control transport unavailable.".into()))?;
    let response = client
        .post(&delivery.endpoint_url)
        .header(header::CONTENT_TYPE.as_str(), CONTROL_CONTENT_TYPE)
        .header(header::ACCEPT.as_str(), CONTROL_RECEIPT_CONTENT_TYPE)
        .body(delivery.body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                AppError::Timeout("Room control delivery timed out.".into())
            } else {
                AppError::Network("Room control delivery failed.".into())
            }
        })?;
    if !response.status().is_success() {
        return Err(control_response_failure(response).await);
    }
    if response.content_length().unwrap_or(0) > MAX_CONTROL_RESPONSE_BYTES as u64 {
        return Err(AppError::Network(
            "Room control receipt is too large.".into(),
        ));
    }
    let response_bytes = response
        .bytes()
        .await
        .map_err(|_| AppError::Network("Room control receipt is invalid.".into()))?;
    if response_bytes.len() > MAX_CONTROL_RESPONSE_BYTES {
        return Err(AppError::Network(
            "Room control receipt is too large.".into(),
        ));
    }
    let receipt_envelope: EncryptedRoomControlReceipt = serde_json::from_slice(&response_bytes)
        .map_err(|_| AppError::Network("Room control receipt is invalid.".into()))?;
    if receipt_envelope.schema_version != CONTROL_RECEIPT_ENVELOPE_SCHEMA {
        return Err(AppError::Network("Room control receipt is invalid.".into()));
    }
    let receipt_ciphertext = STANDARD
        .decode(receipt_envelope.ciphertext)
        .map_err(|_| AppError::Network("Room control receipt is invalid.".into()))?;
    let receipt_nonce = crypto::decode_nonce(&receipt_envelope.receipt_nonce)
        .map_err(|_| AppError::Network("Room control receipt is invalid.".into()))?;
    let receipt_plaintext =
        crypto::decrypt_bytes(&receipt_ciphertext, &delivery.event_key, &receipt_nonce)
            .map_err(|_| AppError::Network("Room control receipt is invalid.".into()))?;
    let receipt: RoomControlDeliveryReceipt = serde_json::from_slice(&receipt_plaintext)
        .map_err(|_| AppError::Network("Room control receipt is invalid.".into()))?;
    if receipt.schema_version != CONTROL_DELIVERY_SCHEMA
        || receipt.event_id != delivery.event_id
        || !receipt.accepted_for_local_inbox
    {
        return Err(AppError::Network("Room control receipt is invalid.".into()));
    }
    Ok(receipt)
}

pub fn list_received_room_control_events(
    state: &Arc<AppState>,
    room_id: &str,
) -> AppResult<Vec<ReceivedRoomControlEvent>> {
    let _ = room_control_session_context(state, room_id)?;
    Ok(state
        .room_control
        .lock()
        .rooms
        .get(room_id)
        .map(|room| room.history.iter().cloned().collect())
        .unwrap_or_default())
}

/// One class's traffic on one Bridge, as received here.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlClassDiagnosticsV1 {
    class: ControlClassV1,
    accepted: u64,
    /// Refusals by reason (`rate_limited`, `event_replayed`,
    /// `refused_after_admission`).
    refused: Vec<(&'static str, u64)>,
    /// Accepted in the last minute, against `per_minute`.
    last_minute: usize,
    per_minute: usize,
    burst: usize,
    /// Since when this class's events are being refused for its rate
    /// budget, if they are now, and how many so far.
    backpressure_since: Option<i64>,
    backpressure_refused: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutboundFailureDiagnosticsV1 {
    class: ControlClassV1,
    failure: &'static str,
    count: u64,
}

/// Room Control on one Bridge, for diagnostics: counts, classes, reasons
/// and times only, never an event, a payload or an identifier.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoomControlDiagnosticsV1 {
    observed_at: i64,
    /// Bridge Plan events kept for display, and the most kept.
    history_depth: usize,
    history_limit: usize,
    history_dropped: u64,
    /// History entries past their age: always 0, since every addition
    /// prunes them first; a non-zero value would mean nothing consumes it.
    history_stale: usize,
    replay_window: usize,
    replay_limit: usize,
    classes: Vec<ControlClassDiagnosticsV1>,
    last_refusal: Option<ControlRefusalV1>,
    outbound_failures: Vec<OutboundFailureDiagnosticsV1>,
    last_outbound_failure: Option<ControlRefusalV1>,
    native_reviews: crate::native_agent::NativeReviewCountsV1,
}

pub(crate) fn room_control_diagnostics(
    state: &AppState,
    room_id: &str,
) -> RoomControlDiagnosticsV1 {
    let now = storage::now_ts();
    let native_reviews = state.native_agents.lock().review_counts(room_id);
    let runtime = state.room_control.lock();
    let limiter_now = runtime.limiter_now();
    let empty = RoomControlRoomState::default();
    let room = runtime.rooms.get(room_id).unwrap_or(&empty);
    let stats = &room.stats;
    let mut outbound_failures = stats
        .outbound_failures
        .iter()
        .map(|((class, failure), count)| OutboundFailureDiagnosticsV1 {
            class: *class,
            failure,
            count: *count,
        })
        .collect::<Vec<_>>();
    outbound_failures.sort_by_key(|failure| (failure.class.label(), failure.failure));
    RoomControlDiagnosticsV1 {
        observed_at: now,
        history_depth: room.history.len(),
        history_limit: MAX_HISTORY_ITEMS,
        history_dropped: stats.history_dropped,
        history_stale: room
            .history
            .iter()
            .filter(|kept| {
                OffsetDateTime::parse(
                    &kept.received_at,
                    &time::format_description::well_known::Rfc3339,
                )
                .is_ok_and(|at| at.unix_timestamp() <= now - MAX_HISTORY_AGE_SECONDS)
            })
            .count(),
        replay_window: room.seen_event_ids.len(),
        replay_limit: MAX_REPLAY_ITEMS,
        classes: ControlClassV1::ALL
            .iter()
            .map(|class| {
                let (per_minute, burst) = class.budget();
                let mut refused = stats
                    .refused
                    .iter()
                    .filter(|((refused_class, _), _)| refused_class == class)
                    .map(|((_, reason), count)| (*reason, *count))
                    .collect::<Vec<_>>();
                refused.sort();
                let backpressure = stats.backpressure.get(class).copied();
                ControlClassDiagnosticsV1 {
                    class: *class,
                    accepted: stats.accepted.get(class).copied().unwrap_or(0),
                    refused,
                    last_minute: room.received_at_seconds.get(class).map_or(0, |window| {
                        window.iter().filter(|at| **at > limiter_now - 60).count()
                    }),
                    per_minute,
                    burst,
                    backpressure_since: backpressure.map(|(since, _)| since),
                    backpressure_refused: backpressure.map_or(0, |(_, refused)| refused),
                }
            })
            .collect(),
        last_refusal: stats.last_refusal,
        outbound_failures,
        last_outbound_failure: stats.last_outbound_failure,
        native_reviews,
    }
}

pub fn clear_room_control_state(state: &Arc<AppState>, room_id: &str) {
    state.room_control.lock().rooms.remove(room_id);
    state.purge_room(room_id);
}

pub async fn receive_room_control_event_handler(
    AxumPath(room_id): AxumPath<String>,
    ConnectInfo(_source): ConnectInfo<SocketAddr>,
    State(ctx): State<RoomServerContext>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    if room_id != ctx.room_id {
        return control_error(StatusCode::NOT_FOUND, "room_not_found", "Room not found.");
    }
    let room = match storage::get_room_by_id(&ctx.state.paths, &room_id) {
        Ok(room) if room.status == RoomStatus::Active => room,
        Ok(_) => return control_error(StatusCode::GONE, "room_unavailable", "Room unavailable."),
        Err(_) => return control_error(StatusCode::NOT_FOUND, "room_not_found", "Room not found."),
    };
    let _ = storage::sync_legacy_bridge_peer_endpoint(&ctx.state.paths, &room);
    let peers = match storage::list_bridge_peer_endpoints(&ctx.state.paths, &room_id) {
        Ok(peers) => peers,
        Err(_) => return control_error(StatusCode::GONE, "room_unavailable", "Room unavailable."),
    };
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(CONTROL_CONTENT_TYPE)
    {
        return control_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Unsupported room control content type.",
        );
    }
    let body = match body {
        Ok(body) if body.len() <= MAX_CONTROL_REQUEST_BYTES => body,
        Ok(_) => {
            return control_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "Room control request is too large.",
            )
        }
        Err(error) => {
            return control_error(
                error.status(),
                "invalid_request",
                "Invalid room control request.",
            )
        }
    };
    let envelope: EncryptedRoomControlEnvelope = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "invalid_envelope",
                "Invalid room control envelope.",
            )
        }
    };
    if envelope.schema_version != CONTROL_TRANSPORT_SCHEMA
        || envelope.sender_public_key.trim().is_empty()
    {
        return control_error(
            StatusCode::FORBIDDEN,
            "session_mismatch",
            "Room session mismatch.",
        );
    }
    let inbound_peer = match resolve_inbound_room_control_peer(&peers, &envelope.sender_public_key)
    {
        Ok(peer) => peer,
        Err(_) => {
            return control_error(
                StatusCode::FORBIDDEN,
                "session_mismatch",
                "Room session mismatch.",
            )
        }
    };
    let (local_secret, local_key) = {
        let servers = ctx.state.active_servers.lock();
        let Some(server) = servers.get(&room_id) else {
            return control_error(StatusCode::GONE, "room_unavailable", "Room unavailable.");
        };
        (server.transport_secret, server.transport_public_key())
    };
    let event_key = match crypto::unwrap_control_key_from_sender(
        &envelope.wrapped_event_key,
        &envelope.key_wrap_nonce,
        &envelope.sender_public_key,
        &local_secret,
    ) {
        Ok(key) => key,
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "decrypt_failed",
                "Invalid room control envelope.",
            )
        }
    };
    let ciphertext = match STANDARD.decode(&envelope.ciphertext) {
        Ok(value) => value,
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "decrypt_failed",
                "Invalid room control envelope.",
            )
        }
    };
    let nonce = match crypto::decode_nonce(&envelope.event_nonce) {
        Ok(value) => value,
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "decrypt_failed",
                "Invalid room control envelope.",
            )
        }
    };
    let plaintext = match crypto::decrypt_bytes(&ciphertext, &event_key, &nonce) {
        Ok(value) if value.len() <= MAX_CONTROL_EVENT_BYTES => value,
        Ok(_) => {
            return control_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "event_too_large",
                "Room control event is too large.",
            )
        }
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "decrypt_failed",
                "Invalid room control envelope.",
            )
        }
    };
    let event: Value = match serde_json::from_slice(&plaintext) {
        Ok(value) => value,
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "invalid_event",
                "Invalid room control event.",
            )
        }
    };
    let validated = match validate_control_event(
        event.clone(),
        &room_id,
        &session_ref(&inbound_peer.transport_public_key),
        &session_ref(&local_key),
        OffsetDateTime::now_utc(),
    ) {
        Ok(value) => value,
        Err(AppError::InvalidInput(message)) if message.contains("expired") => {
            if let Some(stage) = peer_capability_rejection_stage(&event) {
                log_peer_capability(stage, None, Some(peer_capability_rejection_code(&message)));
            }
            if event
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("bridge_plan."))
            {
                log_bridge_plan_validation_rejected(
                    &event,
                    bridge_plan_validation_reason(&message),
                );
            }
            return control_error(
                StatusCode::GONE,
                "event_expired",
                "Room control event expired.",
            );
        }
        Err(AppError::InvalidInput(message)) if message.contains("session mismatch") => {
            if let Some(stage) = peer_capability_rejection_stage(&event) {
                log_peer_capability(stage, None, Some(peer_capability_rejection_code(&message)));
            }
            if event
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("bridge_plan."))
            {
                log_bridge_plan_validation_rejected(
                    &event,
                    bridge_plan_validation_reason(&message),
                );
            }
            return control_error(
                StatusCode::FORBIDDEN,
                "session_mismatch",
                "Room control session mismatch.",
            );
        }
        Err(AppError::InvalidInput(message)) => {
            if let Some(stage) = peer_capability_rejection_stage(&event) {
                log_peer_capability(stage, None, Some(peer_capability_rejection_code(&message)));
            }
            if event
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("developer_terminal."))
            {
                return control_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_terminal_event",
                    "Invalid Developer Terminal event.",
                );
            }
            if event
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("bridge_membership."))
            {
                return control_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_membership_event",
                    "Invalid Bridge membership event.",
                );
            }
            let reason = bridge_plan_validation_reason(&message);
            if event
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("bridge_plan."))
            {
                log_bridge_plan_validation_rejected(&event, reason);
            }
            return control_error(
                StatusCode::BAD_REQUEST,
                reason,
                "Bridge Plan review validation failed.",
            );
        }
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "invalid_event",
                "Invalid room control event.",
            )
        }
    };
    if validated.kind.starts_with("bridge_plan.") {
        log_bridge_plan_control_event(&validated, "inbound_validated");
    }
    if validated.kind == "bridge_membership.departure" {
        {
            let mut runtime = ctx.state.room_control.lock();
            let now = runtime.limiter_now();
            let room_state = runtime.rooms.entry(room_id.clone()).or_default();
            if let Err(refused) = admit_received_event(
                room_state,
                &validated,
                ControlClassV1::Membership,
                now,
                &room_id,
            ) {
                return refused;
            }
        }

        let removed = match storage::remove_departed_bridge_peer(
            &ctx.state.paths,
            &room_id,
            &inbound_peer.peer_session_id,
        ) {
            Ok(removed) => removed,
            Err(_) => {
                let mut runtime = ctx.state.room_control.lock();
                if let Some(room_state) = runtime.rooms.get_mut(&room_id) {
                    forget_replay_id(
                        &mut room_state.seen_event_ids,
                        &mut room_state.seen_event_id_set,
                        &validated.event_id,
                    );
                }
                return control_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "membership_update_failed",
                    "Bridge membership update failed.",
                );
            }
        };
        if !removed {
            return control_error(
                StatusCode::CONFLICT,
                "event_replayed",
                "Room control event was already received.",
            );
        }

        let _ = crate::transfer::cancel_room_transfers(
            ctx.state.clone(),
            &room_id,
            "Peer left the Bridge.",
            false,
            Some("peer_disconnected"),
        )
        .await;
        if crate::commands::purge_bridge_runtime_authority(&ctx.state, &room_id).is_err() {
            return control_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "membership_cleanup_failed",
                "Bridge membership cleanup failed.",
            );
        }
        return encrypted_receipt_response(&event_key, &validated.event_id, &now_iso());
    }
    if validated.kind == "bridge_membership.carrier_card" {
        {
            let mut runtime = ctx.state.room_control.lock();
            let now = runtime.limiter_now();
            let room_state = runtime.rooms.entry(room_id.clone()).or_default();
            if let Err(refused) = admit_received_event(
                room_state,
                &validated,
                ControlClassV1::Membership,
                now,
                &room_id,
            ) {
                return refused;
            }
        }
        // Pinned for the peer this session authenticated, and only while
        // the Bridge is pinned to that same device.
        let stored = crate::host_runtime::current_host_session_binding(
            &ctx.state,
            &room_id,
            &inbound_peer.peer_session_id,
        )
        .and_then(|binding| {
            let card = serde_json::from_value::<crate::connectivity::CarrierCardV1>(
                validated
                    .event
                    .get("payload")
                    .and_then(|payload| payload.get("card"))
                    .cloned()
                    .unwrap_or(Value::Null),
            )?;
            crate::connectivity::receive_card(
                &ctx.state,
                &room_id,
                binding.peer_host_ref.as_str(),
                &card,
            )
        });
        if stored.is_err() {
            return control_error(
                StatusCode::FORBIDDEN,
                "carrier_card_refused",
                "This Bridge does not pin the device that sent the carrier card.",
            );
        }
        return encrypted_receipt_response(&event_key, &validated.event_id, &now_iso());
    }
    if validated.kind.starts_with("developer_terminal.") {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        {
            let mut runtime = ctx.state.room_control.lock();
            let limiter_now = runtime.limiter_now();
            let room_state = runtime.rooms.entry(room_id.clone()).or_default();
            if room_state
                .terminal_seen_event_id_set
                .contains(&validated.event_id)
            {
                return control_error(
                    StatusCode::CONFLICT,
                    "event_replayed",
                    "Room control event was already received.",
                );
            }
            if !accept_terminal_rate_limited_event(room_state, limiter_now) {
                return control_error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "terminal_rate_limited",
                    "Developer Terminal event rate exceeded.",
                );
            }
            record_replay_id(
                &mut room_state.terminal_seen_event_ids,
                &mut room_state.terminal_seen_event_id_set,
                validated.event_id.clone(),
            );
        }
        let message = match validated
            .event
            .get("payload")
            .and_then(Value::as_object)
            .and_then(|payload| {
                crate::developer_terminal::validate_wire_message(&validated.kind, payload).ok()
            }) {
            Some(message) => message,
            None => {
                return control_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_terminal_event",
                    "Invalid Developer Terminal event.",
                )
            }
        };
        let host_to_controller = matches!(
            validated.kind.as_str(),
            "developer_terminal.open_accepted"
                | "developer_terminal.open_denied"
                | "developer_terminal.output"
                | "developer_terminal.exit"
        );
        let binding = if host_to_controller {
            crate::host_runtime::inbound_controller_binding(
                &room_id,
                &validated.target_peer_ref,
                &validated.source_device_ref,
                &inbound_peer.peer_session_id,
            )
        } else {
            crate::host_runtime::inbound_controller_binding(
                &room_id,
                &validated.source_device_ref,
                &validated.target_peer_ref,
                &inbound_peer.peer_session_id,
            )
        };
        let service = &ctx.state.developer_terminal;
        let accepted = match validated.kind.as_str() {
            "developer_terminal.open_request" => {
                service.receive_open_request(binding, &message, now)
            }
            "developer_terminal.open_accepted" => service.receive_accepted(&binding, &message, now),
            "developer_terminal.open_denied" => service.receive_denied(&binding, &message),
            "developer_terminal.input" => service.receive_input(&binding, &message, now),
            "developer_terminal.output" => service.receive_output(&binding, &message),
            "developer_terminal.resize" => service.receive_resize(&binding, &message, now),
            "developer_terminal.exit" => service.receive_exit(&binding, &message),
            "developer_terminal.close" => service.receive_close(&binding, &message),
            _ => Err(AppError::InvalidInput(
                "Unsupported Developer Terminal event.".into(),
            )),
        };
        if let Err(error) = accepted {
            let (status, code, message) = terminal_control_rejection(&error);
            return control_error(status, code, message);
        }
        if validated.kind == "developer_terminal.output" {
            if let Ok(output) =
                crate::developer_terminal::DeveloperTerminalOutputUiEvent::from_message(
                    &room_id, &message,
                )
            {
                // This local, non-persistent UI notification does not change
                // terminal authority. The bounded workspace snapshot remains
                // the fallback if the renderer misses an event.
                let _ = ctx
                    .state
                    .emit(crate::developer_terminal::OUTPUT_UI_EVENT, &output);
            }
        }
        // Terminal traffic is delivered directly to the process-local Host
        // service. It is intentionally absent from ordinary room history and
        // the bounded Room Control inbox.
        return encrypted_receipt_response(&event_key, &validated.event_id, &now_iso());
    }
    if validated.kind.starts_with("native_agent.") {
        // Native Agent invocation is a bounded product protocol over the
        // existing authenticated current-session channel. Admit (replay,
        // then this class's own rate budget) before starting anything:
        // receipt replay must never start a second task, and a refusal
        // here has done nothing.
        {
            let mut runtime = ctx.state.room_control.lock();
            let now = runtime.limiter_now();
            let room_state = runtime.rooms.entry(room_id.clone()).or_default();
            if let Err(refused) = admit_received_event(
                room_state,
                &validated,
                ControlClassV1::NativeAgent,
                now,
                &room_id,
            ) {
                return refused;
            }
        }
        let payload = validated
            .event
            .get("payload")
            .cloned()
            .unwrap_or(Value::Null);
        match validated.kind.as_str() {
            "native_agent.invoke" => {
                let request = match crate::native_agent::parse_invoke(payload) {
                    Ok(request) => request,
                    Err(_) => {
                        return control_error(
                            StatusCode::BAD_REQUEST,
                            "invalid_native_agent",
                            "Invalid native Agent invocation.",
                        )
                    }
                };
                // The invoking principal is the exact authenticated current
                // peer session, resolved before anything else is considered.
                let binding = match crate::host_runtime::current_host_session_binding(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                ) {
                    Ok(binding) => binding,
                    Err(_) => {
                        return control_error(
                            StatusCode::FORBIDDEN,
                            "host_mismatch",
                            "Native Agent invocation has no bound peer Host.",
                        )
                    }
                };
                if request.target_host_ref != ctx.state.local_host_ref.as_str() {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent target Host does not match this Host.",
                    );
                }
                if ctx
                    .state
                    .native_agents
                    .lock()
                    .require_capability(&request.agent_capability)
                    .is_err()
                {
                    return control_error(
                        StatusCode::BAD_REQUEST,
                        "native_agent_incompatible",
                        "Native Agent interface is incompatible on this Host.",
                    );
                }
                // Ownership, replay, availability, then admission: a new
                // remote invocation is held for Host Review, unstarted.
                // A device identity proven in exactly this session lets a
                // Host-local unattended grant apply; nothing else does.
                let verified_device =
                    crate::host_runtime::verified_peer_device_for(&ctx.state, &binding);
                // A folder this Host shares with the requesting device is
                // named by its reference, never by a path, and resolved here
                // from this Host's own shares, before anything else. Running
                // there needs the share to allow agent requests; continuing a
                // thread also needs it to allow reading sessions. Each run
                // still waits for this Host's Review below.
                let mut request = request;
                let mut share_requirement = None;
                match crate::session_read_access::resolve_agent_request(
                    &ctx.state.paths,
                    verified_device.as_ref(),
                    &binding,
                    &request.agent_capability,
                    &request.input,
                    storage::now_ts(),
                ) {
                    Ok(Some(input)) => {
                        share_requirement = verified_device.as_ref().and_then(|device| {
                            crate::session_read_access::ShareRequirementV1::of_resolved(
                                device,
                                &binding,
                                &request.agent_capability,
                                &input,
                            )
                        });
                        request.input = input;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        crate::logging::write_transfer_line(&format!(
                            "[pastey native-agent] event=shared_workspace_refused task={} capability={}",
                            request.task_id, request.agent_capability
                        ));
                        return control_error(
                            StatusCode::FORBIDDEN,
                            "native_agent_not_authorized",
                            "This device may not ask to run agents in that folder.",
                        );
                    }
                }
                // One lock: the Host's user cannot Accept between admission
                // and recording which share that Accept must check again.
                let _admitted = {
                    let mut agents = ctx.state.native_agents.lock();
                    match agents.receive_bridge_invocation(
                        &binding,
                        &request,
                        storage::now_ts(),
                        verified_device.as_ref(),
                    ) {
                        Ok(status) => {
                            if let Some(requirement) = share_requirement {
                                agents.require_share_at_accept(&request.task_id, requirement);
                            }
                            status
                        }
                        // As many Reviews from this Bridge as allowed are
                        // waiting: nothing was queued, and none was dropped.
                        Err(_) if agents.review_capacity_reached(&binding.bridge_id) => {
                            return control_error(
                                StatusCode::TOO_MANY_REQUESTS,
                                "native_agent_review_capacity",
                                "Too many native capability invocations are awaiting Review.",
                            )
                        }
                        Err(_) => {
                            return control_error(
                                StatusCode::BAD_REQUEST,
                                "native_agent_unavailable",
                                "Native Agent invocation was rejected by this Host.",
                            )
                        }
                    }
                };
                let response_context = match room_control_session_context_for_peer(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                ) {
                    Ok(context) => context,
                    Err(_) => {
                        return control_error(
                            StatusCode::GONE,
                            "host_binding_unavailable",
                            "Native Agent response route is unavailable.",
                        )
                    }
                };
                let response_state = ctx.state.clone();
                let response_room = room_id.clone();
                let response_route =
                    selected_peer_control_route(&room_id, &inbound_peer.peer_session_id);
                let task_id = request.task_id;
                let host_ref = ctx.state.local_host_ref.as_str().to_owned();
                ctx.state.spawn(async move {
                    let mut last = None;
                    // A status that did not reach the requester is sent
                    // again, a bounded number of times: a running task can
                    // stay unchanged (for example waiting for its requester's
                    // decision) and would otherwise never be reported.
                    let mut failed_sends = 0_u32;
                    let mut rate_limited = false;
                    let mut backpressure_since: Option<tokio::time::Instant> = None;
                    loop {
                        let status = {
                            let mut agents = response_state.native_agents.lock();
                            // A Review ends by Accept, Deny or its bounded
                            // expiry, which this loop also evaluates.
                            agents.expire_invocation_reviews(storage::now_ts());
                            agents.task_status(&task_id)
                        };
                        let Ok(status) = status else {
                            return;
                        };
                        let mut delivered = false;
                        if last.as_ref() != Some(&status) {
                            let payload =
                                serde_json::to_value(crate::native_agent::NativeAgentStatusV1 {
                                    schema_version:
                                        crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                                    task_id: task_id.clone(),
                                    executing_host_ref: host_ref.clone(),
                                    status: status.clone(),
                                });
                            if let Ok(payload) = payload {
                                let payload = native_agent_event(
                                    "native_agent.status",
                                    payload,
                                    &response_context,
                                );
                                if let Ok(payload) = payload {
                                    match send_room_control_event(
                                        response_state.clone(),
                                        &response_room,
                                        payload,
                                        Some(response_route.clone()),
                                    )
                                    .await
                                    {
                                        Ok(_) => delivered = true,
                                        // Refused for the requester's rate
                                        // budget: nothing reached it, so the
                                        // same status is offered again, for
                                        // up to `MAX_STATUS_BACKPRESSURE`,
                                        // without using up its resends.
                                        Err(error)
                                            if delivery_failure(&error)
                                                == ControlDeliveryFailureV1::RateLimited =>
                                        {
                                            let since = *backpressure_since
                                                .get_or_insert_with(tokio::time::Instant::now);
                                            rate_limited =
                                                since.elapsed() < MAX_STATUS_BACKPRESSURE;
                                        }
                                        Err(_) => {}
                                    }
                                }
                            }
                            if delivered {
                                backpressure_since = None;
                            }
                            if rate_limited {
                                rate_limited = false;
                            } else if native_status_settled(delivered, failed_sends) {
                                last = Some(status.clone());
                                failed_sends = 0;
                                backpressure_since = None;
                            } else {
                                failed_sends += 1;
                            }
                        }
                        // A final status ends the loop once it is settled
                        // (delivered, or its bounded resends used up), not at
                        // its first attempt: the requester learns the outcome
                        // from it, without waiting for reconciliation.
                        if !matches!(
                            status.state,
                            crate::native_agent::NativeAgentTaskStateV1::Queued
                                | crate::native_agent::NativeAgentTaskStateV1::Running
                        ) && last.as_ref() == Some(&status)
                        {
                            return;
                        }
                        let pause = if failed_sends > 0 || backpressure_since.is_some() {
                            1_000
                        } else {
                            250
                        };
                        tokio::time::sleep(Duration::from_millis(pause)).await;
                    }
                });
            }
            "native_agent.status" => {
                let remote: crate::native_agent::NativeAgentStatusV1 =
                    match serde_json::from_value(payload) {
                        Ok(status) if crate::native_agent::validate_status(&status).is_ok() => {
                            status
                        }
                        _ => {
                            return control_error(
                                StatusCode::BAD_REQUEST,
                                "invalid_native_agent",
                                "Invalid native Agent status.",
                            )
                        }
                    };
                if ctx
                    .state
                    .native_agents
                    .lock()
                    .record_bridge_remote_status(&room_id, remote)
                    .is_err()
                {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent status does not match its selected Host.",
                    );
                }
            }
            "native_agent.cancel" => {
                let cancel: crate::native_agent::NativeAgentCancelV1 =
                    match serde_json::from_value(payload) {
                        Ok(cancel) if crate::native_agent::validate_cancel(&cancel).is_ok() => {
                            cancel
                        }
                        _ => {
                            return control_error(
                                StatusCode::BAD_REQUEST,
                                "invalid_native_agent",
                                "Invalid native Agent cancellation.",
                            )
                        }
                    };
                if cancel.target_host_ref != ctx.state.local_host_ref.as_str() {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent cancellation targets another Host.",
                    );
                }
                if ctx
                    .state
                    .native_agents
                    .lock()
                    .cancel_bridge_task(&room_id, &cancel.task_id)
                    .is_err()
                {
                    return control_error(
                        StatusCode::BAD_REQUEST,
                        "native_agent_unavailable",
                        "Native Agent task is unavailable.",
                    );
                }
            }
            "native_agent.reconcile" => {
                let request: crate::native_agent::NativeAgentReconcileV1 =
                    match serde_json::from_value(payload) {
                        Ok(request)
                            if crate::native_agent::validate_reconcile(&request).is_ok() =>
                        {
                            request
                        }
                        _ => {
                            return control_error(
                                StatusCode::BAD_REQUEST,
                                "invalid_native_agent",
                                "Invalid native Agent reconciliation query.",
                            )
                        }
                    };
                if request.target_host_ref != ctx.state.local_host_ref.as_str() {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent reconciliation targets another Host.",
                    );
                }
                let Some(source_host_ref) = peers
                    .iter()
                    .find(|peer| peer.peer_session_id == inbound_peer.peer_session_id)
                    .and_then(|peer| peer.logical_host_ref.as_deref())
                else {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent reconciliation has no bound source Host.",
                    );
                };
                let fact = match ctx.state.native_agents.lock().reconciliation_fact(
                    &room_id,
                    &request.task_id,
                    request.movement_id.as_deref(),
                    ctx.state.local_host_ref.as_str(),
                    source_host_ref,
                ) {
                    Ok(fact) => fact,
                    Err(_) => {
                        return control_error(
                            StatusCode::NOT_FOUND,
                            "native_agent_unavailable",
                            "Native Agent durable fact is unavailable.",
                        )
                    }
                };
                let response_context = match room_control_session_context_for_peer(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                ) {
                    Ok(context) => context,
                    Err(_) => {
                        return control_error(
                            StatusCode::GONE,
                            "host_binding_unavailable",
                            "Native Agent response route is unavailable.",
                        )
                    }
                };
                let response =
                    match serde_json::to_value(fact)
                        .map_err(AppError::from)
                        .and_then(|payload| {
                            native_agent_event(
                                "native_agent.reconciliation",
                                payload,
                                &response_context,
                            )
                        }) {
                        Ok(event) => event,
                        Err(_) => {
                            return control_error(
                                StatusCode::BAD_REQUEST,
                                "invalid_native_agent",
                                "Native Agent reconciliation response is invalid.",
                            )
                        }
                    };
                let response_state = ctx.state.clone();
                let response_room = room_id.clone();
                let response_route =
                    selected_peer_control_route(&room_id, &inbound_peer.peer_session_id);
                ctx.state.spawn(async move {
                    let _ = send_room_control_event(
                        response_state,
                        &response_room,
                        response,
                        Some(response_route),
                    )
                    .await;
                });
            }
            "native_agent.decision" => {
                let Ok(request) =
                    serde_json::from_value::<crate::native_agent::NativeAgentDecisionV1>(payload)
                else {
                    return control_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_native_agent",
                        "Invalid native Agent decision.",
                    );
                };
                // The deciding principal is the exact authenticated current
                // peer session and the device key proven in it.
                let binding = match crate::host_runtime::current_host_session_binding(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                ) {
                    Ok(binding) => binding,
                    Err(_) => {
                        return control_error(
                            StatusCode::FORBIDDEN,
                            "host_mismatch",
                            "Native Agent decision has no bound peer Host.",
                        )
                    }
                };
                let verified_device =
                    crate::host_runtime::verified_peer_device_for(&ctx.state, &binding);
                if let Err(error) = ctx.state.native_agents.lock().submit_native_decision(
                    &binding,
                    verified_device.as_ref(),
                    &request,
                    storage::now_ts(),
                ) {
                    let (code, message) = native_decision_rejection(&error);
                    return control_error(StatusCode::CONFLICT, code, message);
                }
            }
            "native_agent.device_identity" => {
                // Verified against this Host's own current binding of the
                // exact session it arrived on; it admits and authorizes
                // nothing by itself.
                let Ok(attestation) = serde_json::from_value::<
                    crate::device_identity::DeviceSessionAttestationV1,
                >(payload) else {
                    return control_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_native_agent",
                        "Invalid device identity proof.",
                    );
                };
                let verified = crate::host_runtime::current_host_session_binding(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                )
                .and_then(|binding| {
                    crate::device_identity::verify_attestation(&attestation, &binding)
                });
                match verified {
                    Ok(device) => {
                        crate::host_runtime::record_verified_peer_device(&ctx.state, device)
                    }
                    Err(_) => {
                        return control_error(
                            StatusCode::FORBIDDEN,
                            "identity_unverified",
                            "Device identity proof does not match this session.",
                        )
                    }
                }
            }
            "native_agent.retry_result_return" => {
                let request = match serde_json::from_value::<
                    crate::native_agent::NativeAgentRetryResultReturnV1,
                >(payload)
                .map_err(AppError::from)
                .and_then(|request| {
                    crate::native_agent::validate_retry_result_return(&request)?;
                    Ok(request)
                }) {
                    Ok(request) => request,
                    Err(_) => {
                        return control_error(
                            StatusCode::BAD_REQUEST,
                            "invalid_native_agent",
                            "Invalid native Agent result Return retry.",
                        )
                    }
                };
                let Some(source_host_ref) = peers
                    .iter()
                    .find(|peer| peer.peer_session_id == inbound_peer.peer_session_id)
                    .and_then(|peer| peer.logical_host_ref.as_deref())
                else {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent result Return retry has no bound source Host.",
                    );
                };
                if ctx
                    .state
                    .native_agents
                    .lock()
                    .authorize_result_return_retry(
                        &room_id,
                        &request,
                        source_host_ref,
                        ctx.state.local_host_ref.as_str(),
                    )
                    .is_err()
                {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent result Return retry does not match this Bridge session.",
                    );
                }
                let retry_state = ctx.state.clone();
                let retry_room = room_id.clone();
                let movement_id = request.movement_id;
                ctx.state.spawn(async move {
                    if crate::native_agent::retry_workspace_result_return(
                        retry_state.clone(),
                        &retry_room,
                        &movement_id,
                    )
                    .await
                    .is_err()
                    {
                        retry_state
                            .native_agents
                            .lock()
                            .mark_result_return_pending(&movement_id);
                    }
                });
            }
            "native_agent.session_query" => {
                use crate::native_agent::sessions::remote;
                let query: remote::RemoteSessionQueryV1 = match serde_json::from_value(payload) {
                    Ok(query) if remote::validate_query(&query).is_ok() => query,
                    _ => {
                        return control_error(
                            StatusCode::BAD_REQUEST,
                            "invalid_native_agent",
                            "Invalid native session query.",
                        )
                    }
                };
                // The asking principal is the exact authenticated current
                // peer session and the device identity proven in it.
                let binding = match crate::host_runtime::current_host_session_binding(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                ) {
                    Ok(binding) => binding,
                    Err(_) => {
                        return control_error(
                            StatusCode::FORBIDDEN,
                            "host_mismatch",
                            "Native session query has no bound peer Host.",
                        )
                    }
                };
                if query.target_host_ref != ctx.state.local_host_ref.as_str() {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native session query targets another Host.",
                    );
                }
                let response_context = match room_control_session_context_for_peer(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                ) {
                    Ok(context) => context,
                    Err(_) => {
                        return control_error(
                            StatusCode::GONE,
                            "host_binding_unavailable",
                            "Native session response route is unavailable.",
                        )
                    }
                };
                // Only workspaces this Host's own shares name for this device;
                // sessions only of those it may read.
                let scopes = crate::host_runtime::verified_peer_device_for(&ctx.state, &binding)
                    .map(|device| {
                        crate::session_read_access::shared_workspaces(
                            &ctx.state.paths,
                            &device,
                            &binding,
                            storage::now_ts(),
                        )
                        .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let local_host_ref = ctx.state.local_host_ref.as_str().to_owned();
                let answer_state = ctx.state.clone();
                let answer_room = room_id.clone();
                let answer_route =
                    selected_peer_control_route(&room_id, &inbound_peer.peer_session_id);
                ctx.state.spawn(async move {
                    let answer = match remote::begin_answer(&answer_room) {
                        None => remote::busy(&query, &local_host_ref),
                        Some(guard) => {
                            let (claude, codex) =
                                answer_state.native_agents.lock().session_sources();
                            let blocking_host = local_host_ref.clone();
                            let blocking_query = query.clone();
                            let answer = tokio::task::spawn_blocking(move || {
                                remote::answer(
                                    &blocking_query,
                                    &scopes,
                                    &claude,
                                    &codex,
                                    &blocking_host,
                                )
                            })
                            .await;
                            drop(guard);
                            match answer {
                                Ok(answer) => answer,
                                Err(_) => remote::busy(&query, &local_host_ref),
                            }
                        }
                    };
                    if let Ok(payload) = serde_json::to_value(answer) {
                        // The asker waits up to 30 s for this answer.
                        let _ = send_room_control_event_with_backpressure(
                            answer_state,
                            &answer_room,
                            || {
                                native_agent_event(
                                    "native_agent.session_response",
                                    payload.clone(),
                                    &response_context,
                                )
                            },
                            Some(answer_route),
                            Duration::from_secs(10),
                        )
                        .await;
                    }
                });
            }
            "native_agent.session_response" => {
                use crate::native_agent::sessions::remote;
                let answer: remote::RemoteSessionResponseV1 = match serde_json::from_value(payload)
                {
                    Ok(answer) if remote::validate_response(&answer).is_ok() => answer,
                    _ => {
                        return control_error(
                            StatusCode::BAD_REQUEST,
                            "invalid_native_agent",
                            "Invalid native session answer.",
                        )
                    }
                };
                let Ok(binding) = crate::host_runtime::current_host_session_binding(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                ) else {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native session answer has no bound peer Host.",
                    );
                };
                if ctx
                    .state
                    .native_session_queries
                    .lock()
                    .record(&room_id, binding.peer_host_ref.as_str(), answer)
                    .is_err()
                {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native session answer does not match an outstanding query.",
                    );
                }
            }
            "native_agent.reconciliation" => {
                let fact: crate::native_agent::NativeAgentReconciliationV1 =
                    match serde_json::from_value(payload) {
                        Ok(fact) if crate::native_agent::validate_reconciliation(&fact).is_ok() => {
                            fact
                        }
                        _ => {
                            return control_error(
                                StatusCode::BAD_REQUEST,
                                "invalid_native_agent",
                                "Invalid native Agent reconciliation fact.",
                            )
                        }
                    };
                if ctx
                    .state
                    .native_agents
                    .lock()
                    .record_bridge_remote_reconciliation(&room_id, fact)
                    .is_err()
                {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent reconciliation does not match its selected Host.",
                    );
                }
            }
            "native_agent.workspace_prepare" => {
                let request: crate::native_agent::NativeAgentWorkspacePrepareV1 =
                    match serde_json::from_value(payload) {
                        Ok(request)
                            if crate::native_agent::validate_workspace_prepare(&request)
                                .is_ok() =>
                        {
                            request
                        }
                        _ => {
                            return control_error(
                                StatusCode::BAD_REQUEST,
                                "invalid_native_agent_workspace",
                                "Invalid native Agent workspace preparation.",
                            )
                        }
                    };
                if request.target_host_ref != ctx.state.local_host_ref.as_str() {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent workspace targets another Host.",
                    );
                }
                // The exact authenticated peer session is the movement's
                // principal: its HostRef must be the claimed source, and it is
                // the session the landed workspace's executor Review binds.
                let Ok(binding) = crate::host_runtime::current_host_session_binding(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                ) else {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent workspace has no bound source Host.",
                    );
                };
                if request.source_host_ref != binding.peer_host_ref.as_str() {
                    return control_error(
                        StatusCode::FORBIDDEN,
                        "host_mismatch",
                        "Native Agent workspace source is not the authenticated peer Host.",
                    );
                }
                if ctx
                    .state
                    .native_agents
                    .lock()
                    .require_codex_compatibility()
                    .is_err()
                {
                    return control_error(
                        StatusCode::BAD_REQUEST,
                        "native_agent_incompatible",
                        "Native Agent interface is incompatible on this Host.",
                    );
                }
                let movement_id = request.movement_id.clone();
                if ctx
                    .state
                    .native_agents
                    .lock()
                    .accept_bridge_workspace_prepare(&binding, request)
                    .is_err()
                {
                    return control_error(
                        StatusCode::BAD_REQUEST,
                        "native_agent_workspace_rejected",
                        "Native Agent workspace preparation was rejected by this Host.",
                    );
                }
                let runtime = ctx.state.clone();
                let watcher_room = room_id.clone();
                let watcher_peer = inbound_peer.peer_session_id.clone();
                ctx.state.spawn(async move {
                    crate::native_agent::monitor_received_workspace_task(
                        runtime,
                        watcher_room,
                        watcher_peer,
                        movement_id,
                    )
                    .await;
                });
            }
            _ => {
                return control_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_native_agent",
                    "Unsupported native Agent event.",
                )
            }
        }
        return encrypted_receipt_response(&event_key, &validated.event_id, &now_iso());
    }
    // Capability and Bridge Plan events: admitted before anything acts on
    // them, so a replayed or rate-refused event never answers, records or
    // starts anything. If the handler refuses the event afterwards, its
    // replay identities are released again (`AdmittedEventV1`).
    let class = ControlClassV1::of(&validated.kind);
    {
        let mut runtime = ctx.state.room_control.lock();
        let now = runtime.limiter_now();
        let room_state = runtime.rooms.entry(room_id.clone()).or_default();
        if let Err(refused) = admit_received_event(room_state, &validated, class, now, &room_id) {
            return refused;
        }
    }
    let admitted = AdmittedEventV1 {
        state: ctx.state.clone(),
        room_id: room_id.clone(),
        class,
        event_id: validated.event_id.clone(),
        envelope_id: validated.envelope_id.clone(),
        request_id: validated.request_id.clone(),
        kept: false,
    };
    let mut protocol_action = crate::bridge_plan::InboundProtocolAction::None;
    if validated.kind == "peer_capability.response" {
        log_peer_capability("response_received", None, None);
        log_peer_capability("response_validated", None, None);
        let projection: crate::peer_capabilities::PeerCapabilityProjection =
            match serde_json::from_value(
                validated
                    .event
                    .get("payload")
                    .cloned()
                    .unwrap_or(Value::Null),
            ) {
                Ok(value) => value,
                Err(_) => {
                    log_peer_capability("response_rejected", None, Some("invalid_schema"));
                    return control_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_capability",
                        "Invalid peer capability fact.",
                    );
                }
            };
        if ctx
            .state
            .peer_capabilities
            .lock()
            .observe(
                &room_id,
                &inbound_peer.peer_session_id,
                &peer_observation_ref(&inbound_peer),
                projection.clone(),
                storage::now_ts(),
            )
            .is_err()
        {
            log_peer_capability("response_rejected", None, Some("session_or_route_mismatch"));
            return control_error(
                StatusCode::BAD_REQUEST,
                "invalid_capability",
                "Invalid peer capability fact.",
            );
        }
        log_peer_capability_projection("projection_stored", &projection);
    } else if validated.kind == "peer_capability.query" {
        log_peer_capability("query_received", None, None);
        let payload = validated.event.get("payload").and_then(Value::as_object);
        let peer_session_id = payload
            .and_then(|payload| payload.get("peerSessionId"))
            .and_then(Value::as_str);
        let Some((peer_session_id, capability_ids)) = peer_session_id
            .zip(payload.and_then(|payload| requested_peer_capability_ids(payload).ok()))
        else {
            log_peer_capability("query_rejected", None, Some("invalid_schema"));
            return control_error(
                StatusCode::BAD_REQUEST,
                "invalid_capability",
                "Invalid peer capability query.",
            );
        };
        let response_context = RoomControlSessionContext {
            room_id: room_id.clone(),
            local_session_ref: session_ref(&local_key),
            peer_session_ref: session_ref(&inbound_peer.transport_public_key),
            peer_route_ref: inbound_peer.peer_session_id.clone(),
            peer_observation_ref: peer_observation_ref(&inbound_peer),
            peer_connected: true,
        };
        // This is the requester's selected-peer session identifier. The
        // receiver must echo it, rather than substituting the requester's
        // inbound session id, so the requester can bind the fact to its
        // current selected remote peer after authenticated delivery.
        let peer_session_id = peer_session_id.to_owned();
        let response_route = selected_peer_control_route(&room_id, &inbound_peer.peer_session_id);
        let response_state = ctx.state.clone();
        let response_room_id = room_id.clone();
        // The answer is built and sent after this receipt, off the request
        // path: observing a capability can start probe processes (when its
        // cached observation is missing), which must never hold up this or
        // any other request, such as the liveness ping every request needs.
        ctx.state.spawn(async move {
            let probe_state = response_state.clone();
            let projection = tokio::task::spawn_blocking(move || {
                local_peer_capability_response_projection(
                    &probe_state,
                    &peer_session_id,
                    storage::now_ts(),
                    &capability_ids,
                )
            })
            .await;
            let Ok(Ok(projection)) = projection else {
                log_peer_capability("response_rejected", None, Some("construction_failed"));
                return;
            };
            log_peer_capability_projection("local_projection_created", &projection);
            let payload =
                serde_json::to_value(&projection).expect("peer capability projection serializes");
            log_peer_capability_projection("response_dispatch", &projection);
            // The requester waits up to 8 s; a refusal for its rate budget
            // did nothing there, so a fresh response may follow.
            match send_room_control_event_with_backpressure(
                response_state,
                &response_room_id,
                || {
                    peer_capability_event(
                        "peer_capability.response",
                        payload.clone(),
                        &response_context,
                    )
                },
                Some(response_route),
                Duration::from_secs(5),
            )
            .await
            {
                Ok(_) => log_peer_capability_projection("response_delivered", &projection),
                Err(_) => log_peer_capability("response_rejected", None, Some("delivery_failed")),
            }
        });
    } else if validated.kind.starts_with("bridge_plan.v2.") {
        let now = storage::now_ts();
        let captured_binding = match crate::host_runtime::current_host_session_binding(
            &ctx.state,
            &room_id,
            &inbound_peer.peer_session_id,
        ) {
            Ok(binding) => binding,
            Err(_) => {
                return control_error(
                    StatusCode::FORBIDDEN,
                    "host_binding_unavailable",
                    "Bridge Plan v2 Host binding is unavailable.",
                )
            }
        };
        let payload = validated
            .event
            .get("payload")
            .cloned()
            .unwrap_or(Value::Null);
        let mut native_response: Option<(&'static str, Value)> = None;
        let mut native_events = Vec::new();
        let mut committed_attempt: Option<String> = None;
        let mut cancelled_transfer: Option<String> = None;
        let accepted = match validated.kind.as_str() {
            "bridge_plan.v2.review_request" => {
                serde_json::from_value(payload)
                    .map_err(AppError::from)
                    .and_then(|review| {
                        crate::bridge_plan_v2::BridgePlanV2Store::new(&ctx.state.paths)
                            .record_review(&review, &captured_binding, now)
                    })
            }
            "bridge_plan.v2.attempt_start" => {
                let current_binding = crate::host_runtime::current_host_session_binding(
                    &ctx.state,
                    &room_id,
                    &inbound_peer.peer_session_id,
                );
                serde_json::from_value(payload)
                    .map_err(AppError::from)
                    .and_then(|start: crate::bridge_plan_v2::AttemptStartV2| {
                        let current_binding = current_binding?;
                        let coordinated =
                            crate::native_v2_orchestration::coordinated_receiver_review(
                                &ctx.state.paths,
                                &start.correlation_id,
                                &start.attempt_id,
                            )?;
                        let decision = if coordinated {
                            ctx.state.accept_live_v2_managed_attempt_deferred(
                                start.clone(),
                                captured_binding.clone(),
                                current_binding,
                                now,
                            )?
                        } else {
                            ctx.state.accept_live_v2_managed_attempt(
                                start.clone(),
                                captured_binding.clone(),
                                current_binding,
                                now,
                            )?
                        };
                        match decision {
                            crate::bridge_plan_v2::AttemptStartDecisionV2::Accepted(accepted) => {
                                if coordinated {
                                    let prepared =
                                        crate::native_v2_orchestration::record_receiver_prepared(
                                            &ctx.state.paths,
                                            &start,
                                            &accepted,
                                            &captured_binding,
                                            now,
                                        )?;
                                    native_response = Some((
                                        crate::native_v2_orchestration::PREPARED_KIND,
                                        serde_json::to_value(prepared)?,
                                    ));
                                }
                                Ok(())
                            }
                            crate::bridge_plan_v2::AttemptStartDecisionV2::Denied(_) => {
                                Err(AppError::InvalidInput(
                                    "Bridge Plan v2 Host admission denied.".into(),
                                ))
                            }
                        }
                    })
            }
            crate::native_v2_orchestration::READINESS_REQUEST_KIND => {
                match serde_json::from_value(payload).map_err(AppError::from) {
                    Ok(request) => {
                        match crate::native_v2_orchestration::accept_readiness_request(
                            &ctx.state,
                            request,
                            &captured_binding,
                            now,
                        )
                        .await
                        {
                            Ok(result) => {
                                match serde_json::to_value(result).map_err(AppError::from) {
                                    Ok(value) => {
                                        native_response = Some((
                                            crate::native_v2_orchestration::READINESS_KIND,
                                            value,
                                        ));
                                        Ok(())
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            crate::native_v2_orchestration::READINESS_KIND => serde_json::from_value(payload)
                .map_err(AppError::from)
                .and_then(|result| {
                    native_events = crate::native_v2_orchestration::accept_requester_readiness(
                        &ctx.state,
                        result,
                        &captured_binding,
                        now,
                    )?;
                    Ok(())
                }),
            crate::native_v2_orchestration::PREPARED_KIND => serde_json::from_value(payload)
                .map_err(AppError::from)
                .and_then(|prepared| {
                    native_events = crate::native_v2_orchestration::accept_requester_prepared(
                        &ctx.state,
                        prepared,
                        &captured_binding,
                        now,
                    )?;
                    Ok(())
                }),
            crate::native_v2_orchestration::COMMIT_KIND => serde_json::from_value(payload)
                .map_err(AppError::from)
                .and_then(
                    |commit: crate::native_v2_orchestration::NativeV2AttemptCommitV1| {
                        crate::native_v2_orchestration::accept_receiver_commit(
                            &ctx.state,
                            &commit,
                            &crate::host_identity::HostExecutionFreshness::Remote(
                                captured_binding.clone(),
                            ),
                            now,
                        )?;
                        committed_attempt = Some(commit.attempt_id);
                        Ok(())
                    },
                ),
            crate::native_v2_orchestration::STEP_RESULT_KIND => serde_json::from_value(payload)
                .map_err(AppError::from)
                .and_then(|result| {
                    native_events = crate::native_v2_orchestration::accept_requester_step_result(
                        &ctx.state,
                        result,
                        &captured_binding,
                        now,
                    )?;
                    Ok(())
                }),
            crate::native_v2_orchestration::STEP_FAILURE_KIND => serde_json::from_value(payload)
                .map_err(AppError::from)
                .and_then(|failure| {
                    native_events = crate::native_v2_orchestration::accept_requester_step_failure(
                        &ctx.state,
                        failure,
                        &captured_binding,
                        now,
                    )?;
                    Ok(())
                }),
            crate::native_v2_orchestration::STEP_COMMIT_KIND => serde_json::from_value(payload)
                .map_err(AppError::from)
                .and_then(
                    |commit: crate::native_v2_orchestration::NativeV2StepCommitV1| {
                        crate::native_v2_orchestration::accept_receiver_step_commit(
                            &ctx.state,
                            &commit,
                            &crate::host_identity::HostExecutionFreshness::Remote(
                                captured_binding.clone(),
                            ),
                            now,
                        )?;
                        committed_attempt = Some(commit.attempt_id);
                        Ok(())
                    },
                ),
            crate::native_v2_orchestration::CANCEL_KIND => serde_json::from_value(payload)
                .map_err(AppError::from)
                .and_then(
                    |cancel: crate::native_v2_orchestration::NativeV2AttemptCancelV1| {
                        cancelled_transfer =
                            crate::native_v2_orchestration::accept_receiver_cancel(
                                &ctx.state,
                                &cancel,
                                &crate::host_identity::HostExecutionFreshness::Remote(
                                    captured_binding.clone(),
                                ),
                                now,
                            )?;
                        let _ = ctx
                            .state
                            .cancel_live_v2_managed_attempt(&cancel.attempt_id, now);
                        Ok(())
                    },
                ),
            _ => Err(AppError::InvalidInput(
                "Unsupported Bridge Plan protocol v2 event.".into(),
            )),
        };
        if let Err(error) = accepted {
            let reason = bridge_plan_protocol_rejection_reason(&error);
            log_bridge_plan_validation_rejected(&validated.event, reason);
            return control_error(
                StatusCode::BAD_REQUEST,
                reason,
                "Bridge Plan v2 validation or Host admission failed.",
            );
        }
        if let Some((kind, payload)) = native_response {
            let response_context = match room_control_session_context_for_peer(
                &ctx.state,
                &room_id,
                &inbound_peer.peer_session_id,
            ) {
                Ok(context) => context,
                Err(_) => {
                    return control_error(
                        StatusCode::GONE,
                        "host_binding_unavailable",
                        "Native v2 response route is unavailable.",
                    )
                }
            };
            let response = match crate::native_v2_orchestration::control_event_for_session(
                kind,
                payload,
                &response_context,
            ) {
                Ok(event) => event,
                Err(_) => {
                    return control_error(
                        StatusCode::BAD_REQUEST,
                        "native_v2_response_invalid",
                        "Native v2 response could not be constructed.",
                    )
                }
            };
            let response_state = ctx.state.clone();
            let response_room = room_id.clone();
            let response_route =
                selected_peer_control_route(&room_id, &inbound_peer.peer_session_id);
            ctx.state.spawn(async move {
                let _ = send_room_control_event(
                    response_state,
                    &response_room,
                    response,
                    Some(response_route),
                )
                .await;
            });
        }
        crate::native_v2_orchestration::spawn_native_v2_events(ctx.state.clone(), native_events);
        if let Some(attempt_id) = validated
            .event
            .get("payload")
            .and_then(|payload| payload.get("attemptId"))
            .and_then(Value::as_str)
        {
            crate::native_v2_orchestration::emit_product_status_for_attempt(&ctx.state, attempt_id);
        }
        if let Some(attempt_id) = committed_attempt {
            crate::native_v2_orchestration::start_receiver_attempt(
                ctx.state.clone(),
                attempt_id,
                crate::host_identity::HostExecutionFreshness::Remote(captured_binding.clone()),
            );
        }
        if let Some(transfer_id) = cancelled_transfer {
            let cancel_state = ctx.state.clone();
            ctx.state.spawn(async move {
                let _ = crate::transfer::cancel_transfer(
                    cancel_state,
                    &transfer_id,
                    Some("native_v2_requester_cancelled".into()),
                )
                .await;
            });
        }
    } else {
        protocol_action = match crate::bridge_plan::accept_inbound_protocol_event(
            &ctx.state.paths,
            &ctx.state.bridge_plan_protocol_authority.lock(),
            &mut ctx.state.bridge_plan_candidate_store.lock(),
            &validated.kind,
            &validated.event,
            storage::now_ts(),
        ) {
            Ok(action) => action,
            Err(error) => {
                let reason = bridge_plan_protocol_rejection_reason(&error);
                log_bridge_plan_validation_rejected(&validated.event, reason);
                return control_error(
                    StatusCode::BAD_REQUEST,
                    reason,
                    "Bridge Plan review validation failed.",
                );
            }
        };
    }
    let return_route = selected_peer_control_route(&room_id, &inbound_peer.peer_session_id);
    match protocol_action {
        crate::bridge_plan::InboundProtocolAction::None => {}
        crate::bridge_plan::InboundProtocolAction::ExecuteSearch { attempt_id } => {
            let execution_state = ctx.state.clone();
            let execution_room = room_id.clone();
            ctx.state.spawn(async move {
                let _ = crate::commands::execute_bridge_plan_search_attempt_inner(
                    execution_state,
                    execution_room,
                    attempt_id,
                    Some(return_route),
                )
                .await;
            });
        }
        crate::bridge_plan::InboundProtocolAction::ExecuteTransfer { attempt_id } => {
            let execution_state = ctx.state.clone();
            let execution_room = room_id.clone();
            ctx.state.spawn(async move {
                let _ = crate::commands::execute_bridge_plan_transfer_attempt_inner(
                    execution_state,
                    execution_room,
                    attempt_id,
                    Some(return_route),
                )
                .await;
            });
        }
        crate::bridge_plan::InboundProtocolAction::ContinueAttempt { attempt_id } => {
            let continuation_state = ctx.state.clone();
            let continuation_room = room_id.clone();
            ctx.state.spawn(async move {
                let _ = crate::commands::continue_bridge_plan_attempt_inner(
                    continuation_state,
                    continuation_room,
                    attempt_id,
                    Some(return_route),
                )
                .await;
            });
        }
    }
    let received_at = now_iso();
    // Capability facts and protocol v2 events were acted on above and keep
    // no copy here. Bridge Plan v1 events are also kept, as bounded display
    // history (Find File reads its progress from them).
    if validated.kind.starts_with("bridge_plan.") && !validated.kind.starts_with("bridge_plan.v2.")
    {
        log_bridge_plan_control_event(&validated, "inbound_persisted");
        let mut runtime = ctx.state.room_control.lock();
        let room_state = runtime.rooms.entry(room_id.clone()).or_default();
        keep_in_history(
            room_state,
            ReceivedRoomControlEvent {
                event_id: validated.event_id.clone(),
                kind: validated.kind,
                room_ref: validated.room_ref,
                source_device_ref: validated.source_device_ref,
                target_peer_ref: validated.target_peer_ref,
                created_at: validated.created_at,
                expires_at: validated.expires_at,
                received_at: received_at.clone(),
                event: validated.event,
            },
            storage::now_ts(),
        );
    }
    admitted.keep();
    encrypted_receipt_response(&event_key, &validated.event_id, &received_at)
}

fn validate_control_event(
    event: Value,
    expected_room: &str,
    expected_source: &str,
    expected_target: &str,
    now: OffsetDateTime,
) -> AppResult<ValidatedControlEvent> {
    if serde_json::to_vec(&event)?.len() > MAX_CONTROL_EVENT_BYTES {
        return Err(AppError::InvalidInput(
            "Room control event is too large.".into(),
        ));
    }
    let object = event
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("Invalid room control event.".into()))?;
    let raw_kind = object
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    require_exact_fields(
        object,
        if raw_kind.starts_with("bridge_plan.")
            || raw_kind.starts_with("peer_capability.")
            || raw_kind.starts_with("developer_terminal.")
            || raw_kind.starts_with("bridge_membership.")
            || raw_kind.starts_with("native_agent.")
        {
            &[
                "schemaVersion",
                "eventId",
                "kind",
                "protocolFamily",
                "roomRef",
                "sourceDeviceRef",
                "targetPeerRef",
                "createdAt",
                "expiresAt",
                "previewOnly",
                "payload",
            ]
        } else {
            &[
                "schemaVersion",
                "eventId",
                "kind",
                "roomRef",
                "sourceDeviceRef",
                "targetPeerRef",
                "createdAt",
                "expiresAt",
                "previewOnly",
                "payload",
            ]
        },
    )?;
    if string_field(object, "schemaVersion")? != ROOM_CONTROL_SCHEMA {
        return Err(AppError::InvalidInput("Invalid room control event.".into()));
    }
    let event_id = bounded_string_field(object, "eventId", 256)?;
    let kind = string_field(object, "kind")?.to_string();
    if !ALLOWED_EVENT_KINDS.contains(&kind.as_str()) {
        return Err(AppError::InvalidInput(
            "Unsupported room control event kind.".into(),
        ));
    }
    let room_ref = bounded_string_field(object, "roomRef", 256)?;
    let source_device_ref = bounded_string_field(object, "sourceDeviceRef", 256)?;
    let target_peer_ref = bounded_string_field(object, "targetPeerRef", 256)?;
    if room_ref != expected_room
        || source_device_ref != expected_source
        || target_peer_ref != expected_target
    {
        return Err(AppError::InvalidInput(
            "Room control event session mismatch.".into(),
        ));
    }
    let created_at = string_field(object, "createdAt")?.to_string();
    let expires_at = string_field(object, "expiresAt")?.to_string();
    let created = OffsetDateTime::parse(&created_at, &Rfc3339)
        .map_err(|_| AppError::InvalidInput("Invalid room control event time.".into()))?;
    let expires = OffsetDateTime::parse(&expires_at, &Rfc3339)
        .map_err(|_| AppError::InvalidInput("Invalid room control event time.".into()))?;
    if expires <= now {
        return Err(AppError::InvalidInput("Room control event expired.".into()));
    }
    if expires <= created || expires - created > time::Duration::seconds(MAX_EVENT_LIFETIME_SECONDS)
    {
        return Err(AppError::InvalidInput(
            "Invalid room control event lifetime.".into(),
        ));
    }
    let payload = object
        .get("payload")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::InvalidInput("Invalid room control event payload.".into()))?;
    let allow_validated_native_code = matches!(
        kind.as_str(),
        "native_agent.status" | "native_agent.reconciliation"
    );
    if contains_unsafe_field(
        &transport_field_view(&kind, &event),
        allow_validated_native_code,
    ) {
        return Err(AppError::InvalidInput(
            "Room control event contains unsafe fields.".into(),
        ));
    }
    let (envelope_id, request_id) = if kind.starts_with("bridge_plan.") {
        if string_field(object, "protocolFamily")? != BRIDGE_PLAN_PROTOCOL_FAMILY
            || object.get("previewOnly") != Some(&Value::Bool(false))
        {
            return Err(AppError::InvalidInput(
                "Invalid Bridge Plan protocol event.".into(),
            ));
        }
        let replay_id = if kind.starts_with("bridge_plan.v2.") {
            crate::bridge_plan_v2::protocol_metadata(
                &kind,
                payload,
                expected_room,
                now.unix_timestamp(),
            )?
            .replay_id
        } else {
            crate::bridge_plan::protocol_metadata(
                &kind,
                payload,
                expected_room,
                expected_source,
                expected_target,
                now.unix_timestamp(),
            )?
            .replay_id
        };
        (None, Some(replay_id))
    } else if kind.starts_with("peer_capability.") {
        if string_field(object, "protocolFamily")? != PEER_CAPABILITY_PROTOCOL_FAMILY
            || object.get("previewOnly") != Some(&Value::Bool(false))
        {
            return Err(AppError::InvalidInput(
                "Invalid peer capability event.".into(),
            ));
        }
        match kind.as_str() {
            "peer_capability.query" => {
                require_exact_fields(
                    payload,
                    &["schemaVersion", "peerSessionId", "capabilityIds"],
                )?;
                if string_field(payload, "schemaVersion")?
                    != crate::peer_capabilities::PEER_CAPABILITY_SCHEMA
                    || bounded_string_field(payload, "peerSessionId", 256)?.is_empty()
                {
                    return Err(AppError::InvalidInput(
                        "Invalid peer capability query.".into(),
                    ));
                }
                requested_peer_capability_ids(payload)?;
            }
            "peer_capability.response" => {
                let projection: crate::peer_capabilities::PeerCapabilityProjection =
                    serde_json::from_value(Value::Object(payload.clone())).map_err(|_| {
                        AppError::InvalidInput("Invalid peer capability projection.".into())
                    })?;
                crate::peer_capabilities::validate_projection(&projection)?;
            }
            _ => {
                return Err(AppError::InvalidInput(
                    "Unsupported peer capability event kind.".into(),
                ))
            }
        }
        (None, None)
    } else if kind.starts_with("developer_terminal.") {
        if string_field(object, "protocolFamily")? != DEVELOPER_TERMINAL_PROTOCOL_FAMILY
            || object.get("previewOnly") != Some(&Value::Bool(false))
        {
            return Err(AppError::InvalidInput(
                "Invalid Developer Terminal protocol event.".into(),
            ));
        }
        let _ = crate::developer_terminal::validate_wire_message(&kind, payload)?;
        (None, None)
    } else if kind.starts_with("native_agent.") {
        if string_field(object, "protocolFamily")? != NATIVE_AGENT_PROTOCOL_FAMILY
            || object.get("previewOnly") != Some(&Value::Bool(false))
        {
            return Err(AppError::InvalidInput(
                "Invalid native Agent protocol event.".into(),
            ));
        }
        let replay_id = match kind.as_str() {
            "native_agent.invoke" => {
                let request = crate::native_agent::parse_invoke(Value::Object(payload.clone()))?;
                format!("native-agent-invoke:{}", request.task_id)
            }
            "native_agent.status" => {
                let status = serde_json::from_value::<crate::native_agent::NativeAgentStatusV1>(
                    Value::Object(payload.clone()),
                )
                .map_err(AppError::from)?;
                crate::native_agent::validate_status(&status)?;
                format!(
                    "native-agent-status:{}:{}",
                    status.task_id,
                    canonical_safe_fact_digest(&status)?
                )
            }
            "native_agent.cancel" => {
                let cancel = serde_json::from_value::<crate::native_agent::NativeAgentCancelV1>(
                    Value::Object(payload.clone()),
                )
                .map_err(AppError::from)?;
                crate::native_agent::validate_cancel(&cancel)?;
                format!("native-agent-cancel:{}", cancel.task_id)
            }
            "native_agent.workspace_prepare" => {
                let request = serde_json::from_value::<
                    crate::native_agent::NativeAgentWorkspacePrepareV1,
                >(Value::Object(payload.clone()))
                .map_err(AppError::from)?;
                crate::native_agent::validate_workspace_prepare(&request)?;
                format!("native-agent-workspace-prepare:{}", request.movement_id)
            }
            "native_agent.reconcile" => {
                let request =
                    serde_json::from_value::<crate::native_agent::NativeAgentReconcileV1>(
                        Value::Object(payload.clone()),
                    )
                    .map_err(AppError::from)?;
                crate::native_agent::validate_reconcile(&request)?;
                // Reconciliation is a read-only query. Its authenticated event
                // id is replay-protected, but stable task/movement identity is
                // intentionally not a once-only mutation key.
                return Ok(ValidatedControlEvent {
                    event_id,
                    kind,
                    room_ref,
                    source_device_ref,
                    target_peer_ref,
                    created_at,
                    expires_at,
                    envelope_id: None,
                    request_id: None,
                    event,
                });
            }
            "native_agent.decision" => {
                let request = serde_json::from_value::<crate::native_agent::NativeAgentDecisionV1>(
                    Value::Object(payload.clone()),
                )
                .map_err(AppError::from)?;
                crate::native_agent::validate_decision(&request)?;
                format!(
                    "native-agent-decision:{}:{}:{}",
                    request.task_id, request.review_ref, request.decision_id
                )
            }
            "native_agent.device_identity" => {
                let attestation = serde_json::from_value::<
                    crate::device_identity::DeviceSessionAttestationV1,
                >(Value::Object(payload.clone()))
                .map_err(AppError::from)?;
                if attestation.schema_version != crate::device_identity::DEVICE_ATTESTATION_SCHEMA
                    || attestation.nonce.len() > 64
                {
                    return Err(AppError::InvalidInput(
                        "Invalid device identity proof.".into(),
                    ));
                }
                format!("native-agent-device-identity:{}", attestation.nonce)
            }
            "native_agent.retry_result_return" => {
                let request = serde_json::from_value::<
                    crate::native_agent::NativeAgentRetryResultReturnV1,
                >(Value::Object(payload.clone()))
                .map_err(AppError::from)?;
                crate::native_agent::validate_retry_result_return(&request)?;
                format!(
                    "native-agent-result-return-retry:{}:{}",
                    request.movement_id, request.retry_id
                )
            }
            "native_agent.reconciliation" => {
                let fact =
                    serde_json::from_value::<crate::native_agent::NativeAgentReconciliationV1>(
                        Value::Object(payload.clone()),
                    )
                    .map_err(AppError::from)?;
                crate::native_agent::validate_reconciliation(&fact)?;
                format!(
                    "native-agent-reconciliation:{}:{}:{}",
                    fact.task_id,
                    fact.movement_id.clone().unwrap_or_default(),
                    canonical_safe_fact_digest(&fact)?
                )
            }
            "native_agent.session_query" => {
                let query = serde_json::from_value::<
                    crate::native_agent::sessions::remote::RemoteSessionQueryV1,
                >(Value::Object(payload.clone()))
                .map_err(AppError::from)?;
                crate::native_agent::sessions::remote::validate_query(&query)?;
                format!("native-session-query:{}", query.request_id)
            }
            "native_agent.session_response" => {
                let response = serde_json::from_value::<
                    crate::native_agent::sessions::remote::RemoteSessionResponseV1,
                >(Value::Object(payload.clone()))
                .map_err(AppError::from)?;
                crate::native_agent::sessions::remote::validate_response(&response)?;
                format!("native-session-response:{}", response.request_id)
            }
            _ => {
                return Err(AppError::InvalidInput(
                    "Unsupported native Agent event kind.".into(),
                ))
            }
        };
        (None, Some(replay_id))
    } else if kind == "bridge_membership.departure" {
        if string_field(object, "protocolFamily")? != BRIDGE_MEMBERSHIP_PROTOCOL_FAMILY
            || object.get("previewOnly") != Some(&Value::Bool(false))
        {
            return Err(AppError::InvalidInput(
                "Invalid Bridge membership protocol event.".into(),
            ));
        }
        require_exact_fields(payload, &["schemaVersion", "departureKind"])?;
        if string_field(payload, "schemaVersion")? != BRIDGE_MEMBERSHIP_SCHEMA
            || !matches!(string_field(payload, "departureKind")?, "leave" | "burn")
        {
            return Err(AppError::InvalidInput(
                "Invalid Bridge departure event.".into(),
            ));
        }
        (None, None)
    } else if kind == "bridge_membership.carrier_card" {
        if string_field(object, "protocolFamily")? != BRIDGE_MEMBERSHIP_PROTOCOL_FAMILY
            || object.get("previewOnly") != Some(&Value::Bool(false))
        {
            return Err(AppError::InvalidInput(
                "Invalid Bridge membership protocol event.".into(),
            ));
        }
        require_exact_fields(payload, &["schemaVersion", "card"])?;
        if string_field(payload, "schemaVersion")? != BRIDGE_MEMBERSHIP_SCHEMA {
            return Err(AppError::InvalidInput("Invalid carrier card event.".into()));
        }
        let card = serde_json::from_value::<crate::connectivity::CarrierCardV1>(
            payload.get("card").cloned().unwrap_or(Value::Null),
        )
        .map_err(|_| AppError::InvalidInput("Invalid carrier card event.".into()))?;
        crate::connectivity::validate_card(&card)?;
        (None, None)
    } else {
        return Err(AppError::InvalidInput(
            "Unsupported room control event kind.".into(),
        ));
    };
    Ok(ValidatedControlEvent {
        event_id,
        kind,
        room_ref,
        source_device_ref,
        target_peer_ref,
        created_at,
        expires_at,
        envelope_id,
        request_id,
        event,
    })
}

fn encrypted_receipt_response(event_key: &[u8; 32], event_id: &str, received_at: &str) -> Response {
    let receipt = RoomControlDeliveryReceipt {
        schema_version: CONTROL_DELIVERY_SCHEMA.into(),
        event_id: event_id.into(),
        accepted_for_local_inbox: true,
        received_at: received_at.into(),
    };
    let plaintext = match serde_json::to_vec(&receipt) {
        Ok(value) => value,
        Err(_) => {
            return control_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "receipt_failed",
                "Room control receipt failed.",
            )
        }
    };
    let (ciphertext, nonce) = match crypto::encrypt_bytes(&plaintext, event_key) {
        Ok(value) => value,
        Err(_) => {
            return control_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "receipt_failed",
                "Room control receipt failed.",
            )
        }
    };
    let envelope = EncryptedRoomControlReceipt {
        schema_version: CONTROL_RECEIPT_ENVELOPE_SCHEMA.into(),
        receipt_nonce: crypto::encode_nonce(&nonce),
        ciphertext: STANDARD.encode(ciphertext),
    };
    let body = match serde_json::to_vec(&envelope) {
        Ok(body) if body.len() <= MAX_CONTROL_RESPONSE_BYTES => body,
        _ => {
            return control_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "receipt_failed",
                "Room control receipt failed.",
            )
        }
    };
    (
        StatusCode::ACCEPTED,
        [(header::CONTENT_TYPE, CONTROL_RECEIPT_CONTENT_TYPE)],
        body,
    )
        .into_response()
}

/// A fixed, content-free reason a decision was not given to Codex.
fn native_decision_rejection(error: &AppError) -> (&'static str, &'static str) {
    let message = error.message();
    if message.contains("expired") {
        ("native_decision_expired", "This request expired.")
    } else if message.contains("already given") || message.contains("no longer waits") {
        (
            "native_decision_resolved",
            "Codex no longer waits for this decision.",
        )
    } else if message.contains("changed") {
        (
            "native_decision_changed",
            "What Codex asks changed; review it again.",
        )
    } else if message.contains("no longer running") || message.contains("being stopped") {
        (
            "native_decision_task_ended",
            "This task is no longer running.",
        )
    } else {
        (
            "native_decision_unauthorized",
            "Only the device that started this task, in a current session, can decide its requests.",
        )
    }
}

fn control_error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, CONTROL_ERROR_CONTENT_TYPE)],
        Json(ControlError {
            code: code.into(),
            message: message.into(),
        }),
    )
        .into_response()
}

fn terminal_control_rejection(error: &AppError) -> (StatusCode, &'static str, &'static str) {
    let message = error.message();
    if message.contains("sequence mismatch") {
        return (
            StatusCode::CONFLICT,
            "terminal_sequence_rejected",
            "Developer Terminal sequence was rejected.",
        );
    }
    if message.contains("authority")
        || message.contains("grant")
        || message.contains("session is unavailable")
        || message.contains("Host session binding")
        || message.contains("Host mismatch")
    {
        return (
            StatusCode::FORBIDDEN,
            "terminal_authority_rejected",
            "Developer Terminal authority rejected the event.",
        );
    }
    (
        StatusCode::BAD_REQUEST,
        "terminal_event_rejected",
        "Developer Terminal rejected the event.",
    )
}

/// Why a Room Control delivery failed, as far as the sender can know.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlDeliveryFailureV1 {
    /// Never sent: no current route, session or room here.
    NotSent,
    /// The receiver's rate budget for this kind of event is spent. It
    /// refuses before acting on anything, so nothing happened there and a
    /// fresh event may be sent after a short wait.
    RateLimited,
    /// The receiver already holds as many Reviews from this Bridge as it
    /// allows. Nothing was queued and no waiting Review was dropped.
    ReviewCapacity,
    /// The receiver refused this device or this request (authority,
    /// interface, target, validity) before admitting anything.
    Refused,
    /// The receiver had already received this exact event.
    Replayed,
    Expired,
    /// An older receiver whose inbox was full. It refused only after it had
    /// acted on the event, so this says nothing certain.
    LegacyInboxFull,
    /// Refused for another reason; whether it acted first is not known.
    Rejected,
    /// No answer, or one that cannot be read: whether it arrived is unknown.
    Transport,
}

impl ControlDeliveryFailureV1 {
    /// The receiver provably did nothing with it: an invocation it carried
    /// never started there.
    pub(crate) fn proves_nothing_happened(self) -> bool {
        matches!(
            self,
            Self::NotSent | Self::RateLimited | Self::ReviewCapacity | Self::Refused
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::NotSent => "not_sent",
            Self::RateLimited => "rate_limited",
            Self::ReviewCapacity => "review_capacity",
            Self::Refused => "refused",
            Self::Replayed => "replayed",
            Self::Expired => "expired",
            Self::LegacyInboxFull => "inbox_full",
            Self::Rejected => "rejected",
            Self::Transport => "transport",
        }
    }
}

/// Each refusal code a receiver sends, the sender's message for it, and
/// what it means. The one table both produces and classifies the message.
const DELIVERY_REFUSALS: &[(&str, &str, ControlDeliveryFailureV1)] = {
    use ControlDeliveryFailureV1::*;
    &[
        ("event_expired", "Room control event expired before delivery.", Expired),
        ("event_replayed", "Room control event was already received.", Replayed),
        ("native_decision_expired", "This request expired.", Rejected),
        ("native_decision_resolved", "Codex no longer waits for this decision.", Rejected),
        ("native_decision_changed", "What Codex asks changed; review it again.", Rejected),
        ("native_decision_task_ended", "This task is no longer running.", Rejected),
        (
            "native_decision_unauthorized",
            "Only the device that started this task, in a current session, can decide its requests.",
            Rejected,
        ),
        ("session_mismatch", "Room control session mismatch.", Rejected),
        (
            "rate_limited",
            "Room control rate limit was reached: the other device refused this before acting on it. Try again in a few seconds.",
            RateLimited,
        ),
        (
            "inbox_full",
            "Room control inbox is full on the other device, which runs an older Pastey. Update Pastey there, or restart it.",
            LegacyInboxFull,
        ),
        (
            "native_agent_review_capacity",
            "The other device already holds as many requests from this Bridge waiting for Accept as it allows. Nothing was queued; try again after someone there answers one.",
            ReviewCapacity,
        ),
        (
            "native_agent_not_authorized",
            "The other device refused: this device may not ask to run agents in that folder.",
            Refused,
        ),
        (
            "native_agent_incompatible",
            "The other device's Native Agent interface is incompatible.",
            Refused,
        ),
        ("host_mismatch", "That request was for another device.", Refused),
        (
            "invalid_native_agent",
            "The other device found the Native Agent request invalid.",
            Refused,
        ),
        (
            "native_agent_unavailable",
            "The other device rejected the Native Agent request.",
            Rejected,
        ),
        ("terminal_rate_limited", "Developer Terminal flow-control limit was reached.", RateLimited),
        ("terminal_sequence_rejected", "Developer Terminal sequence was rejected.", Rejected),
        ("terminal_authority_rejected", "Developer Terminal authority rejected the event.", Rejected),
        ("terminal_event_rejected", "Developer Terminal rejected the event.", Rejected),
        ("request_too_large", "Room control event is too large.", Rejected),
        ("event_too_large", "Room control event is too large.", Rejected),
        (
            "review_revision_hash_mismatch",
            "Bridge Plan review validation failed: review_revision_hash_mismatch.",
            Rejected,
        ),
        (
            "review_step_digest_mismatch",
            "Bridge Plan review validation failed: review_step_digest_mismatch.",
            Rejected,
        ),
        (
            "review_session_mismatch",
            "Bridge Plan review validation failed: review_session_mismatch.",
            Rejected,
        ),
        (
            "review_unknown_approval",
            "Bridge Plan review validation failed: review_unknown_approval.",
            Rejected,
        ),
        (
            "review_expiry_invalid",
            "Bridge Plan review validation failed: review_expired.",
            Rejected,
        ),
        ("review_expired", "Bridge Plan review validation failed: review_expired.", Rejected),
        (
            "review_payload_invalid",
            "Bridge Plan review validation failed: review_payload_invalid.",
            Rejected,
        ),
        (
            "attempt_missing",
            "Bridge Plan protocol validation failed: attempt_missing.",
            Rejected,
        ),
        (
            "attempt_correlation_mismatch",
            "Bridge Plan protocol validation failed: attempt_correlation_mismatch.",
            Rejected,
        ),
        ("invalid_event", "Room control event validation failed.", Rejected),
        ("invalid_envelope", "Room control event validation failed.", Rejected),
        ("invalid_request", "Room control event validation failed.", Rejected),
    ]
};

/// What one failed `send_room_control_event` means for the sender.
pub(crate) fn delivery_failure(error: &AppError) -> ControlDeliveryFailureV1 {
    match error {
        AppError::Network(message) => DELIVERY_REFUSALS
            .iter()
            .find(|(_, known, _)| known == message)
            .map_or(ControlDeliveryFailureV1::Transport, |(_, _, failure)| {
                *failure
            }),
        AppError::Timeout(_) | AppError::Http(_) => ControlDeliveryFailureV1::Transport,
        // Preparation failed here: no route, room or session; never sent.
        AppError::InvalidInput(_) | AppError::NotFound(_) => ControlDeliveryFailureV1::NotSent,
        _ => ControlDeliveryFailureV1::Transport,
    }
}

async fn control_response_failure(response: reqwest::Response) -> AppError {
    let status = response.status();
    let error_code = if response.content_length().unwrap_or(0) <= MAX_CONTROL_RESPONSE_BYTES as u64
    {
        response
            .bytes()
            .await
            .ok()
            .filter(|body| body.len() <= MAX_CONTROL_RESPONSE_BYTES)
            .and_then(|body| serde_json::from_slice::<ControlError>(&body).ok())
            .map(|error| error.code)
    } else {
        None
    };
    let known = error_code
        .as_deref()
        .and_then(|code| {
            DELIVERY_REFUSALS
                .iter()
                .find(|(known, _, _)| *known == code)
        })
        .map(|(_, message, _)| *message);
    let message = known.unwrap_or(match status {
        StatusCode::CONFLICT => "Room control event was already received.",
        StatusCode::GONE => "Room control event or room is unavailable.",
        StatusCode::PAYLOAD_TOO_LARGE => "Room control event is too large.",
        StatusCode::TOO_MANY_REQUESTS => "Room control transport rejected the event.",
        StatusCode::FORBIDDEN => "Room control session mismatch.",
        _ => "Room control delivery failed.",
    });
    AppError::Network(message.into())
}

fn canonical_safe_fact_digest<T: Serialize>(value: &T) -> AppResult<String> {
    Ok(blake3::hash(&serde_json::to_vec(value)?)
        .to_hex()
        .to_string())
}

fn record_replay_id(queue: &mut VecDeque<String>, set: &mut HashSet<String>, id: String) {
    if set.insert(id.clone()) {
        queue.push_back(id);
    }
    while queue.len() > MAX_REPLAY_ITEMS {
        if let Some(removed) = queue.pop_front() {
            set.remove(&removed);
        }
    }
}

fn forget_replay_id(queue: &mut VecDeque<String>, set: &mut HashSet<String>, id: &str) {
    set.remove(id);
    queue.retain(|candidate| candidate != id);
}

fn is_replayed(room: &RoomControlRoomState, event: &ValidatedControlEvent) -> bool {
    room.seen_event_id_set.contains(&event.event_id)
        || event
            .envelope_id
            .as_ref()
            .is_some_and(|id| room.seen_envelope_id_set.contains(id))
        || event
            .request_id
            .as_ref()
            .is_some_and(|id| room.seen_request_id_set.contains(id))
}

fn accept_rate_limited_event(
    room: &mut RoomControlRoomState,
    class: ControlClassV1,
    now_seconds: i64,
) -> bool {
    let (per_minute, burst) = class.budget();
    let window = room.received_at_seconds.entry(class).or_default();
    while window
        .front()
        .is_some_and(|timestamp| *timestamp <= now_seconds - 60)
    {
        window.pop_front();
    }
    let burst_count = window
        .iter()
        .filter(|timestamp| **timestamp > now_seconds - 2)
        .count();
    if window.len() >= per_minute || burst_count >= burst {
        return false;
    }
    window.push_back(now_seconds);
    true
}

/// Admits one received event before anything acts on it: it must be new to
/// this Bridge's replay window and fit its class's rate budget. Only an
/// admitted event records its replay identities; a refusal changes nothing
/// but the counters, so the sender may send a fresh event later.
fn admit_received_event(
    room: &mut RoomControlRoomState,
    validated: &ValidatedControlEvent,
    class: ControlClassV1,
    limiter_now: i64,
    room_id: &str,
) -> Result<(), Response> {
    if is_replayed(room, validated) {
        note_refusal(room, class, "event_replayed", room_id);
        return Err(control_error(
            StatusCode::CONFLICT,
            "event_replayed",
            "Room control event was already received.",
        ));
    }
    if !accept_rate_limited_event(room, class, limiter_now) {
        note_refusal(room, class, "rate_limited", room_id);
        return Err(control_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Room control event rate exceeded.",
        ));
    }
    note_acceptance(room, class, room_id);
    record_replay_id(
        &mut room.seen_event_ids,
        &mut room.seen_event_id_set,
        validated.event_id.clone(),
    );
    if let Some(id) = validated.envelope_id.clone() {
        record_replay_id(
            &mut room.seen_envelope_ids,
            &mut room.seen_envelope_id_set,
            id,
        );
    }
    if let Some(id) = validated.request_id.clone() {
        record_replay_id(
            &mut room.seen_request_ids,
            &mut room.seen_request_id_set,
            id,
        );
    }
    Ok(())
}

fn short_room(room_id: &str) -> &str {
    room_id.get(..8).unwrap_or(room_id)
}

fn note_refusal(
    room: &mut RoomControlRoomState,
    class: ControlClassV1,
    reason: &'static str,
    room_id: &str,
) {
    let now = storage::now_ts();
    let stats = &mut room.stats;
    *stats.refused.entry((class, reason)).or_default() += 1;
    stats.last_refusal = Some(ControlRefusalV1 {
        class,
        reason,
        at: now,
    });
    if reason != "rate_limited" {
        return;
    }
    let run = stats.backpressure.entry(class).or_insert((now, 0));
    run.1 += 1;
    // The first refusal of a run, then one line per 100: never a flood.
    if run.1 == 1 || run.1.is_multiple_of(100) {
        logging::write_transfer_line(&format!(
            "[pastey room-control] event=backpressure room={} class={} refused={} since_s={}",
            short_room(room_id),
            class.label(),
            run.1,
            now - run.0
        ));
    }
}

fn note_acceptance(room: &mut RoomControlRoomState, class: ControlClassV1, room_id: &str) {
    let stats = &mut room.stats;
    *stats.accepted.entry(class).or_default() += 1;
    if let Some((since, refused)) = stats.backpressure.remove(&class) {
        logging::write_transfer_line(&format!(
            "[pastey room-control] event=backpressure_ended room={} class={} refused={} lasted_s={}",
            short_room(room_id),
            class.label(),
            refused,
            storage::now_ts() - since
        ));
    }
}

/// Keeps one Bridge Plan event for display, dropping the oldest (and any
/// older than the history's age) first. Never a reason to refuse an event.
fn keep_in_history(room: &mut RoomControlRoomState, event: ReceivedRoomControlEvent, now: i64) {
    let before = room.history.len();
    room.history.retain(|kept| {
        OffsetDateTime::parse(
            &kept.received_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_or(true, |at| {
            at.unix_timestamp() > now - MAX_HISTORY_AGE_SECONDS
        })
    });
    while room.history.len() >= MAX_HISTORY_ITEMS {
        room.history.pop_front();
    }
    room.stats.history_dropped += (before - room.history.len()) as u64;
    room.history.push_back(event);
}

/// An admitted capability or Bridge Plan event. If the handler refuses it
/// after admission (validation, authority, session), its replay identities
/// are released again, so a refused event leaves no trace beyond the
/// counters, exactly as when these checks came last.
struct AdmittedEventV1 {
    state: Arc<AppState>,
    room_id: String,
    class: ControlClassV1,
    event_id: String,
    envelope_id: Option<String>,
    request_id: Option<String>,
    kept: bool,
}

impl AdmittedEventV1 {
    fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for AdmittedEventV1 {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        let mut runtime = self.state.room_control.lock();
        let Some(room) = runtime.rooms.get_mut(&self.room_id) else {
            return;
        };
        forget_replay_id(
            &mut room.seen_event_ids,
            &mut room.seen_event_id_set,
            &self.event_id,
        );
        if let Some(id) = &self.envelope_id {
            forget_replay_id(
                &mut room.seen_envelope_ids,
                &mut room.seen_envelope_id_set,
                id,
            );
        }
        if let Some(id) = &self.request_id {
            forget_replay_id(
                &mut room.seen_request_ids,
                &mut room.seen_request_id_set,
                id,
            );
        }
        let stats = &mut room.stats;
        *stats
            .refused
            .entry((self.class, "refused_after_admission"))
            .or_default() += 1;
        stats.last_refusal = Some(ControlRefusalV1 {
            class: self.class,
            reason: "refused_after_admission",
            at: storage::now_ts(),
        });
    }
}

fn accept_terminal_rate_limited_event(room: &mut RoomControlRoomState, now_seconds: i64) -> bool {
    while room
        .terminal_received_at_seconds
        .front()
        .is_some_and(|timestamp| *timestamp <= now_seconds - 60)
    {
        room.terminal_received_at_seconds.pop_front();
    }
    let burst_count = room
        .terminal_received_at_seconds
        .iter()
        .filter(|timestamp| **timestamp > now_seconds - 2)
        .count();
    if room.terminal_received_at_seconds.len() >= MAX_TERMINAL_EVENTS_PER_MINUTE
        || burst_count >= MAX_TERMINAL_BURST_EVENTS
    {
        return false;
    }
    room.terminal_received_at_seconds.push_back(now_seconds);
    true
}

fn require_exact_fields(object: &Map<String, Value>, fields: &[&str]) -> AppResult<()> {
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return Err(AppError::InvalidInput(
            "Invalid room control event fields.".into(),
        ));
    }
    Ok(())
}

fn string_field<'a>(object: &'a Map<String, Value>, field: &str) -> AppResult<&'a str> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidInput("Invalid room control event field.".into()))
}

fn bounded_string_field(object: &Map<String, Value>, field: &str, max: usize) -> AppResult<String> {
    let value = string_field(object, field)?;
    if value.trim().is_empty() || value.len() > max {
        return Err(AppError::InvalidInput(
            "Invalid room control event field.".into(),
        ));
    }
    Ok(value.to_string())
}

/// The event as the transport's field-name check sees it. A native
/// capability's invocation input and terminal output are capability-owned
/// JSON whose field names Pastey does not interpret, so exactly that one
/// subtree of exactly these kinds and schemas is replaced by null. Every
/// other part of the event, including that subtree's siblings, is checked as
/// before; the subtree itself is still bounded and typed with its message.
fn transport_field_view<'a>(kind: &str, event: &'a Value) -> Cow<'a, Value> {
    if event.get("protocolFamily").and_then(Value::as_str) != Some(NATIVE_AGENT_PROTOCOL_FAMILY) {
        return Cow::Borrowed(event);
    }
    let payload_schema = event
        .pointer("/payload/schemaVersion")
        .and_then(Value::as_str);
    let opaque = match (kind, payload_schema) {
        ("native_agent.invoke", Some(crate::native_agent::NATIVE_AGENT_INVOKE_V2_SCHEMA)) => {
            "/payload/input"
        }
        ("native_agent.status", Some(crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA)) => {
            "/payload/status/output"
        }
        (
            "native_agent.reconciliation",
            Some(crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA),
        ) => "/payload/taskOutput",
        _ => return Cow::Borrowed(event),
    };
    // A status's native decision reviews carry Codex's own request content
    // (command, folder, file paths and diffs) for display only, after
    // `validate_status` bounded it. It never names an action this side takes.
    let review_count = match kind {
        "native_agent.status" => event
            .pointer("/payload/status/nativeDecisions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        _ => 0,
    };
    if event.pointer(opaque).is_none() && review_count == 0 {
        return Cow::Borrowed(event);
    }
    let mut view = event.clone();
    if let Some(subtree) = view.pointer_mut(opaque) {
        *subtree = Value::Null;
    }
    for index in 0..review_count {
        if let Some(content) =
            view.pointer_mut(&format!("/payload/status/nativeDecisions/{index}/content"))
        {
            *content = Value::Null;
        }
    }
    Cow::Owned(view)
}

fn contains_unsafe_field(value: &Value, allow_validated_native_code: bool) -> bool {
    match value {
        Value::Array(values) => values
            .iter()
            .any(|value| contains_unsafe_field(value, allow_validated_native_code)),
        Value::Object(object) => object.iter().any(|(key, value)| {
            let normalized = normalize_field(key);
            (UNSAFE_FIELDS.contains(&normalized.as_str())
                && !(allow_validated_native_code && normalized == "code"))
                || contains_unsafe_field(value, allow_validated_native_code)
        }),
        _ => false,
    }
}

fn normalize_field(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn session_ref(public_key: &str) -> String {
    format!(
        "room-session:{}",
        blake3::hash(public_key.as_bytes()).to_hex()
    )
}

fn now_iso() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_peer(peer_session_id: &str) -> StoredBridgePeerEndpoint {
        StoredBridgePeerEndpoint {
            room_id: "room".into(),
            peer_session_id: peer_session_id.into(),
            display_name: Some("Peer".into()),
            endpoint_host: Some("127.0.0.1".into()),
            endpoint_port: Some(9000),
            transport_public_key: Some("target-key".into()),
            liveness: BridgePeerLiveness::Connected,
            join_method: crate::models::BridgePeerJoinMethod::NearbyAccept,
            logical_host_ref: None,
            durable_identity_id: None,
            updated_at: 1,
        }
    }

    fn route_room() -> crate::models::StoredRoom {
        crate::models::StoredRoom {
            id: "room".into(),
            room_code_hash: "hash".into(),
            created_at: 1,
            expires_at: 2,
            status: RoomStatus::Active,
            local_role: crate::models::LocalRole::Creator,
            peer_device_name: Some("Peer".into()),
            auto_burn_after_expiry: false,
            wrapped_room_code: "wrapped".into(),
            code_nonce: "nonce".into(),
            peer_host: Some("legacy-host".into()),
            peer_port: Some(1000),
            peer_transport_public_key: Some("legacy-key".into()),
            local_burned_at: None,
            peer_burned_at: None,
        }
    }

    #[test]
    fn terminal_stream_rate_and_replay_state_is_separate_from_room_inbox() {
        let mut room = RoomControlRoomState::default();
        assert!(accept_terminal_rate_limited_event(&mut room, 10));
        record_replay_id(
            &mut room.terminal_seen_event_ids,
            &mut room.terminal_seen_event_id_set,
            "terminal-event".into(),
        );
        assert!(room.terminal_seen_event_id_set.contains("terminal-event"));
        assert!(room.history.is_empty());
        assert!(room.received_at_seconds.is_empty());
    }

    #[test]
    fn bridge_departure_event_is_typed_session_bound_and_replay_protected() {
        let now = OffsetDateTime::now_utc();
        let context = RoomControlSessionContext {
            room_id: "room".into(),
            local_session_ref: "source".into(),
            peer_session_ref: "target".into(),
            peer_route_ref: "peer".into(),
            peer_observation_ref: "observation".into(),
            peer_connected: true,
        };
        let burn = bridge_departure_event(BridgeDepartureKind::Burn, &context).unwrap();
        let mut leave = burn.clone();
        leave["payload"]["departureKind"] = Value::String("leave".into());
        for event in [leave, burn] {
            let validated = validate_control_event(event, "room", "source", "target", now).unwrap();
            assert_eq!(validated.kind, "bridge_membership.departure");
            let mut room = RoomControlRoomState::default();
            assert!(!is_replayed(&room, &validated));
            record_replay_id(
                &mut room.seen_event_ids,
                &mut room.seen_event_id_set,
                validated.event_id.clone(),
            );
            assert!(is_replayed(&room, &validated));
            assert!(room.history.is_empty());
        }

        let wrong_session = bridge_departure_event(BridgeDepartureKind::Burn, &context).unwrap();
        assert!(validate_control_event(wrong_session, "room", "other", "target", now).is_err());
    }

    #[test]
    fn bridge_departure_rejects_unknown_semantics_and_authority_fields() {
        let now = OffsetDateTime::now_utc();
        let context = RoomControlSessionContext {
            room_id: "room".into(),
            local_session_ref: "source".into(),
            peer_session_ref: "target".into(),
            peer_route_ref: "peer".into(),
            peer_observation_ref: "observation".into(),
            peer_connected: true,
        };
        let mut unknown = bridge_departure_event(BridgeDepartureKind::Burn, &context).unwrap();
        unknown["payload"]["departureKind"] = Value::String("disconnect".into());
        assert!(validate_control_event(unknown, "room", "source", "target", now).is_err());

        let mut authority = bridge_departure_event(BridgeDepartureKind::Burn, &context).unwrap();
        authority["payload"]["authorityToken"] = Value::String("forbidden".into());
        assert!(validate_control_event(authority, "room", "source", "target", now).is_err());
    }

    #[test]
    fn terminal_stream_rate_limit_allows_bounded_bursts_and_rejects_excess() {
        let mut room = RoomControlRoomState::default();
        for _ in 0..MAX_TERMINAL_BURST_EVENTS {
            assert!(accept_terminal_rate_limited_event(&mut room, 10));
        }
        assert!(!accept_terminal_rate_limited_event(&mut room, 10));
        assert!(accept_terminal_rate_limited_event(&mut room, 13));
        assert!(room.received_at_seconds.is_empty());
    }

    #[test]
    fn terminal_rejections_keep_flow_sequence_and_authority_categories_distinct() {
        assert_eq!(
            terminal_control_rejection(&AppError::InvalidInput(
                "Developer terminal input sequence mismatch.".into(),
            )),
            (
                StatusCode::CONFLICT,
                "terminal_sequence_rejected",
                "Developer Terminal sequence was rejected.",
            )
        );
        assert_eq!(
            terminal_control_rejection(&AppError::InvalidInput(
                "Developer terminal authority is unavailable.".into(),
            )),
            (
                StatusCode::FORBIDDEN,
                "terminal_authority_rejected",
                "Developer Terminal authority rejected the event.",
            )
        );
    }

    #[test]
    fn production_terminal_event_and_receiver_schema_agree() {
        let context = RoomControlSessionContext {
            room_id: "room".into(),
            local_session_ref: "controller-session".into(),
            peer_session_ref: "host-session".into(),
            peer_route_ref: "peer".into(),
            peer_observation_ref: "observation".into(),
            peer_connected: true,
        };
        let binding = crate::host_runtime::DeveloperTerminalBinding::new(
            "room",
            "controller-session",
            "host-session",
            "peer",
        );
        let service = crate::developer_terminal::DeveloperTerminalService::default();
        let ui = service.enter_mode("room", storage::now_ts());
        let message = service
            .request_open(&ui.token, binding, storage::now_ts())
            .unwrap();
        let event = crate::developer_terminal::terminal_event(
            "developer_terminal.open_request",
            &message,
            &context,
        )
        .unwrap();
        let validated = validate_control_event(
            event,
            "room",
            "controller-session",
            "host-session",
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        assert_eq!(validated.kind, "developer_terminal.open_request");
    }

    fn selected_route(peer_session_id: &str) -> Value {
        serde_json::json!({
            "schemaVersion": CONTROL_BRIDGE_ROUTE_SCHEMA_VERSION,
            "bridgeSessionId": "legacy-room:room",
            "target": {
                "kind": "selected_peer",
                "peerSessionId": peer_session_id
            }
        })
    }

    fn assert_control_route_error(result: AppResult<RoomControlRouteEndpoint>, code: &str) {
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(&format!("code={code}")),
            "expected route error code {code}, got {error}"
        );
    }

    #[test]
    fn selected_peer_room_control_route_resolves_through_bridge_peers() {
        let room = route_room();
        let peers = vec![route_peer("legacy-room-peer:room")];
        let target = resolve_room_control_route(
            Some(&selected_route("legacy-room-peer:room")),
            "room",
            &room,
            &peers,
        )
        .unwrap();

        assert_eq!(target.peer_session_id, "legacy-room-peer:room");
        assert_eq!(target.host, "127.0.0.1");
        assert_eq!(target.port, 9000);
        assert_eq!(target.transport_public_key, "target-key");
        assert_ne!(target.host, "legacy-host");
        assert_ne!(target.transport_public_key, "legacy-key");
    }

    #[test]
    fn selected_peer_room_control_route_rejects_stale_disconnected_and_missing_endpoint() {
        let room = route_room();
        for (peer, code) in [
            {
                let mut peer = route_peer("legacy-room-peer:room");
                peer.liveness = BridgePeerLiveness::Stale;
                peer.endpoint_host = None;
                peer.endpoint_port = None;
                peer.transport_public_key = None;
                (peer, "route_expired")
            },
            {
                let mut peer = route_peer("legacy-room-peer:room");
                peer.liveness = BridgePeerLiveness::Disconnected;
                (peer, "peer_unrouteable")
            },
            {
                let mut peer = route_peer("legacy-room-peer:room");
                peer.endpoint_host = None;
                (peer, "peer_unrouteable")
            },
        ] {
            assert_control_route_error(
                resolve_room_control_route(
                    Some(&selected_route("legacy-room-peer:room")),
                    "room",
                    &room,
                    &[peer],
                ),
                code,
            );
        }
    }

    #[test]
    fn room_control_route_rejects_mismatch_unknown_and_no_arbitrary_fallback() {
        let room = route_room();
        let mut peers = vec![route_peer("legacy-room-peer:room")];
        peers.push(route_peer("legacy-room-peer:room:reconnect:1"));
        let unknown = selected_route("legacy-room-peer:unknown");
        assert_control_route_error(
            resolve_room_control_route(Some(&unknown), "room", &room, &peers),
            "unknown_peer",
        );

        let mismatch = serde_json::json!({
            "schemaVersion": CONTROL_BRIDGE_ROUTE_SCHEMA_VERSION,
            "bridgeSessionId": "legacy-room:other",
            "target": {
                "kind": "selected_peer",
                "peerSessionId": "legacy-room-peer:room"
            }
        });
        assert_control_route_error(
            resolve_room_control_route(Some(&mismatch), "room", &room, &peers),
            "route_mismatch",
        );

        assert_control_route_error(
            resolve_room_control_route(None, "room", &room, &peers),
            "malformed_route",
        );
    }

    #[test]
    fn room_control_route_rejects_selected_peers_and_broadcast() {
        let room = route_room();
        let peers = vec![route_peer("legacy-room-peer:room")];
        let selected_peers = serde_json::json!({
            "schemaVersion": CONTROL_BRIDGE_ROUTE_SCHEMA_VERSION,
            "bridgeSessionId": "legacy-room:room",
            "target": {
                "kind": "selected_peers",
                "peerSessionIds": ["legacy-room-peer:room", "legacy-room-peer:room:1"]
            }
        });
        let broadcast = serde_json::json!({
            "schemaVersion": CONTROL_BRIDGE_ROUTE_SCHEMA_VERSION,
            "bridgeSessionId": "legacy-room:room",
            "target": {
                "kind": "broadcast_bridge",
                "explicit": true
            }
        });

        assert_control_route_error(
            resolve_room_control_route(Some(&selected_peers), "room", &room, &peers),
            "unsupported_selected_peers",
        );
        assert_control_route_error(
            resolve_room_control_route(Some(&broadcast), "room", &room, &peers),
            "unsupported_broadcast",
        );
    }

    #[test]
    fn durable_identity_display_does_not_satisfy_room_control_target_binding() {
        let room = route_room();
        let mut old_paired = route_peer("legacy-room-peer:room");
        old_paired.durable_identity_id = Some("paired-device:one".into());
        old_paired.liveness = BridgePeerLiveness::Stale;
        old_paired.endpoint_host = None;
        old_paired.endpoint_port = None;
        old_paired.transport_public_key = None;
        let mut current = route_peer("legacy-room-peer:room:reconnect:1");
        current.durable_identity_id = Some("paired-device:one".into());
        current.updated_at = 2;
        let peers = vec![old_paired, current];

        assert_control_route_error(
            resolve_room_control_route(
                Some(&selected_route("legacy-room-peer:room")),
                "room",
                &room,
                &peers,
            ),
            "route_expired",
        );
        assert_eq!(
            resolve_room_control_route(
                Some(&selected_route("legacy-room-peer:room:reconnect:1")),
                "room",
                &room,
                &peers,
            )
            .unwrap()
            .peer_session_id,
            "legacy-room-peer:room:reconnect:1"
        );
    }

    #[test]
    fn inbound_room_control_sender_uses_unique_current_session_key_not_observed_ip() {
        let mut old = route_peer("legacy-room-peer:room");
        old.liveness = BridgePeerLiveness::Stale;
        old.endpoint_host = None;
        old.transport_public_key = None;
        let mut current = route_peer("legacy-room-peer:room:reconnect:1");
        current.endpoint_host = Some("127.0.0.2".into());
        current.transport_public_key = Some("new-target-key".into());
        let peers = vec![old, current];

        assert_control_route_error(
            resolve_inbound_room_control_peer(&peers, "target-key"),
            "route_mismatch",
        );
        assert_eq!(
            resolve_inbound_room_control_peer(&peers, "new-target-key")
                .unwrap()
                .peer_session_id,
            "legacy-room-peer:room:reconnect:1"
        );
    }

    #[test]
    fn workspace_source_host_is_the_authenticated_current_session_peer() {
        let mut old = route_peer("legacy-room-peer:room");
        old.liveness = BridgePeerLiveness::Stale;
        old.logical_host_ref = Some("host:source".into());
        let mut current = route_peer("legacy-room-peer:room:reconnect:1");
        current.transport_public_key = Some("new-target-key".into());
        current.logical_host_ref = Some("host:source".into());
        let mut other = route_peer("legacy-room-peer:room:other");
        other.transport_public_key = Some("other-key".into());
        other.logical_host_ref = Some("host:other".into());
        let mut unbound = route_peer("legacy-room-peer:room:unbound");
        unbound.transport_public_key = Some("unbound-key".into());
        let peers = vec![old, current, other, unbound];

        // A replaced route's key no longer authenticates any source Host.
        assert_control_route_error(
            resolve_inbound_room_control_peer(&peers, "target-key"),
            "route_mismatch",
        );
        // The source HostRef is the one Bridge membership bound to the exact
        // authenticated sender's route, as `current_host_session_binding`
        // resolves it.
        let source_of = |key: &str| {
            let inbound = resolve_inbound_room_control_peer(&peers, key).unwrap();
            peers
                .iter()
                .find(|peer| peer.peer_session_id == inbound.peer_session_id)
                .and_then(|peer| peer.logical_host_ref.clone())
        };
        assert_eq!(source_of("new-target-key").as_deref(), Some("host:source"));
        assert_eq!(source_of("other-key").as_deref(), Some("host:other"));
        // A peer without a bound HostRef has no source identity to claim.
        assert_eq!(source_of("unbound-key"), None);
    }

    #[test]
    fn control_key_wrap_is_domain_separated_and_receipt_is_transport_only() {
        let sender = crypto::generate_transport_secret();
        let receiver = crypto::generate_transport_secret();
        let receiver_public = crypto::transport_public_key(&receiver);
        let event_key = crypto::random_key();
        let (wrapped, nonce, sender_public) =
            crypto::wrap_control_key_for_receiver(&event_key, &sender, &receiver_public).unwrap();
        assert_eq!(
            crypto::unwrap_control_key_from_sender(&wrapped, &nonce, &sender_public, &receiver)
                .unwrap(),
            event_key
        );
        assert!(
            crypto::unwrap_session_from_sender(&wrapped, &nonce, &sender_public, &receiver)
                .is_err()
        );
        let receipt = RoomControlDeliveryReceipt {
            schema_version: CONTROL_DELIVERY_SCHEMA.into(),
            event_id: "event".into(),
            accepted_for_local_inbox: true,
            received_at: "now".into(),
        };
        let serialized = serde_json::to_string(&receipt).unwrap();
        assert!(!serialized.contains("acknowledged_preview_only"));
        assert!(!serialized.contains("stdout"));
        assert!(!serialized.contains("stderr"));
        assert!(!serialized.contains("exitCode"));
    }

    #[test]
    fn replay_history_and_rate_bounds_are_finite() {
        let mut queue = VecDeque::new();
        let mut set = HashSet::new();
        for index in 0..(MAX_REPLAY_ITEMS + 10) {
            record_replay_id(&mut queue, &mut set, format!("event-{index}"));
        }
        assert_eq!(queue.len(), MAX_REPLAY_ITEMS);
        assert_eq!(set.len(), MAX_REPLAY_ITEMS);
        assert_eq!(MAX_HISTORY_ITEMS, 64);
        assert_eq!(MAX_CONTROL_REQUEST_BYTES, 96 * 1024);
        assert_eq!(MAX_CONTROL_EVENT_BYTES, 64 * 1024);
        assert_eq!(MAX_CONTROL_RESPONSE_BYTES, 4 * 1024);
        // Every class has a finite burst and a finite minute, refused alike.
        for class in ControlClassV1::ALL {
            let (per_minute, burst) = class.budget();
            assert!(burst <= per_minute && per_minute <= 120, "{class:?}");
            let mut room = RoomControlRoomState::default();
            for _ in 0..burst {
                assert!(accept_rate_limited_event(&mut room, class, 100));
            }
            assert!(!accept_rate_limited_event(&mut room, class, 100));
            // Spread over one minute, below the burst: the minute's budget.
            let mut room = RoomControlRoomState::default();
            for index in 0..per_minute as i64 {
                let at = 1_000 + index * 59 / per_minute as i64;
                assert!(
                    accept_rate_limited_event(&mut room, class, at),
                    "{class:?} {index}"
                );
            }
            assert!(!accept_rate_limited_event(&mut room, class, 1_059));
            // A minute after the first, the budget is back.
            assert!(accept_rate_limited_event(&mut room, class, 1_061));
        }
    }

    /// One class's traffic cannot use up another's budget: a capability
    /// flood leaves a running task's statuses and decisions their own.
    #[test]
    fn each_class_has_its_own_rate_budget() {
        let mut room = RoomControlRoomState::default();
        while accept_rate_limited_event(&mut room, ControlClassV1::CapabilityQuery, 500) {}
        assert!(!accept_rate_limited_event(
            &mut room,
            ControlClassV1::CapabilityQuery,
            500
        ));
        assert!(accept_rate_limited_event(
            &mut room,
            ControlClassV1::CapabilityAnswer,
            500
        ));
        assert!(accept_rate_limited_event(
            &mut room,
            ControlClassV1::NativeAgent,
            500
        ));
        assert!(accept_rate_limited_event(
            &mut room,
            ControlClassV1::BridgePlan,
            500
        ));
        assert!(accept_rate_limited_event(
            &mut room,
            ControlClassV1::Membership,
            500
        ));
        assert_eq!(
            ControlClassV1::of("peer_capability.query"),
            ControlClassV1::CapabilityQuery
        );
        assert_eq!(
            ControlClassV1::of("peer_capability.response"),
            ControlClassV1::CapabilityAnswer
        );
        assert_eq!(
            ControlClassV1::of("native_agent.status"),
            ControlClassV1::NativeAgent
        );
        assert_eq!(
            ControlClassV1::of("bridge_plan.v2.step_result"),
            ControlClassV1::BridgePlan
        );
        assert_eq!(
            ControlClassV1::of("bridge_plan.review_request"),
            ControlClassV1::BridgePlan
        );
        assert_eq!(
            ControlClassV1::of("bridge_membership.departure"),
            ControlClassV1::Membership
        );
    }

    fn history_event(index: usize, received_at: i64) -> ReceivedRoomControlEvent {
        ReceivedRoomControlEvent {
            event_id: format!("event-{index}"),
            kind: "bridge_plan.step_progress".into(),
            room_ref: "room".into(),
            source_device_ref: "source".into(),
            target_peer_ref: "target".into(),
            created_at: "now".into(),
            expires_at: "later".into(),
            event: serde_json::json!({}),
            received_at: OffsetDateTime::from_unix_timestamp(received_at)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        }
    }

    /// Display history is bounded by count and age, drops the oldest first,
    /// and never makes the receiver refuse anything.
    #[test]
    fn history_is_bounded_oldest_first_and_never_refuses() {
        let now = 1_800_000_000;
        let mut room = RoomControlRoomState::default();
        for index in 0..200 {
            keep_in_history(&mut room, history_event(index, now), now);
        }
        assert_eq!(room.history.len(), MAX_HISTORY_ITEMS);
        assert_eq!(room.history.front().unwrap().event_id, "event-136");
        assert_eq!(room.history.back().unwrap().event_id, "event-199");
        assert_eq!(room.stats.history_dropped, 136);
        // Entries past their age leave at the next addition.
        let later = now + MAX_HISTORY_AGE_SECONDS + 1;
        keep_in_history(&mut room, history_event(500, later), later);
        assert_eq!(room.history.len(), 1);
        assert_eq!(room.stats.history_dropped, 136 + 64);
    }

    /// The sender's reading of each refusal comes from the same table that
    /// wrote its message, and only refusals made before the receiver acted
    /// count as proof that nothing happened there.
    #[test]
    fn delivery_failures_are_classified_from_the_table_that_wrote_them() {
        for (code, message, failure) in DELIVERY_REFUSALS {
            let expected = DELIVERY_REFUSALS
                .iter()
                .find(|(_, known, _)| known == message)
                .unwrap()
                .2;
            assert_eq!(
                delivery_failure(&AppError::Network((*message).into())),
                expected,
                "{code}"
            );
            assert_eq!(
                *failure, expected,
                "{code}: two codes share a message with different meanings"
            );
        }
        let definite = |code: &str| {
            let message = DELIVERY_REFUSALS
                .iter()
                .find(|(known, _, _)| *known == code)
                .unwrap()
                .1;
            delivery_failure(&AppError::Network(message.into())).proves_nothing_happened()
        };
        for code in [
            "rate_limited",
            "native_agent_review_capacity",
            "native_agent_not_authorized",
            "native_agent_incompatible",
            "host_mismatch",
            "invalid_native_agent",
        ] {
            assert!(definite(code), "{code}");
        }
        // Received, or refused after acting, or unknown: never proof.
        for code in [
            "event_replayed",
            "event_expired",
            "inbox_full",
            "native_agent_unavailable",
            "session_mismatch",
        ] {
            assert!(!definite(code), "{code}");
        }
        assert_eq!(
            delivery_failure(&AppError::Timeout(
                "Room control delivery timed out.".into()
            )),
            ControlDeliveryFailureV1::Transport
        );
        assert_eq!(
            delivery_failure(&AppError::Network("Room control delivery failed.".into())),
            ControlDeliveryFailureV1::Transport
        );
        assert_eq!(
            delivery_failure(&AppError::Network(
                "Room control receipt is invalid.".into()
            )),
            ControlDeliveryFailureV1::Transport
        );
        assert_eq!(
            delivery_failure(&AppError::InvalidInput(
                "Room session is unavailable.".into()
            )),
            ControlDeliveryFailureV1::NotSent
        );
    }

    /// Replay and rate admission come before anything acts on a capability
    /// or Bridge Plan event, and only Bridge Plan v1 events are kept.
    #[test]
    fn general_events_are_admitted_before_any_side_effect() {
        let source = include_str!("room_control.rs");
        let start = source
            .find("pub async fn receive_room_control_event_handler")
            .unwrap();
        let end = start
            + source[start..]
                .find("\nfn validate_control_event(")
                .unwrap();
        let handler = &source[start..end];
        let admit = handler
            .find("admit_received_event(room_state, &validated, class, now, &room_id)")
            .unwrap();
        for side_effect in [
            "if validated.kind == \"peer_capability.response\" {",
            "crate::bridge_plan::accept_inbound_protocol_event(",
            "crate::native_v2_orchestration::accept_requester_readiness(",
        ] {
            assert!(admit < handler.find(side_effect).unwrap(), "{side_effect}");
        }
        assert_eq!(handler.matches("keep_in_history(").count(), 1);
        assert!(handler.contains("if validated.kind.starts_with(\"bridge_plan.\") && !validated.kind.starts_with(\"bridge_plan.v2.\")"));
        assert!(
            !handler.contains("inbox_full"),
            "no refusal for lack of room"
        );
    }

    #[test]
    fn burn_purge_removes_room_control_inbox_replay_and_rate_state() {
        let mut runtime = RoomControlRuntimeState::default();
        let mut room = RoomControlRoomState::default();
        room.history.push_back(ReceivedRoomControlEvent {
            event_id: "event".into(),
            kind: "bridge_plan.step_progress".into(),
            room_ref: "room".into(),
            source_device_ref: "source".into(),
            target_peer_ref: "target".into(),
            created_at: "now".into(),
            expires_at: "later".into(),
            event: serde_json::json!({}),
            received_at: "now".into(),
        });
        record_replay_id(
            &mut room.seen_event_ids,
            &mut room.seen_event_id_set,
            "event".into(),
        );
        record_replay_id(
            &mut room.seen_envelope_ids,
            &mut room.seen_envelope_id_set,
            "envelope".into(),
        );
        record_replay_id(
            &mut room.seen_request_ids,
            &mut room.seen_request_id_set,
            "request".into(),
        );
        room.received_at_seconds
            .entry(ControlClassV1::CapabilityQuery)
            .or_default()
            .push_back(1);
        runtime.rooms.insert("room".into(), room);
        assert!(runtime.purge_room("room"));
        assert!(runtime.rooms.get("room").is_none());
    }

    #[test]
    fn send_errors_are_structured_and_sanitized() {
        let replay = RoomControlSendError::from_app_error(AppError::Network(
            "Room control event was already received.".into(),
        ));
        assert_eq!(replay.code, "replay");
        assert_eq!(replay.message, "Room control event was already received.");

        let receipt = RoomControlSendError::from_app_error(AppError::Network(
            "Room control receipt is invalid.".into(),
        ));
        assert_eq!(receipt.code, "malformed_receipt");
        assert!(!receipt.message.contains("ciphertext"));

        assert_eq!(
            RoomControlSendError::from_app_error(AppError::Network(
                "Room control rate limit was reached.".into(),
            ))
            .code,
            "rate_limited"
        );
        assert_eq!(
            RoomControlSendError::from_app_error(AppError::Network(
                "Room control inbox is full.".into(),
            ))
            .code,
            "inbox_full"
        );
    }

    #[test]
    fn generic_capability_control_events_remain_unavailable() {
        let now = OffsetDateTime::now_utc();
        let event = serde_json::json!({
            "schemaVersion": ROOM_CONTROL_SCHEMA,
            "eventId": "generic-capability-event",
            "kind": "capability.request",
            "roomRef": "room",
            "sourceDeviceRef": "source",
            "targetPeerRef": "target",
            "createdAt": now.format(&Rfc3339).unwrap(),
            "expiresAt": (now + time::Duration::seconds(60)).format(&Rfc3339).unwrap(),
            "previewOnly": false,
            "payload": {},
        });
        let error = match validate_control_event(event, "room", "source", "target", now) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("generic capability event must be rejected"),
        };
        assert!(error.contains("Unsupported room control event kind."));
    }

    #[test]
    fn receiver_review_decision_is_not_a_live_room_control_event() {
        assert!(!ALLOWED_EVENT_KINDS.contains(&"bridge_plan.review_decision"));
        assert!(ALLOWED_EVENT_KINDS.contains(&"bridge_plan.review_request"));
        assert!(ALLOWED_EVENT_KINDS.contains(&"bridge_plan.v2.review_request"));
        assert!(ALLOWED_EVENT_KINDS.contains(&"bridge_plan.v2.attempt_start"));
    }

    #[test]
    fn peer_capability_events_are_typed_bounded_and_not_generic_control() {
        let now = OffsetDateTime::now_utc();
        let context = RoomControlSessionContext {
            room_id: "room".into(),
            local_session_ref: "source".into(),
            peer_session_ref: "target".into(),
            peer_route_ref: "selected-peer-session".into(),
            peer_observation_ref: "route-binding".into(),
            peer_connected: true,
        };
        let empty_projection = crate::peer_capabilities::local_projection(
            "selected-peer-session".into(),
            now.unix_timestamp(),
        );
        assert!(empty_projection.capabilities.is_empty());
        log_peer_capability_projection("test_empty_projection", &empty_projection);
        let valid = peer_capability_event(
            "peer_capability.response",
            serde_json::to_value(empty_projection).unwrap(),
            &context,
        )
        .unwrap();
        assert!(validate_control_event(valid, "room", "source", "target", now).is_ok());

        let malformed = peer_capability_event(
            "peer_capability.response",
            serde_json::json!({
                "schemaVersion": crate::peer_capabilities::PEER_CAPABILITY_SCHEMA,
                "peerSessionId": "selected-peer-session",
                "observedAt": now.unix_timestamp(),
                "capabilities": [{
                    "capabilityId": "unknown_capability",
                    "available": true,
                    "acceptedInputMediaTypes": ["text/plain"],
                    "authorityToken": "not-allowed"
                }]
            }),
            &context,
        )
        .unwrap();
        assert!(validate_control_event(malformed, "room", "source", "target", now).is_err());
    }

    #[test]
    fn peer_capability_query_accepts_only_bounded_known_semantic_ids() {
        let now = OffsetDateTime::now_utc();
        let context = RoomControlSessionContext {
            room_id: "room".into(),
            local_session_ref: "source".into(),
            peer_session_ref: "target".into(),
            peer_route_ref: "selected-peer-session".into(),
            peer_observation_ref: "route-binding".into(),
            peer_connected: true,
        };
        let valid = peer_capability_event(
            "peer_capability.query",
            serde_json::json!({
                "schemaVersion": crate::peer_capabilities::PEER_CAPABILITY_SCHEMA,
                "peerSessionId": "selected-peer-session",
                "capabilityIds": ["runtime.powershell", "runtime.powershell", "runtime.zsh"],
            }),
            &context,
        )
        .unwrap();
        assert!(validate_control_event(valid, "room", "source", "target", now).is_ok());
        let normalized = requested_peer_capability_ids(
            &serde_json::json!({
                "capabilityIds": ["runtime.powershell", "runtime.powershell", "runtime.zsh"],
            })
            .as_object()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(normalized, vec!["runtime.powershell", "runtime.zsh"]);

        for payload in [
            serde_json::json!({
                "schemaVersion": crate::peer_capabilities::PEER_CAPABILITY_SCHEMA,
                "peerSessionId": "selected-peer-session",
                "capabilityIds": ["runtime.not_a_probe"],
            }),
            serde_json::json!({
                "schemaVersion": crate::peer_capabilities::PEER_CAPABILITY_SCHEMA,
                "peerSessionId": "selected-peer-session",
                "capabilityIds": ["runtime.python"],
                "command": "curl",
            }),
        ] {
            let event = peer_capability_event("peer_capability.query", payload, &context).unwrap();
            assert!(validate_control_event(event, "room", "source", "target", now).is_err());
        }
    }

    #[test]
    fn room_control_query_builds_a_valid_empty_projection_without_fallback_fact() {
        let projection =
            crate::peer_capabilities::local_projection("selected-peer-session".into(), 1);
        assert_eq!(projection.peer_session_id, "selected-peer-session");
        assert!(projection.capabilities.is_empty());
        let encoded = serde_json::to_string(&projection).unwrap();
        let decoded: crate::peer_capabilities::PeerCapabilityProjection =
            serde_json::from_str(&encoded).unwrap();
        assert!(decoded.capabilities.is_empty());
        for forbidden in ["authority", "topology", "deviceSelection", "fallback"] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn peer_capability_response_route_targets_the_authenticated_inbound_peer() {
        let room = route_room();
        let inbound_peer = route_peer("requester-current-session");
        let response_route = selected_peer_control_route("room", &inbound_peer.peer_session_id);
        let resolved =
            resolve_room_control_route(Some(&response_route), "room", &room, &[inbound_peer])
                .unwrap();
        assert_eq!(resolved.peer_session_id, "requester-current-session");
    }

    #[test]
    fn native_agent_event_binds_the_exact_current_peer_session() {
        let context = RoomControlSessionContext {
            room_id: "room".into(),
            local_session_ref: "source".into(),
            peer_session_ref: "target".into(),
            peer_route_ref: "peer".into(),
            peer_observation_ref: "observation".into(),
            peer_connected: true,
        };
        let payload = serde_json::to_value(crate::native_agent::NativeAgentInvokeV1 {
            schema_version: crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task".into(),
            target_host_ref: "host:remote".into(),
            agent_capability: crate::native_agent::CODEX_CAPABILITY_ID.into(),
            workspace: "/existing/workspace".into(),
            task: "work".into(),
            resume: true,
        })
        .unwrap();
        let event = native_agent_event("native_agent.invoke", payload, &context).unwrap();
        let now = OffsetDateTime::now_utc();
        assert!(validate_control_event(event.clone(), "room", "source", "target", now).is_ok());
        assert!(validate_control_event(event, "room", "source", "stale-target", now).is_err());
    }

    fn native_agent_test_context() -> RoomControlSessionContext {
        RoomControlSessionContext {
            room_id: "room".into(),
            local_session_ref: "source".into(),
            peer_session_ref: "target".into(),
            peer_route_ref: "peer".into(),
            peer_observation_ref: "observation".into(),
            peer_connected: true,
        }
    }

    #[test]
    fn reconciliation_queries_replay_only_the_exact_control_event() {
        let now = OffsetDateTime::now_utc();
        let payload = serde_json::to_value(crate::native_agent::NativeAgentReconcileV1 {
            schema_version: crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task".into(),
            movement_id: Some("movement".into()),
            target_host_ref: "host:remote".into(),
        })
        .unwrap();
        let first = validate_control_event(
            native_agent_event(
                "native_agent.reconcile",
                payload.clone(),
                &native_agent_test_context(),
            )
            .unwrap(),
            "room",
            "source",
            "target",
            now,
        )
        .unwrap();
        let second = validate_control_event(
            native_agent_event(
                "native_agent.reconcile",
                payload,
                &native_agent_test_context(),
            )
            .unwrap(),
            "room",
            "source",
            "target",
            now,
        )
        .unwrap();
        assert_ne!(first.event_id, second.event_id);
        assert!(first.request_id.is_none());
        assert!(second.request_id.is_none());
        let mut room = RoomControlRoomState::default();
        record_replay_id(
            &mut room.seen_event_ids,
            &mut room.seen_event_id_set,
            first.event_id.clone(),
        );
        assert!(is_replayed(&room, &first));
        assert!(!is_replayed(&room, &second));
    }

    #[test]
    fn native_capabilities_share_the_fixed_native_agent_event_family() {
        // A new native capability adds no event kind: every capability uses
        // these, and only these, Native Agent events. The two session kinds
        // are read-only session discovery, not a capability.
        let native = ALLOWED_EVENT_KINDS
            .iter()
            .copied()
            .filter(|kind| kind.starts_with("native_agent."))
            .collect::<Vec<_>>();
        assert_eq!(
            native,
            [
                "native_agent.invoke",
                "native_agent.status",
                "native_agent.cancel",
                "native_agent.workspace_prepare",
                "native_agent.reconcile",
                "native_agent.reconciliation",
                "native_agent.retry_result_return",
                // Core's session-bound requester identity, shared by every
                // capability; not capability-specific.
                "native_agent.device_identity",
                // The requesting user's decision on a native request; Core
                // relays it to whichever capability raised the request.
                "native_agent.decision",
                "native_agent.session_query",
                "native_agent.session_response",
            ]
        );
    }

    #[test]
    fn a_generic_invoke_keeps_the_task_keyed_replay_id_and_an_opaque_input() {
        let now = OffsetDateTime::now_utc();
        let invoke = |input: Value| {
            native_agent_event(
                "native_agent.invoke",
                serde_json::to_value(crate::native_agent::NativeAgentInvokeV2 {
                    schema_version: crate::native_agent::NATIVE_AGENT_INVOKE_V2_SCHEMA.into(),
                    task_id: "task-generic".into(),
                    target_host_ref: "host:remote".into(),
                    agent_capability: "fake.longjob.v1".into(),
                    input: serde_json::from_value(input).unwrap(),
                })
                .unwrap(),
                &native_agent_test_context(),
            )
            .unwrap()
        };
        let first = validate_control_event(
            invoke(serde_json::json!({ "steps": 2, "payload": { "a": 1 } })),
            "room",
            "source",
            "target",
            now,
        )
        .unwrap();
        let other_input = validate_control_event(
            invoke(serde_json::json!({ "steps": 3, "payload": { "a": 2 } })),
            "room",
            "source",
            "target",
            now,
        )
        .unwrap();
        // Replay is keyed by task identity, never by the opaque input.
        assert_eq!(first.request_id, other_input.request_id);
        assert_eq!(
            first.request_id.as_deref(),
            Some("native-agent-invoke:task-generic")
        );
        // The input is capability-owned: its field names are not transport
        // vocabulary, so names Room Control forbids elsewhere pass inside it.
        let opaque_names = validate_control_event(
            invoke(serde_json::json!({ "steps": 2, "payload": { "command": "x" } })),
            "room",
            "source",
            "target",
            now,
        )
        .unwrap();
        assert_eq!(opaque_names.request_id, first.request_id);
    }

    /// Field names Room Control forbids as transport vocabulary, nested in
    /// objects and in objects inside arrays.
    fn capability_owned_json() -> Value {
        serde_json::json!({
            "path": "/srv/job",
            "command": "run",
            "args": ["--fast", { "env": { "token": "t" } }],
            "env": { "contents": "x" },
            "token": "opaque",
            "nested": { "deep": [{ "path": 1 }, [{ "command": { "args": [] } }]] },
        })
    }

    const UNSAFE_REJECTION: &str = "Room control event contains unsafe fields.";

    fn validate_native(event: Value) -> AppResult<ValidatedControlEvent> {
        validate_control_event(event, "room", "source", "target", OffsetDateTime::now_utc())
    }

    fn rejected_as_unsafe(event: Value) {
        let error = validate_native(event).err().expect("event was accepted");
        assert_eq!(error.message(), UNSAFE_REJECTION);
    }

    fn generic_invoke_payload(input: Value) -> Value {
        serde_json::to_value(crate::native_agent::NativeAgentInvokeV2 {
            schema_version: crate::native_agent::NATIVE_AGENT_INVOKE_V2_SCHEMA.into(),
            task_id: "task-opaque".into(),
            target_host_ref: "host:remote".into(),
            agent_capability: "fake.longjob.v1".into(),
            input: serde_json::from_value(input).unwrap(),
        })
        .unwrap()
    }

    fn opaque_status_payload(output: Option<Value>) -> Value {
        serde_json::to_value(crate::native_agent::NativeAgentStatusV1 {
            schema_version: crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task-opaque".into(),
            executing_host_ref: "host:remote".into(),
            status: crate::native_agent::NativeAgentTaskStatusV1 {
                schema_version: crate::native_agent::NATIVE_AGENT_TASK_SCHEMA.into(),
                task_id: "task-opaque".into(),
                agent_id: "fake.longjob.v1".into(),
                workspace_name: "longjob".into(),
                session_reused: false,
                state: crate::native_agent::NativeAgentTaskStateV1::Completed,
                result: None,
                code: None,
                output: output.map(|value| serde_json::from_value(value).unwrap()),
                conversation: None,
                native_decisions: Vec::new(),
            },
        })
        .unwrap()
    }

    fn opaque_reconciliation_payload(output: Option<Value>) -> Value {
        serde_json::to_value(crate::native_agent::NativeAgentReconciliationV1 {
            schema_version: crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task-opaque".into(),
            movement_id: None,
            executing_host_ref: "host:remote".into(),
            task_state: crate::native_agent::NativeAgentTaskStateV1::Completed,
            movement_state: None,
            result_digest: None,
            apply_completed: false,
            code: None,
            task_output: output.map(|value| serde_json::from_value(value).unwrap()),
        })
        .unwrap()
    }

    fn native_event(kind: &str, payload: Value) -> Value {
        native_agent_event(kind, payload, &native_agent_test_context()).unwrap()
    }

    fn review_status_payload() -> Value {
        use crate::native_agent::*;
        let content = NativeDecisionContentV1 {
            command: Some("cargo test --all".into()),
            cwd: Some("C:\\REPZ".into()),
            reason: Some("Run the tests".into()),
            file_changes: vec![NativeFileChangeV1 {
                path: "src/lib.rs".into(),
                change: "update".into(),
                move_to: None,
                diff: "-a\n+b\n".into(),
            }],
            ..NativeDecisionContentV1::default()
        };
        let review = NativeDecisionReviewV1 {
            review_ref: "native-decision:0f6c1c1e-5a2e-4b8e-9d55-3d1b2f0c9a11".into(),
            kind: NativeDecisionKindV1::CommandExecution,
            state: NativeDecisionStateV1::Pending,
            submitted: None,
            can_accept: true,
            can_decline: true,
            unavailable_reason: None,
            content_digest: native_decision_content_digest(
                NativeDecisionKindV1::CommandExecution,
                Some(&content),
            ),
            content: Some(content),
            requested_at: 1,
            expires_at: 2,
        };
        serde_json::to_value(NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task-review".into(),
            executing_host_ref: "host:remote".into(),
            status: NativeAgentTaskStatusV1 {
                schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                task_id: "task-review".into(),
                agent_id: CODEX_CAPABILITY_ID.into(),
                workspace_name: "REPZ".into(),
                session_reused: false,
                state: NativeAgentTaskStateV1::Running,
                result: None,
                code: Some(AWAITING_NATIVE_DECISION_CODE.into()),
                output: None,
                conversation: None,
                native_decisions: vec![review],
            },
        })
        .unwrap()
    }

    fn decision_payload() -> Value {
        serde_json::to_value(crate::native_agent::NativeAgentDecisionV1 {
            schema_version: crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            decision_id: "4b0e0c7a-8d0f-4d4b-9a5e-0d5a3c1e2f10".into(),
            task_id: "task-review".into(),
            target_host_ref: "host:remote".into(),
            review_ref: "native-decision:0f6c1c1e-5a2e-4b8e-9d55-3d1b2f0c9a11".into(),
            content_digest: "a".repeat(64),
            decision: crate::native_agent::NativeDecisionV1::Accept,
        })
        .unwrap()
    }

    #[test]
    fn an_undelivered_native_status_is_sent_again_a_bounded_number_of_times() {
        assert!(native_status_settled(true, 0));
        // A failed send is retried, so an unchanging status (a task waiting
        // for its requester's decision) still reaches the requester...
        for failures in 0..MAX_STATUS_RESENDS {
            assert!(!native_status_settled(false, failures));
        }
        // ...but not forever.
        assert!(native_status_settled(false, MAX_STATUS_RESENDS));
        let source = include_str!("room_control.rs");
        assert!(source.contains("if native_status_settled(delivered, failed_sends) {"));
    }

    #[test]
    fn native_review_content_crosses_only_inside_each_reviews_content() {
        let status = native_event("native_agent.status", review_status_payload());
        validate_native(status.clone()).unwrap();
        // The same names anywhere else in the status are still refused.
        let mut beside = status.clone();
        beside["payload"]["status"]["nativeDecisions"][0]["command"] = Value::from("ls");
        rejected_as_unsafe(beside);
        let mut top = status.clone();
        top["payload"]["status"]["path"] = Value::from("/");
        rejected_as_unsafe(top);
        // Content of a review the status validation refuses never passes.
        let mut forged = status;
        forged["payload"]["status"]["nativeDecisions"][0]["content"]["command"] =
            Value::from("rm -rf /");
        assert!(validate_native(forged).is_err());
    }

    #[test]
    fn a_native_decision_carries_no_content_and_replays_per_attempt() {
        let decision = native_event("native_agent.decision", decision_payload());
        let validated = validate_native(decision.clone()).unwrap();
        assert_eq!(
            validated.request_id.as_deref(),
            Some(
                "native-agent-decision:task-review:native-decision:0f6c1c1e-5a2e-4b8e-9d55-3d1b2f0c9a11:4b0e0c7a-8d0f-4d4b-9a5e-0d5a3c1e2f10"
            )
        );
        // No command, path or other field rides along.
        let mut carrying = decision.clone();
        carrying["payload"]["command"] = Value::from("rm -rf /");
        rejected_as_unsafe(carrying);
        let mut extra = decision.clone();
        extra["payload"]["scope"] = Value::from("session");
        assert!(validate_native(extra).is_err());
        // Only one-time answers exist.
        for answer in ["acceptForSession", "cancel", "approved"] {
            let mut other = decision.clone();
            other["payload"]["decision"] = Value::from(answer);
            assert!(validate_native(other).is_err(), "{answer}");
        }
        let mut native_id = decision;
        native_id["payload"]["reviewRef"] = Value::from("approval-cmd");
        assert!(validate_native(native_id).is_err());
    }

    #[test]
    fn capability_owned_json_is_opaque_only_at_its_exact_subtree() {
        // Allowed: exactly the opaque subtree of exactly these schemas.
        let invoke = native_event(
            "native_agent.invoke",
            generic_invoke_payload(capability_owned_json()),
        );
        let status = native_event(
            "native_agent.status",
            opaque_status_payload(Some(capability_owned_json())),
        );
        let reconciliation = native_event(
            "native_agent.reconciliation",
            opaque_reconciliation_payload(Some(capability_owned_json())),
        );
        for event in [&invoke, &status, &reconciliation] {
            validate_native(event.clone()).unwrap();
        }
        assert_eq!(
            validate_native(invoke.clone())
                .unwrap()
                .request_id
                .as_deref(),
            Some("native-agent-invoke:task-opaque")
        );

        // Rejected: the same names anywhere else in those very events.
        let hostile = capability_owned_json();
        let mut payload_top = invoke.clone();
        payload_top["payload"]["path"] = Value::from("/etc");
        rejected_as_unsafe(payload_top);
        for sibling in ["extra", "inputs", "output"] {
            let mut event = invoke.clone();
            event["payload"][sibling] = hostile.clone();
            rejected_as_unsafe(event);
        }
        for sibling in ["output", "extra"] {
            let mut event = status.clone();
            event["payload"][sibling] = hostile.clone();
            rejected_as_unsafe(event);
        }
        let mut status_sibling = status.clone();
        status_sibling["payload"]["status"]["outputs"] = hostile.clone();
        rejected_as_unsafe(status_sibling);
        for sibling in ["output", "input", "taskOutputs"] {
            let mut event = reconciliation.clone();
            event["payload"][sibling] = hostile.clone();
            rejected_as_unsafe(event);
        }
        // The event top level keeps its exact field set.
        let mut top = invoke.clone();
        top["command"] = Value::from("run");
        assert!(validate_native(top).is_err());
    }

    #[test]
    fn capability_owned_names_stay_forbidden_outside_the_exact_schemas() {
        let hostile = capability_owned_json();
        // A v1 invoke keeps its existing transport rules at the same path.
        let mut v1 = native_event(
            "native_agent.invoke",
            serde_json::to_value(crate::native_agent::NativeAgentInvokeV1 {
                schema_version: crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                task_id: "task-v1".into(),
                target_host_ref: "host:remote".into(),
                agent_capability: crate::native_agent::CODEX_CAPABILITY_ID.into(),
                workspace: "/workspace".into(),
                task: "edit".into(),
                resume: true,
            })
            .unwrap(),
        );
        validate_native(v1.clone()).unwrap();
        v1["payload"]["input"] = hostile.clone();
        rejected_as_unsafe(v1);
        // Other native Agent kinds get no exemption at the same paths.
        let mut cancel = native_event(
            "native_agent.cancel",
            serde_json::to_value(crate::native_agent::NativeAgentCancelV1 {
                schema_version: crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                task_id: "task-opaque".into(),
                target_host_ref: "host:remote".into(),
            })
            .unwrap(),
        );
        validate_native(cancel.clone()).unwrap();
        cancel["payload"]["input"] = hostile.clone();
        rejected_as_unsafe(cancel);
        // A matching shape under another kind, family or schema is checked.
        let invoke = native_event(
            "native_agent.invoke",
            generic_invoke_payload(hostile.clone()),
        );
        let status = native_event(
            "native_agent.status",
            opaque_status_payload(Some(hostile.clone())),
        );
        for (kind, family) in [
            (
                "developer_terminal.input",
                DEVELOPER_TERMINAL_PROTOCOL_FAMILY,
            ),
            ("peer_capability.response", PEER_CAPABILITY_PROTOCOL_FAMILY),
            ("native_agent.status", PEER_CAPABILITY_PROTOCOL_FAMILY),
        ] {
            let mut event = invoke.clone();
            event["kind"] = Value::from(kind);
            event["protocolFamily"] = Value::from(family);
            rejected_as_unsafe(event);
        }
        for kind in ["developer_terminal.output", "native_agent.reconciliation"] {
            let mut event = status.clone();
            event["kind"] = Value::from(kind);
            if kind.starts_with("developer_terminal.") {
                event["protocolFamily"] = Value::from(DEVELOPER_TERMINAL_PROTOCOL_FAMILY);
            }
            rejected_as_unsafe(event);
        }
        let mut other_schema = invoke.clone();
        other_schema["payload"]["schemaVersion"] =
            Value::from(crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA);
        rejected_as_unsafe(other_schema);
        let mut other_status_schema = status;
        other_status_schema["payload"]["schemaVersion"] =
            Value::from(crate::native_agent::NATIVE_AGENT_INVOKE_V2_SCHEMA);
        rejected_as_unsafe(other_status_schema);
    }

    #[test]
    fn opaque_capability_json_keeps_its_size_bound() {
        let sized = |bytes: usize| serde_json::json!({ "steps": 1, "payload": "x".repeat(bytes) });
        // The adapter seam's opaque payload bound (16 KiB).
        let limit = 16 * 1024;
        validate_native(native_event(
            "native_agent.invoke",
            generic_invoke_payload(sized(limit - 64)),
        ))
        .unwrap();
        assert!(validate_native(native_event(
            "native_agent.invoke",
            generic_invoke_payload(sized(limit + 1)),
        ))
        .is_err());
        assert!(validate_native(native_event(
            "native_agent.status",
            opaque_status_payload(Some(sized(limit + 1))),
        ))
        .is_err());
        assert!(validate_native(native_event(
            "native_agent.reconciliation",
            opaque_reconciliation_payload(Some(sized(limit + 1))),
        ))
        .is_err());
    }

    #[test]
    fn materially_different_native_agent_facts_have_distinct_semantic_replay_ids() {
        let now = OffsetDateTime::now_utc();
        let status_event = |code: &str| {
            native_agent_event(
                "native_agent.status",
                serde_json::to_value(crate::native_agent::NativeAgentStatusV1 {
                    schema_version: crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    task_id: "task".into(),
                    executing_host_ref: "host:remote".into(),
                    status: crate::native_agent::NativeAgentTaskStatusV1 {
                        schema_version: crate::native_agent::NATIVE_AGENT_TASK_SCHEMA.into(),
                        task_id: "task".into(),
                        agent_id: crate::native_agent::CODEX_CAPABILITY_ID.into(),
                        workspace_name: "workspace".into(),
                        session_reused: false,
                        state: crate::native_agent::NativeAgentTaskStateV1::Running,
                        result: None,
                        code: Some(code.into()),
                        output: None,
                        conversation: None,
                        native_decisions: Vec::new(),
                    },
                })
                .unwrap(),
                &native_agent_test_context(),
            )
            .unwrap()
        };
        let first = validate_control_event(status_event("code_a"), "room", "source", "target", now)
            .unwrap();
        let second =
            validate_control_event(status_event("code_b"), "room", "source", "target", now)
                .unwrap();
        assert_ne!(first.request_id, second.request_id);

        let reconciliation_event = |digest: &str, code: &str| {
            native_agent_event(
                "native_agent.reconciliation",
                serde_json::to_value(crate::native_agent::NativeAgentReconciliationV1 {
                    schema_version: crate::native_agent::NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    task_id: "task".into(),
                    movement_id: Some("movement".into()),
                    executing_host_ref: "host:remote".into(),
                    task_state: crate::native_agent::NativeAgentTaskStateV1::Completed,
                    movement_state: Some(
                        crate::native_agent::NativeAgentWorkspaceMovementStateV1::ReturningResult,
                    ),
                    result_digest: Some(digest.into()),
                    apply_completed: false,
                    code: Some(code.into()),
                    task_output: None,
                })
                .unwrap(),
                &native_agent_test_context(),
            )
            .unwrap()
        };
        let third = validate_control_event(
            reconciliation_event(&"a".repeat(64), "code_a"),
            "room",
            "source",
            "target",
            now,
        )
        .unwrap();
        let fourth = validate_control_event(
            reconciliation_event(&"b".repeat(64), "code_b"),
            "room",
            "source",
            "target",
            now,
        )
        .unwrap();
        assert_ne!(third.request_id, fourth.request_id);
    }

    #[test]
    fn peer_observation_binding_normalizes_equivalent_ip_endpoints() {
        let ipv4 = RoomControlRouteEndpoint {
            peer_session_id: "peer-session".into(),
            host: "127.0.0.1".into(),
            port: 9000,
            transport_public_key: "key".into(),
        };
        let mapped_ipv6 = RoomControlRouteEndpoint {
            host: "::ffff:127.0.0.1".into(),
            ..ipv4.clone()
        };
        assert_eq!(
            peer_observation_ref(&ipv4),
            peer_observation_ref(&mapped_ipv6)
        );
    }

    #[test]
    fn windows_search_review_envelope_is_validated_before_inbox_persistence() {
        let now = OffsetDateTime::now_utc();
        let revision = crate::bridge_plan::build_file_search_revision(
            "room".into(),
            "source".into(),
            "target".into(),
            r"Find C:\Users\admin\Downloads\INFO2222-2026-PD.pdf".into(),
            "INFO2222-2026-PD.pdf".into(),
            vec!["pdf".into()],
            vec!["downloads".into()],
        )
        .unwrap();
        let approval = crate::bridge_plan::BridgePlanApproval {
            approval_id: "approval".into(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: revision.bridge_id.clone(),
            requester_device_ref: "source".into(),
            selected_device_ref: "target".into(),
            // The requester may be one second ahead of the receiver. A
            // one-day immutable review must not become invalid at that
            // boundary solely because of ordinary device clock skew.
            expires_at: now.unix_timestamp() + (24 * 60 * 60) + 1,
        };
        let event = serde_json::json!({
            "schemaVersion": ROOM_CONTROL_SCHEMA,
            "eventId": "event",
            "kind": "bridge_plan.review_request",
            "protocolFamily": BRIDGE_PLAN_PROTOCOL_FAMILY,
            "roomRef": "room",
            "sourceDeviceRef": "source",
            "targetPeerRef": "target",
            "createdAt": now.format(&Rfc3339).unwrap(),
            "expiresAt": (now + time::Duration::seconds(60)).format(&Rfc3339).unwrap(),
            "previewOnly": false,
            "payload": crate::bridge_plan::review_request_payload(&approval, &revision).unwrap(),
        });

        assert!(validate_control_event(event.clone(), "room", "source", "target", now).is_ok());

        let mut beyond_clock_skew = event;
        beyond_clock_skew["payload"]["reviewExpiresAt"] =
            Value::Number((now.unix_timestamp() + (24 * 60 * 60) + (5 * 60) + 1).into());
        let error = match validate_control_event(beyond_clock_skew, "room", "source", "target", now)
        {
            Err(error) => error.to_string(),
            Ok(_) => panic!("review expiry beyond permitted clock skew must be rejected"),
        };
        assert!(error.contains("review expiry"));
    }

    fn session_answer() -> crate::native_agent::sessions::remote::RemoteSessionResponseV1 {
        use crate::native_agent::sessions::remote::*;
        use crate::native_agent::sessions::*;
        let session = RemoteSessionV1 {
            provider: PROVIDER_CLAUDE_CODE.into(),
            session_id: "77777777-7777-4777-8777-777777777777".into(),
            scope_ref: "scope:0123456789abcdef".into(),
            workspace_label: "Pastey".into(),
            title: Some("Fix the transfer retry bug".into()),
            origin: "desktop".into(),
            created_at_ms: Some(1),
            last_activity_at_ms: Some(2),
            git_branch: Some("main".into()),
            live: RemoteLiveV1 {
                state: LiveStateV1::NotRunning,
                source: "claude agents --json".into(),
                observed_at_ms: 3,
                detail: None,
            },
            last_turn: RemoteLastTurnV1 {
                outcome: LastTurnV1::Completed,
                at_ms: Some(2),
                source: "transcript".into(),
                marker: None,
            },
            status: SessionStatusV1::Completed,
            status_basis: EvidenceBasisV1::Fact,
            resume: RemoteResumeV1 {
                verdict: ResumeVerdictV1::Safe,
                reasons: vec!["no process holds it".into()],
            },
        };
        RemoteSessionResponseV1 {
            schema_version: NATIVE_SESSION_SCHEMA.into(),
            request_id: "req-1".into(),
            source_host_ref: "host:windows".into(),
            outcome: RemoteSessionOutcomeV1::Ok,
            scopes: vec![RemoteScopeV1 {
                scope_ref: "scope:0123456789abcdef".into(),
                label: "Pastey".into(),
                share_excerpts: true,
                read_sessions: true,
                agent_requests: false,
                expires_at: None,
            }],
            discovery: None,
            snapshot: Some(RemoteSnapshotV1 {
                session,
                activity: RemoteActivityV1 {
                    user_turns: 1,
                    commands: 2,
                    failed_commands: 1,
                    changed_files: vec!["src/lib.rs".into()],
                    ..Default::default()
                },
                history_complete: true,
                excerpts_shared: true,
                recent: vec![RemoteEntryV1 {
                    role: "agent_claim".into(),
                    at_ms: Some(2),
                    text: "Done; tests pass.".into(),
                }],
                workspace: RemoteWorkspaceV1 {
                    exists: true,
                    git_branch: Some("main".into()),
                    uncommitted_paths: Some(0),
                    ..Default::default()
                },
                evidence: vec![RemoteEvidenceV1 {
                    basis: EvidenceBasisV1::Unknown,
                    statement: "Whether the work the agent describes was done is not verified."
                        .into(),
                    source: "agent reply".into(),
                }],
            }),
        }
    }

    #[test]
    fn native_session_query_and_answer_are_typed_session_bound_native_events() {
        use crate::native_agent::sessions::remote::*;
        let query = RemoteSessionQueryV1 {
            schema_version: NATIVE_SESSION_SCHEMA.into(),
            request_id: "req-1".into(),
            target_host_ref: "host:windows".into(),
            operation: RemoteSessionOperationV1::Inspect,
            provider: Some("claude_code".into()),
            text: None,
            active_within_secs: None,
            scope_ref: None,
            session_id: Some("77777777-7777-4777-8777-777777777777".into()),
        };
        let validated = validate_native(native_event(
            "native_agent.session_query",
            serde_json::to_value(&query).unwrap(),
        ))
        .unwrap();
        assert_eq!(
            validated.request_id.as_deref(),
            Some("native-session-query:req-1")
        );
        let answer = serde_json::to_value(session_answer()).unwrap();
        let validated = validate_native(native_event(
            "native_agent.session_response",
            answer.clone(),
        ))
        .unwrap();
        assert_eq!(
            validated.request_id.as_deref(),
            Some("native-session-response:req-1")
        );

        // Unknown fields, an inspection without its session, excerpts the
        // answer says were not shared, and unsafe fields are all refused.
        let mut extra = serde_json::to_value(&query).unwrap();
        extra["transcript"] = serde_json::json!("all of it");
        assert!(validate_native(native_event("native_agent.session_query", extra)).is_err());
        let mut no_session = query.clone();
        no_session.session_id = None;
        assert!(validate_native(native_event(
            "native_agent.session_query",
            serde_json::to_value(&no_session).unwrap()
        ))
        .is_err());
        let mut withheld = answer.clone();
        withheld["snapshot"]["excerptsShared"] = serde_json::json!(false);
        assert!(validate_native(native_event("native_agent.session_response", withheld)).is_err());
        let mut unsafe_answer = answer;
        unsafe_answer["snapshot"]["workspace"]["path"] = serde_json::json!("/Users/me/Pastey");
        assert!(
            validate_native(native_event("native_agent.session_response", unsafe_answer)).is_err()
        );
    }
}

#[cfg(all(test, unix))]
mod native_session_pair_tests;
#[cfg(test)]
pub(crate) mod runtime_pair_tests;

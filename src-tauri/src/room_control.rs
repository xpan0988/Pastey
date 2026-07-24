use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
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
    models::{BridgePeerLiveness, RoomStatus, StoredBridgePeerEndpoint},
    storage,
    transfer::RoomServerContext,
    AppState,
};

pub const MAX_CONTROL_REQUEST_BYTES: usize = 96 * 1024;
const MAX_CONTROL_EVENT_BYTES: usize = 64 * 1024;
const MAX_CONTROL_RESPONSE_BYTES: usize = 4 * 1024;
const MAX_EVENT_LIFETIME_SECONDS: i64 = 120;
const MAX_INBOX_ITEMS: usize = 64;
const MAX_REPLAY_ITEMS: usize = 256;
const MAX_EVENTS_PER_MINUTE: usize = 30;
const MAX_BURST_EVENTS: usize = 8;
const CONTROL_CONTENT_TYPE: &str = "application/vnd.pastey.room-control-envelope+json";
const CONTROL_RECEIPT_CONTENT_TYPE: &str = "application/vnd.pastey.room-control-receipt+json";
const CONTROL_ERROR_CONTENT_TYPE: &str = "application/vnd.pastey.room-control-error+json";
const CONTROL_TRANSPORT_SCHEMA: &str = "pastey-room-control-transport-v1";
const CONTROL_RECEIPT_ENVELOPE_SCHEMA: &str = "pastey-room-control-receipt-envelope-v1";
const CONTROL_DELIVERY_SCHEMA: &str = "pastey-room-control-delivery-v1";
const ROOM_CONTROL_SCHEMA: &str = "pastey-room-control-event-v1";
const CONTROL_BRIDGE_ROUTE_SCHEMA_VERSION: &str = "pastey-bridge-control-route-v1";
const ALLOWED_EVENT_KINDS: &[&str] = &[
    "bridge_plan.review_request",
    "bridge_plan.review_decision",
    "bridge_plan.attempt_start",
    "bridge_plan.transfer_start",
    "bridge_plan.transform_start",
    "bridge_plan.search_selection",
    "bridge_plan.attempt_ack",
    "bridge_plan.step_progress",
    "bridge_plan.step_result",
    "bridge_plan.step_failed",
    "bridge_plan.cancel",
];
const BRIDGE_PLAN_PROTOCOL_FAMILY: &str = "bridge_plan";

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
}

impl RoomControlRuntimeState {
    /// Burn is terminal for a Bridge: retain no inbox, replay cache, or rate
    /// state that could replay or display an old control action.
    pub fn purge_room(&mut self, room_id: &str) -> bool {
        self.rooms.remove(room_id).is_some()
    }
}

#[derive(Default)]
struct RoomControlRoomState {
    inbox: VecDeque<ReceivedRoomControlEvent>,
    seen_event_ids: VecDeque<String>,
    seen_event_id_set: HashSet<String>,
    seen_envelope_ids: VecDeque<String>,
    seen_envelope_id_set: HashSet<String>,
    seen_request_ids: VecDeque<String>,
    seen_request_id_set: HashSet<String>,
    received_at_seconds: VecDeque<i64>,
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomControlSendError {
    pub code: &'static str,
    pub message: &'static str,
}

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
        peer_route_ref: peer.peer_session_id,
        peer_connected: true,
    })
}

pub async fn send_room_control_event(
    state: Arc<AppState>,
    room_id: &str,
    event: Value,
    bridge_route: Option<Value>,
) -> AppResult<RoomControlDeliveryReceipt> {
    send_room_control_event_internal(state, room_id, event, bridge_route).await
}

async fn send_room_control_event_internal(
    state: Arc<AppState>,
    room_id: &str,
    event: Value,
    bridge_route: Option<Value>,
) -> AppResult<RoomControlDeliveryReceipt> {
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

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| AppError::Network("Room control transport unavailable.".into()))?;
    let response = client
        .post(format!(
            "http://{}:{}/rooms/{room_id}/control-events",
            peer.host, peer.port
        ))
        .header(header::CONTENT_TYPE.as_str(), CONTROL_CONTENT_TYPE)
        .header(header::ACCEPT.as_str(), CONTROL_RECEIPT_CONTENT_TYPE)
        .body(body)
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
    let receipt_plaintext = crypto::decrypt_bytes(&receipt_ciphertext, &event_key, &receipt_nonce)
        .map_err(|_| AppError::Network("Room control receipt is invalid.".into()))?;
    let receipt: RoomControlDeliveryReceipt = serde_json::from_slice(&receipt_plaintext)
        .map_err(|_| AppError::Network("Room control receipt is invalid.".into()))?;
    if receipt.schema_version != CONTROL_DELIVERY_SCHEMA
        || receipt.event_id != validated.event_id
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
        .map(|room| room.inbox.iter().cloned().collect())
        .unwrap_or_default())
}

pub fn clear_room_control_state(state: &Arc<AppState>, room_id: &str) {
    state.room_control.lock().rooms.remove(room_id);
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
        event,
        &room_id,
        &session_ref(&inbound_peer.transport_public_key),
        &session_ref(&local_key),
        OffsetDateTime::now_utc(),
    ) {
        Ok(value) => value,
        Err(AppError::InvalidInput(message)) if message.contains("expired") => {
            return control_error(
                StatusCode::GONE,
                "event_expired",
                "Room control event expired.",
            )
        }
        Err(AppError::InvalidInput(message)) if message.contains("session mismatch") => {
            return control_error(
                StatusCode::FORBIDDEN,
                "session_mismatch",
                "Room control session mismatch.",
            )
        }
        Err(_) => {
            return control_error(
                StatusCode::BAD_REQUEST,
                "invalid_event",
                "Invalid room control event.",
            )
        }
    };
    if crate::bridge_plan::accept_inbound_protocol_event(
        &ctx.state.paths,
        &ctx.state.bridge_plan_protocol_authority.lock(),
        &mut ctx.state.bridge_plan_candidate_store.lock(),
        &validated.kind,
        &validated.event,
        storage::now_ts(),
    )
    .is_err()
    {
        return control_error(
            StatusCode::BAD_REQUEST,
            "invalid_event",
            "Invalid room control event.",
        );
    }
    let received_at = now_iso();
    {
        let mut runtime = ctx.state.room_control.lock();
        let room_state = runtime.rooms.entry(room_id.clone()).or_default();
        if room_state.inbox.len() >= MAX_INBOX_ITEMS {
            return control_error(
                StatusCode::TOO_MANY_REQUESTS,
                "inbox_full",
                "Room control inbox is full.",
            );
        }
        if !accept_rate_limited_event(room_state, OffsetDateTime::now_utc().unix_timestamp()) {
            return control_error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Room control event rate exceeded.",
            );
        }
        if is_replayed(room_state, &validated) {
            return control_error(
                StatusCode::CONFLICT,
                "event_replayed",
                "Room control event was already received.",
            );
        }
        record_replay_id(
            &mut room_state.seen_event_ids,
            &mut room_state.seen_event_id_set,
            validated.event_id.clone(),
        );
        if let Some(id) = validated.envelope_id.clone() {
            record_replay_id(
                &mut room_state.seen_envelope_ids,
                &mut room_state.seen_envelope_id_set,
                id,
            );
        }
        if let Some(id) = validated.request_id.clone() {
            record_replay_id(
                &mut room_state.seen_request_ids,
                &mut room_state.seen_request_id_set,
                id,
            );
        }
        room_state.inbox.push_back(ReceivedRoomControlEvent {
            event_id: validated.event_id.clone(),
            kind: validated.kind,
            room_ref: validated.room_ref,
            source_device_ref: validated.source_device_ref,
            target_peer_ref: validated.target_peer_ref,
            created_at: validated.created_at,
            expires_at: validated.expires_at,
            received_at: received_at.clone(),
            event: validated.event,
        });
    }
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
        if raw_kind.starts_with("bridge_plan.") {
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
    if contains_unsafe_field(&event) {
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
        let metadata = crate::bridge_plan::protocol_metadata(
            &kind,
            payload,
            expected_room,
            expected_source,
            expected_target,
            now.unix_timestamp(),
        )?;
        (None, Some(metadata.replay_id))
    } else {
        return Err(AppError::InvalidInput("Unsupported room control event kind.".into()));
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
    let message = match error_code.as_deref() {
        Some("event_expired") => "Room control event expired before delivery.",
        Some("event_replayed") => "Room control event was already received.",
        Some("session_mismatch") => "Room control session mismatch.",
        Some("inbox_full") => "Room control inbox is full.",
        Some("rate_limited") => "Room control rate limit was reached.",
        Some("request_too_large" | "event_too_large") => "Room control event is too large.",
        Some("invalid_event" | "invalid_envelope" | "invalid_request") => {
            "Room control event validation failed."
        }
        _ => match status {
            StatusCode::CONFLICT => "Room control event was already received.",
            StatusCode::GONE => "Room control event or room is unavailable.",
            StatusCode::PAYLOAD_TOO_LARGE => "Room control event is too large.",
            StatusCode::TOO_MANY_REQUESTS => "Room control transport rejected the event.",
            StatusCode::FORBIDDEN => "Room control session mismatch.",
            _ => "Room control delivery failed.",
        },
    };
    AppError::Network(message.into())
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

fn accept_rate_limited_event(room: &mut RoomControlRoomState, now_seconds: i64) -> bool {
    while room
        .received_at_seconds
        .front()
        .is_some_and(|timestamp| *timestamp <= now_seconds - 60)
    {
        room.received_at_seconds.pop_front();
    }
    let burst_count = room
        .received_at_seconds
        .iter()
        .filter(|timestamp| **timestamp > now_seconds - 2)
        .count();
    if room.received_at_seconds.len() >= MAX_EVENTS_PER_MINUTE || burst_count >= MAX_BURST_EVENTS {
        return false;
    }
    room.received_at_seconds.push_back(now_seconds);
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

fn contains_unsafe_field(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_unsafe_field),
        Value::Object(object) => object.iter().any(|(key, value)| {
            UNSAFE_FIELDS.contains(&normalize_field(key).as_str()) || contains_unsafe_field(value)
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
    fn replay_and_inbox_bounds_are_finite() {
        let mut queue = VecDeque::new();
        let mut set = HashSet::new();
        for index in 0..(MAX_REPLAY_ITEMS + 10) {
            record_replay_id(&mut queue, &mut set, format!("event-{index}"));
        }
        assert_eq!(queue.len(), MAX_REPLAY_ITEMS);
        assert_eq!(set.len(), MAX_REPLAY_ITEMS);
        assert_eq!(MAX_INBOX_ITEMS, 64);
        assert_eq!(MAX_CONTROL_REQUEST_BYTES, 96 * 1024);
        assert_eq!(MAX_CONTROL_EVENT_BYTES, 64 * 1024);
        assert_eq!(MAX_CONTROL_RESPONSE_BYTES, 4 * 1024);
        let mut room = RoomControlRoomState::default();
        for _ in 0..MAX_BURST_EVENTS {
            assert!(accept_rate_limited_event(&mut room, 100));
        }
        assert!(!accept_rate_limited_event(&mut room, 100));
    }

    #[test]
    fn burn_purge_removes_room_control_inbox_replay_and_rate_state() {
        let mut runtime = RoomControlRuntimeState::default();
        let mut room = RoomControlRoomState::default();
        room.inbox.push_back(ReceivedRoomControlEvent {
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
        room.received_at_seconds.push_back(1);
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


}

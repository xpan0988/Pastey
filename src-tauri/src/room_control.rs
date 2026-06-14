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
    models::RoomStatus,
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
const CONTROL_TRANSPORT_SCHEMA: &str = "pastey-room-control-transport/v1";
const CONTROL_RECEIPT_ENVELOPE_SCHEMA: &str = "pastey-room-control-receipt-envelope/v1";
const CONTROL_DELIVERY_SCHEMA: &str = "pastey-room-control-delivery/v1";
const ROOM_CONTROL_SCHEMA: &str = "pastey-room-control-event/v1";

const ALLOWED_EVENT_KINDS: &[&str] = &[
    "capability_preview",
    "capability_preview_ack",
    "capability_preview_deny",
    "capability_preview_invalid",
    "capability_preview_expired",
    "capability_execute_request",
    "capability_execution_result",
];

const UNSAFE_FIELDS: &[&str] = &[
    "command",
    "cmd",
    "shell",
    "script",
    "code",
    "path",
    "absolutepath",
    "filepath",
    "filesystemtree",
    "rawlogs",
    "secret",
    "token",
    "apikey",
    "roomkey",
    "roomcode",
    "transportkey",
    "hiddentransfer",
    "peerfilesystemsearch",
    "stdout",
    "stderr",
    "exitcode",
    "process",
    "spawn",
];

#[derive(Default)]
pub struct RoomControlRuntimeState {
    rooms: HashMap<String, RoomControlRoomState>,
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

pub fn room_control_session_context(
    state: &Arc<AppState>,
    room_id: &str,
) -> AppResult<RoomControlSessionContext> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    if room.status != RoomStatus::Active {
        return Err(AppError::InvalidInput("Room is not active.".into()));
    }
    let peer_key = room
        .peer_transport_public_key
        .ok_or_else(|| AppError::InvalidInput("Peer is unavailable.".into()))?;
    let local_key = state
        .active_servers
        .lock()
        .get(room_id)
        .map(|server| server.transport_public_key())
        .ok_or_else(|| AppError::InvalidInput("Room session is unavailable.".into()))?;
    Ok(RoomControlSessionContext {
        room_id: room_id.to_string(),
        local_session_ref: session_ref(&local_key),
        peer_session_ref: session_ref(&peer_key),
        peer_connected: room.peer_host.is_some() && room.peer_port.is_some(),
    })
}

pub async fn send_room_control_event(
    state: Arc<AppState>,
    room_id: &str,
    event: Value,
) -> AppResult<RoomControlDeliveryReceipt> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    if room.status != RoomStatus::Active {
        return Err(AppError::InvalidInput("Room is not active.".into()));
    }
    let peer_host = room
        .peer_host
        .ok_or_else(|| AppError::InvalidInput("Peer is unavailable.".into()))?;
    let peer_port = room
        .peer_port
        .ok_or_else(|| AppError::InvalidInput("Peer is unavailable.".into()))?;
    let peer_key = room
        .peer_transport_public_key
        .ok_or_else(|| AppError::InvalidInput("Peer is unavailable.".into()))?;
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
        &session_ref(&peer_key),
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
    let receiver_key = crypto::decode_key(&peer_key)?;
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
            "http://{peer_host}:{peer_port}/rooms/{room_id}/control-events"
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
    ConnectInfo(source): ConnectInfo<SocketAddr>,
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
    if room.peer_host.as_deref() != Some(&source.ip().to_string()) {
        return control_error(
            StatusCode::FORBIDDEN,
            "session_mismatch",
            "Room session mismatch.",
        );
    }
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
        || room.peer_transport_public_key.as_deref() != Some(&envelope.sender_public_key)
    {
        return control_error(
            StatusCode::FORBIDDEN,
            "session_mismatch",
            "Room session mismatch.",
        );
    }
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
        &session_ref(&envelope.sender_public_key),
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
    if contains_unsafe_field(&event) {
        return Err(AppError::InvalidInput(
            "Room control event contains unsafe fields.".into(),
        ));
    }
    let object = event
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("Invalid room control event.".into()))?;
    require_exact_fields(
        object,
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
        ],
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
    let (envelope_id, request_id) = if kind == "capability_preview" {
        if object.get("previewOnly") != Some(&Value::Bool(true)) {
            return Err(AppError::InvalidInput(
                "Invalid capability preview event.".into(),
            ));
        }
        require_exact_fields(
            payload,
            &[
                "schemaVersion",
                "envelopeId",
                "createdAt",
                "expiresAt",
                "roomRef",
                "sourceDeviceRef",
                "targetPeerRef",
                "request",
                "previewOnly",
                "status",
            ],
        )?;
        if string_field(payload, "schemaVersion")? != "pastey-capability-preview/v1"
            || payload.get("previewOnly") != Some(&Value::Bool(true))
            || string_field(payload, "status")? != "outbound_preview"
            || string_field(payload, "roomRef")? != expected_room
            || string_field(payload, "sourceDeviceRef")? != expected_source
            || string_field(payload, "targetPeerRef")? != expected_target
        {
            return Err(AppError::InvalidInput(
                "Invalid capability preview envelope.".into(),
            ));
        }
        let envelope_id = bounded_string_field(payload, "envelopeId", 256)?;
        let request = payload
            .get("request")
            .and_then(Value::as_object)
            .ok_or_else(|| AppError::InvalidInput("Invalid preview request.".into()))?;
        if string_field(request, "sourceDeviceRef")? != expected_source
            || string_field(request, "targetPeerRef")? != expected_target
            || string_field(request, "transportStatus")? != "preview_only"
        {
            return Err(AppError::InvalidInput("Invalid preview request.".into()));
        }
        (
            Some(envelope_id),
            Some(bounded_string_field(request, "requestId", 256)?),
        )
    } else if kind == "capability_execute_request" {
        if object.get("previewOnly") != Some(&Value::Bool(false)) {
            return Err(AppError::InvalidInput(
                "Invalid execution request event.".into(),
            ));
        }
        require_exact_fields(
            payload,
            &[
                "schemaVersion",
                "executionId",
                "consentId",
                "sourcePreviewEventId",
                "envelopeId",
                "requestId",
                "requestPayloadHash",
                "roomRef",
                "sourceDeviceRef",
                "targetPeerRef",
                "capability",
                "exactMessage",
                "createdAt",
                "expiresAt",
            ],
        )?;
        if string_field(payload, "schemaVersion")? != "pastey-hello-peer-execution-request/v1"
            || string_field(payload, "roomRef")? != expected_room
            || string_field(payload, "sourceDeviceRef")? != expected_source
            || string_field(payload, "targetPeerRef")? != expected_target
            || string_field(payload, "capability")? != "runtime.execute_hello_template"
            || string_field(payload, "exactMessage")? != "hello peer!"
        {
            return Err(AppError::InvalidInput(
                "Invalid execution request payload.".into(),
            ));
        }
        for field in [
            "consentId",
            "sourcePreviewEventId",
            "envelopeId",
            "requestId",
            "requestPayloadHash",
        ] {
            let _ = bounded_string_field(payload, field, 256)?;
        }
        let execution_id = bounded_string_field(payload, "executionId", 256)?;
        let payload_created = OffsetDateTime::parse(string_field(payload, "createdAt")?, &Rfc3339)
            .map_err(|_| AppError::InvalidInput("Invalid execution request time.".into()))?;
        let payload_expires = OffsetDateTime::parse(string_field(payload, "expiresAt")?, &Rfc3339)
            .map_err(|_| AppError::InvalidInput("Invalid execution request time.".into()))?;
        if payload_expires <= now || payload_expires <= payload_created || payload_expires > expires
        {
            return Err(AppError::InvalidInput(
                "Invalid execution request time.".into(),
            ));
        }
        (None, Some(format!("exec-request:{execution_id}")))
    } else if kind == "capability_execution_result" {
        if object.get("previewOnly") != Some(&Value::Bool(false)) {
            return Err(AppError::InvalidInput(
                "Invalid execution result event.".into(),
            ));
        }
        let allowed_len = if payload.contains_key("output") || payload.contains_key("errorCode") {
            7
        } else {
            6
        };
        if payload.len() != allowed_len
            || !payload.contains_key("schemaVersion")
            || !payload.contains_key("executionId")
            || !payload.contains_key("requestId")
            || !payload.contains_key("consentId")
            || !payload.contains_key("status")
            || !payload.contains_key("createdAt")
        {
            return Err(AppError::InvalidInput(
                "Invalid execution result payload.".into(),
            ));
        }
        if string_field(payload, "schemaVersion")? != "pastey-hello-peer-execution-result/v1" {
            return Err(AppError::InvalidInput(
                "Invalid execution result payload.".into(),
            ));
        }
        let execution_id = bounded_string_field(payload, "executionId", 256)?;
        let _ = bounded_string_field(payload, "requestId", 256)?;
        let _ = bounded_string_field(payload, "consentId", 256)?;
        let _ = OffsetDateTime::parse(string_field(payload, "createdAt")?, &Rfc3339)
            .map_err(|_| AppError::InvalidInput("Invalid execution result time.".into()))?;
        let status = string_field(payload, "status")?;
        if status == "succeeded" {
            if payload.get("output") != Some(&Value::String("hello peer!".into()))
                || payload.contains_key("errorCode")
            {
                return Err(AppError::InvalidInput(
                    "Invalid execution result payload.".into(),
                ));
            }
        } else if !["rejected", "expired", "already_consumed", "failed"].contains(&status)
            || payload.contains_key("output")
            || !payload.contains_key("errorCode")
        {
            return Err(AppError::InvalidInput(
                "Invalid execution result payload.".into(),
            ));
        } else {
            let _ = bounded_string_field(payload, "errorCode", 64)?;
        }
        (None, Some(format!("exec-result:{execution_id}")))
    } else {
        if object.get("previewOnly") != Some(&Value::Bool(true)) {
            return Err(AppError::InvalidInput(
                "Invalid preview status event.".into(),
            ));
        }
        let base_len = if payload.contains_key("reason") { 4 } else { 3 };
        let allowed_len = if payload.contains_key("consent") {
            base_len + 1
        } else {
            base_len
        };
        if payload.len() != allowed_len
            || !payload.contains_key("envelopeId")
            || !payload.contains_key("requestId")
            || !payload.contains_key("status")
        {
            return Err(AppError::InvalidInput(
                "Invalid preview status payload.".into(),
            ));
        }
        let expected_status = match kind.as_str() {
            "capability_preview_ack" => "acknowledged_preview_only",
            "capability_preview_deny" => "denied",
            "capability_preview_invalid" => "invalid",
            "capability_preview_expired" => "expired",
            _ => unreachable!(),
        };
        if string_field(payload, "status")? != expected_status {
            return Err(AppError::InvalidInput(
                "Invalid preview status payload.".into(),
            ));
        }
        let _ = bounded_string_field(payload, "envelopeId", 256)?;
        let _ = bounded_string_field(payload, "requestId", 256)?;
        if payload.contains_key("reason") {
            let _ = bounded_string_field(payload, "reason", 512)?;
        }
        if payload.contains_key("consent") {
            if kind != "capability_preview_ack" {
                return Err(AppError::InvalidInput(
                    "Invalid preview status payload.".into(),
                ));
            }
            let consent = payload
                .get("consent")
                .and_then(Value::as_object)
                .ok_or_else(|| AppError::InvalidInput("Invalid consent grant.".into()))?;
            require_exact_fields(
                consent,
                &[
                    "schemaVersion",
                    "consentId",
                    "sourcePreviewEventId",
                    "envelopeId",
                    "requestId",
                    "requestPayloadHash",
                    "capability",
                    "exactMessage",
                    "expiresAt",
                ],
            )?;
            if string_field(consent, "schemaVersion")? != "pastey-hello-peer-consent-grant/v1"
                || string_field(consent, "capability")? != "runtime.execute_hello_template"
                || string_field(consent, "exactMessage")? != "hello peer!"
            {
                return Err(AppError::InvalidInput("Invalid consent grant.".into()));
            }
            for field in [
                "consentId",
                "sourcePreviewEventId",
                "envelopeId",
                "requestId",
                "requestPayloadHash",
            ] {
                let _ = bounded_string_field(consent, field, 256)?;
            }
            let consent_expires =
                OffsetDateTime::parse(string_field(consent, "expiresAt")?, &Rfc3339)
                    .map_err(|_| AppError::InvalidInput("Invalid consent grant.".into()))?;
            if consent_expires <= now {
                return Err(AppError::InvalidInput("Invalid consent grant.".into()));
            }
        }
        (None, None)
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

    fn preview_event(event_id: &str, envelope_id: &str, request_id: &str) -> Value {
        let now = OffsetDateTime::now_utc();
        let created_at = now.format(&Rfc3339).unwrap();
        let expires_at = (now + time::Duration::seconds(60))
            .format(&Rfc3339)
            .unwrap();
        serde_json::json!({
            "schemaVersion": ROOM_CONTROL_SCHEMA,
            "eventId": event_id,
            "kind": "capability_preview",
            "roomRef": "room",
            "sourceDeviceRef": "source",
            "targetPeerRef": "target",
            "createdAt": created_at,
            "expiresAt": expires_at,
            "previewOnly": true,
            "payload": {
                "schemaVersion": "pastey-capability-preview/v1",
                "envelopeId": envelope_id,
                "createdAt": created_at,
                "expiresAt": expires_at,
                "roomRef": "room",
                "sourceDeviceRef": "source",
                "targetPeerRef": "target",
                "request": {
                    "requestId": request_id,
                    "sourceDeviceRef": "source",
                    "targetPeerRef": "target",
                    "transportStatus": "preview_only"
                },
                "previewOnly": true,
                "status": "outbound_preview"
            }
        })
    }

    fn status_event(event_id: &str) -> Value {
        let now = OffsetDateTime::now_utc();
        serde_json::json!({
            "schemaVersion": ROOM_CONTROL_SCHEMA,
            "eventId": event_id,
            "kind": "capability_preview_ack",
            "roomRef": "room",
            "sourceDeviceRef": "source",
            "targetPeerRef": "target",
            "createdAt": now.format(&Rfc3339).unwrap(),
            "expiresAt": (now + time::Duration::seconds(60)).format(&Rfc3339).unwrap(),
            "previewOnly": true,
            "payload": {
                "envelopeId": "envelope",
                "requestId": "request",
                "status": "acknowledged_preview_only"
            }
        })
    }

    fn execution_request_event(event_id: &str, execution_id: &str) -> Value {
        let now = OffsetDateTime::now_utc();
        let created_at = now.format(&Rfc3339).unwrap();
        let expires_at = (now + time::Duration::seconds(60))
            .format(&Rfc3339)
            .unwrap();
        serde_json::json!({
            "schemaVersion": ROOM_CONTROL_SCHEMA,
            "eventId": event_id,
            "kind": "capability_execute_request",
            "roomRef": "room",
            "sourceDeviceRef": "source",
            "targetPeerRef": "target",
            "createdAt": created_at,
            "expiresAt": expires_at,
            "previewOnly": false,
            "payload": {
                "schemaVersion": "pastey-hello-peer-execution-request/v1",
                "executionId": execution_id,
                "consentId": "consent",
                "sourcePreviewEventId": "preview",
                "envelopeId": "envelope",
                "requestId": "request",
                "requestPayloadHash": "hash",
                "roomRef": "room",
                "sourceDeviceRef": "source",
                "targetPeerRef": "target",
                "capability": "runtime.execute_hello_template",
                "exactMessage": "hello peer!",
                "createdAt": created_at,
                "expiresAt": expires_at
            }
        })
    }

    fn execution_result_event(event_id: &str, execution_id: &str) -> Value {
        let now = OffsetDateTime::now_utc();
        serde_json::json!({
            "schemaVersion": ROOM_CONTROL_SCHEMA,
            "eventId": event_id,
            "kind": "capability_execution_result",
            "roomRef": "room",
            "sourceDeviceRef": "source",
            "targetPeerRef": "target",
            "createdAt": now.format(&Rfc3339).unwrap(),
            "expiresAt": (now + time::Duration::seconds(60)).format(&Rfc3339).unwrap(),
            "previewOnly": false,
            "payload": {
                "schemaVersion": "pastey-hello-peer-execution-result/v1",
                "executionId": execution_id,
                "requestId": "request",
                "consentId": "consent",
                "status": "succeeded",
                "output": "hello peer!",
                "createdAt": now.format(&Rfc3339).unwrap()
            }
        })
    }

    #[test]
    fn validates_preview_and_rejects_unknown_or_execution_fields() {
        let now = OffsetDateTime::now_utc();
        assert!(validate_control_event(
            preview_event("e1", "v1", "r1"),
            "room",
            "source",
            "target",
            now
        )
        .is_ok());
        let mut unknown = preview_event("e2", "v2", "r2");
        unknown["kind"] = Value::String("capability_request".into());
        assert!(validate_control_event(unknown, "room", "source", "target", now).is_err());
        let mut unsafe_event = preview_event("e3", "v3", "r3");
        unsafe_event["stdout"] = Value::String("not allowed".into());
        assert!(validate_control_event(unsafe_event, "room", "source", "target", now).is_err());
        assert!(
            validate_control_event(status_event("status-1"), "room", "source", "target", now)
                .is_ok()
        );
    }

    #[test]
    fn validates_only_fixed_bounded_hello_peer_execution_events() {
        let now = OffsetDateTime::now_utc();
        assert!(validate_control_event(
            execution_request_event("execute", "execution"),
            "room",
            "source",
            "target",
            now
        )
        .is_ok());
        assert!(validate_control_event(
            execution_result_event("result", "execution"),
            "room",
            "source",
            "target",
            now
        )
        .is_ok());

        let mut wrong_message = execution_request_event("wrong-message", "execution-2");
        wrong_message["payload"]["exactMessage"] = Value::String("arbitrary".into());
        assert!(validate_control_event(wrong_message, "room", "source", "target", now).is_err());

        let mut arbitrary_output = execution_result_event("wrong-output", "execution-3");
        arbitrary_output["payload"]["output"] = Value::String("arbitrary".into());
        assert!(validate_control_event(arbitrary_output, "room", "source", "target", now).is_err());

        let mut stdout = execution_result_event("stdout", "execution-4");
        stdout["payload"]["stdout"] = Value::String("no".into());
        assert!(validate_control_event(stdout, "room", "source", "target", now).is_err());
    }

    #[test]
    fn rejects_expired_room_and_target_mismatch() {
        let now = OffsetDateTime::now_utc();
        let mut expired = preview_event("e1", "v1", "r1");
        expired["expiresAt"] =
            Value::String((now - time::Duration::seconds(1)).format(&Rfc3339).unwrap());
        assert!(validate_control_event(expired, "room", "source", "target", now).is_err());
        assert!(validate_control_event(
            preview_event("e2", "v2", "r2"),
            "other",
            "source",
            "target",
            now
        )
        .is_err());
        assert!(validate_control_event(
            preview_event("e3", "v3", "r3"),
            "room",
            "source",
            "other",
            now
        )
        .is_err());
    }

    #[test]
    fn rejects_oversized_events_reasons_and_malformed_ciphertext() {
        let now = OffsetDateTime::now_utc();
        let mut oversized = preview_event("e1", "v1", "r1");
        oversized["payload"]["request"]["padding"] =
            Value::String("x".repeat(MAX_CONTROL_EVENT_BYTES));
        assert!(validate_control_event(oversized, "room", "source", "target", now).is_err());

        let mut oversized_reason = status_event("status-1");
        oversized_reason["payload"]["reason"] = Value::String("x".repeat(513));
        assert!(validate_control_event(oversized_reason, "room", "source", "target", now).is_err());

        assert!(crypto::decrypt_bytes(&[1, 2, 3], &crypto::random_key(), &[0; 12]).is_err());
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
    fn replay_checks_event_preview_envelope_and_request_ids() {
        let now = OffsetDateTime::now_utc();
        let event = validate_control_event(
            preview_event("e1", "v1", "r1"),
            "room",
            "source",
            "target",
            now,
        )
        .unwrap();
        let mut room = RoomControlRoomState::default();
        assert!(!is_replayed(&room, &event));

        record_replay_id(
            &mut room.seen_event_ids,
            &mut room.seen_event_id_set,
            event.event_id.clone(),
        );
        assert!(is_replayed(&room, &event));

        room = RoomControlRoomState::default();
        record_replay_id(
            &mut room.seen_envelope_ids,
            &mut room.seen_envelope_id_set,
            event.envelope_id.clone().unwrap(),
        );
        assert!(is_replayed(&room, &event));

        room = RoomControlRoomState::default();
        record_replay_id(
            &mut room.seen_request_ids,
            &mut room.seen_request_id_set,
            event.request_id.clone().unwrap(),
        );
        assert!(is_replayed(&room, &event));
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

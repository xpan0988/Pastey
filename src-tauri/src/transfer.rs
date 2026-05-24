use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, ConnectInfo, DefaultBodyLimit, Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{stream::FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tokio::{
    fs::OpenOptions,
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    net::TcpListener,
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;

use crate::{
    chunk_frame::{
        decode_binary_chunk_frame, encode_binary_chunk_frame, BinaryChunkFrame,
        BinaryChunkFrameError, BINARY_CHUNK_NONCE_LEN,
    },
    config, crypto, discovery,
    error::{AppError, AppResult},
    logging,
    models::{
        ChunkAckResponse, ChunkUploadRequest, FileTransferFinishRequest, FileTransferProgressEvent,
        FileTransferStartRequest, JoinRoomRequest, JoinRoomResponse, PayloadType, RoomItemStatus,
        RoomItemUpload, RoomStatus, TransferErrorResponse,
    },
    storage,
    transfer_tuning::{self, TransferTuning},
    ActiveRoomServer, AppState,
};

pub const DEFAULT_CHUNK_SIZE_BYTES: u64 = 4 * 1024 * 1024;
const DISK_SPACE_MARGIN_BYTES: u64 = 512 * 1024 * 1024;
const TRANSFER_EVENT: &str = "pastey://transfer-progress";
const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CHUNK_BODY_BYTES: usize = 16 * 1024 * 1024;
const CHUNK_PROTOCOL_HEADER: &str = "x-pastey-chunk-protocol";
const CHUNK_PROTOCOL_BINARY_V1: &str = "binary-v1";
const CHUNK_PROTOCOL_JSON_V1: &str = "json-v1";
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(250);
const TRANSFER_CANCELLED_MESSAGE: &str = "Transfer cancelled";
const TRANSFER_INTERRUPTED_MESSAGE: &str = "Transfer interrupted";
const PEER_DISCONNECTED_MESSAGE: &str = "Peer disconnected";
const ROOM_BURNED_MESSAGE: &str = "Room burned";
const TERMINAL_TRANSFER_REASON_TTL: Duration = Duration::from_secs(120);
const CHUNK_RETRY_BACKOFFS: [Duration; 3] = [
    Duration::from_millis(300),
    Duration::from_millis(800),
    Duration::from_millis(1500),
];

pub struct ActiveFileTransfer {
    room_id: String,
    item_id: String,
    file_name: String,
    file_size: u64,
    chunk_size: u64,
    total_chunks: u64,
    started_at: Instant,
    last_report_at: Instant,
    last_report_bytes: u64,
    cancel_token: CancellationToken,
    kind: ActiveFileTransferKind,
}

#[derive(Clone)]
pub struct TerminalTransferReason {
    code: String,
    message: String,
    recorded_at: Instant,
}

enum ActiveFileTransferKind {
    Sender,
    Receiver {
        session_key: [u8; 32],
        part_path: PathBuf,
        final_path: PathBuf,
        mime_type: Option<String>,
        created_at: i64,
        transferred_bytes: u64,
        expected_chunk_index: u64,
        received_chunks: Vec<bool>,
        timing: ReceiverTimingSummary,
    },
}

#[derive(Clone)]
struct RoomServerContext {
    state: Arc<AppState>,
    room_id: String,
}

#[derive(Clone)]
struct ActiveRoomSnapshot {
    port: u16,
    transport_secret: [u8; 32],
    transport_public_key: String,
}

#[derive(Deserialize, Serialize)]
struct TransferOkResponse {
    ok: bool,
}

#[derive(Deserialize, Serialize)]
struct FileTransferStartResponse {
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preferred_chunk_protocol: Option<String>,
    #[serde(default)]
    supported_chunk_protocols: Vec<String>,
}

#[derive(Deserialize)]
struct TransferCancelRequest {
    status: Option<String>,
    message: Option<String>,
    reason: Option<String>,
}

struct ResponseErrorDetails {
    status: StatusCode,
    code: Option<String>,
    message: String,
    body_text: String,
}

#[derive(Debug)]
struct ReceiverWriteFailure {
    code: &'static str,
    message: &'static str,
    status: StatusCode,
    cause: String,
    parent_exists: bool,
    file_exists: bool,
}

#[derive(Debug)]
struct ChunkSendFailure {
    message: String,
    kind: ChunkSendFailureKind,
    retryable: bool,
}

#[derive(Debug)]
struct ReceivedChunkUpload {
    chunk_index: u64,
    nonce: [u8; BINARY_CHUNK_NONCE_LEN],
    ciphertext: Vec<u8>,
    plaintext_size: u64,
    is_final: bool,
    protocol: ChunkProtocol,
    payload_body_size: usize,
    encoded_ciphertext_bytes: usize,
}

#[derive(Debug)]
struct ChunkPayloadDecodeFailure {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    cause: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct SenderTimingSummary {
    chunks: u64,
    read_ms: u128,
    encrypt_ms: u128,
    request_ms: u128,
}

#[derive(Clone, Copy, Debug, Default)]
struct ReceiverTimingSummary {
    chunks: u64,
    decode_ms: u128,
    decrypt_ms: u128,
    write_ms: u128,
    ui_emit_ms: u128,
    duplicate_chunks: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChunkProtocol {
    BinaryV1,
    JsonV1,
}

impl ChunkProtocol {
    fn as_str(self) -> &'static str {
        match self {
            Self::BinaryV1 => CHUNK_PROTOCOL_BINARY_V1,
            Self::JsonV1 => CHUNK_PROTOCOL_JSON_V1,
        }
    }

    fn log_label(self) -> &'static str {
        match self {
            Self::BinaryV1 => "binary-v1",
            Self::JsonV1 => "json-legacy",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChunkSendFailureKind {
    Cancelled,
    ChunkTooLarge,
    HttpStatus,
    InvalidAck,
    PeerLeft,
    Timeout,
    UnsupportedProtocol,
    Unreachable,
}

impl ChunkSendFailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::ChunkTooLarge => "chunk_too_large",
            Self::HttpStatus => "http_status",
            Self::InvalidAck => "invalid_ack",
            Self::PeerLeft => "peer_left",
            Self::Timeout => "timeout",
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::Unreachable => "unreachable",
        }
    }
}

pub async fn start_room_server(state: Arc<AppState>, room_id: &str) -> AppResult<u16> {
    if let Some(port) = {
        let servers = state.active_servers.lock();
        servers.get(room_id).map(|server| server.port)
    } {
        return Ok(port);
    }

    let room = storage::get_room_by_id(&state.paths, room_id)?;

    let transport_secret = crypto::generate_transport_secret();
    let router = Router::new()
        .route("/rooms/:room_id/join", post(join_handler))
        .route("/rooms/:room_id/items", post(receive_item_handler))
        .route(
            "/rooms/:room_id/transfers/start",
            post(start_file_transfer_handler),
        )
        .route(
            "/rooms/:room_id/transfers/:transfer_id/chunks",
            post(receive_file_chunk_handler).layer(DefaultBodyLimit::max(MAX_CHUNK_BODY_BYTES)),
        )
        .route(
            "/rooms/:room_id/transfers/:transfer_id/finish",
            post(finish_file_transfer_handler),
        )
        .route(
            "/rooms/:room_id/transfers/:transfer_id/cancel",
            post(cancel_file_transfer_handler),
        )
        .route("/rooms/:room_id/burn", post(remote_burn_handler))
        .route("/rooms/:room_id/leave", post(remote_leave_handler))
        .with_state(RoomServerContext {
            state: state.clone(),
            room_id: room.id.clone(),
        });

    let listener = TcpListener::bind(("0.0.0.0", 0))
        .await
        .map_err(|_| AppError::Network("Network connection lost.".into()))?;
    let port = listener
        .local_addr()
        .map_err(|_| AppError::Network("Network connection lost.".into()))?
        .port();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let server = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        });
        let _ = server.await;
    });

    state.active_servers.lock().insert(
        room_id.to_string(),
        ActiveRoomServer {
            room_id: room_id.to_string(),
            room_code_hash: room.room_code_hash,
            port,
            started_at: storage::now_ts(),
            expires_at: room.expires_at,
            transport_secret,
            shutdown: Some(shutdown_tx),
        },
    );

    if room.status != RoomStatus::Burned {
        storage::set_room_status(&state.paths, room_id, RoomStatus::Active)?;
    }

    discovery::ensure_service(state).await?;
    Ok(port)
}

pub async fn stop_room_server(state: Arc<AppState>, room_id: &str) -> AppResult<bool> {
    let _ = cancel_room_transfers(
        state.clone(),
        room_id,
        TRANSFER_INTERRUPTED_MESSAGE,
        false,
        Some("peer_disconnected"),
    )
    .await;
    let maybe_server = state.active_servers.lock().remove(room_id);
    if let Some(mut server) = maybe_server {
        if let Some(shutdown) = server.shutdown.take() {
            let _ = shutdown.send(());
        }
        discovery::maybe_stop_service(state).await;
        return Ok(true);
    }

    Ok(false)
}

pub async fn announce_join(
    state: Arc<AppState>,
    room_id: &str,
    peer_host: &str,
    peer_port: u16,
) -> AppResult<JoinRoomResponse> {
    let snapshot = room_server_snapshot(&state, room_id)?;
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{peer_host}:{peer_port}/rooms/{room_id}/join"
        ))
        .json(&JoinRoomRequest {
            port: snapshot.port,
            device_name: device_name(),
            transport_public_key: snapshot.transport_public_key,
        })
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(AppError::Network(PEER_DISCONNECTED_MESSAGE.into()));
    }

    response.json().await.map_err(Into::into)
}

pub async fn send_room_item(state: Arc<AppState>, room_id: &str, item_id: &str) -> AppResult<()> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    if room.status == RoomStatus::Burned {
        return Err(AppError::InvalidInput(ROOM_BURNED_MESSAGE.into()));
    }
    let peer_host = room
        .peer_host
        .clone()
        .ok_or_else(|| AppError::InvalidInput(PEER_DISCONNECTED_MESSAGE.into()))?;
    let peer_port = room
        .peer_port
        .ok_or_else(|| AppError::InvalidInput(PEER_DISCONNECTED_MESSAGE.into()))?;
    let peer_transport_public_key = room
        .peer_transport_public_key
        .clone()
        .ok_or_else(|| AppError::InvalidInput(PEER_DISCONNECTED_MESSAGE.into()))?;

    let item = storage::get_room_item_by_id(&state.paths, item_id)?;
    let master_key = {
        let config = state.config.read();
        config::master_key(&config)?
    };
    let payload_key = storage::read_room_item_key(&item, &master_key)?;
    let snapshot = room_server_snapshot(&state, room_id)?;
    let receiver_public_key = crypto::decode_key(&peer_transport_public_key)?;
    let (wrapped_session_key, transport_nonce, sender_public_key) =
        crypto::wrap_session_for_receiver(
            &payload_key,
            &snapshot.transport_secret,
            &receiver_public_key,
        )?;
    let encrypted_payload = tokio::fs::read(storage::encrypted_file_path(
        &state.paths,
        &item.encrypted_path,
    ))
    .await
    .map_err(map_missing_payload_error)?;

    let upload = RoomItemUpload {
        item_id: item.id.clone(),
        payload_type: item.payload_type,
        display_name: item.display_name,
        mime_type: item.mime_type,
        size_bytes: item.size_bytes,
        created_at: item.created_at,
        payload_nonce: item.nonce,
        wrapped_session_key,
        transport_nonce,
        sender_public_key,
        encrypted_payload: STANDARD.encode(encrypted_payload),
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| AppError::Network("Network connection lost.".into()))?;
    let response = client
        .post(format!(
            "http://{peer_host}:{peer_port}/rooms/{room_id}/items"
        ))
        .json(&upload)
        .send()
        .await;

    match response {
        Ok(response) if response.status().is_success() => {
            storage::set_room_item_status(&state.paths, item_id, RoomItemStatus::Sent)?;
            Ok(())
        }
        Ok(response) => {
            storage::set_room_item_status(&state.paths, item_id, RoomItemStatus::Failed)?;
            Err(AppError::Network(response_error_message(response).await))
        }
        Err(error) => {
            storage::set_room_item_status(&state.paths, item_id, RoomItemStatus::Failed)?;
            Err(AppError::Http(error))
        }
    }
}

pub async fn send_room_file(
    state: Arc<AppState>,
    room_id: &str,
    item_id: &str,
    file_path: &Path,
) -> AppResult<()> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    if room.status == RoomStatus::Burned {
        return Err(AppError::InvalidInput(ROOM_BURNED_MESSAGE.into()));
    }
    let peer_host = room
        .peer_host
        .clone()
        .ok_or_else(|| AppError::InvalidInput(PEER_DISCONNECTED_MESSAGE.into()))?;
    let peer_port = room
        .peer_port
        .ok_or_else(|| AppError::InvalidInput(PEER_DISCONNECTED_MESSAGE.into()))?;
    let peer_transport_public_key = room
        .peer_transport_public_key
        .clone()
        .ok_or_else(|| AppError::InvalidInput(PEER_DISCONNECTED_MESSAGE.into()))?;

    let metadata = tokio::fs::metadata(file_path)
        .await
        .map_err(|_| AppError::InvalidInput("Could not read selected file.".into()))?;
    storage::validate_file_size(metadata.len())?;

    let item = storage::get_room_item_by_id(&state.paths, item_id)?;
    let master_key = {
        let config = state.config.read();
        config::master_key(&config)?
    };
    let payload_key = storage::read_room_item_key(&item, &master_key)?;
    let snapshot = room_server_snapshot(&state, room_id)?;
    let receiver_public_key = crypto::decode_key(&peer_transport_public_key)?;
    let (wrapped_session_key, transport_nonce, sender_public_key) =
        crypto::wrap_session_for_receiver(
            &payload_key,
            &snapshot.transport_secret,
            &receiver_public_key,
        )?;

    let transfer_id = item.id.clone();
    let file_size = item.size_bytes;
    if metadata.len() != file_size {
        return Err(AppError::InvalidInput(
            "File changed after it was selected. Choose it again to transfer.".into(),
        ));
    }
    let chunk_size = DEFAULT_CHUNK_SIZE_BYTES as usize;
    let total_chunks = chunk_count(file_size, chunk_size);
    let file_name = item
        .display_name
        .clone()
        .unwrap_or_else(|| "pastey_file".to_string());
    let cancel_token = CancellationToken::new();
    register_sender_transfer(
        &state,
        &transfer_id,
        room_id,
        item_id,
        &file_name,
        file_size,
        chunk_size as u64,
        total_chunks,
        cancel_token.clone(),
    )?;
    emit_progress(
        &state,
        &transfer_id,
        "outgoing",
        "pending",
        0,
        0.0,
        0.0,
        None,
        None,
    );

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(TRANSFER_REQUEST_TIMEOUT)
        .build()
        .map_err(|_| AppError::Network("Network connection lost.".into()))?;
    let base_url = format!("http://{peer_host}:{peer_port}/rooms/{room_id}");
    let start_url = format!("{base_url}/transfers/start");
    let chunk_url = format!("{base_url}/transfers/{transfer_id}/chunks");
    dev_log_sender_transfer_start(
        &transfer_id,
        room_id,
        &base_url,
        &start_url,
        &chunk_url,
        chunk_size as u64,
        total_chunks,
        file_size,
    );
    let start = FileTransferStartRequest {
        transfer_id: transfer_id.clone(),
        item_id: item.id.clone(),
        display_name: item.display_name.clone(),
        mime_type: item.mime_type.clone(),
        size_bytes: file_size,
        chunk_size: chunk_size as u64,
        total_chunks,
        created_at: item.created_at,
        wrapped_session_key,
        transport_nonce,
        sender_public_key,
        preferred_chunk_protocol: Some(CHUNK_PROTOCOL_BINARY_V1.to_string()),
    };

    let start_response = client.post(&start_url).json(&start).send().await;
    let chunk_protocol = match start_response {
        Ok(response) if response.status().is_success() => {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            dev_log_sender_transfer_start_response(&transfer_id, room_id, status, &body_text);
            let protocol = selected_chunk_protocol_from_start_response(&body_text);
            dev_log_sender_chunk_protocol_selected(&transfer_id, room_id, protocol, &body_text);
            if protocol == ChunkProtocol::JsonV1 {
                dev_log_sender_binary_fallback_to_json(
                    &transfer_id,
                    room_id,
                    None,
                    "receiver_capability_unknown",
                );
            }
            protocol
        }
        Ok(response) => {
            let details = response_error_details(response).await;
            dev_log_sender_transfer_start_response(
                &transfer_id,
                room_id,
                details.status,
                &details.body_text,
            );
            let message = map_response_error_message(&details);
            let status = if details.code.as_deref() == Some("room_burned") {
                "burned"
            } else if details.status == StatusCode::GONE
                || matches!(
                    details.code.as_deref(),
                    Some("room_not_found") | Some("transfer_missing")
                )
            {
                "interrupted"
            } else {
                "failed"
            };
            if status == "failed" {
                fail_transfer(&state, &transfer_id, item_id, message.clone());
            } else {
                finish_sender_terminal(&state, &transfer_id, item_id, status, &message);
            }
            return Err(AppError::Network(message));
        }
        Err(error) => {
            let message = map_reqwest_transfer_message(&error);
            dev_log_sender_final_error(
                &transfer_id,
                room_id,
                None,
                "start_failed",
                &error.to_string(),
            );
            finish_sender_terminal(&state, &transfer_id, item_id, "interrupted", &message);
            return Err(AppError::Http(error));
        }
    };

    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|_| AppError::InvalidInput("Could not read selected file.".into()))?;
    let mut buffer = vec![0u8; chunk_size];
    let started_at = Instant::now();
    let mut last_report_at = started_at;
    let mut last_report_bytes = 0u64;
    let mut chunk_index = 0u64;
    let mut chunk_protocol = chunk_protocol;
    let transfer_tuning = current_transfer_tuning(&state);
    dev_log_sender_transfer_tuning(
        &transfer_id,
        room_id,
        transfer_tuning,
        chunk_size,
        chunk_protocol,
    );

    if chunk_protocol == ChunkProtocol::BinaryV1 {
        chunk_index = match send_binary_chunks_pipelined(
            &state,
            &client,
            &base_url,
            room_id,
            &transfer_id,
            &mut file,
            &mut buffer,
            chunk_size,
            total_chunks,
            file_size,
            &payload_key,
            &cancel_token,
            transfer_tuning,
        )
        .await
        {
            Ok(sent_chunks) => sent_chunks,
            Err(error) => {
                return Err(finish_sender_after_chunk_error(
                    &state,
                    &client,
                    &base_url,
                    room_id,
                    item_id,
                    &transfer_id,
                    None,
                    error,
                    &cancel_token,
                )
                .await);
            }
        };
    } else {
        loop {
            if cancel_token.is_cancelled() {
                notify_transfer_cancel(&client, &base_url, &transfer_id).await;
                storage::set_room_item_status(&state.paths, item_id, RoomItemStatus::Cancelled)?;
                finish_transfer_locally(
                    &state,
                    &transfer_id,
                    "cancelled",
                    Some(TRANSFER_CANCELLED_MESSAGE.into()),
                );
                return Err(AppError::InvalidInput(TRANSFER_CANCELLED_MESSAGE.into()));
            }

            if let Some((status, message)) = sender_room_terminal_state(&state, room_id) {
                notify_transfer_failed(&client, &base_url, &transfer_id, &message).await;
                finish_sender_terminal(&state, &transfer_id, item_id, status, &message);
                return Err(AppError::Network(message));
            }

            let bytes_read = read_next_chunk(&mut file, &mut buffer, chunk_size)
                .await
                .map_err(|_| AppError::InvalidInput("Could not read selected file.".into()))?;
            if bytes_read == 0 {
                break;
            }

            let is_final = chunk_index.checked_add(1) == Some(total_chunks);
            if bytes_read != chunk_size && !is_final {
                let message = "Internal chunk size mismatch".to_string();
                notify_transfer_failed(&client, &base_url, &transfer_id, &message).await;
                dev_log_sender_final_error(
                    &transfer_id,
                    room_id,
                    Some(chunk_index),
                    "internal_chunk_size_mismatch",
                    &format!(
                    "actual_plaintext_size={bytes_read} expected_non_final_chunk_size={chunk_size}"
                ),
                );
                fail_transfer(&state, &transfer_id, item_id, message.clone());
                return Err(AppError::Network(message));
            }

            let (encrypted_bytes, nonce) =
                crypto::encrypt_bytes(&buffer[..bytes_read], &payload_key)?;
            let ack = match send_chunk_with_retry(
                &client,
                &base_url,
                room_id,
                &transfer_id,
                chunk_index,
                total_chunks,
                bytes_read,
                &nonce,
                &encrypted_bytes,
                &cancel_token,
                chunk_protocol,
            )
            .await
            {
                Ok(ack) => ack,
                Err(error) => {
                    if chunk_protocol == ChunkProtocol::BinaryV1
                        && error.kind == ChunkSendFailureKind::UnsupportedProtocol
                    {
                        dev_log_sender_binary_fallback_to_json(
                            &transfer_id,
                            room_id,
                            Some(chunk_index),
                            &error.message,
                        );
                        chunk_protocol = ChunkProtocol::JsonV1;
                        match send_chunk_with_retry(
                            &client,
                            &base_url,
                            room_id,
                            &transfer_id,
                            chunk_index,
                            total_chunks,
                            bytes_read,
                            &nonce,
                            &encrypted_bytes,
                            &cancel_token,
                            chunk_protocol,
                        )
                        .await
                        {
                            Ok(ack) => ack,
                            Err(error) => {
                                if error.kind == ChunkSendFailureKind::Cancelled
                                    || cancel_token.is_cancelled()
                                {
                                    notify_transfer_cancel(&client, &base_url, &transfer_id).await;
                                    let _ = storage::set_room_item_status(
                                        &state.paths,
                                        item_id,
                                        RoomItemStatus::Cancelled,
                                    );
                                    finish_transfer_locally(
                                        &state,
                                        &transfer_id,
                                        "cancelled",
                                        Some(TRANSFER_CANCELLED_MESSAGE.into()),
                                    );
                                    return Err(AppError::InvalidInput(
                                        TRANSFER_CANCELLED_MESSAGE.into(),
                                    ));
                                }

                                notify_transfer_failed(
                                    &client,
                                    &base_url,
                                    &transfer_id,
                                    &error.message,
                                )
                                .await;
                                dev_log_sender_final_error(
                                    &transfer_id,
                                    room_id,
                                    Some(chunk_index),
                                    error.kind.as_str(),
                                    &error.message,
                                );
                                if matches!(
                                    error.kind,
                                    ChunkSendFailureKind::PeerLeft
                                        | ChunkSendFailureKind::Timeout
                                        | ChunkSendFailureKind::Unreachable
                                ) {
                                    finish_sender_terminal(
                                        &state,
                                        &transfer_id,
                                        item_id,
                                        "interrupted",
                                        &error.message,
                                    );
                                } else {
                                    fail_transfer(
                                        &state,
                                        &transfer_id,
                                        item_id,
                                        error.message.clone(),
                                    );
                                }
                                return if error.kind == ChunkSendFailureKind::Timeout {
                                    Err(AppError::Timeout(error.message))
                                } else {
                                    Err(AppError::Network(error.message))
                                };
                            }
                        }
                    } else {
                        if error.kind == ChunkSendFailureKind::Cancelled
                            || cancel_token.is_cancelled()
                        {
                            notify_transfer_cancel(&client, &base_url, &transfer_id).await;
                            let _ = storage::set_room_item_status(
                                &state.paths,
                                item_id,
                                RoomItemStatus::Cancelled,
                            );
                            finish_transfer_locally(
                                &state,
                                &transfer_id,
                                "cancelled",
                                Some(TRANSFER_CANCELLED_MESSAGE.into()),
                            );
                            return Err(AppError::InvalidInput(TRANSFER_CANCELLED_MESSAGE.into()));
                        }

                        notify_transfer_failed(&client, &base_url, &transfer_id, &error.message)
                            .await;
                        dev_log_sender_final_error(
                            &transfer_id,
                            room_id,
                            Some(chunk_index),
                            error.kind.as_str(),
                            &error.message,
                        );
                        if matches!(
                            error.kind,
                            ChunkSendFailureKind::PeerLeft
                                | ChunkSendFailureKind::Timeout
                                | ChunkSendFailureKind::Unreachable
                        ) {
                            finish_sender_terminal(
                                &state,
                                &transfer_id,
                                item_id,
                                "interrupted",
                                &error.message,
                            );
                        } else {
                            fail_transfer(&state, &transfer_id, item_id, error.message.clone());
                        }
                        return if error.kind == ChunkSendFailureKind::Timeout {
                            Err(AppError::Timeout(error.message))
                        } else {
                            Err(AppError::Network(error.message))
                        };
                    }
                }
            };

            chunk_index += 1;
            let transferred = ack.total_received_bytes;
            let now = Instant::now();
            let interval = now.duration_since(last_report_at).as_secs_f64().max(0.001);
            let current_speed = (transferred - last_report_bytes) as f64 / interval;
            let average_speed =
                transferred as f64 / now.duration_since(started_at).as_secs_f64().max(0.001);
            let eta = eta_seconds(file_size, transferred, current_speed);
            update_sender_transfer_report(&state, &transfer_id, transferred, now);
            emit_progress(
                &state,
                &transfer_id,
                "outgoing",
                "transferring",
                transferred,
                current_speed,
                average_speed,
                eta,
                None,
            );
            last_report_at = now;
            last_report_bytes = transferred;
        }
    }

    if chunk_index != total_chunks {
        let message = "Transfer metadata mismatch".to_string();
        notify_transfer_failed(&client, &base_url, &transfer_id, &message).await;
        dev_log_sender_final_error(
            &transfer_id,
            room_id,
            None,
            "metadata_mismatch",
            &format!("sent_chunks={chunk_index} total_chunks={total_chunks}"),
        );
        fail_transfer(&state, &transfer_id, item_id, message.clone());
        return Err(AppError::Network(message));
    }

    let finish_response = client
        .post(format!("{base_url}/transfers/{transfer_id}/finish"))
        .json(&FileTransferFinishRequest {
            item_id: item.id.clone(),
        })
        .send()
        .await;
    match finish_response {
        Ok(response) if response.status().is_success() => {
            storage::set_room_item_status(&state.paths, item_id, RoomItemStatus::Sent)?;
            finish_transfer_locally(&state, &transfer_id, "completed", None);
            Ok(())
        }
        Ok(response) => {
            let details = response_error_details(response).await;
            let message = map_response_error_message(&details);
            if matches!(
                details.code.as_deref(),
                Some("room_burned") | Some("room_not_found") | Some("transfer_missing")
            ) || details.status == StatusCode::GONE
            {
                let status = if details.code.as_deref() == Some("room_burned") {
                    "burned"
                } else {
                    "interrupted"
                };
                finish_sender_terminal(&state, &transfer_id, item_id, status, &message);
            } else {
                fail_transfer(&state, &transfer_id, item_id, message.clone());
            }
            Err(AppError::Network(message))
        }
        Err(error) => {
            dev_log_sender_final_error(
                &transfer_id,
                room_id,
                None,
                "finish_failed",
                &error.to_string(),
            );
            let message = map_reqwest_transfer_message(&error);
            finish_sender_terminal(&state, &transfer_id, item_id, "interrupted", &message);
            Err(AppError::Http(error))
        }
    }
}

fn update_sender_transfer_report(
    state: &Arc<AppState>,
    transfer_id: &str,
    transferred: u64,
    reported_at: Instant,
) {
    if let Some(transfer) = state.active_file_transfers.lock().get_mut(transfer_id) {
        transfer.last_report_bytes = transferred;
        transfer.last_report_at = reported_at;
    }
}

fn current_transfer_tuning(state: &Arc<AppState>) -> TransferTuning {
    let dev_window_override = {
        let config = state.config.read();
        config.transfer_window_override
    };
    transfer_tuning::effective_transfer_tuning_from_env(
        dev_window_override,
        crate::dev_tools::is_dev_tools_enabled(),
    )
}

async fn finish_sender_after_chunk_error(
    state: &Arc<AppState>,
    client: &reqwest::Client,
    base_url: &str,
    room_id: &str,
    item_id: &str,
    transfer_id: &str,
    chunk_index: Option<u64>,
    error: ChunkSendFailure,
    cancel_token: &CancellationToken,
) -> AppError {
    if error.kind == ChunkSendFailureKind::Cancelled || cancel_token.is_cancelled() {
        notify_transfer_cancel(client, base_url, transfer_id).await;
        let _ = storage::set_room_item_status(&state.paths, item_id, RoomItemStatus::Cancelled);
        finish_transfer_locally(
            state,
            transfer_id,
            "cancelled",
            Some(TRANSFER_CANCELLED_MESSAGE.into()),
        );
        return AppError::InvalidInput(TRANSFER_CANCELLED_MESSAGE.into());
    }

    notify_transfer_failed(client, base_url, transfer_id, &error.message).await;
    dev_log_sender_final_error(
        transfer_id,
        room_id,
        chunk_index,
        error.kind.as_str(),
        &error.message,
    );
    if matches!(
        error.kind,
        ChunkSendFailureKind::PeerLeft
            | ChunkSendFailureKind::Timeout
            | ChunkSendFailureKind::Unreachable
    ) {
        finish_sender_terminal(state, transfer_id, item_id, "interrupted", &error.message);
    } else {
        fail_transfer(state, transfer_id, item_id, error.message.clone());
    }

    if error.kind == ChunkSendFailureKind::Timeout {
        AppError::Timeout(error.message)
    } else {
        AppError::Network(error.message)
    }
}

async fn send_binary_chunks_pipelined<R>(
    state: &Arc<AppState>,
    client: &reqwest::Client,
    base_url: &str,
    room_id: &str,
    transfer_id: &str,
    file: &mut R,
    buffer: &mut Vec<u8>,
    chunk_size: usize,
    total_chunks: u64,
    file_size: u64,
    payload_key: &[u8; 32],
    cancel_token: &CancellationToken,
    tuning: TransferTuning,
) -> Result<u64, ChunkSendFailure>
where
    R: AsyncRead + Unpin,
{
    let started_at = Instant::now();
    let mut next_chunk_index = 0u64;
    let mut eof = false;
    let mut max_acknowledged_bytes = 0u64;
    let mut last_report_at = started_at;
    let mut last_report_bytes = 0u64;
    let mut in_flight = FuturesUnordered::new();
    let mut timing_summary = SenderTimingSummary::default();

    while !eof || !in_flight.is_empty() {
        while !eof && in_flight.len() < tuning.effective_window_size {
            if cancel_token.is_cancelled() {
                return Err(ChunkSendFailure {
                    message: TRANSFER_CANCELLED_MESSAGE.into(),
                    kind: ChunkSendFailureKind::Cancelled,
                    retryable: false,
                });
            }
            if let Some((_status, message)) = sender_room_terminal_state(state, room_id) {
                return Err(ChunkSendFailure {
                    message,
                    kind: ChunkSendFailureKind::PeerLeft,
                    retryable: false,
                });
            }

            let read_started = Instant::now();
            let bytes_read = read_next_chunk(file, buffer, chunk_size)
                .await
                .map_err(|_| ChunkSendFailure {
                    message: "Could not read selected file.".into(),
                    kind: ChunkSendFailureKind::HttpStatus,
                    retryable: false,
                })?;
            let read_elapsed = read_started.elapsed();
            if bytes_read == 0 {
                eof = true;
                break;
            }

            let chunk_index = next_chunk_index;
            let is_final = chunk_index.checked_add(1) == Some(total_chunks);
            if bytes_read != chunk_size && !is_final {
                return Err(ChunkSendFailure {
                    message: "Internal chunk size mismatch".into(),
                    kind: ChunkSendFailureKind::HttpStatus,
                    retryable: false,
                });
            }

            let encrypt_started = Instant::now();
            let (encrypted_bytes, nonce) =
                crypto::encrypt_bytes(&buffer[..bytes_read], payload_key).map_err(|_| {
                    ChunkSendFailure {
                        message: "Invalid chunk payload".into(),
                        kind: ChunkSendFailureKind::HttpStatus,
                        retryable: false,
                    }
                })?;
            let encrypt_elapsed = encrypt_started.elapsed();

            let client = client.clone();
            let base_url = base_url.to_string();
            let room_id = room_id.to_string();
            let transfer_id = transfer_id.to_string();
            let cancel_token = cancel_token.clone();
            in_flight.push(async move {
                let send_started = Instant::now();
                let result = send_chunk_with_retry(
                    &client,
                    &base_url,
                    &room_id,
                    &transfer_id,
                    chunk_index,
                    total_chunks,
                    bytes_read,
                    &nonce,
                    &encrypted_bytes,
                    &cancel_token,
                    ChunkProtocol::BinaryV1,
                )
                .await;
                (
                    chunk_index,
                    bytes_read,
                    read_elapsed,
                    encrypt_elapsed,
                    send_started.elapsed(),
                    result,
                )
            });
            next_chunk_index += 1;
        }

        let Some((
            chunk_index,
            plaintext_size,
            read_elapsed,
            encrypt_elapsed,
            send_elapsed,
            result,
        )) = in_flight.next().await
        else {
            continue;
        };
        let ack = result?;
        timing_summary.chunks += 1;
        timing_summary.read_ms += read_elapsed.as_millis();
        timing_summary.encrypt_ms += encrypt_elapsed.as_millis();
        timing_summary.request_ms += send_elapsed.as_millis();
        max_acknowledged_bytes = max_acknowledged_bytes.max(ack.total_received_bytes);
        let now = Instant::now();
        let should_emit_progress = now.duration_since(last_report_at) >= PROGRESS_EMIT_INTERVAL
            || max_acknowledged_bytes >= file_size;
        update_sender_transfer_report(state, transfer_id, max_acknowledged_bytes, now);
        if should_emit_progress {
            let interval = now.duration_since(last_report_at).as_secs_f64().max(0.001);
            let current_speed = (max_acknowledged_bytes - last_report_bytes) as f64 / interval;
            let average_speed = max_acknowledged_bytes as f64
                / now.duration_since(started_at).as_secs_f64().max(0.001);
            emit_progress(
                state,
                transfer_id,
                "outgoing",
                "transferring",
                max_acknowledged_bytes,
                current_speed,
                average_speed,
                eta_seconds(file_size, max_acknowledged_bytes, current_speed),
                None,
            );
            last_report_at = now;
            last_report_bytes = max_acknowledged_bytes;
        }
        dev_log_sender_chunk_timing(
            transfer_id,
            room_id,
            chunk_index,
            plaintext_size,
            read_elapsed,
            encrypt_elapsed,
            send_elapsed,
        );
    }

    dev_log_sender_transfer_summary(
        transfer_id,
        room_id,
        tuning,
        chunk_size,
        file_size,
        started_at.elapsed(),
        timing_summary,
    );
    Ok(next_chunk_index)
}

async fn read_next_chunk<R>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
    chunk_size: usize,
) -> std::io::Result<usize>
where
    R: AsyncRead + Unpin,
{
    if chunk_size == 0 {
        buffer.clear();
        return Ok(0);
    }

    if buffer.len() != chunk_size {
        buffer.resize(chunk_size, 0);
    }

    let mut bytes_read = 0;
    while bytes_read < chunk_size {
        let read = reader.read(&mut buffer[bytes_read..chunk_size]).await?;
        if read == 0 {
            break;
        }
        bytes_read += read;
    }

    Ok(bytes_read)
}

async fn send_chunk_with_retry(
    client: &reqwest::Client,
    base_url: &str,
    room_id: &str,
    transfer_id: &str,
    chunk_index: u64,
    total_chunks: u64,
    plaintext_size: usize,
    nonce: &[u8; 12],
    encrypted_bytes: &[u8],
    cancel_token: &CancellationToken,
    protocol: ChunkProtocol,
) -> Result<ChunkAckResponse, ChunkSendFailure> {
    for retry_count in 0..=CHUNK_RETRY_BACKOFFS.len() {
        if cancel_token.is_cancelled() {
            return Err(ChunkSendFailure {
                message: TRANSFER_CANCELLED_MESSAGE.into(),
                kind: ChunkSendFailureKind::Cancelled,
                retryable: false,
            });
        }

        let attempt_started_at = Instant::now();
        let result = send_chunk_once(
            client,
            base_url,
            room_id,
            transfer_id,
            chunk_index,
            total_chunks,
            plaintext_size,
            nonce,
            encrypted_bytes,
            protocol,
        )
        .await;
        let elapsed = attempt_started_at.elapsed();

        match result {
            Ok(ack) => {
                dev_log_sender_chunk_attempt(
                    transfer_id,
                    room_id,
                    chunk_index,
                    retry_count,
                    "ok",
                    elapsed,
                );
                return Ok(ack);
            }
            Err(error) => {
                dev_log_sender_chunk_attempt(
                    transfer_id,
                    room_id,
                    chunk_index,
                    retry_count,
                    error.kind.as_str(),
                    elapsed,
                );
                if cancel_token.is_cancelled() {
                    return Err(ChunkSendFailure {
                        message: TRANSFER_CANCELLED_MESSAGE.into(),
                        kind: ChunkSendFailureKind::Cancelled,
                        retryable: false,
                    });
                }
                if !error.retryable || retry_count == CHUNK_RETRY_BACKOFFS.len() {
                    return Err(error);
                }
                tokio::time::sleep(CHUNK_RETRY_BACKOFFS[retry_count]).await;
            }
        }
    }

    Err(ChunkSendFailure {
        message: TRANSFER_INTERRUPTED_MESSAGE.into(),
        kind: ChunkSendFailureKind::Timeout,
        retryable: false,
    })
}

async fn send_chunk_once(
    client: &reqwest::Client,
    base_url: &str,
    room_id: &str,
    transfer_id: &str,
    chunk_index: u64,
    total_chunks: u64,
    plaintext_size: usize,
    nonce: &[u8; 12],
    encrypted_bytes: &[u8],
    protocol: ChunkProtocol,
) -> Result<ChunkAckResponse, ChunkSendFailure> {
    match protocol {
        ChunkProtocol::BinaryV1 => {
            send_binary_chunk_once(
                client,
                base_url,
                room_id,
                transfer_id,
                chunk_index,
                total_chunks,
                plaintext_size,
                nonce,
                encrypted_bytes,
            )
            .await
        }
        ChunkProtocol::JsonV1 => {
            send_json_chunk_once(
                client,
                base_url,
                room_id,
                transfer_id,
                chunk_index,
                total_chunks,
                plaintext_size,
                nonce,
                encrypted_bytes,
            )
            .await
        }
    }
}

async fn send_binary_chunk_once(
    client: &reqwest::Client,
    base_url: &str,
    room_id: &str,
    transfer_id: &str,
    chunk_index: u64,
    total_chunks: u64,
    plaintext_size: usize,
    nonce: &[u8; 12],
    encrypted_bytes: &[u8],
) -> Result<ChunkAckResponse, ChunkSendFailure> {
    let plaintext_size_u32 = u32::try_from(plaintext_size).map_err(|_| ChunkSendFailure {
        message: "Invalid chunk payload".into(),
        kind: ChunkSendFailureKind::HttpStatus,
        retryable: false,
    })?;
    let frame = BinaryChunkFrame {
        chunk_index,
        nonce: *nonce,
        ciphertext: encrypted_bytes.to_vec(),
        plaintext_size: plaintext_size_u32,
        is_final: chunk_index.checked_add(1) == Some(total_chunks),
    };
    let request_body = encode_binary_chunk_frame(&frame).map_err(|_| ChunkSendFailure {
        message: "Invalid chunk payload".into(),
        kind: ChunkSendFailureKind::HttpStatus,
        retryable: false,
    })?;
    let request_body_size = request_body.len();
    let json_estimated_len = json_base64_estimated_chunk_len(
        chunk_index,
        plaintext_size,
        encrypted_bytes.len(),
        frame.is_final,
    );
    dev_log_sender_binary_chunk_encode(
        transfer_id,
        room_id,
        chunk_index,
        plaintext_size,
        encrypted_bytes.len(),
        request_body_size,
        json_estimated_len,
    );
    let chunk_url = format!("{base_url}/transfers/{transfer_id}/chunks");
    dev_log_sender_chunk_request(
        transfer_id,
        room_id,
        chunk_index,
        &chunk_url,
        "POST",
        plaintext_size,
        encrypted_bytes.len(),
        request_body_size,
        ChunkProtocol::BinaryV1,
    );
    let response = client
        .post(&chunk_url)
        .timeout(CHUNK_REQUEST_TIMEOUT)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(CHUNK_PROTOCOL_HEADER, CHUNK_PROTOCOL_BINARY_V1)
        .body(request_body)
        .send()
        .await
        .map_err(chunk_failure_from_reqwest)?;

    receive_chunk_ack_response(
        response,
        transfer_id,
        room_id,
        chunk_index,
        plaintext_size,
        encrypted_bytes.len(),
        request_body_size,
        ChunkProtocol::BinaryV1,
    )
    .await
}

async fn send_json_chunk_once(
    client: &reqwest::Client,
    base_url: &str,
    room_id: &str,
    transfer_id: &str,
    chunk_index: u64,
    total_chunks: u64,
    plaintext_size: usize,
    nonce: &[u8; 12],
    encrypted_bytes: &[u8],
) -> Result<ChunkAckResponse, ChunkSendFailure> {
    let ciphertext = STANDARD.encode(encrypted_bytes);
    let upload = ChunkUploadRequest {
        chunk_index,
        nonce: crypto::encode_nonce(nonce),
        ciphertext,
        plaintext_size: plaintext_size as u64,
        is_final: chunk_index.checked_add(1) == Some(total_chunks),
    };
    let request_body = serde_json::to_vec(&upload).map_err(|_| ChunkSendFailure {
        message: "Invalid chunk payload".into(),
        kind: ChunkSendFailureKind::HttpStatus,
        retryable: false,
    })?;
    let request_body_size = request_body.len();
    let chunk_url = format!("{base_url}/transfers/{transfer_id}/chunks");
    dev_log_sender_chunk_request(
        transfer_id,
        room_id,
        chunk_index,
        &chunk_url,
        "POST",
        plaintext_size,
        encrypted_bytes.len(),
        request_body_size,
        ChunkProtocol::JsonV1,
    );
    let response = client
        .post(&chunk_url)
        .timeout(CHUNK_REQUEST_TIMEOUT)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(request_body)
        .send()
        .await
        .map_err(chunk_failure_from_reqwest)?;

    receive_chunk_ack_response(
        response,
        transfer_id,
        room_id,
        chunk_index,
        plaintext_size,
        encrypted_bytes.len(),
        request_body_size,
        ChunkProtocol::JsonV1,
    )
    .await
}

async fn receive_chunk_ack_response(
    response: reqwest::Response,
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_size: usize,
    ciphertext_bytes: usize,
    request_body_size: usize,
    protocol: ChunkProtocol,
) -> Result<ChunkAckResponse, ChunkSendFailure> {
    let status = response.status();
    if !response.status().is_success() {
        return Err(chunk_failure_from_response(
            response,
            transfer_id,
            room_id,
            chunk_index,
            plaintext_size,
            ciphertext_bytes,
            request_body_size,
            protocol,
        )
        .await);
    }

    let body_text = response.text().await.map_err(chunk_failure_from_reqwest)?;
    dev_log_sender_chunk_response(
        transfer_id,
        room_id,
        chunk_index,
        plaintext_size,
        ciphertext_bytes,
        request_body_size,
        protocol,
        status,
        &body_text,
    );

    let ack =
        serde_json::from_str::<ChunkAckResponse>(&body_text).map_err(|_| ChunkSendFailure {
            message: "Receiver returned an invalid chunk ack.".into(),
            kind: ChunkSendFailureKind::InvalidAck,
            retryable: true,
        })?;
    if !ack.ok || ack.chunk_index != chunk_index || ack.written_bytes != plaintext_size as u64 {
        return Err(ChunkSendFailure {
            message: "Receiver returned an invalid chunk ack.".into(),
            kind: ChunkSendFailureKind::InvalidAck,
            retryable: true,
        });
    }

    Ok(ack)
}

fn chunk_failure_from_reqwest(error: reqwest::Error) -> ChunkSendFailure {
    if error.is_timeout() {
        return ChunkSendFailure {
            message: TRANSFER_INTERRUPTED_MESSAGE.into(),
            kind: ChunkSendFailureKind::Timeout,
            retryable: true,
        };
    }
    if error.is_connect() {
        return ChunkSendFailure {
            message: PEER_DISCONNECTED_MESSAGE.into(),
            kind: ChunkSendFailureKind::Unreachable,
            retryable: true,
        };
    }
    ChunkSendFailure {
        message: TRANSFER_INTERRUPTED_MESSAGE.into(),
        kind: ChunkSendFailureKind::Unreachable,
        retryable: true,
    }
}

async fn chunk_failure_from_response(
    response: reqwest::Response,
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_size: usize,
    ciphertext_bytes: usize,
    request_body_size: usize,
    protocol: ChunkProtocol,
) -> ChunkSendFailure {
    let details = response_error_details(response).await;
    dev_log_sender_chunk_response(
        transfer_id,
        room_id,
        chunk_index,
        plaintext_size,
        ciphertext_bytes,
        request_body_size,
        protocol,
        details.status,
        &details.body_text,
    );
    if details.status == StatusCode::UNSUPPORTED_MEDIA_TYPE
        || details.code.as_deref() == Some("unsupported_chunk_protocol")
    {
        return ChunkSendFailure {
            message: "Unsupported chunk protocol".into(),
            kind: ChunkSendFailureKind::UnsupportedProtocol,
            retryable: false,
        };
    }
    if details.status == StatusCode::PAYLOAD_TOO_LARGE
        || details.code.as_deref() == Some("chunk_too_large")
    {
        return ChunkSendFailure {
            message: "Chunk too large for receiver".into(),
            kind: ChunkSendFailureKind::ChunkTooLarge,
            retryable: false,
        };
    }
    if details.code.as_deref() == Some("room_not_found") {
        return ChunkSendFailure {
            message: PEER_DISCONNECTED_MESSAGE.into(),
            kind: ChunkSendFailureKind::PeerLeft,
            retryable: false,
        };
    }
    if details.code.as_deref() == Some("transfer_missing") {
        return ChunkSendFailure {
            message: "Transfer session not found on receiver.".into(),
            kind: ChunkSendFailureKind::HttpStatus,
            retryable: false,
        };
    }
    if details
        .code
        .as_deref()
        .is_some_and(is_receiver_terminal_reason)
    {
        let reason = details.code.as_deref().unwrap_or_default();
        logging::write_transfer_line(&format!(
            "[pastey transfer][transfer_id={transfer_id}] event=sender_mapped_terminal_reason reason={reason}"
        ));
        return ChunkSendFailure {
            message: map_response_error_message(&details),
            kind: ChunkSendFailureKind::PeerLeft,
            retryable: false,
        };
    }
    if details.status == StatusCode::GONE {
        return ChunkSendFailure {
            message: map_response_error_message(&details),
            kind: ChunkSendFailureKind::PeerLeft,
            retryable: false,
        };
    }

    if details.code.as_deref() == Some("cancelled") {
        return ChunkSendFailure {
            message: TRANSFER_CANCELLED_MESSAGE.into(),
            kind: ChunkSendFailureKind::Cancelled,
            retryable: false,
        };
    }

    if details.code.as_deref().is_some_and(|code| {
        matches!(
            code,
            "integrity_failed"
                | "invalid_chunk"
                | "invalid_chunk_encoding"
                | "invalid_chunk_order"
                | "invalid_chunk_payload"
                | "invalid_payload"
                | "invalid_transfer"
                | "metadata_mismatch"
                | "not_enough_disk_space"
                | "receiver_cannot_write"
                | "size_mismatch"
                | "temp_file_disappeared"
                | "write_failed"
        )
    }) {
        return ChunkSendFailure {
            message: map_response_error_message(&details),
            kind: ChunkSendFailureKind::HttpStatus,
            retryable: false,
        };
    }

    ChunkSendFailure {
        message: map_response_error_message(&details),
        kind: ChunkSendFailureKind::HttpStatus,
        retryable: details.status.is_server_error()
            || details.status == StatusCode::REQUEST_TIMEOUT,
    }
}

fn emit_transfer_log(line: String) {
    #[cfg(debug_assertions)]
    eprintln!("{line}");

    if is_transfer_error_log(&line) {
        logging::write_error_line(&line);
    } else {
        logging::write_transfer_line(&line);
    }
}

fn should_log_chunk_sample(chunk_index: u64) -> bool {
    chunk_index < 4 || chunk_index % 32 == 0
}

fn is_transfer_error_log(line: &str) -> bool {
    line.contains("event=final_error")
        || line.contains("event=start_failure")
        || line.contains("event=chunk_failure")
        || line.contains("event=finalize_failure")
}

fn receiver_failure_log_message(error_cause: &str) -> &'static str {
    if error_cause.contains("integrity_failed") || error_cause.contains("plaintext_size_mismatch") {
        "Chunk integrity check failed"
    } else if error_cause.contains("write_failed") {
        "Receiver failed to write chunk"
    } else if error_cause.contains("temp_file_disappeared") {
        "Receiver temporary file disappeared"
    } else if error_cause.contains("invalid_chunk_encoding")
        || error_cause.contains("invalid_nonce_encoding")
        || error_cause.contains("invalid_ciphertext_encoding")
    {
        "Invalid chunk encoding"
    } else if error_cause.contains("metadata_mismatch")
        || error_cause.contains("chunk_larger_than_metadata_chunk_size")
    {
        "Transfer metadata mismatch"
    } else if error_cause.contains("invalid_chunk_order") {
        "Unexpected chunk index"
    } else {
        "Invalid chunk payload"
    }
}

fn dev_log_sender_transfer_start(
    transfer_id: &str,
    room_id: &str,
    peer_url: &str,
    start_url: &str,
    chunk_url: &str,
    chunk_size: u64,
    total_chunks: u64,
    file_size: u64,
) {
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=start_request method=POST peer_url={peer_url} start_url={start_url} chunk_url={chunk_url} chunk_payload_format=binary-v1 chunk_size={chunk_size} total_chunks={total_chunks} file_size={file_size}"
    ));
}

fn dev_log_sender_transfer_start_response(
    transfer_id: &str,
    room_id: &str,
    status: StatusCode,
    body_text: &str,
) {
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=start_response response_status={status} response_body={body_text:?}"
    ));
}

fn dev_log_sender_chunk_protocol_selected(
    transfer_id: &str,
    room_id: &str,
    protocol: ChunkProtocol,
    start_response_body: &str,
) {
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=chunk_protocol_selected protocol={} start_response_body={start_response_body:?}",
        protocol.as_str()
    ));
}

fn sender_transfer_tuning_log_line(
    transfer_id: &str,
    room_id: &str,
    tuning: TransferTuning,
    chunk_size: usize,
    protocol: ChunkProtocol,
) -> String {
    format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=transfer_tuning effective_window_size={} chunk_size={} override_source={} transfer_protocol={}",
        tuning.effective_window_size,
        chunk_size,
        tuning.override_source.as_str(),
        protocol.log_label()
    )
}

fn dev_log_sender_transfer_tuning(
    transfer_id: &str,
    room_id: &str,
    tuning: TransferTuning,
    chunk_size: usize,
    protocol: ChunkProtocol,
) {
    emit_transfer_log(sender_transfer_tuning_log_line(
        transfer_id,
        room_id,
        tuning,
        chunk_size,
        protocol,
    ));
}

fn dev_log_sender_transfer_summary(
    transfer_id: &str,
    room_id: &str,
    tuning: TransferTuning,
    chunk_size: usize,
    total_bytes: u64,
    total_duration: Duration,
    timing: SenderTimingSummary,
) {
    let duration_secs = total_duration.as_secs_f64().max(0.001);
    let average_mb_per_sec = total_bytes as f64 / 1024.0 / 1024.0 / duration_secs;
    let chunks = timing.chunks.max(1) as u128;
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=transfer_benchmark_summary transfer_protocol=binary-v1 effective_window_size={} total_bytes={} duration_ms={} average_MBps={average_mb_per_sec:.2} chunk_size={} chunk_count={} sender_avg_read_ms={} sender_avg_encrypt_ms={} sender_avg_send_ack_ms={} receiver_avg_decode_ms=0 receiver_avg_decrypt_ms=0 receiver_avg_write_ms=0 failed_chunks=0 duplicate_chunks=0 finalize_status=chunks_acknowledged",
        tuning.effective_window_size,
        total_bytes,
        total_duration.as_millis(),
        chunk_size,
        timing.chunks,
        timing.read_ms / chunks,
        timing.encrypt_ms / chunks,
        timing.request_ms / chunks,
    ));
}

fn dev_log_sender_chunk_request(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    chunk_url: &str,
    method: &str,
    plaintext_bytes: usize,
    ciphertext_bytes: usize,
    encoded_payload_bytes: usize,
    protocol: ChunkProtocol,
) {
    if !should_log_chunk_sample(chunk_index) {
        return;
    }
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_request method={method} chunk_url={chunk_url} actual_plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_payload_bytes={encoded_payload_bytes} payload_format={}"
        , protocol.as_str()
    ));
}

fn dev_log_sender_chunk_response(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_bytes: usize,
    ciphertext_bytes: usize,
    encoded_payload_bytes: usize,
    protocol: ChunkProtocol,
    status: StatusCode,
    body_text: &str,
) {
    if !should_log_chunk_sample(chunk_index) && status.is_success() {
        return;
    }
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_response actual_plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_payload_bytes={encoded_payload_bytes} payload_format={} response_status={status} response_body={body_text:?}"
        , protocol.as_str()
    ));
}

fn dev_log_sender_binary_chunk_encode(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_size: usize,
    ciphertext_len: usize,
    frame_len: usize,
    json_base64_estimated_len: usize,
) {
    if !should_log_chunk_sample(chunk_index) {
        return;
    }
    let overhead_saved_estimate = json_base64_estimated_len.saturating_sub(frame_len);
    let overhead_ratio = frame_len as f64 / plaintext_size.max(1) as f64;
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=binary_chunk_encode plaintext_size={plaintext_size} ciphertext_len={ciphertext_len} frame_len={frame_len} json_base64_estimated_len={json_base64_estimated_len} overhead_saved_estimate={overhead_saved_estimate} overhead_ratio={overhead_ratio:.4}"
    ));
}

fn dev_log_sender_chunk_timing(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_size: usize,
    read_elapsed: Duration,
    encrypt_elapsed: Duration,
    send_elapsed: Duration,
) {
    if !should_log_chunk_sample(chunk_index) {
        return;
    }
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_timing protocol=binary-v1 plaintext_size={plaintext_size} read_ms={} encrypt_ms={} encode_ms=0 http_send_ms={} ack_wait_ms={} total_chunk_ms={}",
        read_elapsed.as_millis(),
        encrypt_elapsed.as_millis(),
        send_elapsed.as_millis(),
        send_elapsed.as_millis(),
        read_elapsed
            .saturating_add(encrypt_elapsed)
            .saturating_add(send_elapsed)
            .as_millis()
    ));
}

fn dev_log_sender_binary_fallback_to_json(
    transfer_id: &str,
    room_id: &str,
    chunk_index: Option<u64>,
    reason: &str,
) {
    match chunk_index {
        Some(chunk_index) => emit_transfer_log(format!(
            "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=binary_chunk_fallback_to_json reason={reason:?}"
        )),
        None => emit_transfer_log(format!(
            "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=binary_chunk_fallback_to_json reason={reason:?}"
        )),
    }
}

fn dev_log_sender_chunk_attempt(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    retry_count: usize,
    error_kind: &str,
    elapsed: Duration,
) {
    if !should_log_chunk_sample(chunk_index) && error_kind == "ok" {
        return;
    }
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_attempt retry_count={retry_count} result={error_kind} elapsed_ms={}",
        elapsed.as_millis()
    ));
}

fn dev_log_sender_final_error(
    transfer_id: &str,
    room_id: &str,
    chunk_index: Option<u64>,
    error_kind: &str,
    message: &str,
) {
    match chunk_index {
        Some(chunk_index) => emit_transfer_log(format!(
            "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=final_error error_kind={error_kind} message={message:?}"
        )),
        None => emit_transfer_log(format!(
            "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=final_error error_kind={error_kind} message={message:?}"
        )),
    }
}

fn dev_log_receiver_start_route_hit(
    transfer_id: &str,
    room_id: &str,
    chunk_size: u64,
    total_chunks: u64,
    file_size: u64,
) {
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}] event=start_route_hit chunk_size={chunk_size} total_chunks={total_chunks} file_size={file_size}"
    ));
}

fn dev_log_receiver_start_registered(
    transfer_id: &str,
    room_id: &str,
    chunk_size: u64,
    total_chunks: u64,
) {
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}] event=start_registered chunk_size={chunk_size} total_chunks={total_chunks}"
    ));
}

fn dev_log_receiver_start_failure(
    transfer_id: &str,
    room_id: &str,
    response_status: StatusCode,
    error_cause: &str,
) {
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}] event=start_failure response_status={response_status} error_cause={error_cause}"
    ));
}

fn dev_log_receiver_chunk_route_hit(transfer_id: &str, room_id: &str, chunk_index: Option<u64>) {
    match chunk_index {
        Some(chunk_index) => {
            if should_log_chunk_sample(chunk_index) {
                emit_transfer_log(format!(
                    "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_route_hit"
                ));
            }
        }
        None => emit_transfer_log(format!(
            "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk=unknown] event=chunk_route_hit"
        )),
    }
}

fn dev_log_receiver_chunk_received(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_bytes: u64,
    encoded_ciphertext_bytes: usize,
    encoded_payload_bytes: usize,
    protocol: ChunkProtocol,
) {
    if !should_log_chunk_sample(chunk_index) {
        return;
    }
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_received plaintext_size={plaintext_bytes} encoded_ciphertext_bytes={encoded_ciphertext_bytes} encoded_payload_bytes={encoded_payload_bytes} payload_format={}"
        , protocol.as_str()
    ));
}

fn dev_log_receiver_chunk_write_success(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_bytes: u64,
    ciphertext_bytes: usize,
    encoded_payload_bytes: usize,
    protocol: ChunkProtocol,
) {
    if !should_log_chunk_sample(chunk_index) {
        return;
    }
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_write plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_payload_bytes={encoded_payload_bytes} payload_format={} response_status={} result=success",
        protocol.as_str(),
        StatusCode::OK
    ));
}

fn dev_log_receiver_chunk_timing(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_size: u64,
    decode_elapsed: Duration,
    decrypt_elapsed: Duration,
    write_elapsed: Duration,
    ui_emit_elapsed: Duration,
    total_elapsed: Duration,
) {
    if !should_log_chunk_sample(chunk_index) {
        return;
    }
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_timing plaintext_size={plaintext_size} receiver_decode_ms={} decrypt_ms={} write_ms={} sqlite_ms=0 log_ms=0 ui_emit_ms={} total_chunk_ms={}",
        decode_elapsed.as_millis(),
        decrypt_elapsed.as_millis(),
        write_elapsed.as_millis(),
        ui_emit_elapsed.as_millis(),
        total_elapsed.as_millis()
    ));
}

fn dev_log_receiver_transfer_summary(
    transfer_id: &str,
    room_id: &str,
    chunk_size: u64,
    total_bytes: u64,
    received_bytes: u64,
    total_duration: Duration,
    timing: ReceiverTimingSummary,
    finalize_status: &str,
) {
    let duration_secs = total_duration.as_secs_f64().max(0.001);
    let average_mb_per_sec = received_bytes as f64 / 1024.0 / 1024.0 / duration_secs;
    let chunks = timing.chunks.max(1) as u128;
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}] event=transfer_benchmark_summary transfer_protocol=receiver effective_window_size=unknown total_bytes={total_bytes} received_bytes={received_bytes} duration_ms={} average_MBps={average_mb_per_sec:.2} chunk_size={chunk_size} chunk_count={} sender_avg_read_ms=0 sender_avg_encrypt_ms=0 sender_avg_send_ack_ms=0 receiver_avg_decode_ms={} receiver_avg_decrypt_ms={} receiver_avg_write_ms={} failed_chunks=0 duplicate_chunks={} finalize_status={finalize_status}",
        total_duration.as_millis(),
        timing.chunks,
        timing.decode_ms / chunks,
        timing.decrypt_ms / chunks,
        timing.write_ms / chunks,
        timing.duplicate_chunks,
    ));
}

fn dev_log_receiver_chunk_ack(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    written_bytes: u64,
    total_received_bytes: u64,
    result: &str,
) {
    if !should_log_chunk_sample(chunk_index) && result == "ok" {
        return;
    }
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_ack written_bytes={written_bytes} total_received_bytes={total_received_bytes} result={result}"
    ));
}

fn dev_log_receiver_chunk_failure(
    transfer_id: &str,
    room_id: &str,
    chunk_index: Option<u64>,
    plaintext_bytes: u64,
    ciphertext_bytes: usize,
    encoded_payload_bytes: usize,
    response_status: StatusCode,
    error_cause: &str,
) {
    match chunk_index {
        Some(chunk_index) => emit_transfer_log(format!(
            "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_failure plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_payload_bytes={encoded_payload_bytes} response_status={response_status} error_cause={error_cause} mapped_error_message={:?}",
            receiver_failure_log_message(error_cause)
        )),
        None => emit_transfer_log(format!(
            "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk=unknown] event=chunk_failure plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_payload_bytes={encoded_payload_bytes} response_status={response_status} error_cause={error_cause} mapped_error_message={:?}",
            receiver_failure_log_message(error_cause)
        )),
    }
}

fn dev_log_receiver_finalize(transfer_id: &str, room_id: &str, event: &str, details: &str) {
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}] event={event} {details}"
    ));
}

pub async fn cancel_transfer(state: Arc<AppState>, transfer_id: &str) -> AppResult<bool> {
    let removed = state.active_file_transfers.lock().remove(transfer_id);
    let Some(transfer) = removed else {
        return Ok(false);
    };

    transfer.cancel_token.cancel();
    if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
        record_terminal_transfer_reason(&state, transfer_id, "receiver_cancelled");
        logging::write_transfer_line(&format!(
            "[pastey transfer][transfer_id={transfer_id}] event=active_transfer_removed reason=receiver_cancelled"
        ));
        logging::write_transfer_line(&format!(
            "[pastey transfer][transfer_id={transfer_id}] event=receiver_cancelled_transfer"
        ));
        remove_active_receiver_part_file(&transfer.room_id, "active_part", part_path).await?;
        notify_peer_transfer_terminal_reason(
            &state,
            &transfer.room_id,
            transfer_id,
            "receiver_cancelled",
        )
        .await;
    } else if let Ok(room) = storage::get_room_by_id(&state.paths, &transfer.room_id) {
        if let (Some(peer_host), Some(peer_port)) = (room.peer_host, room.peer_port) {
            let client = reqwest::Client::new();
            let base_url = format!("http://{peer_host}:{peer_port}/rooms/{}", transfer.room_id);
            notify_transfer_cancel(&client, &base_url, transfer_id).await;
        }
    }
    let _ =
        storage::set_room_item_status(&state.paths, &transfer.item_id, RoomItemStatus::Cancelled);
    emit_event(
        &state,
        &transfer,
        "cancelled",
        current_transferred(&transfer),
        0.0,
        average_speed(&transfer, current_transferred(&transfer)),
        None,
        Some(TRANSFER_CANCELLED_MESSAGE.into()),
    );
    Ok(true)
}

pub async fn notify_room_burn_with_peer(peer_host: &str, peer_port: u16, room_id: &str) {
    notify_room_event(peer_host, peer_port, room_id, "burn").await;
}

pub async fn notify_room_leave(state: Arc<AppState>, room_id: &str) {
    let Ok(room) = storage::get_room_by_id(&state.paths, room_id) else {
        return;
    };
    let (Some(peer_host), Some(peer_port)) = (room.peer_host, room.peer_port) else {
        return;
    };

    notify_room_event(&peer_host, peer_port, room_id, "leave").await;
}

pub async fn cancel_room_transfers(
    state: Arc<AppState>,
    room_id: &str,
    message: &str,
    notify_peer: bool,
    receiver_reason: Option<&str>,
) -> AppResult<()> {
    let transfer_ids = {
        let transfers = state.active_file_transfers.lock();
        transfers
            .iter()
            .filter(|(_, transfer)| transfer.room_id == room_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };

    let mut cleanup_failed = false;
    for transfer_id in transfer_ids {
        if notify_peer {
            if cancel_transfer_with_reason(state.clone(), &transfer_id, receiver_reason)
                .await
                .is_err()
            {
                cleanup_failed = true;
            }
        } else {
            let transfer = {
                let mut transfers = state.active_file_transfers.lock();
                transfers.remove(&transfer_id)
            };
            if let Some(transfer) = transfer {
                transfer.cancel_token.cancel();
                if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
                    if let Some(reason) = receiver_reason {
                        record_terminal_transfer_reason(&state, &transfer_id, reason);
                        logging::write_transfer_line(&format!(
                            "[pastey transfer][transfer_id={transfer_id}] event=active_transfer_removed reason={reason}"
                        ));
                    }
                    if remove_active_receiver_part_file(room_id, "active_part", part_path)
                        .await
                        .is_err()
                    {
                        cleanup_failed = true;
                    }
                }
                let status = if message == ROOM_BURNED_MESSAGE {
                    "burned"
                } else {
                    "cancelled"
                };
                emit_event(
                    &state,
                    &transfer,
                    status,
                    current_transferred(&transfer),
                    0.0,
                    average_speed(&transfer, current_transferred(&transfer)),
                    None,
                    Some(message.to_string()),
                );
            }
        }
    }

    if cleanup_failed {
        return Err(AppError::InvalidInput(
            "Could not delete local room files. Check folder permissions.".into(),
        ));
    }
    Ok(())
}

pub fn active_transfer_room_ids(state: &Arc<AppState>) -> Vec<String> {
    state
        .active_file_transfers
        .lock()
        .values()
        .map(|transfer| transfer.room_id.clone())
        .collect()
}

fn active_receiver_final_paths(state: &Arc<AppState>) -> Vec<PathBuf> {
    state
        .active_file_transfers
        .lock()
        .values()
        .filter_map(|transfer| match &transfer.kind {
            ActiveFileTransferKind::Receiver { final_path, .. } => Some(final_path.clone()),
            ActiveFileTransferKind::Sender => None,
        })
        .collect()
}

async fn notify_room_event(peer_host: &str, peer_port: u16, room_id: &str, action: &str) {
    let _ = reqwest::Client::new()
        .post(format!(
            "http://{peer_host}:{peer_port}/rooms/{room_id}/{action}"
        ))
        .send()
        .await;
}

fn map_missing_payload_error(error: std::io::Error) -> AppError {
    if error.kind() == std::io::ErrorKind::NotFound {
        AppError::NotFound("File is no longer available.".into())
    } else {
        error.into()
    }
}

pub fn device_name() -> String {
    std::env::var("PASTEY_DEVICE_NAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "Nearby device".to_string())
}

fn room_server_snapshot(state: &Arc<AppState>, room_id: &str) -> AppResult<ActiveRoomSnapshot> {
    let servers = state.active_servers.lock();
    let server = servers
        .get(room_id)
        .ok_or_else(|| AppError::NotFound(PEER_DISCONNECTED_MESSAGE.into()))?;
    Ok(ActiveRoomSnapshot {
        port: server.port,
        transport_secret: server.transport_secret,
        transport_public_key: server.transport_public_key(),
    })
}

async fn join_handler(
    AxumPath(room_id): AxumPath<String>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    State(ctx): State<RoomServerContext>,
    Json(request): Json<JoinRoomRequest>,
) -> Result<Json<JoinRoomResponse>, StatusCode> {
    if room_id != ctx.room_id {
        return Err(StatusCode::NOT_FOUND);
    }

    let room =
        storage::get_room_by_id(&ctx.state.paths, &room_id).map_err(|_| StatusCode::NOT_FOUND)?;
    if room.status == RoomStatus::Burned {
        return Err(StatusCode::GONE);
    }

    let snapshot = room_server_snapshot(&ctx.state, &room_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    storage::update_room_peer(
        &ctx.state.paths,
        &room_id,
        Some(&source.ip().to_string()),
        Some(request.port),
        Some(&request.device_name),
        Some(&request.transport_public_key),
        RoomStatus::Active,
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(JoinRoomResponse {
        device_name: device_name(),
        expires_at: room.expires_at,
        transport_public_key: snapshot.transport_public_key,
    }))
}

async fn receive_item_handler(
    AxumPath(room_id): AxumPath<String>,
    State(ctx): State<RoomServerContext>,
    Json(upload): Json<RoomItemUpload>,
) -> Response {
    if room_id != ctx.room_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(response) = unavailable_room_response(&ctx.state, &room_id) {
        return response;
    }
    if let Err(error) = storage::validate_file_size(upload.size_bytes) {
        return transfer_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_too_large",
            error.message(),
        );
    }
    match storage::room_item_exists(&ctx.state.paths, &upload.item_id) {
        Ok(true) => return StatusCode::OK.into_response(),
        Ok(false) => {}
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }

    let snapshot = match room_server_snapshot(&ctx.state, &room_id) {
        Ok(snapshot) => snapshot,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let session_key = match crypto::unwrap_session_from_sender(
        &upload.wrapped_session_key,
        &upload.transport_nonce,
        &upload.sender_public_key,
        &snapshot.transport_secret,
    ) {
        Ok(session_key) => session_key,
        Err(_) => {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                legacy_payload_error_message(&upload.payload_type).into(),
            )
        }
    };
    let encrypted_payload = match STANDARD.decode(&upload.encrypted_payload) {
        Ok(encrypted_payload) => encrypted_payload,
        Err(_) => {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                legacy_payload_error_message(&upload.payload_type).into(),
            )
        }
    };
    let payload_nonce = match crypto::decode_nonce(&upload.payload_nonce) {
        Ok(payload_nonce) => payload_nonce,
        Err(_) => {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                legacy_payload_error_message(&upload.payload_type).into(),
            )
        }
    };
    let plaintext = match crypto::decrypt_bytes(&encrypted_payload, &session_key, &payload_nonce) {
        Ok(plaintext) => plaintext,
        Err(_) => {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "integrity_failed",
                "Chunk failed integrity verification.".into(),
            )
        }
    };

    let saved_path = if upload.payload_type == PayloadType::File {
        let destination_dir = {
            let config = ctx.state.config.read();
            config::received_item_destination_dir(
                &ctx.state.paths,
                &config,
                upload.mime_type.as_deref(),
            )
        };
        let output_path =
            match storage::next_inbox_path(&destination_dir, upload.display_name.as_deref()) {
                Ok(path) => path,
                Err(_) => {
                    return transfer_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "write_failed",
                        "Could not write to destination folder.".into(),
                    )
                }
            };
        if tokio::fs::create_dir_all(&destination_dir).await.is_err()
            || tokio::fs::write(&output_path, &plaintext).await.is_err()
        {
            return transfer_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "write_failed",
                "Could not write to destination folder.".into(),
            );
        }
        Some(output_path.display().to_string())
    } else {
        None
    };

    let master_key = {
        let config = ctx.state.config.read();
        match config::master_key(&config) {
            Ok(key) => key,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    };
    if storage::persist_incoming_item(
        &ctx.state.paths,
        &master_key,
        &room_id,
        &upload.item_id,
        upload.payload_type,
        &plaintext,
        upload.display_name,
        upload.mime_type,
        upload.created_at,
        saved_path,
    )
    .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let _ = storage::set_room_status(&ctx.state.paths, &room_id, RoomStatus::Active);
    StatusCode::OK.into_response()
}

async fn start_file_transfer_handler(
    AxumPath(room_id): AxumPath<String>,
    State(ctx): State<RoomServerContext>,
    Json(start): Json<FileTransferStartRequest>,
) -> Response {
    dev_log_receiver_start_route_hit(
        &start.transfer_id,
        &room_id,
        start.chunk_size,
        start.total_chunks,
        start.size_bytes,
    );
    if room_id != ctx.room_id {
        return transfer_error(
            StatusCode::NOT_FOUND,
            "room_not_found",
            "Room not found on receiver.".into(),
        );
    }
    if let Some(response) = unavailable_room_response(&ctx.state, &room_id) {
        return response;
    }
    if let Err(error) = storage::validate_file_size(start.size_bytes) {
        return transfer_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_too_large",
            error.message(),
        );
    }
    if start.chunk_size == 0 || start.chunk_size > DEFAULT_CHUNK_SIZE_BYTES {
        return transfer_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "chunk_too_large",
            "Chunk too large for receiver".into(),
        );
    }
    if start.total_chunks != total_chunks_for(start.size_bytes, start.chunk_size) {
        return transfer_error(
            StatusCode::BAD_REQUEST,
            "metadata_mismatch",
            "Transfer metadata mismatch".into(),
        );
    }
    match storage::room_item_exists(&ctx.state.paths, &start.item_id) {
        Ok(true) => return Json(file_transfer_start_response()).into_response(),
        Ok(false) => {}
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }

    let snapshot = match room_server_snapshot(&ctx.state, &room_id) {
        Ok(snapshot) => snapshot,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let session_key = match crypto::unwrap_session_from_sender(
        &start.wrapped_session_key,
        &start.transport_nonce,
        &start.sender_public_key,
        &snapshot.transport_secret,
    ) {
        Ok(session_key) => session_key,
        Err(_) => {
            dev_log_receiver_start_failure(
                &start.transfer_id,
                &room_id,
                StatusCode::BAD_REQUEST,
                "invalid_payload",
            );
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                "Chunk integrity check failed".into(),
            );
        }
    };
    let destination_dir = {
        let config = ctx.state.config.read();
        config::received_item_destination_dir(&ctx.state.paths, &config, start.mime_type.as_deref())
    };
    if !has_enough_disk_space(&destination_dir, start.size_bytes) {
        return transfer_error(
            StatusCode::INSUFFICIENT_STORAGE,
            "not_enough_disk_space",
            "Not enough disk space to receive this file.".into(),
        );
    }
    let reserved_final_paths = active_receiver_final_paths(&ctx.state);
    let final_path = match storage::next_inbox_path_excluding(
        &destination_dir,
        start.display_name.as_deref(),
        &reserved_final_paths,
    ) {
        Ok(path) => path,
        Err(_) => {
            return transfer_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "write_failed",
                "Could not write to destination folder.".into(),
            )
        }
    };
    let part_path = storage::transfer_part_path(&destination_dir, &start.transfer_id);
    let Some(part_dir) = part_path.parent() else {
        return transfer_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "Receiver failed to write chunk".into(),
        );
    };
    if tokio::fs::create_dir_all(part_dir).await.is_err()
        || tokio::fs::File::create(&part_path).await.is_err()
    {
        dev_log_receiver_start_failure(
            &start.transfer_id,
            &room_id,
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
        );
        return transfer_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "Receiver failed to write chunk".into(),
        );
    }

    let file_name = final_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("pastey_file")
        .to_string();
    let now = Instant::now();
    let cancel_token = CancellationToken::new();
    let transfer = ActiveFileTransfer {
        room_id: room_id.clone(),
        item_id: start.item_id.clone(),
        file_name,
        file_size: start.size_bytes,
        chunk_size: start.chunk_size,
        total_chunks: start.total_chunks,
        started_at: now,
        last_report_at: now,
        last_report_bytes: 0,
        cancel_token,
        kind: ActiveFileTransferKind::Receiver {
            session_key,
            part_path,
            final_path,
            mime_type: start.mime_type,
            created_at: start.created_at,
            transferred_bytes: 0,
            expected_chunk_index: 0,
            received_chunks: vec![false; start.total_chunks as usize],
            timing: ReceiverTimingSummary::default(),
        },
    };
    emit_event(&ctx.state, &transfer, "pending", 0, 0.0, 0.0, None, None);
    let registered_transfer_id = start.transfer_id.clone();
    ctx.state
        .active_file_transfers
        .lock()
        .insert(start.transfer_id, transfer);
    dev_log_receiver_start_registered(
        &registered_transfer_id,
        &room_id,
        start.chunk_size,
        start.total_chunks,
    );
    Json(file_transfer_start_response()).into_response()
}

fn decode_received_chunk_upload(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<ReceivedChunkUpload, ChunkPayloadDecodeFailure> {
    match chunk_upload_protocol(headers)? {
        ChunkProtocol::BinaryV1 => decode_binary_chunk_upload(body),
        ChunkProtocol::JsonV1 => decode_json_chunk_upload(body),
    }
}

fn chunk_upload_protocol(headers: &HeaderMap) -> Result<ChunkProtocol, ChunkPayloadDecodeFailure> {
    if let Some(protocol) = headers.get(CHUNK_PROTOCOL_HEADER) {
        let protocol = protocol.to_str().unwrap_or_default();
        return match protocol {
            CHUNK_PROTOCOL_BINARY_V1 => Ok(ChunkProtocol::BinaryV1),
            CHUNK_PROTOCOL_JSON_V1 => Ok(ChunkProtocol::JsonV1),
            _ => Err(ChunkPayloadDecodeFailure {
                status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
                code: "unsupported_chunk_protocol",
                message: "Unsupported chunk protocol",
                cause: format!("unsupported_chunk_protocol: {protocol:?}"),
            }),
        };
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.starts_with("application/octet-stream") {
        Ok(ChunkProtocol::BinaryV1)
    } else {
        Ok(ChunkProtocol::JsonV1)
    }
}

fn decode_binary_chunk_upload(
    body: &[u8],
) -> Result<ReceivedChunkUpload, ChunkPayloadDecodeFailure> {
    let frame = decode_binary_chunk_frame(body).map_err(binary_chunk_decode_failure)?;
    let ciphertext_len = frame.ciphertext.len();
    Ok(ReceivedChunkUpload {
        chunk_index: frame.chunk_index,
        nonce: frame.nonce,
        ciphertext: frame.ciphertext,
        plaintext_size: frame.plaintext_size as u64,
        is_final: frame.is_final,
        protocol: ChunkProtocol::BinaryV1,
        payload_body_size: body.len(),
        encoded_ciphertext_bytes: ciphertext_len,
    })
}

fn binary_chunk_decode_failure(error: BinaryChunkFrameError) -> ChunkPayloadDecodeFailure {
    match error {
        BinaryChunkFrameError::UnsupportedVersion => ChunkPayloadDecodeFailure {
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code: "unsupported_chunk_protocol",
            message: "Unsupported chunk protocol",
            cause: error.as_str().to_string(),
        },
        BinaryChunkFrameError::FrameTooLarge => ChunkPayloadDecodeFailure {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "chunk_too_large",
            message: "Chunk too large for receiver",
            cause: error.as_str().to_string(),
        },
        _ => ChunkPayloadDecodeFailure {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_chunk_payload",
            message: "Invalid chunk payload",
            cause: error.as_str().to_string(),
        },
    }
}

fn decode_json_chunk_upload(body: &[u8]) -> Result<ReceivedChunkUpload, ChunkPayloadDecodeFailure> {
    let upload = serde_json::from_slice::<ChunkUploadRequest>(body).map_err(|error| {
        ChunkPayloadDecodeFailure {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_chunk_payload",
            message: "Invalid chunk payload",
            cause: format!("body_rejected: {error}"),
        }
    })?;
    let nonce = crypto::decode_nonce(&upload.nonce).map_err(|_| ChunkPayloadDecodeFailure {
        status: StatusCode::BAD_REQUEST,
        code: "invalid_chunk_encoding",
        message: "Invalid chunk encoding",
        cause: "invalid_nonce_encoding".to_string(),
    })?;
    let ciphertext =
        STANDARD
            .decode(&upload.ciphertext)
            .map_err(|_| ChunkPayloadDecodeFailure {
                status: StatusCode::BAD_REQUEST,
                code: "invalid_chunk_encoding",
                message: "Invalid chunk encoding",
                cause: "invalid_ciphertext_encoding".to_string(),
            })?;
    let encoded_ciphertext_bytes = upload.ciphertext.len();

    Ok(ReceivedChunkUpload {
        chunk_index: upload.chunk_index,
        nonce,
        ciphertext,
        plaintext_size: upload.plaintext_size,
        is_final: upload.is_final,
        protocol: ChunkProtocol::JsonV1,
        payload_body_size: body.len(),
        encoded_ciphertext_bytes,
    })
}

fn file_transfer_start_response() -> FileTransferStartResponse {
    FileTransferStartResponse {
        ok: true,
        preferred_chunk_protocol: Some(CHUNK_PROTOCOL_BINARY_V1.to_string()),
        supported_chunk_protocols: vec![
            CHUNK_PROTOCOL_BINARY_V1.to_string(),
            CHUNK_PROTOCOL_JSON_V1.to_string(),
        ],
    }
}

async fn receive_file_chunk_handler(
    AxumPath((room_id, transfer_id)): AxumPath<(String, String)>,
    State(ctx): State<RoomServerContext>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    if room_id != ctx.room_id {
        dev_log_receiver_chunk_route_hit(&transfer_id, &room_id, None);
        return transfer_error(
            StatusCode::NOT_FOUND,
            "room_not_found",
            "Room not found on receiver.".into(),
        );
    }
    if let Some(response) = unavailable_room_response_for_active_transfer(&ctx.state, &room_id) {
        return response;
    }

    let body = match body {
        Ok(body) => body,
        Err(error) => {
            let status = error.status();
            let body_text = error.body_text();
            let message = if status == StatusCode::PAYLOAD_TOO_LARGE {
                "Chunk too large for receiver"
            } else {
                "Invalid chunk payload"
            };
            let code = if status == StatusCode::PAYLOAD_TOO_LARGE {
                "chunk_too_large"
            } else {
                "invalid_chunk_payload"
            };
            dev_log_receiver_chunk_failure(
                &transfer_id,
                &room_id,
                None,
                0,
                0,
                0,
                status,
                &format!("body_rejected: {body_text}"),
            );
            fail_receiver_transfer(&ctx.state, &transfer_id, message).await;
            return transfer_error(status, code, message.into());
        }
    };
    let total_started = Instant::now();
    let decode_started = Instant::now();
    let upload = match decode_received_chunk_upload(&headers, &body) {
        Ok(upload) => upload,
        Err(error) => {
            dev_log_receiver_chunk_failure(
                &transfer_id,
                &room_id,
                None,
                0,
                0,
                body.len(),
                error.status,
                &error.cause,
            );
            fail_receiver_transfer(&ctx.state, &transfer_id, error.message).await;
            return transfer_error(error.status, error.code, error.message.into());
        }
    };
    let decode_elapsed = decode_started.elapsed();

    let chunk_index = upload.chunk_index;
    let plaintext_size = upload.plaintext_size;
    let encoded_payload_size = upload.payload_body_size;
    dev_log_receiver_chunk_route_hit(&transfer_id, &room_id, Some(chunk_index));

    if plaintext_size == 0 || plaintext_size > DEFAULT_CHUNK_SIZE_BYTES {
        dev_log_receiver_chunk_failure(
            &transfer_id,
            &room_id,
            Some(chunk_index),
            plaintext_size,
            0,
            encoded_payload_size,
            StatusCode::BAD_REQUEST,
            "invalid_chunk_payload",
        );
        fail_receiver_transfer(&ctx.state, &transfer_id, "Invalid chunk payload").await;
        return transfer_error(
            StatusCode::BAD_REQUEST,
            "invalid_chunk_payload",
            "Invalid chunk payload".into(),
        );
    }

    dev_log_receiver_chunk_received(
        &transfer_id,
        &room_id,
        chunk_index,
        plaintext_size,
        upload.encoded_ciphertext_bytes,
        encoded_payload_size,
        upload.protocol,
    );

    let nonce = upload.nonce;
    let ciphertext = upload.ciphertext;
    let ciphertext_bytes = ciphertext.len();

    let transfer_lookup = {
        let mut transfers = ctx.state.active_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(&transfer_id) else {
            log_late_event_ignored(&transfer_id, &room_id, "chunk");
            if let Some(response) = terminal_transfer_response(&ctx.state, &transfer_id, &room_id) {
                return response;
            }
            return transfer_error(
                StatusCode::NOT_FOUND,
                "transfer_missing",
                "Transfer session not found on receiver.".into(),
            );
        };
        if transfer.cancel_token.is_cancelled() {
            return transfer_error(
                StatusCode::CONFLICT,
                "cancelled",
                TRANSFER_CANCELLED_MESSAGE.into(),
            );
        }
        let ActiveFileTransferKind::Receiver {
            session_key,
            part_path,
            transferred_bytes,
            received_chunks,
            timing,
            ..
        } = &mut transfer.kind
        else {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_transfer",
                "Invalid chunk payload".into(),
            );
        };
        let Some(received) = received_chunks.get(chunk_index as usize) else {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "metadata_mismatch",
                "Transfer metadata mismatch".into(),
            );
        };
        let is_expected_final = chunk_index.checked_add(1) == Some(transfer.total_chunks);
        if upload.is_final != is_expected_final {
            Err((
                "invalid_chunk_payload",
                "Invalid chunk payload",
                "invalid_chunk_payload",
            ))
        } else if plaintext_size > transfer.chunk_size {
            Err((
                "metadata_mismatch",
                "Transfer metadata mismatch",
                "chunk_larger_than_metadata_chunk_size",
            ))
        } else if *received {
            timing.duplicate_chunks += 1;
            let duplicate_written_bytes =
                if chunk_index.checked_add(1) == Some(transfer.total_chunks) {
                    transfer
                        .file_size
                        .saturating_sub(transfer.chunk_size.saturating_mul(chunk_index))
                } else {
                    transfer.chunk_size
                };
            dev_log_receiver_chunk_ack(
                &transfer_id,
                &room_id,
                chunk_index,
                duplicate_written_bytes,
                *transferred_bytes,
                "duplicate",
            );
            return Json(ChunkAckResponse {
                ok: true,
                chunk_index,
                written_bytes: duplicate_written_bytes,
                total_received_bytes: *transferred_bytes,
            })
            .into_response();
        } else {
            Ok((
                *session_key,
                part_path.clone(),
                cancel_token_clone(transfer),
                transfer.chunk_size.saturating_mul(chunk_index),
            ))
        }
    };
    let (session_key, part_path, cancel_token, write_offset) = match transfer_lookup {
        Ok(value) => value,
        Err((code, message, cause)) => {
            dev_log_receiver_chunk_failure(
                &transfer_id,
                &room_id,
                Some(chunk_index),
                plaintext_size,
                ciphertext_bytes,
                encoded_payload_size,
                StatusCode::BAD_REQUEST,
                cause,
            );
            fail_receiver_transfer(&ctx.state, &transfer_id, message).await;
            return transfer_error(StatusCode::BAD_REQUEST, code, message.into());
        }
    };
    if cancel_token.is_cancelled() {
        return transfer_error(
            StatusCode::CONFLICT,
            "cancelled",
            TRANSFER_CANCELLED_MESSAGE.into(),
        );
    }

    let decrypt_started = Instant::now();
    let plaintext = match crypto::decrypt_bytes(&ciphertext, &session_key, &nonce) {
        Ok(plaintext) => plaintext,
        Err(_) => {
            dev_log_receiver_chunk_failure(
                &transfer_id,
                &room_id,
                Some(chunk_index),
                plaintext_size,
                ciphertext_bytes,
                encoded_payload_size,
                StatusCode::BAD_REQUEST,
                "integrity_failed",
            );
            fail_receiver_transfer(&ctx.state, &transfer_id, "Chunk integrity check failed").await;
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "integrity_failed",
                "Chunk integrity check failed".into(),
            );
        }
    };
    let decrypt_elapsed = decrypt_started.elapsed();
    if plaintext.len() as u64 != plaintext_size {
        dev_log_receiver_chunk_failure(
            &transfer_id,
            &room_id,
            Some(chunk_index),
            plaintext_size,
            ciphertext_bytes,
            encoded_payload_size,
            StatusCode::BAD_REQUEST,
            "plaintext_size_mismatch",
        );
        fail_receiver_transfer(&ctx.state, &transfer_id, "Chunk integrity check failed").await;
        return transfer_error(
            StatusCode::BAD_REQUEST,
            "integrity_failed",
            "Chunk integrity check failed".into(),
        );
    }

    let write_started = Instant::now();
    if let Err(error) = write_receiver_chunk(&part_path, &plaintext, write_offset).await {
        dev_log_receiver_chunk_failure(
            &transfer_id,
            &room_id,
            Some(chunk_index),
            plaintext_size,
            ciphertext_bytes,
            encoded_payload_size,
            error.status,
            &format!(
                "{}: part_path={} parent_exists={} file_exists={} transfer_status=active",
                error.cause,
                part_path.display(),
                error.parent_exists,
                error.file_exists
            ),
        );
        fail_receiver_transfer(&ctx.state, &transfer_id, error.message).await;
        return transfer_error(error.status, error.code, error.message.into());
    }
    let write_elapsed = write_started.elapsed();
    dev_log_receiver_chunk_write_success(
        &transfer_id,
        &room_id,
        chunk_index,
        plaintext_size,
        ciphertext_bytes,
        encoded_payload_size,
        upload.protocol,
    );

    let (maybe_event, ack_transferred_bytes) = {
        let mut transfers = ctx.state.active_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(&transfer_id) else {
            log_late_event_ignored(&transfer_id, &room_id, "chunk_progress");
            if let Some(response) = terminal_transfer_response(&ctx.state, &transfer_id, &room_id) {
                return response;
            }
            return transfer_error(
                StatusCode::NOT_FOUND,
                "transfer_missing",
                "Transfer session not found on receiver.".into(),
            );
        };
        let now = Instant::now();
        let previous_report_at = transfer.last_report_at;
        let previous_report_bytes = transfer.last_report_bytes;
        let ActiveFileTransferKind::Receiver {
            transferred_bytes,
            expected_chunk_index,
            received_chunks,
            ..
        } = &mut transfer.kind
        else {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_transfer",
                "Invalid chunk payload".into(),
            );
        };
        let Some(received) = received_chunks.get_mut(chunk_index as usize) else {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "metadata_mismatch",
                "Transfer metadata mismatch".into(),
            );
        };
        if !*received {
            *received = true;
            *transferred_bytes += plaintext_size;
            *expected_chunk_index += 1;
        }
        let current = *transferred_bytes;
        let interval = now
            .duration_since(previous_report_at)
            .as_secs_f64()
            .max(0.001);
        let current_speed = (current - previous_report_bytes) as f64 / interval;
        let average_speed = current as f64
            / now
                .duration_since(transfer.started_at)
                .as_secs_f64()
                .max(0.001);
        let should_emit_progress = now.duration_since(previous_report_at) >= PROGRESS_EMIT_INTERVAL
            || current >= transfer.file_size;
        if should_emit_progress {
            transfer.last_report_at = now;
            transfer.last_report_bytes = current;
            (
                Some(clone_event_base(
                    transfer,
                    "incoming",
                    "transferring",
                    current,
                    current_speed,
                    average_speed,
                    eta_seconds(transfer.file_size, current, current_speed),
                    None,
                )),
                current,
            )
        } else {
            (None, current)
        }
    };
    let ui_emit_started = Instant::now();
    if let Some(event) = maybe_event {
        let _ = ctx.state.app_handle.emit(TRANSFER_EVENT, event);
    }
    let ui_emit_elapsed = ui_emit_started.elapsed();
    dev_log_receiver_chunk_timing(
        &transfer_id,
        &room_id,
        chunk_index,
        plaintext_size,
        decode_elapsed,
        decrypt_elapsed,
        write_elapsed,
        ui_emit_elapsed,
        total_started.elapsed(),
    );
    record_receiver_chunk_timing(
        &ctx.state,
        &transfer_id,
        decode_elapsed,
        decrypt_elapsed,
        write_elapsed,
        ui_emit_elapsed,
    );
    dev_log_receiver_chunk_ack(
        &transfer_id,
        &room_id,
        chunk_index,
        plaintext_size,
        ack_transferred_bytes,
        "ok",
    );

    Json(ChunkAckResponse {
        ok: true,
        chunk_index,
        written_bytes: plaintext_size,
        total_received_bytes: ack_transferred_bytes,
    })
    .into_response()
}

async fn finish_file_transfer_handler(
    AxumPath((room_id, transfer_id)): AxumPath<(String, String)>,
    State(ctx): State<RoomServerContext>,
    Json(finish): Json<FileTransferFinishRequest>,
) -> Response {
    dev_log_receiver_finalize(&transfer_id, &room_id, "finalize_start", "");
    if room_id != ctx.room_id {
        return transfer_error(
            StatusCode::NOT_FOUND,
            "room_not_found",
            "Room not found on receiver.".into(),
        );
    }

    let transfer = match ctx.state.active_file_transfers.lock().remove(&transfer_id) {
        Some(transfer) => transfer,
        None => {
            log_late_event_ignored(&transfer_id, &room_id, "finalize");
            if let Some(response) = terminal_transfer_response(&ctx.state, &transfer_id, &room_id) {
                return response;
            }
            if room_is_burned(&ctx.state, &room_id) {
                return transfer_error(StatusCode::GONE, "room_burned", ROOM_BURNED_MESSAGE.into());
            }
            return transfer_error(
                StatusCode::NOT_FOUND,
                "transfer_missing",
                "Transfer session not found on receiver.".into(),
            );
        }
    };
    let ActiveFileTransferKind::Receiver {
        part_path,
        final_path,
        mime_type,
        created_at,
        transferred_bytes,
        expected_chunk_index,
        timing,
        ..
    } = &transfer.kind
    else {
        return transfer_error(
            StatusCode::BAD_REQUEST,
            "invalid_transfer",
            "Invalid file metadata".into(),
        );
    };
    if room_is_burned(&ctx.state, &room_id) {
        return abort_receiver_finalize_for_burn(
            &ctx.state,
            &transfer,
            &[part_path.clone()],
            *transferred_bytes,
        )
        .await;
    }
    dev_log_receiver_finalize(
        &transfer_id,
        &room_id,
        "finalize_verify_size",
        &format!(
            "item_id={} finish_item_id={} received_bytes={} file_size={} received_chunks={} total_chunks={} chunk_size={}",
            transfer.item_id,
            finish.item_id,
            transferred_bytes,
            transfer.file_size,
            expected_chunk_index,
            transfer.total_chunks,
            transfer.chunk_size
        ),
    );
    if finish.item_id != transfer.item_id {
        dev_log_receiver_finalize(
            &transfer_id,
            &room_id,
            "finalize_failure",
            "error_kind=invalid_transfer message=\"Invalid file metadata\"",
        );
        let _ = tokio::fs::remove_file(part_path).await;
        emit_event(
            &ctx.state,
            &transfer,
            "failed",
            *transferred_bytes,
            0.0,
            average_speed(&transfer, *transferred_bytes),
            None,
            Some("Invalid file metadata".into()),
        );
        return transfer_error(
            StatusCode::BAD_REQUEST,
            "invalid_transfer",
            "Invalid file metadata".into(),
        );
    }
    if let Err((code, message)) = verify_finalize_metadata(
        *transferred_bytes,
        transfer.file_size,
        *expected_chunk_index,
        transfer.total_chunks,
    ) {
        dev_log_receiver_finalize(
            &transfer_id,
            &room_id,
            "finalize_failure",
            &format!("error_kind={code} message={message:?}"),
        );
        let _ = tokio::fs::remove_file(part_path).await;
        emit_event(
            &ctx.state,
            &transfer,
            "failed",
            *transferred_bytes,
            0.0,
            average_speed(&transfer, *transferred_bytes),
            None,
            Some(message.into()),
        );
        return transfer_error(StatusCode::BAD_REQUEST, code, message.into());
    }
    dev_log_receiver_transfer_summary(
        &transfer_id,
        &room_id,
        transfer.chunk_size,
        transfer.file_size,
        *transferred_bytes,
        transfer.started_at.elapsed(),
        *timing,
        "verified",
    );
    if room_is_burned(&ctx.state, &room_id) {
        return abort_receiver_finalize_for_burn(
            &ctx.state,
            &transfer,
            &[part_path.clone()],
            *transferred_bytes,
        )
        .await;
    }

    dev_log_receiver_finalize(
        &transfer_id,
        &room_id,
        "finalize_rename",
        &format!(
            "part_path={} final_path={}",
            part_path.display(),
            final_path.display()
        ),
    );
    if tokio::fs::rename(part_path, final_path).await.is_err() {
        dev_log_receiver_finalize(
            &transfer_id,
            &room_id,
            "finalize_failure",
            "error_kind=write_failed message=\"Receiver failed to write chunk\"",
        );
        let _ = tokio::fs::remove_file(part_path).await;
        emit_event(
            &ctx.state,
            &transfer,
            "failed",
            *transferred_bytes,
            0.0,
            average_speed(&transfer, *transferred_bytes),
            None,
            Some("Receiver failed to write chunk".into()),
        );
        return transfer_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "Receiver failed to write chunk".into(),
        );
    }
    if room_is_burned(&ctx.state, &room_id) {
        return abort_receiver_finalize_for_burn(
            &ctx.state,
            &transfer,
            &[final_path.clone()],
            *transferred_bytes,
        )
        .await;
    }

    dev_log_receiver_finalize(
        &transfer_id,
        &room_id,
        "finalize_item_update",
        &format!(
            "item_kind=incoming_file status=received final_path={} mime_type={:?}",
            final_path.display(),
            mime_type
        ),
    );
    let master_key = {
        let config = ctx.state.config.read();
        match config::master_key(&config) {
            Ok(key) => key,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    };
    if let Err(error) = storage::persist_incoming_file_item_metadata(
        &ctx.state.paths,
        &master_key,
        &room_id,
        &transfer.item_id,
        transfer.file_size,
        Some(transfer.file_name.clone()),
        mime_type.clone(),
        *created_at,
        Some(final_path.display().to_string()),
    ) {
        if room_is_burned(&ctx.state, &room_id) || error.message() == ROOM_BURNED_MESSAGE {
            return abort_receiver_finalize_for_burn(
                &ctx.state,
                &transfer,
                &[final_path.clone()],
                *transferred_bytes,
            )
            .await;
        }
        dev_log_receiver_finalize(
            &transfer_id,
            &room_id,
            "finalize_failure",
            "error_kind=write_failed message=\"Receiver failed to write chunk\"",
        );
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if room_is_burned(&ctx.state, &room_id) {
        return abort_receiver_finalize_for_burn(
            &ctx.state,
            &transfer,
            &[final_path.clone()],
            *transferred_bytes,
        )
        .await;
    }
    let _ = storage::set_room_status(&ctx.state.paths, &room_id, RoomStatus::Active);
    dev_log_receiver_finalize(
        &transfer_id,
        &room_id,
        "finalize_complete",
        "status=completed",
    );
    emit_event(
        &ctx.state,
        &transfer,
        "completed",
        transfer.file_size,
        0.0,
        average_speed(&transfer, transfer.file_size),
        Some(0.0),
        None,
    );
    Json(TransferOkResponse { ok: true }).into_response()
}

async fn cancel_file_transfer_handler(
    AxumPath((room_id, transfer_id)): AxumPath<(String, String)>,
    State(ctx): State<RoomServerContext>,
    request: Option<Json<TransferCancelRequest>>,
) -> Response {
    if room_id != ctx.room_id {
        return transfer_error(
            StatusCode::NOT_FOUND,
            "room_not_found",
            "Room not found on receiver.".into(),
        );
    }
    let Some(transfer) = ctx.state.active_file_transfers.lock().remove(&transfer_id) else {
        log_late_event_ignored(&transfer_id, &room_id, "cancel");
        return Json(TransferOkResponse { ok: true }).into_response();
    };
    transfer.cancel_token.cancel();
    if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
        let _ = tokio::fs::remove_file(part_path).await;
    }
    let reason = request
        .as_ref()
        .and_then(|Json(request)| request.reason.clone());
    if let Some(reason) = reason.as_deref() {
        record_terminal_transfer_reason(&ctx.state, &transfer_id, reason);
    }
    let failure = request
        .as_ref()
        .is_some_and(|Json(request)| request.status.as_deref() == Some("failed"));
    let remote_terminal = reason.as_deref().is_some_and(is_receiver_terminal_reason);
    let status = if remote_terminal {
        "interrupted"
    } else if failure {
        "failed"
    } else {
        "cancelled"
    };
    let message = request
        .and_then(|Json(request)| request.message)
        .or_else(|| {
            reason
                .as_deref()
                .map(terminal_reason_message)
                .map(str::to_string)
        })
        .unwrap_or_else(|| TRANSFER_CANCELLED_MESSAGE.into());
    let item_status = if remote_terminal {
        RoomItemStatus::Interrupted
    } else if failure {
        RoomItemStatus::Failed
    } else {
        RoomItemStatus::Cancelled
    };
    let _ = storage::set_room_item_status(&ctx.state.paths, &transfer.item_id, item_status);
    emit_event(
        &ctx.state,
        &transfer,
        status,
        current_transferred(&transfer),
        0.0,
        average_speed(&transfer, current_transferred(&transfer)),
        None,
        Some(message),
    );
    Json(TransferOkResponse { ok: true }).into_response()
}

async fn remote_burn_handler(
    AxumPath(room_id): AxumPath<String>,
    State(ctx): State<RoomServerContext>,
) -> Result<StatusCode, StatusCode> {
    if room_id != ctx.room_id {
        return Err(StatusCode::NOT_FOUND);
    }

    let _ = cancel_room_transfers(
        ctx.state.clone(),
        &room_id,
        ROOM_BURNED_MESSAGE,
        false,
        Some("peer_disconnected"),
    )
    .await;
    storage::mark_peer_burned(&ctx.state.paths, &room_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let state = ctx.state.clone();
    tokio::spawn(async move {
        let _ = stop_room_server(state, &room_id).await;
    });
    Ok(StatusCode::OK)
}

async fn remote_leave_handler(
    AxumPath(room_id): AxumPath<String>,
    State(ctx): State<RoomServerContext>,
) -> Result<StatusCode, StatusCode> {
    if room_id != ctx.room_id {
        return Err(StatusCode::NOT_FOUND);
    }

    let _ = cancel_room_transfers(
        ctx.state.clone(),
        &room_id,
        "Peer left the room.",
        false,
        Some("peer_disconnected"),
    )
    .await;
    storage::mark_peer_left(&ctx.state.paths, &room_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let state = ctx.state.clone();
    tokio::spawn(async move {
        let _ = stop_room_server(state, &room_id).await;
    });
    Ok(StatusCode::OK)
}

fn unavailable_room_response(state: &Arc<AppState>, room_id: &str) -> Option<Response> {
    let room = match storage::get_room_by_id(&state.paths, room_id) {
        Ok(room) => room,
        Err(_) => {
            return Some(transfer_error(
                StatusCode::NOT_FOUND,
                "room_not_found",
                "Room not found on receiver.".into(),
            ))
        }
    };
    if room.status == RoomStatus::Burned {
        return Some(transfer_error(
            StatusCode::GONE,
            "room_burned",
            ROOM_BURNED_MESSAGE.into(),
        ));
    }
    None
}

fn unavailable_room_response_for_active_transfer(
    state: &Arc<AppState>,
    room_id: &str,
) -> Option<Response> {
    let room = match storage::get_room_by_id(&state.paths, room_id) {
        Ok(room) => room,
        Err(_) => {
            return Some(transfer_error(
                StatusCode::NOT_FOUND,
                "room_not_found",
                "Room not found on receiver.".into(),
            ))
        }
    };
    if room.status == RoomStatus::Burned {
        return Some(transfer_error(
            StatusCode::GONE,
            "room_burned",
            ROOM_BURNED_MESSAGE.into(),
        ));
    }
    None
}

fn legacy_payload_error_message(payload_type: &PayloadType) -> &'static str {
    match payload_type {
        PayloadType::Text => "Could not decode received text",
        PayloadType::File => "Invalid file metadata",
    }
}

fn transfer_error(status: StatusCode, code: &str, message: String) -> Response {
    (
        status,
        Json(TransferErrorResponse {
            code: code.to_string(),
            message,
            max_size_bytes: Some(storage::MAX_FILE_SIZE_BYTES),
        }),
    )
        .into_response()
}

async fn response_error_message(response: reqwest::Response) -> String {
    let details = response_error_details(response).await;
    map_response_error_message(&details)
}

fn selected_chunk_protocol_from_start_response(body_text: &str) -> ChunkProtocol {
    let Ok(response) = serde_json::from_str::<FileTransferStartResponse>(body_text) else {
        return ChunkProtocol::JsonV1;
    };
    if !response.ok {
        return ChunkProtocol::JsonV1;
    }
    if response.preferred_chunk_protocol.as_deref() == Some(CHUNK_PROTOCOL_BINARY_V1)
        || response
            .supported_chunk_protocols
            .iter()
            .any(|protocol| protocol == CHUNK_PROTOCOL_BINARY_V1)
    {
        ChunkProtocol::BinaryV1
    } else {
        ChunkProtocol::JsonV1
    }
}

async fn response_error_details(response: reqwest::Response) -> ResponseErrorDetails {
    let status = response.status();
    let body_text = response.text().await.unwrap_or_default();
    let parsed = serde_json::from_str::<TransferErrorResponse>(&body_text).ok();
    let code = parsed.as_ref().map(|error| error.code.clone());
    let message = parsed
        .map(|error| error.message)
        .unwrap_or_else(|| status_fallback_message(status).to_string());
    ResponseErrorDetails {
        status,
        code,
        message,
        body_text,
    }
}

fn map_response_error_message(details: &ResponseErrorDetails) -> String {
    match details.code.as_deref() {
        Some("room_not_found") => "Room not found on receiver.".into(),
        Some("transfer_missing") => "Transfer session not found on receiver.".into(),
        Some("chunk_too_large") => "Chunk too large for receiver".into(),
        Some("unsupported_chunk_protocol") => "Unsupported chunk protocol".into(),
        Some("invalid_chunk_payload") => "Invalid chunk payload".into(),
        Some("invalid_chunk_encoding") => "Invalid chunk encoding".into(),
        Some("integrity_failed") => "Chunk integrity check failed".into(),
        Some("invalid_chunk_order") => "Unexpected chunk index".into(),
        Some("metadata_mismatch") => "Transfer metadata mismatch".into(),
        Some("not_enough_disk_space") => "Not enough disk space on receiver".into(),
        Some("receiver_cannot_write") => "Receiver cannot write to inbox".into(),
        Some("room_burned") => ROOM_BURNED_MESSAGE.into(),
        Some("receiver_cancelled") => "Receiver cancelled transfer".into(),
        Some("receiver_burned_room") => "Peer burned the room".into(),
        Some("receiver_left_room") => "Peer left the room".into(),
        Some("receiver_interrupted") => "Receiver stopped receiving".into(),
        Some("peer_disconnected") => PEER_DISCONNECTED_MESSAGE.into(),
        Some("transfer_timed_out") => TRANSFER_INTERRUPTED_MESSAGE.into(),
        Some("size_mismatch") => "Received file size mismatch".into(),
        Some("temp_file_disappeared") => "Receiver temporary file disappeared".into(),
        Some("write_failed") => "Receiver failed to write chunk".into(),
        Some("cancelled") => TRANSFER_CANCELLED_MESSAGE.into(),
        _ => {
            if details.status == StatusCode::PAYLOAD_TOO_LARGE {
                "Chunk too large for receiver".into()
            } else if details.status == StatusCode::NOT_FOUND {
                "Transfer session not found on receiver.".into()
            } else if details.status == StatusCode::UNSUPPORTED_MEDIA_TYPE {
                "Unsupported chunk protocol".into()
            } else if details.status == StatusCode::INTERNAL_SERVER_ERROR {
                "Receiver failed to write chunk".into()
            } else if details.status == StatusCode::GONE {
                TRANSFER_INTERRUPTED_MESSAGE.into()
            } else {
                details.message.clone()
            }
        }
    }
}

fn status_fallback_message(status: StatusCode) -> &'static str {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => "Chunk too large for receiver",
        StatusCode::NOT_FOUND => "Transfer session not found on receiver.",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "Unsupported chunk protocol",
        StatusCode::INTERNAL_SERVER_ERROR => "Receiver failed to write chunk",
        StatusCode::REQUEST_TIMEOUT => TRANSFER_INTERRUPTED_MESSAGE,
        StatusCode::GONE => TRANSFER_INTERRUPTED_MESSAGE,
        _ => TRANSFER_INTERRUPTED_MESSAGE,
    }
}

fn is_receiver_terminal_reason(code: &str) -> bool {
    matches!(
        code,
        "receiver_cancelled"
            | "receiver_burned_room"
            | "receiver_left_room"
            | "receiver_interrupted"
            | "peer_disconnected"
            | "transfer_timed_out"
    )
}

fn terminal_reason_message(code: &str) -> &'static str {
    match code {
        "receiver_cancelled" => "Receiver cancelled transfer",
        "receiver_burned_room" => "Peer burned the room",
        "receiver_left_room" => "Peer left the room",
        "receiver_interrupted" => "Receiver stopped receiving",
        "peer_disconnected" => PEER_DISCONNECTED_MESSAGE,
        "transfer_timed_out" => TRANSFER_INTERRUPTED_MESSAGE,
        _ => "Transfer session not found on receiver.",
    }
}

fn purge_terminal_transfer_reasons(state: &Arc<AppState>) {
    let now = Instant::now();
    state
        .terminal_transfer_reasons
        .lock()
        .retain(|_, reason| now.duration_since(reason.recorded_at) <= TERMINAL_TRANSFER_REASON_TTL);
}

fn record_terminal_transfer_reason(state: &Arc<AppState>, transfer_id: &str, code: &str) {
    purge_terminal_transfer_reasons(state);
    let message = terminal_reason_message(code).to_string();
    state.terminal_transfer_reasons.lock().insert(
        transfer_id.to_string(),
        TerminalTransferReason {
            code: code.to_string(),
            message: message.clone(),
            recorded_at: Instant::now(),
        },
    );
    logging::write_transfer_line(&format!(
        "[pastey transfer][transfer_id={transfer_id}] event=transfer_terminal_reason_recorded reason={code} message={message:?}"
    ));
}

fn terminal_transfer_reason(
    state: &Arc<AppState>,
    transfer_id: &str,
) -> Option<TerminalTransferReason> {
    purge_terminal_transfer_reasons(state);
    state
        .terminal_transfer_reasons
        .lock()
        .get(transfer_id)
        .cloned()
}

fn terminal_transfer_response(
    state: &Arc<AppState>,
    transfer_id: &str,
    room_id: &str,
) -> Option<Response> {
    let reason = terminal_transfer_reason(state, transfer_id)?;
    logging::write_transfer_line(&format!(
        "[pastey transfer][transfer_id={transfer_id}][room_id={room_id}] event=chunk_for_terminal_transfer reason={}",
        reason.code
    ));
    Some(transfer_error(
        StatusCode::CONFLICT,
        &reason.code,
        reason.message,
    ))
}

fn register_sender_transfer(
    state: &Arc<AppState>,
    transfer_id: &str,
    room_id: &str,
    item_id: &str,
    file_name: &str,
    file_size: u64,
    chunk_size: u64,
    total_chunks: u64,
    cancel_token: CancellationToken,
) -> AppResult<()> {
    let now = Instant::now();
    let mut transfers = state.active_file_transfers.lock();
    if transfers.contains_key(transfer_id) {
        return Err(AppError::InvalidInput(
            "Transfer is already in progress.".into(),
        ));
    }
    transfers.insert(
        transfer_id.to_string(),
        ActiveFileTransfer {
            room_id: room_id.to_string(),
            item_id: item_id.to_string(),
            file_name: file_name.to_string(),
            file_size,
            chunk_size,
            total_chunks,
            started_at: now,
            last_report_at: now,
            last_report_bytes: 0,
            cancel_token,
            kind: ActiveFileTransferKind::Sender,
        },
    );
    Ok(())
}

fn fail_transfer(state: &Arc<AppState>, transfer_id: &str, item_id: &str, message: String) {
    let transfer = state.active_file_transfers.lock().remove(transfer_id);
    let _ = storage::set_room_item_status(&state.paths, item_id, RoomItemStatus::Failed);
    if let Some(transfer) = transfer {
        emit_event(
            state,
            &transfer,
            "failed",
            current_transferred(&transfer),
            0.0,
            average_speed(&transfer, current_transferred(&transfer)),
            None,
            Some(message),
        );
    }
}

fn finish_sender_terminal(
    state: &Arc<AppState>,
    transfer_id: &str,
    item_id: &str,
    status: &str,
    message: &str,
) {
    let item_status = if status == "cancelled" {
        RoomItemStatus::Cancelled
    } else {
        RoomItemStatus::Interrupted
    };
    let _ = storage::set_room_item_status(&state.paths, item_id, item_status);
    if status == "interrupted" {
        log_transfer_lifecycle(transfer_id, "transfer_interrupted", message);
        if message == PEER_DISCONNECTED_MESSAGE {
            log_transfer_lifecycle(transfer_id, "peer_disconnected", message);
        }
    }
    finish_transfer_locally(state, transfer_id, status, Some(message.to_string()));
}

fn finish_transfer_locally(
    state: &Arc<AppState>,
    transfer_id: &str,
    status: &str,
    message: Option<String>,
) {
    if let Some(transfer) = state.active_file_transfers.lock().remove(transfer_id) {
        let transferred_bytes = if status == "completed" {
            transfer.file_size
        } else {
            current_transferred(&transfer)
        };
        emit_event(
            state,
            &transfer,
            status,
            transferred_bytes,
            0.0,
            average_speed(&transfer, transferred_bytes),
            (status == "completed").then_some(0.0),
            message,
        );
    }
}

fn sender_room_terminal_state(
    state: &Arc<AppState>,
    room_id: &str,
) -> Option<(&'static str, String)> {
    let room = storage::get_room_by_id(&state.paths, room_id).ok()?;
    if room.status == RoomStatus::Burned {
        return Some(("burned", ROOM_BURNED_MESSAGE.into()));
    }
    None
}

fn map_reqwest_transfer_message(error: &reqwest::Error) -> String {
    if error.is_connect() {
        PEER_DISCONNECTED_MESSAGE.into()
    } else {
        TRANSFER_INTERRUPTED_MESSAGE.into()
    }
}

fn log_transfer_lifecycle(transfer_id: &str, event: &str, message: &str) {
    logging::write_transfer_line(&format!(
        "[pastey transfer][transfer_id={transfer_id}] event={event} message={message:?}"
    ));
}

fn log_late_event_ignored(transfer_id: &str, room_id: &str, event_kind: &str) {
    logging::write_transfer_line(&format!(
        "[pastey transfer][transfer_id={transfer_id}][room_id={room_id}] event=late_event_ignored kind={event_kind}"
    ));
}

async fn fail_receiver_transfer(state: &Arc<AppState>, transfer_id: &str, message: &str) {
    let transfer = state.active_file_transfers.lock().remove(transfer_id);
    if let Some(transfer) = transfer {
        record_terminal_transfer_reason(state, transfer_id, "receiver_interrupted");
        logging::write_transfer_line(&format!(
            "[pastey transfer][transfer_id={transfer_id}] event=active_transfer_removed reason=receiver_interrupted"
        ));
        logging::write_transfer_line(&format!(
            "[pastey transfer][transfer_id={transfer_id}] event=receiver_interrupted_transfer"
        ));
        if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
            let _ = tokio::fs::remove_file(part_path).await;
        }
        emit_event(
            state,
            &transfer,
            "failed",
            current_transferred(&transfer),
            0.0,
            average_speed(&transfer, current_transferred(&transfer)),
            None,
            Some(message.to_string()),
        );
    }
}

async fn abort_receiver_finalize_for_burn(
    state: &Arc<AppState>,
    transfer: &ActiveFileTransfer,
    cleanup_paths: &[PathBuf],
    transferred_bytes: u64,
) -> Response {
    for path in cleanup_paths {
        if let Err(error) = tokio::fs::remove_file(path).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                logging::write_error_line(&format!(
                    "[pastey cleanup][room_id={}] event=room_file_cleanup_error category=burn_finalize path_kind=transfer_file error={:?}",
                    transfer.room_id,
                    error.to_string()
                ));
            }
        }
    }
    log_transfer_lifecycle(
        &transfer.item_id,
        "burn_finalize_race_prevented",
        ROOM_BURNED_MESSAGE,
    );
    emit_event(
        state,
        transfer,
        "burned",
        transferred_bytes,
        0.0,
        average_speed(transfer, transferred_bytes),
        None,
        Some(ROOM_BURNED_MESSAGE.into()),
    );
    transfer_error(StatusCode::GONE, "room_burned", ROOM_BURNED_MESSAGE.into())
}

fn room_is_burned(state: &Arc<AppState>, room_id: &str) -> bool {
    storage::get_room_by_id(&state.paths, room_id)
        .map(|room| room.status == RoomStatus::Burned)
        .unwrap_or(true)
}

async fn remove_active_receiver_part_file(
    room_id: &str,
    category: &str,
    part_path: &Path,
) -> AppResult<()> {
    match tokio::fs::remove_file(part_path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            logging::write_error_line(&format!(
                "[pastey cleanup][room_id={room_id}] event=room_file_cleanup_error category={category} path={} error={:?}",
                part_path.display(),
                error.to_string()
            ));
            Err(AppError::InvalidInput(
                "Could not delete local room files. Check folder permissions.".into(),
            ))
        }
    }
}

fn emit_progress(
    state: &Arc<AppState>,
    transfer_id: &str,
    direction: &str,
    status: &str,
    transferred_bytes: u64,
    current_speed_bps: f64,
    average_speed_bps: f64,
    eta_seconds: Option<f64>,
    error_message: Option<String>,
) {
    let transfers = state.active_file_transfers.lock();
    if let Some(transfer) = transfers.get(transfer_id) {
        let event = clone_event_base(
            transfer,
            direction,
            status,
            transferred_bytes,
            current_speed_bps,
            average_speed_bps,
            eta_seconds,
            error_message,
        );
        let _ = state.app_handle.emit(TRANSFER_EVENT, event);
    } else {
        log_late_event_ignored(transfer_id, "", "progress");
    }
}

fn emit_event(
    state: &Arc<AppState>,
    transfer: &ActiveFileTransfer,
    status: &str,
    transferred_bytes: u64,
    current_speed_bps: f64,
    average_speed_bps: f64,
    eta_seconds: Option<f64>,
    error_message: Option<String>,
) {
    let direction = match transfer.kind {
        ActiveFileTransferKind::Sender => "outgoing",
        ActiveFileTransferKind::Receiver { .. } => "incoming",
    };
    let event = clone_event_base(
        transfer,
        direction,
        status,
        transferred_bytes,
        current_speed_bps,
        average_speed_bps,
        eta_seconds,
        error_message,
    );
    let _ = state.app_handle.emit(TRANSFER_EVENT, event);
}

fn record_receiver_chunk_timing(
    state: &Arc<AppState>,
    transfer_id: &str,
    decode_elapsed: Duration,
    decrypt_elapsed: Duration,
    write_elapsed: Duration,
    ui_emit_elapsed: Duration,
) {
    let mut transfers = state.active_file_transfers.lock();
    let Some(transfer) = transfers.get_mut(transfer_id) else {
        return;
    };
    let ActiveFileTransferKind::Receiver { timing, .. } = &mut transfer.kind else {
        return;
    };
    timing.chunks += 1;
    timing.decode_ms += decode_elapsed.as_millis();
    timing.decrypt_ms += decrypt_elapsed.as_millis();
    timing.write_ms += write_elapsed.as_millis();
    timing.ui_emit_ms += ui_emit_elapsed.as_millis();
}

fn clone_event_base(
    transfer: &ActiveFileTransfer,
    direction: &str,
    status: &str,
    transferred_bytes: u64,
    current_speed_bps: f64,
    average_speed_bps: f64,
    eta_seconds: Option<f64>,
    error_message: Option<String>,
) -> FileTransferProgressEvent {
    FileTransferProgressEvent {
        transfer_id: transfer.item_id.clone(),
        room_id: transfer.room_id.clone(),
        item_id: transfer.item_id.clone(),
        direction: direction.to_string(),
        file_name: transfer.file_name.clone(),
        file_size: transfer.file_size,
        chunk_size: transfer.chunk_size,
        total_chunks: transfer.total_chunks,
        transferred_bytes,
        status: status.to_string(),
        current_speed_bps,
        average_speed_bps,
        eta_seconds,
        error_message,
    }
}

fn current_transferred(transfer: &ActiveFileTransfer) -> u64 {
    match &transfer.kind {
        ActiveFileTransferKind::Sender => transfer.last_report_bytes,
        ActiveFileTransferKind::Receiver {
            transferred_bytes, ..
        } => *transferred_bytes,
    }
}

fn average_speed(transfer: &ActiveFileTransfer, transferred: u64) -> f64 {
    transferred as f64 / transfer.started_at.elapsed().as_secs_f64().max(0.001)
}

fn eta_seconds(file_size: u64, transferred: u64, current_speed_bps: f64) -> Option<f64> {
    if current_speed_bps <= 0.0 || transferred >= file_size {
        return None;
    }
    Some((file_size - transferred) as f64 / current_speed_bps)
}

fn total_chunks_for(file_size: u64, chunk_size: u64) -> u64 {
    chunk_count(file_size, chunk_size as usize)
}

fn chunk_count(file_size: u64, chunk_size: usize) -> u64 {
    if chunk_size == 0 {
        return 0;
    }
    file_size.div_ceil(chunk_size as u64)
}

fn json_base64_estimated_chunk_len(
    chunk_index: u64,
    plaintext_size: usize,
    ciphertext_len: usize,
    is_final: bool,
) -> usize {
    let nonce_len = STANDARD.encode([0u8; BINARY_CHUNK_NONCE_LEN]).len();
    let ciphertext_len = ciphertext_len.div_ceil(3) * 4;
    format!(
        r#"{{"chunk_index":{chunk_index},"nonce":"","ciphertext":"","plaintext_size":{plaintext_size},"is_final":{is_final}}}"#
    )
    .len()
        + nonce_len
        + ciphertext_len
}

fn verify_finalize_metadata(
    received_bytes: u64,
    file_size: u64,
    received_chunks: u64,
    total_chunks: u64,
) -> Result<(), (&'static str, &'static str)> {
    if received_bytes != file_size {
        return Err(("size_mismatch", "Received file size mismatch"));
    }
    if received_chunks != total_chunks {
        return Err(("metadata_mismatch", "Transfer metadata mismatch"));
    }
    Ok(())
}

#[cfg(test)]
fn sender_chunk_count_for(file_size: u64, chunk_size: u64) -> u64 {
    sender_chunk_plaintext_sizes(file_size, chunk_size as usize).len() as u64
}

#[cfg(test)]
fn sender_chunk_plaintext_sizes(file_size: u64, chunk_size: usize) -> Vec<u64> {
    if file_size == 0 || chunk_size == 0 {
        return Vec::new();
    }

    let mut remaining = file_size;
    let mut sizes = Vec::new();
    while remaining > 0 {
        let next = remaining.min(chunk_size as u64);
        sizes.push(next);
        remaining -= next;
    }
    sizes
}

fn cancel_token_clone(transfer: &ActiveFileTransfer) -> CancellationToken {
    transfer.cancel_token.clone()
}

async fn write_receiver_chunk(
    part_path: &Path,
    plaintext: &[u8],
    write_offset: u64,
) -> Result<(), ReceiverWriteFailure> {
    let Some(parent) = part_path.parent() else {
        return Err(receiver_write_failure(
            part_path,
            "write_failed",
            "Receiver failed to write chunk",
            StatusCode::INTERNAL_SERVER_ERROR,
            "part_path_missing_parent".to_string(),
        ));
    };

    if let Err(error) = tokio::fs::create_dir_all(parent).await {
        let (code, message, status) = map_receiver_write_error(&error);
        return Err(receiver_write_failure(
            part_path,
            code,
            message,
            status,
            format!("create_parent_failed: {error}"),
        ));
    }

    let file_exists = tokio::fs::try_exists(part_path).await.unwrap_or(false);
    if !file_exists && write_offset > 0 {
        return Err(receiver_write_failure(
            part_path,
            "temp_file_disappeared",
            "Receiver temporary file disappeared",
            StatusCode::INTERNAL_SERVER_ERROR,
            "temp_file_disappeared".to_string(),
        ));
    }

    let mut file = OpenOptions::new()
        .create(write_offset == 0)
        .write(true)
        .open(part_path)
        .await
        .map_err(|error| {
            let (code, message, status) = map_receiver_write_error(&error);
            receiver_write_failure(
                part_path,
                code,
                message,
                status,
                format!("open_failed: {error}"),
            )
        })?;
    file.seek(SeekFrom::Start(write_offset))
        .await
        .map_err(|error| {
            let (code, message, status) = map_receiver_write_error(&error);
            receiver_write_failure(
                part_path,
                code,
                message,
                status,
                format!("seek_failed: {error}"),
            )
        })?;
    file.write_all(plaintext).await.map_err(|error| {
        let (code, message, status) = map_receiver_write_error(&error);
        receiver_write_failure(
            part_path,
            code,
            message,
            status,
            format!("write_failed: {error}"),
        )
    })
}

fn receiver_write_failure(
    part_path: &Path,
    code: &'static str,
    message: &'static str,
    status: StatusCode,
    cause: String,
) -> ReceiverWriteFailure {
    let parent_exists = part_path.parent().is_some_and(Path::exists);
    let file_exists = part_path.exists();
    ReceiverWriteFailure {
        code,
        message,
        status,
        cause,
        parent_exists,
        file_exists,
    }
}

fn map_receiver_write_error(error: &std::io::Error) -> (&'static str, &'static str, StatusCode) {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => (
            "receiver_cannot_write",
            "Receiver cannot write to inbox",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::WriteZero => (
            "not_enough_disk_space",
            "Not enough disk space on receiver",
            StatusCode::INSUFFICIENT_STORAGE,
        ),
        std::io::ErrorKind::NotFound => (
            "temp_file_disappeared",
            "Receiver temporary file disappeared",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        _ => (
            "write_failed",
            "Receiver failed to write chunk",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

async fn notify_transfer_cancel(client: &reqwest::Client, base_url: &str, transfer_id: &str) {
    let _ = client
        .post(format!("{base_url}/transfers/{transfer_id}/cancel"))
        .send()
        .await;
}

async fn notify_transfer_failed(
    client: &reqwest::Client,
    base_url: &str,
    transfer_id: &str,
    message: &str,
) {
    let _ = client
        .post(format!("{base_url}/transfers/{transfer_id}/cancel"))
        .json(&serde_json::json!({
            "status": "failed",
            "message": message,
        }))
        .send()
        .await;
}

async fn notify_peer_transfer_terminal_reason(
    state: &Arc<AppState>,
    room_id: &str,
    transfer_id: &str,
    reason: &str,
) {
    let Ok(room) = storage::get_room_by_id(&state.paths, room_id) else {
        return;
    };
    let (Some(peer_host), Some(peer_port)) = (room.peer_host, room.peer_port) else {
        return;
    };
    let base_url = format!("http://{peer_host}:{peer_port}/rooms/{room_id}");
    let _ = reqwest::Client::new()
        .post(format!("{base_url}/transfers/{transfer_id}/cancel"))
        .json(&serde_json::json!({
            "status": "failed",
            "message": terminal_reason_message(reason),
            "reason": reason,
        }))
        .send()
        .await;
}

async fn cancel_transfer_with_reason(
    state: Arc<AppState>,
    transfer_id: &str,
    receiver_reason: Option<&str>,
) -> AppResult<bool> {
    let Some(reason) = receiver_reason else {
        return cancel_transfer(state, transfer_id).await;
    };

    let removed = state.active_file_transfers.lock().remove(transfer_id);
    let Some(transfer) = removed else {
        return Ok(false);
    };
    transfer.cancel_token.cancel();
    if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
        record_terminal_transfer_reason(&state, transfer_id, reason);
        logging::write_transfer_line(&format!(
            "[pastey transfer][transfer_id={transfer_id}] event=active_transfer_removed reason={reason}"
        ));
        remove_active_receiver_part_file(&transfer.room_id, "active_part", part_path).await?;
        notify_peer_transfer_terminal_reason(&state, &transfer.room_id, transfer_id, reason).await;
    } else if let Ok(room) = storage::get_room_by_id(&state.paths, &transfer.room_id) {
        if let (Some(peer_host), Some(peer_port)) = (room.peer_host, room.peer_port) {
            let client = reqwest::Client::new();
            let base_url = format!("http://{peer_host}:{peer_port}/rooms/{}", transfer.room_id);
            notify_transfer_cancel(&client, &base_url, transfer_id).await;
        }
    }
    let _ =
        storage::set_room_item_status(&state.paths, &transfer.item_id, RoomItemStatus::Cancelled);
    emit_event(
        &state,
        &transfer,
        "cancelled",
        current_transferred(&transfer),
        0.0,
        average_speed(&transfer, current_transferred(&transfer)),
        None,
        Some(terminal_reason_message(reason).into()),
    );
    Ok(true)
}

fn has_enough_disk_space(path: &Path, file_size: u64) -> bool {
    let required = file_size.saturating_add(DISK_SPACE_MARGIN_BYTES);
    match available_disk_space(path) {
        Some(available) => available >= required,
        None => true,
    }
}

fn available_disk_space(path: &Path) -> Option<u64> {
    #[cfg(target_family = "unix")]
    {
        let output = std::process::Command::new("df")
            .arg("-Pk")
            .arg(path)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let line = text.lines().nth(1)?;
        let available_kb = line.split_whitespace().nth(3)?.parse::<u64>().ok()?;
        Some(available_kb * 1024)
    }

    #[cfg(target_family = "windows")]
    {
        let root = path
            .components()
            .next()?
            .as_os_str()
            .to_string_lossy()
            .to_string();
        let output = std::process::Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(format!(
                "(Get-PSDrive -Name '{}').Free",
                root.trim_end_matches(":\\").trim_end_matches(':')
            ))
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        text.trim().parse::<u64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::ReadBuf;

    struct ShortAsyncReader {
        data: Vec<u8>,
        position: usize,
        max_read: usize,
    }

    impl ShortAsyncReader {
        fn new(data: Vec<u8>, max_read: usize) -> Self {
            Self {
                data,
                position: 0,
                max_read,
            }
        }
    }

    impl AsyncRead for ShortAsyncReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.position >= self.data.len() || buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }

            let start = self.position;
            let read_len = (self.data.len() - start)
                .min(self.max_read)
                .min(buf.remaining());
            let end = start + read_len;
            buf.put_slice(&self.data[start..end]);
            self.position = end;

            Poll::Ready(Ok(()))
        }
    }

    fn details(status: StatusCode, code: Option<&str>, message: &str) -> ResponseErrorDetails {
        ResponseErrorDetails {
            status,
            code: code.map(ToString::to_string),
            message: message.to_string(),
            body_text: String::new(),
        }
    }

    #[test]
    fn response_error_mapping_uses_specific_transfer_messages() {
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::PAYLOAD_TOO_LARGE,
                Some("chunk_too_large"),
                "ignored"
            )),
            "Chunk too large for receiver"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::BAD_REQUEST,
                Some("invalid_chunk_payload"),
                "ignored"
            )),
            "Invalid chunk payload"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::BAD_REQUEST,
                Some("invalid_chunk_encoding"),
                "ignored"
            )),
            "Invalid chunk encoding"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::NOT_FOUND,
                Some("transfer_missing"),
                "ignored"
            )),
            "Transfer session not found on receiver."
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::CONFLICT,
                Some("receiver_cancelled"),
                "ignored"
            )),
            "Receiver cancelled transfer"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::NOT_FOUND,
                Some("room_not_found"),
                "ignored"
            )),
            "Room not found on receiver."
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::BAD_REQUEST,
                Some("integrity_failed"),
                "ignored"
            )),
            "Chunk integrity check failed"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::BAD_REQUEST,
                Some("metadata_mismatch"),
                "ignored"
            )),
            "Transfer metadata mismatch"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::BAD_REQUEST,
                Some("size_mismatch"),
                "ignored"
            )),
            "Received file size mismatch"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::BAD_REQUEST,
                Some("invalid_chunk_order"),
                "ignored"
            )),
            "Unexpected chunk index"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::INTERNAL_SERVER_ERROR,
                Some("temp_file_disappeared"),
                "ignored"
            )),
            "Receiver temporary file disappeared"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::INTERNAL_SERVER_ERROR,
                Some("receiver_cannot_write"),
                "ignored"
            )),
            "Receiver cannot write to inbox"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::INSUFFICIENT_STORAGE,
                Some("not_enough_disk_space"),
                "ignored"
            )),
            "Not enough disk space on receiver"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::INTERNAL_SERVER_ERROR,
                Some("write_failed"),
                "ignored"
            )),
            "Receiver failed to write chunk"
        );
        assert_eq!(
            map_response_error_message(&details(StatusCode::REQUEST_TIMEOUT, None, "timed out")),
            "timed out"
        );
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::CONFLICT,
                Some("cancelled"),
                "ignored"
            )),
            TRANSFER_CANCELLED_MESSAGE
        );
    }

    #[test]
    fn terminal_reason_messages_are_specific() {
        assert_eq!(
            terminal_reason_message("receiver_cancelled"),
            "Receiver cancelled transfer"
        );
        assert_eq!(
            terminal_reason_message("receiver_burned_room"),
            "Peer burned the room"
        );
        assert_eq!(
            terminal_reason_message("receiver_left_room"),
            "Peer left the room"
        );
        assert_eq!(
            terminal_reason_message("receiver_interrupted"),
            "Receiver stopped receiving"
        );
    }

    #[test]
    fn four_mib_chunk_json_body_stays_below_receiver_limit() {
        let ciphertext = vec![0u8; DEFAULT_CHUNK_SIZE_BYTES as usize + 16];
        let upload = ChunkUploadRequest {
            chunk_index: 0,
            nonce: STANDARD.encode([0u8; 12]),
            ciphertext: STANDARD.encode(ciphertext),
            plaintext_size: DEFAULT_CHUNK_SIZE_BYTES,
            is_final: false,
        };
        let body = serde_json::to_vec(&upload).expect("chunk upload serializes");

        assert!(body.len() < MAX_CHUNK_BODY_BYTES);
    }

    #[test]
    fn four_mib_binary_chunk_frame_stays_below_receiver_limit() {
        let ciphertext_len = DEFAULT_CHUNK_SIZE_BYTES as usize + 16;
        let frame_len = crate::chunk_frame::BINARY_CHUNK_HEADER_LEN + ciphertext_len;

        assert!(frame_len < MAX_CHUNK_BODY_BYTES);
    }

    #[test]
    fn binary_frame_payload_is_smaller_than_json_base64_estimate() {
        let ciphertext_len = DEFAULT_CHUNK_SIZE_BYTES as usize + 16;
        let binary_len = crate::chunk_frame::BINARY_CHUNK_HEADER_LEN + ciphertext_len;
        let json_len = json_base64_estimated_chunk_len(
            0,
            DEFAULT_CHUNK_SIZE_BYTES as usize,
            ciphertext_len,
            false,
        );

        assert!(binary_len < json_len);
        assert_eq!(json_len - binary_len, 1_398_174);
    }

    #[test]
    fn receiver_accepts_binary_v1_chunk_payload() {
        let frame = BinaryChunkFrame {
            chunk_index: 3,
            nonce: [5u8; BINARY_CHUNK_NONCE_LEN],
            ciphertext: vec![1, 2, 3, 4],
            plaintext_size: 4,
            is_final: true,
        };
        let body = encode_binary_chunk_frame(&frame).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/octet-stream".parse().unwrap(),
        );
        headers.insert(
            CHUNK_PROTOCOL_HEADER,
            CHUNK_PROTOCOL_BINARY_V1.parse().unwrap(),
        );

        let decoded = decode_received_chunk_upload(&headers, &body).unwrap();

        assert_eq!(decoded.protocol, ChunkProtocol::BinaryV1);
        assert_eq!(decoded.chunk_index, frame.chunk_index);
        assert_eq!(decoded.nonce, frame.nonce);
        assert_eq!(decoded.ciphertext, frame.ciphertext);
        assert_eq!(decoded.plaintext_size, frame.plaintext_size as u64);
        assert!(decoded.is_final);
    }

    #[test]
    fn receiver_still_accepts_legacy_json_chunk_upload() {
        let upload = ChunkUploadRequest {
            chunk_index: 2,
            nonce: STANDARD.encode([8u8; BINARY_CHUNK_NONCE_LEN]),
            ciphertext: STANDARD.encode([1u8, 2, 3, 4]),
            plaintext_size: 4,
            is_final: false,
        };
        let body = serde_json::to_vec(&upload).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());

        let decoded = decode_received_chunk_upload(&headers, &body).unwrap();

        assert_eq!(decoded.protocol, ChunkProtocol::JsonV1);
        assert_eq!(decoded.chunk_index, upload.chunk_index);
        assert_eq!(decoded.nonce, [8u8; BINARY_CHUNK_NONCE_LEN]);
        assert_eq!(decoded.ciphertext, vec![1, 2, 3, 4]);
        assert_eq!(decoded.plaintext_size, upload.plaintext_size);
    }

    #[test]
    fn json_protocol_header_keeps_fallback_path_working() {
        let upload = ChunkUploadRequest {
            chunk_index: 0,
            nonce: STANDARD.encode([1u8; BINARY_CHUNK_NONCE_LEN]),
            ciphertext: STANDARD.encode([9u8; 16]),
            plaintext_size: 16,
            is_final: true,
        };
        let body = serde_json::to_vec(&upload).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            CHUNK_PROTOCOL_HEADER,
            CHUNK_PROTOCOL_JSON_V1.parse().unwrap(),
        );

        let decoded = decode_received_chunk_upload(&headers, &body).unwrap();

        assert_eq!(decoded.protocol, ChunkProtocol::JsonV1);
        assert_eq!(decoded.ciphertext.len(), 16);
        assert!(decoded.is_final);
    }

    #[test]
    fn start_response_selects_binary_when_receiver_advertises_support() {
        let body = serde_json::to_string(&file_transfer_start_response()).unwrap();

        assert_eq!(
            selected_chunk_protocol_from_start_response(&body),
            ChunkProtocol::BinaryV1
        );
    }

    #[test]
    fn start_response_uses_json_when_capability_is_unknown() {
        assert_eq!(
            selected_chunk_protocol_from_start_response(r#"{"ok":true}"#),
            ChunkProtocol::JsonV1
        );
        assert_eq!(
            selected_chunk_protocol_from_start_response(""),
            ChunkProtocol::JsonV1
        );
    }

    #[test]
    fn transfer_tuning_log_includes_effective_window_and_protocol() {
        let tuning = transfer_tuning::effective_transfer_tuning(Some(4), true, Some("8"));

        let line = sender_transfer_tuning_log_line(
            "transfer-1",
            "room-1",
            tuning,
            DEFAULT_CHUNK_SIZE_BYTES as usize,
            ChunkProtocol::BinaryV1,
        );

        assert!(line.contains("event=transfer_tuning"));
        assert!(line.contains("effective_window_size=8"));
        assert!(line.contains("chunk_size=4194304"));
        assert!(line.contains("override_source=env"));
        assert!(line.contains("transfer_protocol=binary-v1"));
    }

    #[test]
    fn legacy_protocol_log_label_is_user_facing() {
        assert_eq!(ChunkProtocol::JsonV1.log_label(), "json-legacy");
    }

    #[test]
    fn unsupported_protocol_maps_to_unsupported_chunk_protocol() {
        let mut headers = HeaderMap::new();
        headers.insert(CHUNK_PROTOCOL_HEADER, "binary-v2".parse().unwrap());

        let error = decode_received_chunk_upload(&headers, b"ignored").unwrap_err();

        assert_eq!(error.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(error.code, "unsupported_chunk_protocol");
        assert_eq!(error.message, "Unsupported chunk protocol");
    }

    #[test]
    fn body_limit_status_still_maps_to_chunk_too_large() {
        assert_eq!(
            map_response_error_message(&details(
                StatusCode::PAYLOAD_TOO_LARGE,
                None,
                "body limit exceeded"
            )),
            "Chunk too large for receiver"
        );
    }

    #[test]
    fn corrupted_ciphertext_maps_to_integrity_failure_after_decrypt_attempt() {
        let key = [3u8; 32];
        let (mut ciphertext, nonce) = crypto::encrypt_bytes(b"hello", &key).unwrap();
        ciphertext[0] ^= 0xff;

        assert!(crypto::decrypt_bytes(&ciphertext, &key, &nonce).is_err());
        assert_eq!(
            receiver_failure_log_message("integrity_failed"),
            "Chunk integrity check failed"
        );
    }

    #[test]
    fn sixty_mb_file_total_chunks_matches_sender_read_loop_count() {
        let file_size = 60_755_281;
        let chunk_size = DEFAULT_CHUNK_SIZE_BYTES as usize;
        let total_chunks = chunk_count(file_size, chunk_size);
        let chunk_sizes = sender_chunk_plaintext_sizes(file_size, chunk_size);

        assert_eq!(chunk_size, 4 * 1024 * 1024);
        assert_eq!(total_chunks, 15);
        assert_eq!(
            sender_chunk_count_for(file_size, chunk_size as u64),
            total_chunks
        );
        assert_eq!(chunk_sizes.len() as u64, total_chunks);
        assert_eq!(chunk_sizes.iter().sum::<u64>(), file_size);
        assert!(chunk_sizes[..chunk_sizes.len() - 1]
            .iter()
            .all(|size| *size == chunk_size as u64));
        assert_eq!(*chunk_sizes.last().unwrap(), 2_035_025);
        assert!(chunk_sizes.iter().all(|size| *size != 2 * 1024 * 1024));
    }

    #[test]
    fn sender_default_chunk_size_is_not_two_mib() {
        let two_mib = 2 * 1024 * 1024;
        let chunk_size = DEFAULT_CHUNK_SIZE_BYTES as usize;
        let chunk_sizes = sender_chunk_plaintext_sizes((chunk_size * 2 + 1) as u64, chunk_size);

        assert_eq!(chunk_size, 4 * 1024 * 1024);
        assert_ne!(chunk_size, two_mib);
        assert_eq!(chunk_sizes, vec![chunk_size as u64, chunk_size as u64, 1]);
    }

    #[test]
    fn two_mib_sender_chunks_would_not_match_four_mib_metadata() {
        let file_size = 60_755_281;
        let metadata_total_chunks = total_chunks_for(file_size, DEFAULT_CHUNK_SIZE_BYTES);
        let two_mib_actual_chunks = sender_chunk_count_for(file_size, 2 * 1024 * 1024);

        assert_eq!(metadata_total_chunks, 15);
        assert_eq!(two_mib_actual_chunks, 29);
        assert_ne!(two_mib_actual_chunks, metadata_total_chunks);
    }

    #[tokio::test]
    async fn read_next_chunk_fills_buffer_across_two_mib_short_reads() {
        let chunk_size = DEFAULT_CHUNK_SIZE_BYTES as usize;
        let mut reader = ShortAsyncReader::new(vec![7u8; chunk_size + 123], 2 * 1024 * 1024);
        let mut buffer = Vec::new();

        let first = read_next_chunk(&mut reader, &mut buffer, chunk_size)
            .await
            .unwrap();
        let second = read_next_chunk(&mut reader, &mut buffer, chunk_size)
            .await
            .unwrap();
        let third = read_next_chunk(&mut reader, &mut buffer, chunk_size)
            .await
            .unwrap();

        assert_eq!(first, chunk_size);
        assert_eq!(second, 123);
        assert_eq!(third, 0);
        assert_eq!(buffer.len(), chunk_size);
    }

    #[test]
    fn finalize_metadata_verification_distinguishes_size_and_chunk_mismatch() {
        assert_eq!(
            verify_finalize_metadata(60_109_151, 60_109_151, 15, 15),
            Ok(())
        );
        assert_eq!(
            verify_finalize_metadata(60_109_150, 60_109_151, 15, 15),
            Err(("size_mismatch", "Received file size mismatch"))
        );
        assert_eq!(
            verify_finalize_metadata(60_109_151, 60_109_151, 29, 15),
            Err(("metadata_mismatch", "Transfer metadata mismatch"))
        );
    }

    #[tokio::test]
    async fn writing_chunk_zero_creates_missing_parent_and_part_file() {
        let dir = std::env::temp_dir().join(format!("pastey_chunk_write_{}", uuid::Uuid::new_v4()));
        let part_path = dir.join(".pastey-parts").join("transfer.part");

        write_receiver_chunk(&part_path, b"hello", 0).await.unwrap();

        assert_eq!(tokio::fs::read(&part_path).await.unwrap(), b"hello");
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn writing_pipelined_chunks_uses_file_offsets() {
        let dir =
            std::env::temp_dir().join(format!("pastey_chunk_offset_{}", uuid::Uuid::new_v4()));
        let part_path = dir.join(".pastey-parts").join("transfer.part");
        tokio::fs::create_dir_all(part_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::File::create(&part_path).await.unwrap();

        write_receiver_chunk(&part_path, b"world", 5).await.unwrap();
        write_receiver_chunk(&part_path, b"hello", 0).await.unwrap();
        write_receiver_chunk(&part_path, b"world", 5).await.unwrap();

        assert_eq!(tokio::fs::read(&part_path).await.unwrap(), b"helloworld");
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn missing_part_after_partial_write_is_reported_as_temp_disappeared() {
        let dir =
            std::env::temp_dir().join(format!("pastey_chunk_missing_{}", uuid::Uuid::new_v4()));
        let part_path = dir.join(".pastey-parts").join("transfer.part");

        let error = write_receiver_chunk(&part_path, b"later", 5)
            .await
            .unwrap_err();

        assert_eq!(error.code, "temp_file_disappeared");
        assert_eq!(error.message, "Receiver temporary file disappeared");
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}

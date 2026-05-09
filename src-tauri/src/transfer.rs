use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tokio::{
    fs::OpenOptions,
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;

use crate::{
    config, crypto, discovery,
    error::{AppError, AppResult},
    models::{
        FileTransferFinishRequest, FileTransferProgressEvent, FileTransferStartRequest,
        JoinRoomRequest, JoinRoomResponse, PayloadType, RoomItemStatus, RoomItemUpload, RoomStatus,
        TransferErrorResponse,
    },
    storage, ActiveRoomServer, AppState,
};

pub const DEFAULT_CHUNK_SIZE_BYTES: u64 = 4 * 1024 * 1024;
const DISK_SPACE_MARGIN_BYTES: u64 = 512 * 1024 * 1024;
const TRANSFER_EVENT: &str = "pastey://transfer-progress";
const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CHUNK_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_CHUNK_PLAINTEXT_BYTES: u64 = MAX_CHUNK_BODY_BYTES as u64 - 1024;
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
struct ChunkAckResponse {
    ok: bool,
    chunk_index: u64,
    transferred_bytes: u64,
}

#[derive(Debug)]
struct ChunkSendFailure {
    message: String,
    kind: ChunkSendFailureKind,
    retryable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChunkSendFailureKind {
    Cancelled,
    ChunkTooLarge,
    HttpStatus,
    InvalidAck,
    PeerLeft,
    Timeout,
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
    if room.expires_at <= storage::now_ts() {
        return Err(AppError::InvalidInput("Room expired.".into()));
    }

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
            post(receive_file_chunk_handler),
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
        .layer(DefaultBodyLimit::max(MAX_CHUNK_BODY_BYTES))
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

    if room.status != RoomStatus::Burned && room.status != RoomStatus::Expired {
        storage::set_room_status(&state.paths, room_id, RoomStatus::Active)?;
    }

    discovery::ensure_service(state).await?;
    Ok(port)
}

pub async fn stop_room_server(state: Arc<AppState>, room_id: &str) -> AppResult<bool> {
    cancel_room_transfers(state.clone(), room_id, "Room expired.", false).await;
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
        return Err(AppError::Network("Peer left the room.".into()));
    }

    response.json().await.map_err(Into::into)
}

pub async fn send_room_item(state: Arc<AppState>, room_id: &str, item_id: &str) -> AppResult<()> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    let peer_host = room
        .peer_host
        .clone()
        .ok_or_else(|| AppError::InvalidInput("Peer left the room.".into()))?;
    let peer_port = room
        .peer_port
        .ok_or_else(|| AppError::InvalidInput("Peer left the room.".into()))?;
    let peer_transport_public_key = room
        .peer_transport_public_key
        .clone()
        .ok_or_else(|| AppError::InvalidInput("Peer left the room.".into()))?;

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
    if room.expires_at <= storage::now_ts()
        || matches!(room.status, RoomStatus::Burned | RoomStatus::Expired)
    {
        return Err(AppError::InvalidInput("Room expired.".into()));
    }
    let peer_host = room
        .peer_host
        .clone()
        .ok_or_else(|| AppError::InvalidInput("Peer left the room.".into()))?;
    let peer_port = room
        .peer_port
        .ok_or_else(|| AppError::InvalidInput("Peer left the room.".into()))?;
    let peer_transport_public_key = room
        .peer_transport_public_key
        .clone()
        .ok_or_else(|| AppError::InvalidInput("Peer left the room.".into()))?;

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
    let chunk_size = DEFAULT_CHUNK_SIZE_BYTES;
    let total_chunks = file_size.div_ceil(chunk_size);
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
        chunk_size,
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
    let start = FileTransferStartRequest {
        transfer_id: transfer_id.clone(),
        item_id: item.id.clone(),
        display_name: item.display_name.clone(),
        mime_type: item.mime_type.clone(),
        size_bytes: file_size,
        chunk_size,
        total_chunks,
        created_at: item.created_at,
        wrapped_session_key,
        transport_nonce,
        sender_public_key,
    };

    let start_response = client
        .post(format!("{base_url}/transfers/start"))
        .json(&start)
        .send()
        .await;
    let start_response = match start_response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let message = response_error_message(response).await;
            fail_transfer(&state, &transfer_id, item_id, message.clone());
            return Err(AppError::Network(message));
        }
        Err(error) => {
            fail_transfer(
                &state,
                &transfer_id,
                item_id,
                "Network connection lost.".into(),
            );
            return Err(AppError::Http(error));
        }
    };
    drop(start_response);

    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|_| AppError::InvalidInput("Could not read selected file.".into()))?;
    let mut buffer = vec![0u8; chunk_size as usize];
    let started_at = Instant::now();
    let mut last_report_at = started_at;
    let mut last_report_bytes = 0u64;
    let mut chunk_index = 0u64;

    loop {
        if cancel_token.is_cancelled() {
            notify_transfer_cancel(&client, &base_url, &transfer_id).await;
            storage::set_room_item_status(&state.paths, item_id, RoomItemStatus::Cancelled)?;
            finish_transfer_locally(
                &state,
                &transfer_id,
                "cancelled",
                Some("Transfer cancelled.".into()),
            );
            return Err(AppError::InvalidInput("Transfer cancelled.".into()));
        }

        let bytes_read = file
            .read(&mut buffer)
            .await
            .map_err(|_| AppError::InvalidInput("Could not read selected file.".into()))?;
        if bytes_read == 0 {
            break;
        }

        let (encrypted_bytes, nonce) = crypto::encrypt_bytes(&buffer[..bytes_read], &payload_key)?;
        let ack = match send_chunk_with_retry(
            &client,
            &base_url,
            &transfer_id,
            chunk_index,
            bytes_read,
            &nonce,
            &encrypted_bytes,
            &cancel_token,
        )
        .await
        {
            Ok(ack) => ack,
            Err(error) => {
                if error.kind == ChunkSendFailureKind::Cancelled || cancel_token.is_cancelled() {
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
                        Some("Transfer cancelled.".into()),
                    );
                    return Err(AppError::InvalidInput("Transfer cancelled.".into()));
                }

                notify_transfer_cancel(&client, &base_url, &transfer_id).await;
                fail_transfer(&state, &transfer_id, item_id, error.message.clone());
                return if error.kind == ChunkSendFailureKind::Timeout {
                    Err(AppError::Timeout(error.message))
                } else {
                    Err(AppError::Network(error.message))
                };
            }
        };

        chunk_index += 1;
        let transferred = ack.transferred_bytes;
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
        apply_rate_limit(&state, transferred, started_at).await;
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
            let message = response_error_message(response).await;
            fail_transfer(&state, &transfer_id, item_id, message.clone());
            Err(AppError::Network(message))
        }
        Err(error) => {
            fail_transfer(
                &state,
                &transfer_id,
                item_id,
                "Network connection lost.".into(),
            );
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

async fn send_chunk_with_retry(
    client: &reqwest::Client,
    base_url: &str,
    transfer_id: &str,
    chunk_index: u64,
    plaintext_size: usize,
    nonce: &[u8; 12],
    encrypted_bytes: &[u8],
    cancel_token: &CancellationToken,
) -> Result<ChunkAckResponse, ChunkSendFailure> {
    for retry_count in 0..=CHUNK_RETRY_BACKOFFS.len() {
        if cancel_token.is_cancelled() {
            return Err(ChunkSendFailure {
                message: "Transfer cancelled.".into(),
                kind: ChunkSendFailureKind::Cancelled,
                retryable: false,
            });
        }

        let attempt_started_at = Instant::now();
        let result = send_chunk_once(
            client,
            base_url,
            transfer_id,
            chunk_index,
            plaintext_size,
            nonce,
            encrypted_bytes,
        )
        .await;
        let elapsed = attempt_started_at.elapsed();

        match result {
            Ok(ack) => {
                dev_log_chunk_attempt(transfer_id, chunk_index, retry_count, "ok", elapsed);
                return Ok(ack);
            }
            Err(error) => {
                dev_log_chunk_attempt(
                    transfer_id,
                    chunk_index,
                    retry_count,
                    error.kind.as_str(),
                    elapsed,
                );
                if cancel_token.is_cancelled() {
                    return Err(ChunkSendFailure {
                        message: "Transfer cancelled.".into(),
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
        message: "Transfer timed out.".into(),
        kind: ChunkSendFailureKind::Timeout,
        retryable: false,
    })
}

async fn send_chunk_once(
    client: &reqwest::Client,
    base_url: &str,
    transfer_id: &str,
    chunk_index: u64,
    plaintext_size: usize,
    nonce: &[u8; 12],
    encrypted_bytes: &[u8],
) -> Result<ChunkAckResponse, ChunkSendFailure> {
    let request_body_size = encrypted_bytes.len();
    let response = client
        .post(format!("{base_url}/transfers/{transfer_id}/chunks"))
        .timeout(CHUNK_REQUEST_TIMEOUT)
        .header("x-pastey-chunk-index", chunk_index.to_string())
        .header("x-pastey-plaintext-size", plaintext_size.to_string())
        .header("x-pastey-nonce", crypto::encode_nonce(nonce))
        .body(encrypted_bytes.to_vec())
        .send()
        .await
        .map_err(chunk_failure_from_reqwest)?;

    let status = response.status();
    dev_log_chunk_response(
        transfer_id,
        chunk_index,
        plaintext_size,
        request_body_size,
        status,
    );
    if !response.status().is_success() {
        return Err(chunk_failure_from_response(response).await);
    }

    let ack = response
        .json::<ChunkAckResponse>()
        .await
        .map_err(|_| ChunkSendFailure {
            message: "Network connection lost.".into(),
            kind: ChunkSendFailureKind::InvalidAck,
            retryable: true,
        })?;
    if !ack.ok || ack.chunk_index != chunk_index {
        return Err(ChunkSendFailure {
            message: "Network connection lost.".into(),
            kind: ChunkSendFailureKind::InvalidAck,
            retryable: true,
        });
    }

    Ok(ack)
}

fn chunk_failure_from_reqwest(error: reqwest::Error) -> ChunkSendFailure {
    if error.is_timeout() {
        return ChunkSendFailure {
            message: "Transfer timed out.".into(),
            kind: ChunkSendFailureKind::Timeout,
            retryable: true,
        };
    }
    if error.is_connect() {
        return ChunkSendFailure {
            message: "Network connection lost.".into(),
            kind: ChunkSendFailureKind::Unreachable,
            retryable: true,
        };
    }
    ChunkSendFailure {
        message: "Network connection lost.".into(),
        kind: ChunkSendFailureKind::Unreachable,
        retryable: true,
    }
}

async fn chunk_failure_from_response(response: reqwest::Response) -> ChunkSendFailure {
    let status = response.status();
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        return ChunkSendFailure {
            message: "Chunk too large for receiver.".into(),
            kind: ChunkSendFailureKind::ChunkTooLarge,
            retryable: false,
        };
    }
    if matches!(status, StatusCode::NOT_FOUND | StatusCode::GONE) {
        return ChunkSendFailure {
            message: "Peer left the room.".into(),
            kind: ChunkSendFailureKind::PeerLeft,
            retryable: false,
        };
    }

    let error = response.json::<TransferErrorResponse>().await.ok();
    if error
        .as_ref()
        .is_some_and(|error| error.code == "cancelled")
    {
        return ChunkSendFailure {
            message: "Transfer cancelled.".into(),
            kind: ChunkSendFailureKind::Cancelled,
            retryable: false,
        };
    }

    if error.as_ref().is_some_and(|error| {
        matches!(
            error.code.as_str(),
            "integrity_failed"
                | "invalid_chunk"
                | "invalid_chunk_order"
                | "invalid_payload"
                | "invalid_transfer"
                | "write_failed"
        )
    }) {
        return ChunkSendFailure {
            message: error
                .map(|error| error.message)
                .unwrap_or_else(|| "File transfer failed.".to_string()),
            kind: ChunkSendFailureKind::HttpStatus,
            retryable: false,
        };
    }

    ChunkSendFailure {
        message: error
            .map(|error| error.message)
            .unwrap_or_else(|| "Network connection lost.".to_string()),
        kind: ChunkSendFailureKind::HttpStatus,
        retryable: status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT,
    }
}

fn dev_log_chunk_response(
    transfer_id: &str,
    chunk_index: u64,
    chunk_bytes: usize,
    request_body_size: usize,
    status: StatusCode,
) {
    #[cfg(debug_assertions)]
    eprintln!(
        "transfer chunk response transfer_id={transfer_id} chunk_index={chunk_index} chunk_bytes={chunk_bytes} request_body_size={request_body_size} response_status={status}"
    );

    #[cfg(not(debug_assertions))]
    let _ = (
        transfer_id,
        chunk_index,
        chunk_bytes,
        request_body_size,
        status,
    );
}

fn dev_log_receiver_chunk_received(
    transfer_id: &str,
    chunk_index: u64,
    chunk_bytes: u64,
    request_body_size: usize,
) {
    #[cfg(debug_assertions)]
    eprintln!(
        "transfer receiver chunk received transfer_id={transfer_id} chunk_index={chunk_index} chunk_bytes={chunk_bytes} request_body_size={request_body_size}"
    );

    #[cfg(not(debug_assertions))]
    let _ = (transfer_id, chunk_index, chunk_bytes, request_body_size);
}

fn dev_log_receiver_chunk_write_success(
    transfer_id: &str,
    chunk_index: u64,
    chunk_bytes: u64,
    request_body_size: usize,
) {
    #[cfg(debug_assertions)]
    eprintln!(
        "transfer receiver chunk write transfer_id={transfer_id} chunk_index={chunk_index} chunk_bytes={chunk_bytes} request_body_size={request_body_size} response_status={} result=success",
        StatusCode::OK
    );

    #[cfg(not(debug_assertions))]
    let _ = (transfer_id, chunk_index, chunk_bytes, request_body_size);
}

fn dev_log_receiver_chunk_failure(
    transfer_id: &str,
    chunk_index: u64,
    chunk_bytes: u64,
    request_body_size: usize,
    response_status: StatusCode,
    error_cause: &str,
) {
    #[cfg(debug_assertions)]
    eprintln!(
        "transfer receiver chunk failure transfer_id={transfer_id} chunk_index={chunk_index} chunk_bytes={chunk_bytes} request_body_size={request_body_size} response_status={response_status} error_cause={error_cause}"
    );

    #[cfg(not(debug_assertions))]
    let _ = (
        transfer_id,
        chunk_index,
        chunk_bytes,
        request_body_size,
        response_status,
        error_cause,
    );
}

fn dev_log_chunk_attempt(
    transfer_id: &str,
    chunk_index: u64,
    retry_count: usize,
    error_kind: &str,
    elapsed: Duration,
) {
    #[cfg(debug_assertions)]
    eprintln!(
        "transfer chunk attempt transfer_id={transfer_id} chunk_index={chunk_index} retry_count={retry_count} error_kind={error_kind} elapsed_ms={}",
        elapsed.as_millis()
    );

    #[cfg(not(debug_assertions))]
    let _ = (transfer_id, chunk_index, retry_count, error_kind, elapsed);
}

pub async fn cancel_transfer(state: Arc<AppState>, transfer_id: &str) -> AppResult<bool> {
    let removed = state.active_file_transfers.lock().remove(transfer_id);
    let Some(transfer) = removed else {
        return Ok(false);
    };

    transfer.cancel_token.cancel();
    if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
        let _ = tokio::fs::remove_file(part_path).await;
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
        Some("Transfer cancelled.".into()),
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
) {
    let transfer_ids = {
        let transfers = state.active_file_transfers.lock();
        transfers
            .iter()
            .filter(|(_, transfer)| transfer.room_id == room_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };

    for transfer_id in transfer_ids {
        if notify_peer {
            let _ = cancel_transfer(state.clone(), &transfer_id).await;
        } else {
            let transfer = {
                let mut transfers = state.active_file_transfers.lock();
                transfers.remove(&transfer_id)
            };
            if let Some(transfer) = transfer {
                transfer.cancel_token.cancel();
                if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
                    let _ = tokio::fs::remove_file(part_path).await;
                }
                emit_event(
                    &state,
                    &transfer,
                    "cancelled",
                    current_transferred(&transfer),
                    0.0,
                    average_speed(&transfer, current_transferred(&transfer)),
                    None,
                    Some(message.to_string()),
                );
            }
        }
    }
}

pub fn active_transfer_room_ids(state: &Arc<AppState>) -> Vec<String> {
    state
        .active_file_transfers
        .lock()
        .values()
        .map(|transfer| transfer.room_id.clone())
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
        .ok_or_else(|| AppError::NotFound("Peer left the room.".into()))?;
    Ok(ActiveRoomSnapshot {
        port: server.port,
        transport_secret: server.transport_secret,
        transport_public_key: server.transport_public_key(),
    })
}

fn room_has_active_transfer(state: &Arc<AppState>, room_id: &str) -> bool {
    state
        .active_file_transfers
        .lock()
        .values()
        .any(|transfer| transfer.room_id == room_id)
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
    if room.expires_at <= storage::now_ts()
        || matches!(room.status, RoomStatus::Burned | RoomStatus::Expired)
    {
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
                "Received payload was not valid.".into(),
            )
        }
    };
    let encrypted_payload = match STANDARD.decode(&upload.encrypted_payload) {
        Ok(encrypted_payload) => encrypted_payload,
        Err(_) => {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                "Received payload was not valid.".into(),
            )
        }
    };
    let payload_nonce = match crypto::decode_nonce(&upload.payload_nonce) {
        Ok(payload_nonce) => payload_nonce,
        Err(_) => {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                "Received payload was not valid.".into(),
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
        let inbox_dir = {
            let config = ctx.state.config.read();
            config::effective_inbox_dir(&ctx.state.paths, &config)
        };
        let output_path = match storage::next_inbox_path(&inbox_dir, upload.display_name.as_deref())
        {
            Ok(path) => path,
            Err(_) => {
                return transfer_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "write_failed",
                    "Could not write to destination folder.".into(),
                )
            }
        };
        if tokio::fs::create_dir_all(&inbox_dir).await.is_err()
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
    if room_id != ctx.room_id {
        return StatusCode::NOT_FOUND.into_response();
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
    if start.chunk_size == 0 || start.chunk_size > MAX_CHUNK_PLAINTEXT_BYTES {
        return transfer_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "chunk_too_large",
            "Chunk too large for receiver.".into(),
        );
    }
    if start.total_chunks != start.size_bytes.div_ceil(start.chunk_size) {
        return transfer_error(
            StatusCode::BAD_REQUEST,
            "invalid_transfer",
            "Received payload was not valid.".into(),
        );
    }
    match storage::room_item_exists(&ctx.state.paths, &start.item_id) {
        Ok(true) => return StatusCode::OK.into_response(),
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
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                "Received payload was not valid.".into(),
            )
        }
    };
    let inbox_dir = {
        let config = ctx.state.config.read();
        config::effective_inbox_dir(&ctx.state.paths, &config)
    };
    if !has_enough_disk_space(&inbox_dir, start.size_bytes) {
        return transfer_error(
            StatusCode::INSUFFICIENT_STORAGE,
            "not_enough_disk_space",
            "Not enough disk space to receive this file.".into(),
        );
    }
    let final_path = match storage::next_inbox_path(&inbox_dir, start.display_name.as_deref()) {
        Ok(path) => path,
        Err(_) => {
            return transfer_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "write_failed",
                "Could not write to destination folder.".into(),
            )
        }
    };
    let part_path = storage::part_path_for(&final_path);
    if tokio::fs::create_dir_all(&inbox_dir).await.is_err()
        || tokio::fs::File::create(&part_path).await.is_err()
    {
        return transfer_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "Could not write to destination folder.".into(),
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
        },
    };
    emit_event(&ctx.state, &transfer, "pending", 0, 0.0, 0.0, None, None);
    ctx.state
        .active_file_transfers
        .lock()
        .insert(start.transfer_id, transfer);
    Json(TransferOkResponse { ok: true }).into_response()
}

async fn receive_file_chunk_handler(
    AxumPath((room_id, transfer_id)): AxumPath<(String, String)>,
    State(ctx): State<RoomServerContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if room_id != ctx.room_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(response) = unavailable_room_response_for_active_transfer(&ctx.state, &room_id) {
        return response;
    }
    let chunk_index = match parse_u64_header(&headers, "x-pastey-chunk-index") {
        Some(value) => value,
        None => {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_chunk",
                "Received payload was not valid.".into(),
            )
        }
    };
    let plaintext_size = match parse_u64_header(&headers, "x-pastey-plaintext-size") {
        Some(value) => value,
        None => {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_chunk",
                "Received payload was not valid.".into(),
            )
        }
    };
    let request_body_size = body.len();
    dev_log_receiver_chunk_received(&transfer_id, chunk_index, plaintext_size, request_body_size);
    let nonce = match headers
        .get("x-pastey-nonce")
        .and_then(|value| value.to_str().ok())
        .map(crypto::decode_nonce)
    {
        Some(Ok(value)) => value,
        _ => {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_chunk",
                "Received payload was not valid.".into(),
            )
        }
    };

    let (session_key, part_path, cancel_token) = {
        let transfers = ctx.state.active_file_transfers.lock();
        let Some(transfer) = transfers.get(&transfer_id) else {
            return transfer_error(
                StatusCode::NOT_FOUND,
                "transfer_missing",
                "Network connection lost.".into(),
            );
        };
        if transfer.cancel_token.is_cancelled() {
            return transfer_error(
                StatusCode::CONFLICT,
                "cancelled",
                "Transfer cancelled.".into(),
            );
        }
        let ActiveFileTransferKind::Receiver {
            session_key,
            part_path,
            transferred_bytes,
            expected_chunk_index,
            ..
        } = &transfer.kind
        else {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_transfer",
                "Received payload was not valid.".into(),
            );
        };
        if *expected_chunk_index > 0 && chunk_index + 1 == *expected_chunk_index {
            return Json(ChunkAckResponse {
                ok: true,
                chunk_index,
                transferred_bytes: *transferred_bytes,
            })
            .into_response();
        }
        if *expected_chunk_index != chunk_index {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_chunk_order",
                "Received chunks out of order.".into(),
            );
        }
        (
            *session_key,
            part_path.clone(),
            cancel_token_clone(transfer),
        )
    };
    if cancel_token.is_cancelled() {
        return transfer_error(
            StatusCode::CONFLICT,
            "cancelled",
            "Transfer cancelled.".into(),
        );
    }

    let plaintext = match crypto::decrypt_bytes(&body, &session_key, &nonce) {
        Ok(plaintext) => plaintext,
        Err(_) => {
            dev_log_receiver_chunk_failure(
                &transfer_id,
                chunk_index,
                plaintext_size,
                request_body_size,
                StatusCode::BAD_REQUEST,
                "integrity_failed",
            );
            fail_receiver_transfer(
                &ctx.state,
                &transfer_id,
                "Chunk failed integrity verification.",
            )
            .await;
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "integrity_failed",
                "Chunk failed integrity verification.".into(),
            );
        }
    };
    if plaintext.len() as u64 != plaintext_size {
        dev_log_receiver_chunk_failure(
            &transfer_id,
            chunk_index,
            plaintext_size,
            request_body_size,
            StatusCode::BAD_REQUEST,
            "plaintext_size_mismatch",
        );
        fail_receiver_transfer(
            &ctx.state,
            &transfer_id,
            "Chunk failed integrity verification.",
        )
        .await;
        return transfer_error(
            StatusCode::BAD_REQUEST,
            "integrity_failed",
            "Chunk failed integrity verification.".into(),
        );
    }

    let write_result = async {
        let mut file = OpenOptions::new().append(true).open(&part_path).await?;
        file.write_all(&plaintext).await?;
        file.flush().await
    }
    .await;
    if let Err(error) = write_result {
        dev_log_receiver_chunk_failure(
            &transfer_id,
            chunk_index,
            plaintext_size,
            request_body_size,
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("write_failed: {error}"),
        );
        fail_receiver_transfer(
            &ctx.state,
            &transfer_id,
            "Could not write to destination folder.",
        )
        .await;
        return transfer_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "Could not write to destination folder.".into(),
        );
    }
    dev_log_receiver_chunk_write_success(
        &transfer_id,
        chunk_index,
        plaintext_size,
        request_body_size,
    );

    let maybe_event = {
        let mut transfers = ctx.state.active_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(&transfer_id) else {
            return transfer_error(
                StatusCode::NOT_FOUND,
                "transfer_missing",
                "Network connection lost.".into(),
            );
        };
        let now = Instant::now();
        let previous_report_at = transfer.last_report_at;
        let previous_report_bytes = transfer.last_report_bytes;
        let ActiveFileTransferKind::Receiver {
            transferred_bytes,
            expected_chunk_index,
            ..
        } = &mut transfer.kind
        else {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_transfer",
                "Received payload was not valid.".into(),
            );
        };
        *transferred_bytes += plaintext_size;
        *expected_chunk_index += 1;
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
        transfer.last_report_at = now;
        transfer.last_report_bytes = current;
        Some((
            clone_event_base(
                transfer,
                "incoming",
                "transferring",
                current,
                current_speed,
                average_speed,
                eta_seconds(transfer.file_size, current, current_speed),
                None,
            ),
            current,
        ))
    };
    let ack_transferred_bytes = maybe_event
        .as_ref()
        .map(|(_, current)| *current)
        .unwrap_or(plaintext_size);
    if let Some((event, _)) = maybe_event {
        let _ = ctx.state.app_handle.emit(TRANSFER_EVENT, event);
    }

    Json(ChunkAckResponse {
        ok: true,
        chunk_index,
        transferred_bytes: ack_transferred_bytes,
    })
    .into_response()
}

async fn finish_file_transfer_handler(
    AxumPath((room_id, transfer_id)): AxumPath<(String, String)>,
    State(ctx): State<RoomServerContext>,
    Json(finish): Json<FileTransferFinishRequest>,
) -> Response {
    if room_id != ctx.room_id {
        return StatusCode::NOT_FOUND.into_response();
    }

    let transfer = match ctx.state.active_file_transfers.lock().remove(&transfer_id) {
        Some(transfer) => transfer,
        None => {
            return transfer_error(
                StatusCode::NOT_FOUND,
                "transfer_missing",
                "Network connection lost.".into(),
            )
        }
    };
    let ActiveFileTransferKind::Receiver {
        part_path,
        final_path,
        mime_type,
        created_at,
        transferred_bytes,
        expected_chunk_index,
        ..
    } = &transfer.kind
    else {
        return transfer_error(
            StatusCode::BAD_REQUEST,
            "invalid_transfer",
            "Received payload was not valid.".into(),
        );
    };
    if finish.item_id != transfer.item_id
        || *transferred_bytes != transfer.file_size
        || *expected_chunk_index != transfer.total_chunks
    {
        let _ = tokio::fs::remove_file(part_path).await;
        emit_event(
            &ctx.state,
            &transfer,
            "failed",
            *transferred_bytes,
            0.0,
            average_speed(&transfer, *transferred_bytes),
            None,
            Some("Network connection lost.".into()),
        );
        return transfer_error(
            StatusCode::BAD_REQUEST,
            "invalid_transfer",
            "Network connection lost.".into(),
        );
    }

    if tokio::fs::rename(part_path, final_path).await.is_err() {
        let _ = tokio::fs::remove_file(part_path).await;
        emit_event(
            &ctx.state,
            &transfer,
            "failed",
            *transferred_bytes,
            0.0,
            average_speed(&transfer, *transferred_bytes),
            None,
            Some("Could not write to destination folder.".into()),
        );
        return transfer_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "write_failed",
            "Could not write to destination folder.".into(),
        );
    }

    let master_key = {
        let config = ctx.state.config.read();
        match config::master_key(&config) {
            Ok(key) => key,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    };
    if storage::persist_incoming_file_item_metadata(
        &ctx.state.paths,
        &master_key,
        &room_id,
        &transfer.item_id,
        transfer.file_size,
        Some(transfer.file_name.clone()),
        mime_type.clone(),
        *created_at,
        Some(final_path.display().to_string()),
    )
    .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let _ = storage::set_room_status(&ctx.state.paths, &room_id, RoomStatus::Active);
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
) -> Response {
    if room_id != ctx.room_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(transfer) = ctx.state.active_file_transfers.lock().remove(&transfer_id) else {
        return Json(TransferOkResponse { ok: true }).into_response();
    };
    transfer.cancel_token.cancel();
    if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
        let _ = tokio::fs::remove_file(part_path).await;
    }
    emit_event(
        &ctx.state,
        &transfer,
        "cancelled",
        current_transferred(&transfer),
        0.0,
        average_speed(&transfer, current_transferred(&transfer)),
        None,
        Some("Transfer cancelled.".into()),
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

    cancel_room_transfers(ctx.state.clone(), &room_id, "Transfer cancelled.", false).await;
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

    cancel_room_transfers(ctx.state.clone(), &room_id, "Peer left the room.", false).await;
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
        Err(_) => return Some(StatusCode::NOT_FOUND.into_response()),
    };
    if room.expires_at <= storage::now_ts()
        || matches!(room.status, RoomStatus::Burned | RoomStatus::Expired)
    {
        return Some(transfer_error(
            StatusCode::GONE,
            "room_expired",
            "Room expired.".into(),
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
        Err(_) => return Some(StatusCode::NOT_FOUND.into_response()),
    };
    if room.expires_at <= storage::now_ts() || room.status == RoomStatus::Expired {
        if room_has_active_transfer(state, room_id) {
            return None;
        }
        return Some(transfer_error(
            StatusCode::GONE,
            "room_expired",
            "Room expired.".into(),
        ));
    }
    if room.status == RoomStatus::Burned {
        return Some(transfer_error(
            StatusCode::GONE,
            "room_expired",
            "Room expired.".into(),
        ));
    }
    None
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
    let status = response.status();
    if matches!(status, StatusCode::NOT_FOUND | StatusCode::GONE) {
        return "Peer left the room.".to_string();
    }
    response
        .json::<TransferErrorResponse>()
        .await
        .ok()
        .map(|error| error.message)
        .unwrap_or_else(|| "Network connection lost.".to_string())
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

async fn fail_receiver_transfer(state: &Arc<AppState>, transfer_id: &str, message: &str) {
    let transfer = state.active_file_transfers.lock().remove(transfer_id);
    if let Some(transfer) = transfer {
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

fn cancel_token_clone(transfer: &ActiveFileTransfer) -> CancellationToken {
    transfer.cancel_token.clone()
}

fn parse_u64_header(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

async fn notify_transfer_cancel(client: &reqwest::Client, base_url: &str, transfer_id: &str) {
    let _ = client
        .post(format!("{base_url}/transfers/{transfer_id}/cancel"))
        .send()
        .await;
}

async fn apply_rate_limit(state: &Arc<AppState>, transferred: u64, started_at: Instant) {
    let limit_mbps = {
        let config = state.config.read();
        config.speed_limit_mbps
    };
    let Some(limit_mbps) = limit_mbps else {
        return;
    };
    let bytes_per_second = limit_mbps * 1024.0 * 1024.0;
    if bytes_per_second <= 0.0 {
        return;
    }
    let expected_elapsed = Duration::from_secs_f64(transferred as f64 / bytes_per_second);
    let actual_elapsed = started_at.elapsed();
    if expected_elapsed > actual_elapsed {
        tokio::time::sleep(expected_elapsed - actual_elapsed).await;
    }
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

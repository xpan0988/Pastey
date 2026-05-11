use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{rejection::JsonRejection, ConnectInfo, DefaultBodyLimit, Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tokio::{
    fs::OpenOptions,
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;

use crate::{
    config, crypto, discovery,
    error::{AppError, AppResult},
    logging,
    models::{
        ChunkAckResponse, ChunkUploadRequest, FileTransferFinishRequest, FileTransferProgressEvent,
        FileTransferStartRequest, JoinRoomRequest, JoinRoomResponse, PayloadType, RoomItemStatus,
        RoomItemUpload, RoomStatus, TransferErrorResponse,
    },
    storage, ActiveRoomServer, AppState,
};

pub const DEFAULT_CHUNK_SIZE_BYTES: u64 = 4 * 1024 * 1024;
const DISK_SPACE_MARGIN_BYTES: u64 = 512 * 1024 * 1024;
const TRANSFER_EVENT: &str = "pastey://transfer-progress";
const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CHUNK_BODY_BYTES: usize = 16 * 1024 * 1024;
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

#[derive(Deserialize)]
struct TransferCancelRequest {
    status: Option<String>,
    message: Option<String>,
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
    dev_log_sender_start_transfer_metadata(
        &transfer_id,
        room_id,
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
    };

    let start_response = client.post(&start_url).json(&start).send().await;
    let start_response = match start_response {
        Ok(response) if response.status().is_success() => {
            dev_log_sender_transfer_start_response(&transfer_id, room_id, response.status(), "");
            response
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
            fail_transfer(&state, &transfer_id, item_id, message.clone());
            return Err(AppError::Network(message));
        }
        Err(error) => {
            dev_log_sender_final_error(
                &transfer_id,
                room_id,
                None,
                "start_failed",
                &error.to_string(),
            );
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
    let mut buffer = vec![0u8; chunk_size];
    dev_log_sender_read_loop_config(&transfer_id, room_id, chunk_size, buffer.len(), chunk_size);
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

        let (encrypted_bytes, nonce) = crypto::encrypt_bytes(&buffer[..bytes_read], &payload_key)?;
        dev_log_sender_chunk_plaintext(
            &transfer_id,
            room_id,
            chunk_index,
            bytes_read,
            is_final,
            chunk_size,
        );
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

                notify_transfer_failed(&client, &base_url, &transfer_id, &error.message).await;
                dev_log_sender_final_error(
                    &transfer_id,
                    room_id,
                    Some(chunk_index),
                    error.kind.as_str(),
                    &error.message,
                );
                fail_transfer(&state, &transfer_id, item_id, error.message.clone());
                return if error.kind == ChunkSendFailureKind::Timeout {
                    Err(AppError::Timeout(error.message))
                } else {
                    Err(AppError::Network(error.message))
                };
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
        apply_rate_limit(&state, transferred, started_at).await;
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
            fail_transfer(&state, &transfer_id, item_id, message.clone());
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
            room_id,
            transfer_id,
            chunk_index,
            total_chunks,
            plaintext_size,
            nonce,
            encrypted_bytes,
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
    );
    let response = client
        .post(&chunk_url)
        .timeout(CHUNK_REQUEST_TIMEOUT)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(request_body)
        .send()
        .await
        .map_err(chunk_failure_from_reqwest)?;

    let status = response.status();
    if !response.status().is_success() {
        return Err(chunk_failure_from_response(
            response,
            transfer_id,
            room_id,
            chunk_index,
            plaintext_size,
            encrypted_bytes.len(),
            request_body_size,
        )
        .await);
    }

    let body_text = response.text().await.map_err(chunk_failure_from_reqwest)?;
    dev_log_sender_chunk_response(
        transfer_id,
        room_id,
        chunk_index,
        plaintext_size,
        encrypted_bytes.len(),
        request_body_size,
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
            message: "Transfer timed out.".into(),
            kind: ChunkSendFailureKind::Timeout,
            retryable: true,
        };
    }
    if error.is_connect() {
        return ChunkSendFailure {
            message: "Connection lost.".into(),
            kind: ChunkSendFailureKind::Unreachable,
            retryable: true,
        };
    }
    ChunkSendFailure {
        message: "Connection lost.".into(),
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
) -> ChunkSendFailure {
    let details = response_error_details(response).await;
    dev_log_sender_chunk_response(
        transfer_id,
        room_id,
        chunk_index,
        plaintext_size,
        ciphertext_bytes,
        request_body_size,
        details.status,
        &details.body_text,
    );
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
            message: "Room not found on receiver.".into(),
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
    if details.status == StatusCode::GONE {
        return ChunkSendFailure {
            message: "Peer left the room.".into(),
            kind: ChunkSendFailureKind::PeerLeft,
            retryable: false,
        };
    }

    if details.code.as_deref() == Some("cancelled") {
        return ChunkSendFailure {
            message: "Transfer cancelled.".into(),
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
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=start_request method=POST peer_url={peer_url} start_url={start_url} chunk_url={chunk_url} chunk_payload_format=json chunk_size={chunk_size} total_chunks={total_chunks} file_size={file_size}"
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

fn dev_log_sender_start_transfer_metadata(
    transfer_id: &str,
    room_id: &str,
    chunk_size: u64,
    total_chunks: u64,
    file_size: u64,
) {
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=start_transfer_metadata chunk_size={chunk_size} total_chunks={total_chunks} file_size={file_size}"
    ));
}

fn dev_log_sender_read_loop_config(
    transfer_id: &str,
    room_id: &str,
    metadata_chunk_size: usize,
    read_buffer_len: usize,
    expected_chunk_size: usize,
) {
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}] event=read_loop_config metadata_chunk_size={metadata_chunk_size} read_buffer_len={read_buffer_len} expected_chunk_size={expected_chunk_size}"
    ));
}

fn dev_log_sender_chunk_plaintext(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    actual_plaintext_size: usize,
    is_final: bool,
    expected_non_final_chunk_size: usize,
) {
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_plaintext chunk_index={chunk_index} actual_plaintext_size={actual_plaintext_size} is_final={is_final} expected_non_final_chunk_size={expected_non_final_chunk_size}"
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
    encoded_json_body_bytes: usize,
) {
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_request method={method} chunk_url={chunk_url} actual_plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_json_body_bytes={encoded_json_body_bytes} payload_format=json"
    ));
}

fn dev_log_sender_chunk_response(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_bytes: usize,
    ciphertext_bytes: usize,
    encoded_json_body_bytes: usize,
    status: StatusCode,
    body_text: &str,
) {
    emit_transfer_log(format!(
        "[pastey transfer][sender][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_response actual_plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_json_body_bytes={encoded_json_body_bytes} response_status={status} response_body={body_text:?}"
    ));
}

fn dev_log_sender_chunk_attempt(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    retry_count: usize,
    error_kind: &str,
    elapsed: Duration,
) {
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
        Some(chunk_index) => emit_transfer_log(format!(
            "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_route_hit"
        )),
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
    encoded_json_body_bytes: usize,
) {
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_received plaintext_size={plaintext_bytes} encoded_ciphertext_bytes={encoded_ciphertext_bytes} encoded_json_body_bytes={encoded_json_body_bytes} payload_format=json"
    ));
}

fn dev_log_receiver_chunk_write_success(
    transfer_id: &str,
    room_id: &str,
    chunk_index: u64,
    plaintext_bytes: u64,
    ciphertext_bytes: usize,
    encoded_json_body_bytes: usize,
) {
    emit_transfer_log(format!(
        "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_write plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_json_body_bytes={encoded_json_body_bytes} response_status={} result=success",
        StatusCode::OK
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
    encoded_json_body_bytes: usize,
    response_status: StatusCode,
    error_cause: &str,
) {
    match chunk_index {
        Some(chunk_index) => emit_transfer_log(format!(
            "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk={chunk_index}] event=chunk_failure plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_json_body_bytes={encoded_json_body_bytes} response_status={response_status} error_cause={error_cause} mapped_error_message={:?}",
            receiver_failure_log_message(error_cause)
        )),
        None => emit_transfer_log(format!(
            "[pastey transfer][receiver][transfer_id={transfer_id}][room_id={room_id}][chunk=unknown] event=chunk_failure plaintext_size={plaintext_bytes} ciphertext_bytes={ciphertext_bytes} encoded_json_body_bytes={encoded_json_body_bytes} response_status={response_status} error_cause={error_cause} mapped_error_message={:?}",
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
    let reserved_final_paths = active_receiver_final_paths(&ctx.state);
    let final_path = match storage::next_inbox_path_excluding(
        &inbox_dir,
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
    let part_path = storage::transfer_part_path(&inbox_dir, &start.transfer_id);
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
    Json(TransferOkResponse { ok: true }).into_response()
}

async fn receive_file_chunk_handler(
    AxumPath((room_id, transfer_id)): AxumPath<(String, String)>,
    State(ctx): State<RoomServerContext>,
    upload: Result<Json<ChunkUploadRequest>, JsonRejection>,
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

    let Json(upload) = match upload {
        Ok(upload) => upload,
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

    let chunk_index = upload.chunk_index;
    let plaintext_size = upload.plaintext_size;
    let encoded_json_body_size = serde_json::to_vec(&upload)
        .map(|body| body.len())
        .unwrap_or_default();
    dev_log_receiver_chunk_route_hit(&transfer_id, &room_id, Some(chunk_index));

    if plaintext_size == 0 || plaintext_size > DEFAULT_CHUNK_SIZE_BYTES {
        dev_log_receiver_chunk_failure(
            &transfer_id,
            &room_id,
            Some(chunk_index),
            plaintext_size,
            0,
            encoded_json_body_size,
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
        upload.ciphertext.len(),
        encoded_json_body_size,
    );

    let nonce = match crypto::decode_nonce(&upload.nonce) {
        Ok(nonce) => nonce,
        Err(_) => {
            dev_log_receiver_chunk_failure(
                &transfer_id,
                &room_id,
                Some(chunk_index),
                plaintext_size,
                0,
                encoded_json_body_size,
                StatusCode::BAD_REQUEST,
                "invalid_nonce_encoding",
            );
            fail_receiver_transfer(&ctx.state, &transfer_id, "Invalid chunk encoding").await;
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_chunk_encoding",
                "Invalid chunk encoding".into(),
            );
        }
    };
    let ciphertext = match STANDARD.decode(&upload.ciphertext) {
        Ok(ciphertext) => ciphertext,
        Err(_) => {
            dev_log_receiver_chunk_failure(
                &transfer_id,
                &room_id,
                Some(chunk_index),
                plaintext_size,
                0,
                encoded_json_body_size,
                StatusCode::BAD_REQUEST,
                "invalid_ciphertext_encoding",
            );
            fail_receiver_transfer(&ctx.state, &transfer_id, "Invalid chunk encoding").await;
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_chunk_encoding",
                "Invalid chunk encoding".into(),
            );
        }
    };
    let ciphertext_bytes = ciphertext.len();

    let transfer_lookup = {
        let transfers = ctx.state.active_file_transfers.lock();
        let Some(transfer) = transfers.get(&transfer_id) else {
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
                "Invalid chunk payload".into(),
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
        } else if *expected_chunk_index > 0
            && chunk_index.checked_add(1) == Some(*expected_chunk_index)
        {
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
        } else if *expected_chunk_index != chunk_index {
            Err((
                "invalid_chunk_order",
                "Unexpected chunk index",
                "invalid_chunk_order",
            ))
        } else {
            Ok((
                *session_key,
                part_path.clone(),
                cancel_token_clone(transfer),
                *transferred_bytes,
            ))
        }
    };
    let (session_key, part_path, cancel_token, received_before) = match transfer_lookup {
        Ok(value) => value,
        Err((code, message, cause)) => {
            dev_log_receiver_chunk_failure(
                &transfer_id,
                &room_id,
                Some(chunk_index),
                plaintext_size,
                ciphertext_bytes,
                encoded_json_body_size,
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
            "Transfer cancelled.".into(),
        );
    }

    let plaintext = match crypto::decrypt_bytes(&ciphertext, &session_key, &nonce) {
        Ok(plaintext) => plaintext,
        Err(_) => {
            dev_log_receiver_chunk_failure(
                &transfer_id,
                &room_id,
                Some(chunk_index),
                plaintext_size,
                ciphertext_bytes,
                encoded_json_body_size,
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
    if plaintext.len() as u64 != plaintext_size {
        dev_log_receiver_chunk_failure(
            &transfer_id,
            &room_id,
            Some(chunk_index),
            plaintext_size,
            ciphertext_bytes,
            encoded_json_body_size,
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

    if let Err(error) = write_receiver_chunk(&part_path, &plaintext, received_before).await {
        dev_log_receiver_chunk_failure(
            &transfer_id,
            &room_id,
            Some(chunk_index),
            plaintext_size,
            ciphertext_bytes,
            encoded_json_body_size,
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
    dev_log_receiver_chunk_write_success(
        &transfer_id,
        &room_id,
        chunk_index,
        plaintext_size,
        ciphertext_bytes,
        encoded_json_body_size,
    );

    let maybe_event = {
        let mut transfers = ctx.state.active_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(&transfer_id) else {
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
            ..
        } = &mut transfer.kind
        else {
            return transfer_error(
                StatusCode::BAD_REQUEST,
                "invalid_transfer",
                "Invalid chunk payload".into(),
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
            return transfer_error(
                StatusCode::NOT_FOUND,
                "transfer_missing",
                "Transfer session not found on receiver.".into(),
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
            "Invalid file metadata".into(),
        );
    };
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
        dev_log_receiver_finalize(
            &transfer_id,
            &room_id,
            "finalize_failure",
            "error_kind=write_failed message=\"Receiver failed to write chunk\"",
        );
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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
        return Json(TransferOkResponse { ok: true }).into_response();
    };
    transfer.cancel_token.cancel();
    if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
        let _ = tokio::fs::remove_file(part_path).await;
    }
    let failure = request
        .as_ref()
        .is_some_and(|Json(request)| request.status.as_deref() == Some("failed"));
    let status = if failure { "failed" } else { "cancelled" };
    let message = request
        .and_then(|Json(request)| request.message)
        .unwrap_or_else(|| "Transfer cancelled.".into());
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
        Err(_) => {
            return Some(transfer_error(
                StatusCode::NOT_FOUND,
                "room_not_found",
                "Room not found on receiver.".into(),
            ))
        }
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
        Err(_) => {
            return Some(transfer_error(
                StatusCode::NOT_FOUND,
                "room_not_found",
                "Room not found on receiver.".into(),
            ))
        }
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
        Some("invalid_chunk_payload") => "Invalid chunk payload".into(),
        Some("invalid_chunk_encoding") => "Invalid chunk encoding".into(),
        Some("integrity_failed") => "Chunk integrity check failed".into(),
        Some("invalid_chunk_order") => "Unexpected chunk index".into(),
        Some("metadata_mismatch") => "Transfer metadata mismatch".into(),
        Some("not_enough_disk_space") => "Not enough disk space on receiver".into(),
        Some("receiver_cannot_write") => "Receiver cannot write to inbox".into(),
        Some("size_mismatch") => "Received file size mismatch".into(),
        Some("temp_file_disappeared") => "Receiver temporary file disappeared".into(),
        Some("write_failed") => "Receiver failed to write chunk".into(),
        Some("cancelled") => "Transfer cancelled.".into(),
        _ => {
            if details.status == StatusCode::PAYLOAD_TOO_LARGE {
                "Chunk too large for receiver".into()
            } else if details.status == StatusCode::NOT_FOUND {
                "Transfer session not found on receiver.".into()
            } else if details.status == StatusCode::INTERNAL_SERVER_ERROR {
                "Receiver failed to write chunk".into()
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
        StatusCode::INTERNAL_SERVER_ERROR => "Receiver failed to write chunk",
        StatusCode::REQUEST_TIMEOUT => "Transfer timed out.",
        StatusCode::GONE => "Peer left the room.",
        _ => "Connection lost.",
    }
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

fn total_chunks_for(file_size: u64, chunk_size: u64) -> u64 {
    chunk_count(file_size, chunk_size as usize)
}

fn chunk_count(file_size: u64, chunk_size: usize) -> u64 {
    if chunk_size == 0 {
        return 0;
    }
    file_size.div_ceil(chunk_size as u64)
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
    received_before: u64,
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
    if !file_exists && received_before > 0 {
        return Err(receiver_write_failure(
            part_path,
            "temp_file_disappeared",
            "Receiver temporary file disappeared",
            StatusCode::INTERNAL_SERVER_ERROR,
            "temp_file_disappeared".to_string(),
        ));
    }

    let mut file = OpenOptions::new()
        .create(received_before == 0)
        .append(true)
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
    file.write_all(plaintext).await.map_err(|error| {
        let (code, message, status) = map_receiver_write_error(&error);
        receiver_write_failure(
            part_path,
            code,
            message,
            status,
            format!("write_failed: {error}"),
        )
    })?;
    file.flush().await.map_err(|error| {
        let (code, message, status) = map_receiver_write_error(&error);
        receiver_write_failure(
            part_path,
            code,
            message,
            status,
            format!("flush_failed: {error}"),
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
            "Transfer cancelled."
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

use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use tokio::{net::TcpListener, sync::oneshot};

use crate::{
    config,
    crypto,
    discovery,
    error::{AppError, AppResult},
    models::{JoinRoomRequest, JoinRoomResponse, RoomItemStatus, RoomItemUpload, RoomStatus},
    storage,
    ActiveRoomServer, AppState,
};

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

pub async fn start_room_server(state: Arc<AppState>, room_id: &str) -> AppResult<u16> {
    if let Some(port) = {
        let servers = state.active_servers.lock();
        servers.get(room_id).map(|server| server.port)
    } {
        return Ok(port);
    }

    let room = storage::get_room_by_id(&state.paths, room_id)?;
    if room.expires_at <= storage::now_ts() {
        return Err(AppError::InvalidInput("room has already expired".into()));
    }

    let transport_secret = crypto::generate_transport_secret();
    let router = Router::new()
        .route("/rooms/:room_id/join", post(join_handler))
        .route("/rooms/:room_id/items", post(receive_item_handler))
        .route("/rooms/:room_id/burn", post(remote_burn_handler))
        .route("/rooms/:room_id/leave", post(remote_leave_handler))
        .with_state(RoomServerContext {
            state: state.clone(),
            room_id: room.id.clone(),
        });

    let listener = TcpListener::bind(("0.0.0.0", 0))
        .await
        .map_err(|error| AppError::Network(format!("unable to bind room server: {error}")))?;
    let port = listener
        .local_addr()
        .map_err(|error| AppError::Network(format!("unable to read bound port: {error}")))?
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
        let next_status = if room.peer_host.is_some() && room.peer_port.is_some() {
            RoomStatus::Connected
        } else {
            RoomStatus::Waiting
        };
        storage::set_room_status(&state.paths, room_id, next_status)?;
    }

    discovery::ensure_service(state).await?;
    Ok(port)
}

pub async fn stop_room_server(state: Arc<AppState>, room_id: &str) -> AppResult<bool> {
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
        .post(format!("http://{peer_host}:{peer_port}/rooms/{room_id}/join"))
        .json(&JoinRoomRequest {
            port: snapshot.port,
            device_name: device_name(),
            transport_public_key: snapshot.transport_public_key,
        })
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(AppError::Network(format!(
            "room join request failed ({})",
            response.status()
        )));
    }

    response.json().await.map_err(Into::into)
}

pub async fn send_room_item(state: Arc<AppState>, room_id: &str, item_id: &str) -> AppResult<()> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    let peer_host = room
        .peer_host
        .clone()
        .ok_or_else(|| AppError::InvalidInput("room is not connected yet".into()))?;
    let peer_port = room
        .peer_port
        .ok_or_else(|| AppError::InvalidInput("room is not connected yet".into()))?;
    let peer_transport_public_key = room
        .peer_transport_public_key
        .clone()
        .ok_or_else(|| AppError::InvalidInput("room is missing peer transport details".into()))?;

    let item = storage::get_room_item_by_id(&state.paths, item_id)?;
    let master_key = {
        let config = state.config.read();
        config::master_key(&config)?
    };
    let payload_key = storage::read_room_item_key(&item, &master_key)?;
    let snapshot = room_server_snapshot(&state, room_id)?;
    let receiver_public_key = crypto::decode_key(&peer_transport_public_key)?;
    let (wrapped_session_key, transport_nonce, sender_public_key) =
        crypto::wrap_session_for_receiver(&payload_key, &snapshot.transport_secret, &receiver_public_key)?;
    let encrypted_payload =
        tokio::fs::read(storage::encrypted_file_path(&state.paths, &item.encrypted_path)).await?;

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
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|error| AppError::Network(format!("unable to build HTTP client: {error}")))?;
    let response = client
        .post(format!("http://{peer_host}:{peer_port}/rooms/{room_id}/items"))
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
            Err(AppError::Network(format!(
                "peer rejected room item ({})",
                response.status()
            )))
        }
        Err(error) => {
            storage::set_room_item_status(&state.paths, item_id, RoomItemStatus::Failed)?;
            Err(AppError::Http(error))
        }
    }
}

pub async fn notify_room_burn(state: Arc<AppState>, room_id: &str) {
    let Ok(room) = storage::get_room_by_id(&state.paths, room_id) else {
        return;
    };
    let (Some(peer_host), Some(peer_port)) = (room.peer_host, room.peer_port) else {
        return;
    };

    let _ = reqwest::Client::new()
        .post(format!("http://{peer_host}:{peer_port}/rooms/{room_id}/burn"))
        .send()
        .await;
}

pub async fn notify_room_leave(state: Arc<AppState>, room_id: &str) {
    let Ok(room) = storage::get_room_by_id(&state.paths, room_id) else {
        return;
    };
    let (Some(peer_host), Some(peer_port)) = (room.peer_host, room.peer_port) else {
        return;
    };

    let _ = reqwest::Client::new()
        .post(format!("http://{peer_host}:{peer_port}/rooms/{room_id}/leave"))
        .send()
        .await;
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
        .ok_or_else(|| AppError::NotFound("room server not running".into()))?;
    Ok(ActiveRoomSnapshot {
        port: server.port,
        transport_secret: server.transport_secret,
        transport_public_key: server.transport_public_key(),
    })
}

async fn join_handler(
    Path(room_id): Path<String>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    State(ctx): State<RoomServerContext>,
    Json(request): Json<JoinRoomRequest>,
) -> Result<Json<JoinRoomResponse>, StatusCode> {
    if room_id != ctx.room_id {
        return Err(StatusCode::NOT_FOUND);
    }

    let room = storage::get_room_by_id(&ctx.state.paths, &room_id).map_err(|_| StatusCode::NOT_FOUND)?;
    if room.expires_at <= storage::now_ts() || matches!(room.status, RoomStatus::Burned | RoomStatus::Expired) {
        return Err(StatusCode::GONE);
    }

    let snapshot = room_server_snapshot(&ctx.state, &room_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    storage::update_room_peer(
        &ctx.state.paths,
        &room_id,
        Some(&source.ip().to_string()),
        Some(request.port),
        Some(&request.device_name),
        Some(&request.transport_public_key),
        RoomStatus::Connected,
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(JoinRoomResponse {
        device_name: device_name(),
        expires_at: room.expires_at,
        transport_public_key: snapshot.transport_public_key,
    }))
}

async fn receive_item_handler(
    Path(room_id): Path<String>,
    State(ctx): State<RoomServerContext>,
    Json(upload): Json<RoomItemUpload>,
) -> Result<StatusCode, StatusCode> {
    if room_id != ctx.room_id {
        return Err(StatusCode::NOT_FOUND);
    }

    let room = storage::get_room_by_id(&ctx.state.paths, &room_id).map_err(|_| StatusCode::NOT_FOUND)?;
    if room.expires_at <= storage::now_ts() || matches!(room.status, RoomStatus::Burned | RoomStatus::Expired) {
        return Err(StatusCode::GONE);
    }

    if storage::room_item_exists(&ctx.state.paths, &upload.item_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        return Ok(StatusCode::OK);
    }

    let snapshot = room_server_snapshot(&ctx.state, &room_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let session_key = crypto::unwrap_session_from_sender(
        &upload.wrapped_session_key,
        &upload.transport_nonce,
        &upload.sender_public_key,
        &snapshot.transport_secret,
    )
    .map_err(|_| StatusCode::BAD_REQUEST)?;
    let encrypted_payload = STANDARD
        .decode(&upload.encrypted_payload)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let payload_nonce = crypto::decode_nonce(&upload.payload_nonce).map_err(|_| StatusCode::BAD_REQUEST)?;
    let plaintext =
        crypto::decrypt_bytes(&encrypted_payload, &session_key, &payload_nonce).map_err(|_| StatusCode::BAD_REQUEST)?;

    let saved_path = if upload.payload_type == crate::models::PayloadType::File {
        let inbox_dir = {
            let config = ctx.state.config.read();
            config::effective_inbox_dir(&ctx.state.paths, &config)
        };
        let output_path =
            storage::next_inbox_path(&inbox_dir, upload.display_name.as_deref()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tokio::fs::create_dir_all(&inbox_dir)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tokio::fs::write(&output_path, &plaintext)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Some(output_path.display().to_string())
    } else {
        None
    };

    let master_key = {
        let config = ctx.state.config.read();
        config::master_key(&config).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    storage::persist_incoming_item(
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
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    storage::set_room_status(&ctx.state.paths, &room_id, RoomStatus::Connected)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}

async fn remote_burn_handler(
    Path(room_id): Path<String>,
    State(ctx): State<RoomServerContext>,
) -> Result<StatusCode, StatusCode> {
    if room_id != ctx.room_id {
        return Err(StatusCode::NOT_FOUND);
    }

    storage::burn_room(&ctx.state.paths, &room_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let state = ctx.state.clone();
    tokio::spawn(async move {
        let _ = stop_room_server(state, &room_id).await;
    });
    Ok(StatusCode::OK)
}

async fn remote_leave_handler(
    Path(room_id): Path<String>,
    State(ctx): State<RoomServerContext>,
) -> Result<StatusCode, StatusCode> {
    if room_id != ctx.room_id {
        return Err(StatusCode::NOT_FOUND);
    }

    storage::clear_room_peer(&ctx.state.paths, &room_id, RoomStatus::Waiting)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

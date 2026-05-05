use std::{net::SocketAddr, sync::Arc, time::Duration};

use tokio::{
    net::UdpSocket,
    sync::oneshot,
    time::{sleep, timeout},
};

use crate::{
    error::{AppError, AppResult},
    models::{DiscoveryRequest, DiscoveryResponse},
    AppState,
};

const DISCOVERY_PORT: u16 = 48392;

pub async fn ensure_service(state: Arc<AppState>) -> AppResult<()> {
    if state.discovery_handle.lock().is_some() {
        return Ok(());
    }

    let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))
        .await
        .map_err(|error| AppError::Network(format!("unable to bind discovery socket: {error}")))?;
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let service_state = state.clone();

    tokio::spawn(async move {
        let mut buffer = vec![0u8; 4096];

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    break;
                }
                result = socket.recv_from(&mut buffer) => {
                    let Ok((size, source)) = result else {
                        break;
                    };

                    let Ok(request) = serde_json::from_slice::<DiscoveryRequest>(&buffer[..size]) else {
                        continue;
                    };

                    if request.kind != "discover_room" {
                        continue;
                    }

                    let response = {
                        let servers = service_state.active_servers.lock();
                        servers
                            .values()
                            .find(|server| server.room_code_hash == request.room_code_hash)
                            .map(|server| DiscoveryResponse {
                                kind: "room_offer".to_string(),
                                request_id: request.request_id.clone(),
                                room_id: server.room_id.clone(),
                                port: server.port,
                                expires_at: server.expires_at,
                                transport_public_key: server.transport_public_key(),
                                device_name: crate::transfer::device_name(),
                            })
                    };

                    if let Some(response) = response {
                        let Ok(bytes) = serde_json::to_vec(&response) else {
                            continue;
                        };
                        let _ = socket.send_to(&bytes, source).await;
                    }
                }
            }
        }
    });

    let mut handle = state.discovery_handle.lock();
    if handle.is_none() {
        *handle = Some(crate::DiscoveryHandle {
            shutdown: shutdown_tx,
        });
    }
    Ok(())
}

pub async fn maybe_stop_service(state: Arc<AppState>) {
    if !state.active_servers.lock().is_empty() {
        return;
    }

    if let Some(handle) = state.discovery_handle.lock().take() {
        let _ = handle.shutdown.send(());
    }
}

pub async fn discover_room(room_code_hash: String) -> AppResult<(SocketAddr, DiscoveryResponse)> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .await
        .map_err(|error| AppError::Network(format!("unable to open discovery socket: {error}")))?;
    socket.set_broadcast(true).map_err(|error| {
        AppError::Network(format!("unable to enable discovery broadcast: {error}"))
    })?;

    let request = DiscoveryRequest {
        kind: "discover_room".to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        room_code_hash,
    };
    let request_id = request.request_id.clone();
    let payload = serde_json::to_vec(&request)?;

    for _ in 0..3 {
        socket
            .send_to(&payload, ("255.255.255.255", DISCOVERY_PORT))
            .await
            .map_err(|error| {
                AppError::Network(format!("unable to broadcast discovery: {error}"))
            })?;
        sleep(Duration::from_millis(150)).await;
    }

    let mut buffer = vec![0u8; 4096];
    loop {
        let response = timeout(Duration::from_secs(5), socket.recv_from(&mut buffer))
            .await
            .map_err(|_| AppError::Timeout("timed out while waiting for room discovery".into()))?
            .map_err(|error| AppError::Network(format!("discovery receive failed: {error}")))?;

        let (size, source) = response;
        let payload: DiscoveryResponse = serde_json::from_slice(&buffer[..size])?;
        if payload.kind == "room_offer" && payload.request_id == request_id {
            return Ok((source, payload));
        }
    }
}

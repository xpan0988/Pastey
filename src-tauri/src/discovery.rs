use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    extract::{ConnectInfo, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Emitter;
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::oneshot,
    time::{sleep, timeout},
};

use crate::{
    error::{AppError, AppResult},
    logging,
    models::{DiscoveryRequest, DiscoveryResponse, JoinRequestPrompt, NearbyDevice},
    storage, AppState,
};

const DISCOVERY_PORT: u16 = 48392;
const BEACON_INTERVAL_SECS: u64 = 2;
const BEACON_TTL_SECS: i64 = 6;
const JOIN_REQUEST_TIMEOUT_SECS: u64 = 30;
const JOIN_REQUEST_CONNECT_TIMEOUT_SECS: u64 = 5;
const JOIN_REQUEST_EVENT: &str = "pastey://join-request";

#[derive(Clone, Debug)]
pub struct NearbyDeviceRecord {
    pub device: NearbyDevice,
    pub source: SocketAddr,
    pub join_request_port: u16,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct PendingJoinRequest {
    pub request_id: String,
    pub source: SocketAddr,
    pub device_name: String,
    pub platform: String,
    pub app_version: String,
    pub received_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct OutgoingJoinRequest {
    pub request_id: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NearbyBeacon {
    pub kind: String,
    pub device_id: String,
    pub display_name: String,
    pub platform: String,
    pub app_version: String,
    pub capabilities: Vec<String>,
    pub listen_port: u16,
    pub room_offer_id: String,
    pub expires_at: i64,
    pub availability: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NearbyJoinRequest {
    pub kind: String,
    pub request_id: String,
    pub device_id: String,
    pub display_name: String,
    pub platform: String,
    pub app_version: String,
    #[serde(default)]
    pub response_port: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NearbyJoinResponse {
    pub kind: String,
    pub request_id: String,
    pub accepted: bool,
    pub message: Option<String>,
    pub room_id: Option<String>,
    pub room_code: Option<String>,
    pub port: Option<u16>,
    pub expires_at: Option<i64>,
    pub transport_public_key: Option<String>,
    pub device_name: Option<String>,
}

pub async fn ensure_service(state: Arc<AppState>) -> AppResult<()> {
    ensure_join_request_service(state.clone()).await?;

    if state.discovery_handle.lock().is_none() {
        let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))
            .await
            .map_err(|error| {
                AppError::Network(format!("unable to bind discovery socket: {error}"))
            })?;
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let service_state = state.clone();

        tokio::spawn(async move {
            let mut buffer = vec![0u8; 4096];

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        logging::write_transfer_line("[pastey antenna] event=antenna_stop");
                        break;
                    }
                    result = socket.recv_from(&mut buffer) => {
                        let Ok((size, source)) = result else {
                            break;
                        };

                        handle_discovery_packet(service_state.clone(), &socket, &buffer[..size], source).await;
                    }
                }
            }
        });

        let mut handle = state.discovery_handle.lock();
        if handle.is_none() {
            *handle = Some(crate::DiscoveryHandle {
                shutdown: shutdown_tx,
            });
            logging::write_transfer_line("[pastey antenna] event=antenna_start");
        }
    }

    Ok(())
}

async fn ensure_join_request_service(state: Arc<AppState>) -> AppResult<()> {
    if state.nearby_http_handle.lock().is_some() {
        return Ok(());
    }

    let router = Router::new()
        .route("/nearby/join-request", post(join_request_endpoint))
        .with_state(state.clone());
    let listener = TcpListener::bind(("0.0.0.0", 0)).await.map_err(|error| {
        AppError::Network(format!("unable to bind nearby join socket: {error}"))
    })?;
    let port = listener
        .local_addr()
        .map_err(|error| {
            AppError::Network(format!("unable to inspect nearby join socket: {error}"))
        })?
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

    let mut handle = state.nearby_http_handle.lock();
    if handle.is_none() {
        *handle = Some(crate::NearbyHttpHandle {
            shutdown: shutdown_tx,
            port,
        });
        logging::write_transfer_line(&format!(
            "[pastey antenna] event=join_request_server_start port={port}"
        ));
    }
    Ok(())
}

pub async fn start_antenna(state: Arc<AppState>) {
    if state.antenna_handle.lock().is_some() {
        return;
    }

    let socket = match UdpSocket::bind(("0.0.0.0", 0)).await {
        Ok(socket) => socket,
        Err(error) => {
            logging::write_error_line(&format!(
                "[pastey antenna] event=antenna_start error={:?}",
                error.to_string()
            ));
            return;
        }
    };
    if let Err(error) = socket.set_broadcast(true) {
        logging::write_error_line(&format!(
            "[pastey antenna] event=antenna_start error={:?}",
            error.to_string()
        ));
        return;
    }

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    state.antenna_handle.lock().replace(crate::DiscoveryHandle {
        shutdown: shutdown_tx,
    });
    let beacon_state = state.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    logging::write_transfer_line("[pastey antenna] event=antenna_stop");
                    break;
                }
                _ = sleep(Duration::from_secs(BEACON_INTERVAL_SECS)) => {
                    expire_nearby_devices(&beacon_state);
                    let beacon = current_beacon(&beacon_state);
                    let Ok(bytes) = serde_json::to_vec(&beacon) else {
                        continue;
                    };
                    if socket.send_to(&bytes, ("255.255.255.255", DISCOVERY_PORT)).await.is_ok() {
                        logging::write_transfer_line("[pastey antenna] event=beacon_sent");
                    }
                }
            }
        }
    });
}

pub async fn stop_antenna(state: Arc<AppState>) {
    if let Some(handle) = state.antenna_handle.lock().take() {
        let _ = handle.shutdown.send(());
    }
    maybe_stop_service(state).await;
}

pub async fn maybe_stop_service(state: Arc<AppState>) {
    if !state.active_servers.lock().is_empty() || state.antenna_handle.lock().is_some() {
        return;
    }

    if let Some(handle) = state.discovery_handle.lock().take() {
        let _ = handle.shutdown.send(());
    }
    if let Some(handle) = state.nearby_http_handle.lock().take() {
        let _ = handle.shutdown.send(());
    }
}

pub fn list_nearby_devices(state: &Arc<AppState>) -> Vec<NearbyDevice> {
    expire_nearby_devices(state);
    let now = storage::now_ts();
    let mut devices = state
        .nearby_devices
        .lock()
        .values()
        .map(|record| {
            let mut device = record.device.clone();
            device.last_seen_seconds_ago =
                now.saturating_sub(record.expires_at - BEACON_TTL_SECS) as u64;
            device
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    devices
}

pub async fn request_nearby_join(
    state: Arc<AppState>,
    device_id: &str,
) -> AppResult<(SocketAddr, NearbyJoinResponse)> {
    expire_nearby_devices(&state);
    let record = state
        .nearby_devices
        .lock()
        .get(device_id)
        .cloned()
        .ok_or_else(|| AppError::NotFound("No nearby Pastey devices found.".into()))?;

    if record.device.availability != "Available" {
        return Err(AppError::InvalidInput("Device is busy.".into()));
    }

    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .await
        .map_err(|_| AppError::Network("Firewall may be blocking Pastey.".into()))?;
    let response_port = socket
        .local_addr()
        .map_err(|_| AppError::Network("Firewall may be blocking Pastey.".into()))?
        .port();
    let request = NearbyJoinRequest {
        kind: "join_request".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        device_id: local_device_id(&state),
        display_name: crate::transfer::device_name(),
        platform: platform_name().into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        response_port: Some(response_port),
    };
    state.outgoing_join_requests.lock().insert(
        request.request_id.clone(),
        OutgoingJoinRequest {
            request_id: request.request_id.clone(),
            created_at: storage::now_ts(),
        },
    );
    let join_url = join_request_url(record.source, record.join_request_port);
    logging::write_transfer_line(&format!(
        "[pastey antenna] event=join_request_url url={join_url}"
    ));
    logging::write_transfer_line("[pastey antenna] event=join_request_attempt");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(JOIN_REQUEST_CONNECT_TIMEOUT_SECS))
        .build()
        .map_err(|_| AppError::Network("Firewall may be blocking Pastey.".into()))?;
    let response = client
        .post(&join_url)
        .json(&request)
        .send()
        .await
        .map_err(|_| {
            state
                .outgoing_join_requests
                .lock()
                .remove(&request.request_id);
            logging::write_transfer_line("[pastey antenna] event=nearby_unreachable");
            logging::write_transfer_line("[pastey antenna] event=blocked_network_suspected");
            AppError::Network("Device found, but Pastey could not connect to it.".into())
        })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    logging::write_transfer_line(&format!(
        "[pastey antenna] event=join_request_response_status status={status}"
    ));
    logging::write_transfer_line(&format!(
        "[pastey antenna] event=join_request_response_body body={body:?}"
    ));
    if !status.is_success() {
        state
            .outgoing_join_requests
            .lock()
            .remove(&request.request_id);
        return Err(AppError::Network(
            "Device found, but Pastey could not connect to it.".into(),
        ));
    }

    let mut buffer = vec![0u8; 4096];
    loop {
        let response = timeout(
            Duration::from_secs(JOIN_REQUEST_TIMEOUT_SECS),
            socket.recv_from(&mut buffer),
        )
        .await
        .map_err(|_| {
            state
                .outgoing_join_requests
                .lock()
                .remove(&request.request_id);
            logging::write_transfer_line("[pastey antenna] event=join_timeout");
            AppError::Timeout("Join request timed out.".into())
        })?
        .map_err(|_| AppError::Network("Firewall may be blocking Pastey.".into()))?;
        let (size, source) = response;
        let Ok(response) = serde_json::from_slice::<NearbyJoinResponse>(&buffer[..size]) else {
            continue;
        };
        if response.kind == "join_response" && response.request_id == request.request_id {
            state
                .outgoing_join_requests
                .lock()
                .remove(&request.request_id);
            return Ok((source, response));
        }
    }
}

pub async fn send_join_response(
    request: &PendingJoinRequest,
    response: &NearbyJoinResponse,
) -> AppResult<()> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .await
        .map_err(|_| AppError::Network("Firewall may be blocking Pastey.".into()))?;
    let payload = serde_json::to_vec(response)?;
    socket
        .send_to(&payload, request.source)
        .await
        .map_err(|_| {
            AppError::Network(
                "Device found, but this network may block direct local connections.".into(),
            )
        })?;
    Ok(())
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

async fn handle_discovery_packet(
    state: Arc<AppState>,
    socket: &UdpSocket,
    bytes: &[u8],
    source: SocketAddr,
) {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    let Some(kind) = value.get("kind").and_then(Value::as_str) else {
        return;
    };

    match kind {
        "discover_room" => handle_room_discovery(state, socket, value, source).await,
        "nearby_beacon" => handle_beacon(state, value, source),
        "join_request" => handle_join_request(state, value, source),
        _ => {}
    }
}

async fn handle_room_discovery(
    state: Arc<AppState>,
    socket: &UdpSocket,
    value: Value,
    source: SocketAddr,
) {
    let Ok(request) = serde_json::from_value::<DiscoveryRequest>(value) else {
        return;
    };

    let response = {
        let servers = state.active_servers.lock();
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
            return;
        };
        let _ = socket.send_to(&bytes, source).await;
    }
}

fn handle_beacon(state: Arc<AppState>, value: Value, source: SocketAddr) {
    let Ok(beacon) = serde_json::from_value::<NearbyBeacon>(value) else {
        return;
    };
    if beacon.device_id == local_device_id(&state) {
        return;
    }
    if !beacon_is_fresh(&beacon, storage::now_ts()) {
        logging::write_transfer_line("[pastey antenna] event=stale_beacon_ignored");
        return;
    }

    let device = NearbyDevice {
        device_id: beacon.device_id.clone(),
        display_name: beacon.display_name,
        platform: beacon.platform,
        app_version: beacon.app_version.clone(),
        availability: beacon.availability,
        capabilities: beacon.capabilities,
        last_seen_seconds_ago: 0,
        compatible: major_version(&beacon.app_version) == major_version(env!("CARGO_PKG_VERSION")),
    };
    state.nearby_devices.lock().insert(
        beacon.device_id,
        NearbyDeviceRecord {
            device,
            source,
            join_request_port: beacon.listen_port,
            expires_at: beacon.expires_at,
        },
    );
    logging::write_transfer_line(&format!(
        "[pastey antenna] event=beacon_received source={source}"
    ));
}

fn handle_join_request(state: Arc<AppState>, value: Value, source: SocketAddr) {
    let Ok(request) = serde_json::from_value::<NearbyJoinRequest>(value) else {
        return;
    };
    let response_port = request.response_port.unwrap_or(source.port());
    surface_join_request(state, request, SocketAddr::new(source.ip(), response_port));
}

async fn join_request_endpoint(
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    Json(request): Json<NearbyJoinRequest>,
) -> impl IntoResponse {
    logging::write_transfer_line(&format!(
        "[pastey antenna] event=join_request_endpoint_hit source={source}"
    ));
    let response_port = request.response_port.unwrap_or(source.port());
    surface_join_request(state, request, SocketAddr::new(source.ip(), response_port));
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "ok": true })),
    )
}

fn surface_join_request(state: Arc<AppState>, request: NearbyJoinRequest, source: SocketAddr) {
    if request.device_id == local_device_id(&state) {
        return;
    }
    if !state.outgoing_join_requests.lock().is_empty() {
        logging::write_transfer_line("[pastey antenna] event=simultaneous_join_detected");
    }
    let now = storage::now_ts();
    let pending = PendingJoinRequest {
        request_id: request.request_id.clone(),
        source,
        device_name: request.display_name,
        platform: request.platform,
        app_version: request.app_version,
        received_at: now,
        expires_at: now + JOIN_REQUEST_TIMEOUT_SECS as i64,
    };
    let prompt = JoinRequestPrompt {
        request_id: pending.request_id.clone(),
        device_name: pending.device_name.clone(),
        platform: pending.platform.clone(),
        app_version: pending.app_version.clone(),
        received_at: pending.received_at,
        expires_at: pending.expires_at,
    };
    state
        .pending_join_requests
        .lock()
        .insert(pending.request_id.clone(), pending);
    logging::write_transfer_line(&format!(
        "[pastey antenna] event=join_request_received source={source}"
    ));
    let _ = state.app_handle.emit(JOIN_REQUEST_EVENT, prompt);
    logging::write_transfer_line("[pastey antenna] event=join_request_emitted_to_ui");
}

fn current_beacon(state: &Arc<AppState>) -> NearbyBeacon {
    let availability = if state.active_servers.lock().is_empty() {
        "Available"
    } else {
        "Busy"
    };
    let now = storage::now_ts();
    NearbyBeacon {
        kind: "nearby_beacon".into(),
        device_id: local_device_id(state),
        display_name: crate::transfer::device_name(),
        platform: platform_name().into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        capabilities: vec!["large_file".into(), "nearby_join".into()],
        listen_port: advertised_join_request_port(state),
        room_offer_id: local_device_id(state),
        expires_at: now + BEACON_TTL_SECS,
        availability: availability.into(),
    }
}

fn advertised_join_request_port(state: &Arc<AppState>) -> u16 {
    state
        .nearby_http_handle
        .lock()
        .as_ref()
        .map(|handle| handle.port)
        .unwrap_or_default()
}

fn join_request_url(source: SocketAddr, listen_port: u16) -> String {
    format!("http://{}:{listen_port}/nearby/join-request", source.ip())
}

fn expire_nearby_devices(state: &Arc<AppState>) {
    let now = storage::now_ts();
    state.nearby_devices.lock().retain(|_, record| {
        let fresh = record.expires_at > now;
        if !fresh {
            logging::write_transfer_line("[pastey antenna] event=beacon_expired");
        }
        fresh
    });
}

fn beacon_is_fresh(beacon: &NearbyBeacon, now: i64) -> bool {
    beacon.expires_at > now
}

pub fn pending_join_prompt(request: &PendingJoinRequest) -> JoinRequestPrompt {
    JoinRequestPrompt {
        request_id: request.request_id.clone(),
        device_name: request.device_name.clone(),
        platform: request.platform.clone(),
        app_version: request.app_version.clone(),
        received_at: request.received_at,
        expires_at: request.expires_at,
    }
}

pub fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Desktop"
    }
}

pub fn local_device_id(state: &Arc<AppState>) -> String {
    state.config.read().device_id.clone()
}

fn major_version(version: &str) -> Option<&str> {
    version.split('.').next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacon_serialization_contains_no_sensitive_fields() {
        let beacon = NearbyBeacon {
            kind: "nearby_beacon".into(),
            device_id: "device".into(),
            display_name: "MacBook Air".into(),
            platform: "macOS".into(),
            app_version: "1.4.0".into(),
            capabilities: vec!["large_file".into()],
            listen_port: 43123,
            room_offer_id: "offer".into(),
            expires_at: 123,
            availability: "Available".into(),
        };
        let json = serde_json::to_string(&beacon).unwrap();

        assert!(!json.contains("room_code"));
        assert!(!json.contains("room_key"));
        assert!(!json.contains("file_name"));
        assert!(!json.contains("saved_path"));
        assert!(!json.contains("app_secret"));
    }

    #[test]
    fn pending_join_prompt_exposes_no_network_endpoint() {
        let request = PendingJoinRequest {
            request_id: "request".into(),
            source: "127.0.0.1:12345".parse().unwrap(),
            device_name: "Windows PC".into(),
            platform: "Windows".into(),
            app_version: "1.4.0".into(),
            received_at: 1,
            expires_at: 31,
        };

        let prompt = pending_join_prompt(&request);
        let json = serde_json::to_string(&prompt).unwrap();

        assert!(!json.contains("127.0.0.1"));
        assert!(!json.contains("12345"));
    }

    #[test]
    fn nearby_device_ui_model_exposes_no_network_endpoint() {
        let device = NearbyDevice {
            device_id: "device".into(),
            display_name: "SuperDiao".into(),
            platform: "macOS".into(),
            app_version: "1.4.0".into(),
            availability: "Available".into(),
            capabilities: vec!["large_file".into()],
            last_seen_seconds_ago: 0,
            compatible: true,
        };

        let json = serde_json::to_string(&device).unwrap();

        assert!(!json.contains("ip"));
        assert!(!json.contains("port"));
        assert!(!json.contains("endpoint"));
    }

    #[test]
    fn rejected_join_response_does_not_create_room_details() {
        let response = NearbyJoinResponse {
            kind: "join_response".into(),
            request_id: "request".into(),
            accepted: false,
            message: Some("Join request rejected.".into()),
            room_id: None,
            room_code: None,
            port: None,
            expires_at: None,
            transport_public_key: None,
            device_name: Some("MacBook Air".into()),
        };

        assert!(!response.accepted);
        assert!(response.room_id.is_none());
        assert!(response.room_code.is_none());
        assert!(response.transport_public_key.is_none());
    }

    #[test]
    fn beacon_expiry_rejects_stale_devices() {
        let mut beacon = NearbyBeacon {
            kind: "nearby_beacon".into(),
            device_id: "device".into(),
            display_name: "Windows PC".into(),
            platform: "Windows".into(),
            app_version: "1.4.0".into(),
            capabilities: vec!["large_file".into()],
            listen_port: 43123,
            room_offer_id: "offer".into(),
            expires_at: 10,
            availability: "Available".into(),
        };

        assert!(beacon_is_fresh(&beacon, 9));
        assert!(!beacon_is_fresh(&beacon, 10));
        beacon.expires_at = 11;
        assert!(beacon_is_fresh(&beacon, 10));
    }

    #[test]
    fn join_request_url_uses_advertised_http_port_not_udp_source_port() {
        let source: SocketAddr = "192.168.1.9:54321".parse().unwrap();

        assert_eq!(
            join_request_url(source, 43123),
            "http://192.168.1.9:43123/nearby/join-request"
        );
    }
}

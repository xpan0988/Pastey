use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    process::Command,
    sync::Arc,
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use socket2::{Domain, Protocol, Socket, Type};
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
const BEACON_TTL_SECS: i64 = 12;
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiscoveryInterface {
    name: String,
    ipv4: Ipv4Addr,
    broadcast: Option<Ipv4Addr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BeaconUpdateDiagnostics {
    is_new: bool,
    availability_changed: bool,
    source_changed: bool,
    previous_availability: Option<String>,
    previous_source: Option<SocketAddr>,
    last_seen_at: i64,
    expires_at: i64,
    compatible: bool,
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
        let socket = bind_discovery_socket()?;
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

fn bind_discovery_socket() -> AppResult<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|error| {
        AppError::Network(format!("unable to create discovery socket: {error}"))
    })?;
    socket.set_reuse_address(true).map_err(|error| {
        AppError::Network(format!("unable to reuse discovery socket address: {error}"))
    })?;
    #[cfg(unix)]
    socket.set_reuse_port(true).map_err(|error| {
        AppError::Network(format!("unable to reuse discovery socket port: {error}"))
    })?;
    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT).into())
        .map_err(|error| AppError::Network(format!("unable to bind discovery socket: {error}")))?;
    logging::write_transfer_line(&format!(
        "[pastey antenna] event=discovery_socket_bound bind=0.0.0.0:{DISCOVERY_PORT} so_reuseaddr=true so_reuseport={} ipv4=true ipv6=false",
        cfg!(unix)
    ));
    socket.set_nonblocking(true).map_err(|error| {
        AppError::Network(format!(
            "unable to set discovery socket nonblocking: {error}"
        ))
    })?;
    UdpSocket::from_std(socket.into()).map_err(|error| {
        AppError::Network(format!("unable to initialize discovery socket: {error}"))
    })
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
    let bind_addr = socket
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "unknown".into());
    let interfaces = discovery_interfaces();
    let broadcast_targets = broadcast_targets(&interfaces);
    logging::write_transfer_line(&format!(
        "[pastey antenna] event=beacon_socket_config bind={bind_addr} udp_port={DISCOVERY_PORT} ipv4=true ipv6=false so_broadcast=true targets={:?} interfaces={:?}",
        broadcast_targets
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        interfaces
            .iter()
            .map(|interface| format!(
                "{}:{}/{}",
                interface.name,
                interface.ipv4,
                interface
                    .broadcast
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|| "no_broadcast".into())
            ))
            .collect::<Vec<_>>()
    ));

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
                    let mut sent = 0usize;
                    let mut failed = 0usize;
                    for target in &broadcast_targets {
                        let target_addr = SocketAddrV4::new(*target, DISCOVERY_PORT);
                        match socket.send_to(&bytes, target_addr).await {
                            Ok(_) => sent += 1,
                            Err(error) => {
                                failed += 1;
                                logging::write_error_line(&format!(
                                    "[pastey antenna] event=beacon_send_failed target={target_addr} error={:?}",
                                    error.to_string()
                                ));
                            }
                        }
                    }
                    if sent > 0 {
                        logging::write_transfer_line(&format!(
                            "[pastey antenna] event=beacon_sent targets_sent={sent} targets_failed={failed} peer_identity_key={}",
                            diagnostic_ref(&beacon.device_id)
                        ));
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
                now.saturating_sub(last_seen_at_from_record(record)) as u64;
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
    let received_at = storage::now_ts();
    let sender_expires_in = beacon.expires_at - received_at;
    let peer_identity_key = diagnostic_ref(&beacon.device_id);
    let listen_port = beacon.listen_port;
    let diagnostics = {
        let mut records = state.nearby_devices.lock();
        upsert_nearby_device(&mut records, beacon, source, received_at)
    };
    let transition_reason = if diagnostics.is_new {
        "new_peer"
    } else if diagnostics.availability_changed {
        "availability_changed"
    } else if diagnostics.source_changed {
        "endpoint_changed"
    } else {
        "refresh"
    };
    logging::write_transfer_line(&format!(
        "[pastey antenna] event=beacon_received source={source} peer_identity_key={peer_identity_key} listen_port={listen_port} last_seen_at={} local_expires_at={} sender_expires_in={sender_expires_in} transition_reason={transition_reason} previous_availability={:?} previous_source={:?} compatible={}",
        diagnostics.last_seen_at,
        diagnostics.expires_at,
        diagnostics.previous_availability,
        diagnostics.previous_source,
        diagnostics.compatible
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
    state.nearby_devices.lock().retain(|device_id, record| {
        let fresh = record.expires_at > now;
        if !fresh {
            let last_seen_at = last_seen_at_from_record(record);
            logging::write_transfer_line(&format!(
                "[pastey antenna] event=availability_transition peer_identity_key={} previous_availability={:?} next_availability=Expired reason=ttl_elapsed last_seen_at={last_seen_at} expired_at={} now={now}",
                diagnostic_ref(device_id),
                record.device.availability,
                record.expires_at
            ));
        }
        fresh
    });
}

fn upsert_nearby_device(
    records: &mut HashMap<String, NearbyDeviceRecord>,
    beacon: NearbyBeacon,
    source: SocketAddr,
    received_at: i64,
) -> BeaconUpdateDiagnostics {
    let previous = records.get(&beacon.device_id);
    let previous_availability = previous.map(|record| record.device.availability.clone());
    let previous_source = previous.map(|record| record.source);
    let availability_changed = previous
        .map(|record| record.device.availability != beacon.availability)
        .unwrap_or(false);
    let source_changed = previous
        .map(|record| record.source != source || record.join_request_port != beacon.listen_port)
        .unwrap_or(false);
    let compatible = major_version(&beacon.app_version) == major_version(env!("CARGO_PKG_VERSION"));
    let expires_at = received_beacon_expires_at(received_at);
    let device_id = beacon.device_id.clone();
    let device = NearbyDevice {
        device_id: device_id.clone(),
        display_name: beacon.display_name,
        platform: beacon.platform,
        app_version: beacon.app_version,
        availability: beacon.availability,
        capabilities: beacon.capabilities,
        last_seen_seconds_ago: 0,
        compatible,
    };

    records.insert(
        device_id,
        NearbyDeviceRecord {
            device,
            source,
            join_request_port: beacon.listen_port,
            expires_at,
        },
    );

    BeaconUpdateDiagnostics {
        is_new: previous_availability.is_none(),
        availability_changed,
        source_changed,
        previous_availability,
        previous_source,
        last_seen_at: received_at,
        expires_at,
        compatible,
    }
}

fn received_beacon_expires_at(received_at: i64) -> i64 {
    received_at + BEACON_TTL_SECS
}

fn last_seen_at_from_record(record: &NearbyDeviceRecord) -> i64 {
    record.expires_at - BEACON_TTL_SECS
}

fn broadcast_targets(interfaces: &[DiscoveryInterface]) -> Vec<Ipv4Addr> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    add_broadcast_target(Ipv4Addr::BROADCAST, &mut seen, &mut targets);

    for interface in interfaces {
        if let Some(broadcast) = interface.broadcast {
            add_broadcast_target(broadcast, &mut seen, &mut targets);
        }
    }

    targets
}

fn add_broadcast_target(
    target: Ipv4Addr,
    seen: &mut HashSet<Ipv4Addr>,
    targets: &mut Vec<Ipv4Addr>,
) {
    if target.is_unspecified() || target.is_loopback() {
        return;
    }
    if seen.insert(target) {
        targets.push(target);
    }
}

fn discovery_interfaces() -> Vec<DiscoveryInterface> {
    parse_ip_addr_output(&command_stdout(
        &["ip", "/sbin/ip", "/usr/sbin/ip"],
        &["-o", "-4", "addr", "show", "up"],
    ))
    .or_else(|| parse_ifconfig_output(&command_stdout(&["ifconfig", "/sbin/ifconfig"], &[])))
    .unwrap_or_default()
}

fn command_stdout(commands: &[&str], args: &[&str]) -> Option<String> {
    for command in commands {
        let Ok(output) = Command::new(command).args(args).output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        if let Ok(stdout) = String::from_utf8(output.stdout) {
            return Some(stdout);
        }
    }
    None
}

fn parse_ip_addr_output(output: &Option<String>) -> Option<Vec<DiscoveryInterface>> {
    let output = output.as_ref()?;
    let mut interfaces = Vec::new();

    for line in output.lines() {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() < 4 || parts.get(2) != Some(&"inet") {
            continue;
        }
        let name = parts
            .get(1)
            .map(|value| value.trim_end_matches(':').to_string())
            .unwrap_or_else(|| "unknown".into());
        let Some(ipv4) = parts
            .get(3)
            .and_then(|cidr| cidr.split('/').next())
            .and_then(parse_ipv4)
        else {
            continue;
        };
        if ipv4.is_loopback() {
            continue;
        }
        let broadcast = parts
            .windows(2)
            .find_map(|window| (window[0] == "brd").then(|| window[1]))
            .and_then(parse_ipv4);
        interfaces.push(DiscoveryInterface {
            name,
            ipv4,
            broadcast,
        });
    }

    Some(interfaces)
}

fn parse_ifconfig_output(output: &Option<String>) -> Option<Vec<DiscoveryInterface>> {
    let output = output.as_ref()?;
    let mut interfaces = Vec::new();
    let mut current_name = String::from("unknown");

    for line in output.lines() {
        let starts_with_whitespace = line.chars().next().is_some_and(char::is_whitespace);
        if !starts_with_whitespace && line.contains(':') {
            current_name = line
                .split(':')
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or("unknown")
                .to_string();
            continue;
        }

        let parts = line.split_whitespace().collect::<Vec<_>>();
        let Some(inet_index) = parts.iter().position(|part| *part == "inet") else {
            continue;
        };
        let Some(ipv4) = parts.get(inet_index + 1).and_then(|part| parse_ipv4(part)) else {
            continue;
        };
        if ipv4.is_loopback() {
            continue;
        }
        let broadcast = parts
            .windows(2)
            .find_map(|window| (window[0] == "broadcast").then(|| window[1]))
            .and_then(parse_ipv4);
        interfaces.push(DiscoveryInterface {
            name: current_name.clone(),
            ipv4,
            broadcast,
        });
    }

    Some(interfaces)
}

fn parse_ipv4(value: &str) -> Option<Ipv4Addr> {
    value.parse::<Ipv4Addr>().ok()
}

fn diagnostic_ref(value: &str) -> String {
    value.chars().take(12).collect()
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
    } else if cfg!(target_os = "linux") {
        "Linux"
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
    fn received_beacon_ttl_tolerates_heartbeat_jitter() {
        let received_at = 100;
        let expires_at = received_beacon_expires_at(received_at);

        assert!(BEACON_TTL_SECS >= (BEACON_INTERVAL_SECS as i64 * 5));
        assert_eq!(expires_at, 112);
        assert!(expires_at > received_at + 8);
    }

    #[test]
    fn repeated_beacons_update_existing_peer_without_flicker() {
        let mut records = HashMap::new();
        let source: SocketAddr = "192.168.1.10:48392".parse().unwrap();
        let mut beacon = test_beacon("device-one", "Available", 12);

        let first = upsert_nearby_device(&mut records, beacon.clone(), source, 100);
        assert!(first.is_new);
        assert_eq!(records.len(), 1);
        assert_eq!(records["device-one"].expires_at, 112);

        beacon.expires_at = -999;
        let second = upsert_nearby_device(&mut records, beacon, source, 104);
        assert!(!second.is_new);
        assert!(!second.availability_changed);
        assert!(!second.source_changed);
        assert_eq!(records.len(), 1);
        assert_eq!(last_seen_at_from_record(&records["device-one"]), 104);
        assert_eq!(records["device-one"].expires_at, 116);
    }

    #[test]
    fn device_identity_is_stable_across_endpoint_changes() {
        let mut records = HashMap::new();
        let source: SocketAddr = "192.168.1.10:48392".parse().unwrap();
        let mut beacon = test_beacon("device-one", "Available", 12);
        upsert_nearby_device(&mut records, beacon.clone(), source, 100);

        let new_source: SocketAddr = "192.168.1.11:48392".parse().unwrap();
        beacon.listen_port = 50123;
        let update = upsert_nearby_device(&mut records, beacon, new_source, 102);

        assert!(!update.is_new);
        assert!(update.source_changed);
        assert_eq!(records.len(), 1);
        assert_eq!(records["device-one"].source, new_source);
        assert_eq!(records["device-one"].join_request_port, 50123);
    }

    #[test]
    fn broadcast_targets_include_limited_and_interface_broadcasts() {
        let interfaces = vec![
            DiscoveryInterface {
                name: "en0".into(),
                ipv4: "192.168.1.20".parse().unwrap(),
                broadcast: Some("192.168.1.255".parse().unwrap()),
            },
            DiscoveryInterface {
                name: "wlan0".into(),
                ipv4: "10.0.0.9".parse().unwrap(),
                broadcast: Some("10.0.0.255".parse().unwrap()),
            },
        ];

        let targets = broadcast_targets(&interfaces);

        assert_eq!(
            targets,
            vec![
                Ipv4Addr::BROADCAST,
                "192.168.1.255".parse().unwrap(),
                "10.0.0.255".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn parses_linux_ip_addr_broadcast_interfaces() {
        let output = Some(
            "2: enp0s1    inet 192.168.1.20/24 brd 192.168.1.255 scope global dynamic enp0s1\n\
             1: lo    inet 127.0.0.1/8 scope host lo\n"
                .to_string(),
        );

        let interfaces = parse_ip_addr_output(&output).unwrap();

        assert_eq!(
            interfaces,
            vec![DiscoveryInterface {
                name: "enp0s1".into(),
                ipv4: "192.168.1.20".parse().unwrap(),
                broadcast: Some("192.168.1.255".parse().unwrap()),
            }]
        );
    }

    #[test]
    fn parses_macos_ifconfig_broadcast_interfaces() {
        let output = Some(
            "en0: flags=8863<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\
            \tinet 192.168.1.30 netmask 0xffffff00 broadcast 192.168.1.255\n\
             lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384\n\
            \tinet 127.0.0.1 netmask 0xff000000\n"
                .to_string(),
        );

        let interfaces = parse_ifconfig_output(&output).unwrap();

        assert_eq!(
            interfaces,
            vec![DiscoveryInterface {
                name: "en0".into(),
                ipv4: "192.168.1.30".parse().unwrap(),
                broadcast: Some("192.168.1.255".parse().unwrap()),
            }]
        );
    }

    #[test]
    fn join_request_url_uses_advertised_http_port_not_udp_source_port() {
        let source: SocketAddr = "192.168.1.9:54321".parse().unwrap();

        assert_eq!(
            join_request_url(source, 43123),
            "http://192.168.1.9:43123/nearby/join-request"
        );
    }

    fn test_beacon(device_id: &str, availability: &str, expires_at: i64) -> NearbyBeacon {
        NearbyBeacon {
            kind: "nearby_beacon".into(),
            device_id: device_id.into(),
            display_name: "Linux Box".into(),
            platform: "Linux".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec!["large_file".into(), "nearby_join".into()],
            listen_port: 43123,
            room_offer_id: "offer".into(),
            expires_at,
            availability: availability.into(),
        }
    }
}

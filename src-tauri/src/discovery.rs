use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
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
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::oneshot,
    time::{sleep, timeout},
};

use crate::{
    bridge_auth,
    error::{AppError, AppResult},
    host_runtime::{DiscoveryHandle, HostRuntime as AppState, NearbyHttpHandle},
    logging,
    models::{JoinRequestPrompt, NearbyDevice},
    storage,
};

const DISCOVERY_PORT: u16 = 48392;
const BEACON_INTERVAL_SECS: u64 = 2;
const BEACON_TTL_SECS: i64 = 12;
const JOIN_REQUEST_TIMEOUT_SECS: u64 = 30;
const JOIN_REQUEST_REPLAY_TTL_SECS: i64 = 300;
const JOIN_REQUEST_CONNECT_TIMEOUT_SECS: u64 = 5;
const JOIN_REQUEST_EVENT: &str = "pastey://join-request";
const MAX_PENDING_JOIN_REQUESTS: usize = 4;

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
pub struct OutgoingJoinRequest;

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
    /// Present when the requester paired with this device before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known: Option<bridge_auth::KnownConnectProof>,
}

/// The answer to a nearby join request. It says only whether the other
/// user accepted: the one-time code is shown on that device's screen and
/// typed here, never sent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NearbyJoinResponse {
    pub kind: String,
    pub request_id: String,
    pub accepted: bool,
    pub message: Option<String>,
    pub device_name: Option<String>,
    /// For a verified known device: the new Bridge's signed offer, which
    /// the requester completes with its identity key. Never a code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_offer: Option<bridge_auth::ReconnectOffer>,
    /// With `known_offer`: the Bridge's display code, so both devices show
    /// the same name. No pairing code is open for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_label: Option<String>,
}

/// A nearby request the other device accepted; this device's user must
/// still enter the code shown there.
#[derive(Clone, Debug)]
pub struct AcceptedNearbyJoin {
    pub source: SocketAddr,
    pub expires_at: i64,
}

pub async fn ensure_service(state: Arc<AppState>) -> AppResult<()> {
    ensure_join_request_service(state.clone()).await?;

    if state.discovery_handle.lock().is_none() {
        let socket = bind_discovery_socket()?;
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let service_state = state.clone();

        state.spawn(async move {
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
            *handle = Some(DiscoveryHandle {
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

    state.spawn(async move {
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
        *handle = Some(NearbyHttpHandle {
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
    state.antenna_handle.lock().replace(DiscoveryHandle {
        shutdown: shutdown_tx,
    });
    let beacon_state = state.clone();

    state.spawn(async move {
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
    for device in &mut devices {
        device.known = crate::host_identity::HostRef::from_device_id(&device.device_id)
            .ok()
            .and_then(|host_ref| {
                storage::known_device_for_host_ref(&state.paths, host_ref.as_str()).ok()
            })
            .flatten()
            .is_some();
    }
    devices.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    devices
}

pub async fn request_nearby_join(
    state: Arc<AppState>,
    device_id: &str,
    request_id: String,
    known: Option<bridge_auth::KnownConnectProof>,
) -> AppResult<(SocketAddr, NearbyJoinResponse)> {
    expire_nearby_devices(&state);
    let record = state
        .nearby_devices
        .lock()
        .get(device_id)
        .cloned()
        .ok_or_else(|| AppError::NotFound("No nearby Pastey devices found.".into()))?;

    // A device with a Bridge already is Busy to new devices, but a device it
    // paired with before may start another Bridge with it.
    if record.device.availability != "Available" && known.is_none() {
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
        request_id,
        device_id: local_device_id(&state),
        display_name: crate::transfer::device_name(),
        platform: platform_name().into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        response_port: Some(response_port),
        known,
    };
    state
        .outgoing_join_requests
        .lock()
        .insert(request.request_id.clone(), OutgoingJoinRequest);
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
    send_join_response_to(request.source, response).await
}

pub async fn send_join_response_to(
    target: SocketAddr,
    response: &NearbyJoinResponse,
) -> AppResult<()> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .await
        .map_err(|_| AppError::Network("Firewall may be blocking Pastey.".into()))?;
    let payload = serde_json::to_vec(response)?;
    socket.send_to(&payload, target).await.map_err(|_| {
        AppError::Network(
            "Device found, but this network may block direct local connections.".into(),
        )
    })?;
    Ok(())
}

/// How long a unicast room query to a peer on another network may take. An
/// overlay path (Tailscale, WireGuard) can add hundreds of milliseconds and
/// jitter, and a relayed path more.
const UNICAST_DISCOVERY_WAIT: Duration = Duration::from_secs(10);
const UNICAST_DISCOVERY_SENDS: usize = 4;
const MAX_PEER_ADDRESS_LENGTH: usize = 253;

/// A peer address the user typed to reach a Host on another network, such
/// as its overlay (Tailscale or WireGuard) IP or name. It only says where to
/// send the existing room query: the Bridge code, the room identity and the
/// transport key exchange still decide whether a Bridge forms, and an address
/// never authorizes anything.
pub fn validate_peer_address(input: &str) -> AppResult<String> {
    let address = input.trim();
    let invalid = |message: &str| Err(AppError::InvalidInput(message.into()));
    if address.is_empty() || address.len() > MAX_PEER_ADDRESS_LENGTH {
        return invalid("Enter the other device's network address or name.");
    }
    if let Ok(ip) = address.parse::<IpAddr>() {
        let IpAddr::V4(ip) = ip else {
            return invalid("Use the other device's IPv4 address or its network name.");
        };
        if !is_device_ipv4(ip) {
            return invalid("That address cannot be another device.");
        }
        if !is_private_or_overlay_ipv4(ip) {
            return invalid(PUBLIC_ADDRESS_REFUSAL);
        }
        return Ok(address.to_owned());
    }
    // A DNS name, such as a Tailscale MagicDNS name. No scheme, port, path
    // or credentials: Pastey uses its own fixed ports.
    let labels_ok = address.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    });
    if !labels_ok
        || address
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return invalid("Enter only the device's address or name, without a port, path or scheme.");
    }
    // A system resolver reads names like `0x08080808` or `0x7f.1` as numeric
    // IPv4 addresses without any lookup; they are addresses, not names.
    let numeric_label = |label: &str| {
        label.bytes().all(|byte| byte.is_ascii_digit())
            || label
                .strip_prefix("0x")
                .or_else(|| label.strip_prefix("0X"))
                .is_some_and(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
    };
    if address.split('.').all(numeric_label) {
        return invalid(
            "Enter an IPv4 address as four decimal numbers, or the device's network name.",
        );
    }
    Ok(address.to_ascii_lowercase())
}

const PUBLIC_ADDRESS_REFUSAL: &str = "Use an address on a private or overlay network (for example Tailscale or WireGuard). Pastey must not be exposed directly to the internet.";

/// Whether an IPv4 address can name another device at all.
fn is_device_ipv4(ip: Ipv4Addr) -> bool {
    !(ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_documentation())
}

/// Private, carrier-grade NAT (Tailscale's 100.64.0.0/10) or other overlay
/// ranges. A public address would mean Pastey is exposed directly to the
/// internet, which it must not be. Applies to an address entered directly
/// and to every address a name resolves to.
fn is_private_or_overlay_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    let shared = octets[0] == 100 && (64..=127).contains(&octets[1]);
    is_device_ipv4(ip) && (ip.is_private() || shared)
}

/// How long a LAN pairing or reconnect query waits for an answer.
const LAN_DISCOVERY_WAIT: Duration = Duration::from_secs(5);
const LAN_DISCOVERY_SENDS: usize = 3;

/// Where a pairing or reconnect query goes, and whose answers are read.
/// Every answer is still authenticated on its own; the source only narrows
/// what is read.
#[derive(Clone, Debug)]
pub(crate) enum DiscoveryTargets {
    /// This network's broadcast. Any device may answer.
    Lan,
    /// Exactly these addresses, and only answers from their IPs.
    Unicast(Vec<SocketAddr>),
}

impl DiscoveryTargets {
    /// The discovery service of one nearby device that accepted a request.
    pub(crate) fn nearby(source: SocketAddr) -> Self {
        Self::Unicast(vec![SocketAddr::new(source.ip(), DISCOVERY_PORT)])
    }
}

/// Resolves a peer address the user gave (an overlay IP or name) to the
/// private or overlay IPv4 addresses it names now. Every reconnect resolves
/// it again; nothing is sent to any other address.
pub(crate) async fn resolve_peer_targets(address: &str) -> AppResult<DiscoveryTargets> {
    let address = validate_peer_address(address)?;
    let targets = timeout(
        UNICAST_DISCOVERY_WAIT,
        tokio::net::lookup_host((address.as_str(), DISCOVERY_PORT)),
    )
    .await
    .map_err(|_| {
        AppError::Timeout("The other device's name could not be resolved in time.".into())
    })?
    .map_err(|_| AppError::Network("The other device's name could not be resolved.".into()))?
    .filter(SocketAddr::is_ipv4)
    .collect::<Vec<_>>();
    if targets.is_empty() {
        return Err(AppError::Network(
            "The other device's name has no IPv4 address.".into(),
        ));
    }
    let targets = targets
        .into_iter()
        .filter(|target| match target {
            SocketAddr::V4(target) => is_private_or_overlay_ipv4(*target.ip()),
            SocketAddr::V6(_) => false,
        })
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return Err(AppError::InvalidInput(PUBLIC_ADDRESS_REFUSAL.into()));
    }
    Ok(DiscoveryTargets::Unicast(targets))
}

/// Finds the Bridge whose open pairing code is `code`. Only an offer whose
/// SPAKE2 key confirmation proves the same code is returned; anything else,
/// from anyone, is ignored.
pub(crate) async fn discover_pairing(
    state: &Arc<AppState>,
    code: &str,
    targets: &DiscoveryTargets,
) -> AppResult<(SocketAddr, bridge_auth::VerifiedPairOffer)> {
    let mut attempt = bridge_auth::PairAttempt::new(code);
    let request_id = attempt.request_id().to_string();
    let payload = serde_json::to_vec(&attempt.query())?;
    state
        .bridge_auth
        .lock()
        .outgoing_pair_requests
        .insert(request_id.clone());
    let result = exchange(targets, &payload, |bytes| {
        serde_json::from_slice::<bridge_auth::PairOffer>(bytes)
            .ok()
            .and_then(|offer| attempt.verify_offer(&offer))
    })
    .await;
    state
        .bridge_auth
        .lock()
        .outgoing_pair_requests
        .remove(&request_id);
    result
}

/// Finds a pinned Bridge's peer again. Only an offer signed by the pinned
/// device key for this attempt's nonce is returned.
pub(crate) async fn discover_reconnect(
    attempt: &bridge_auth::ReconnectAttempt,
    targets: &DiscoveryTargets,
) -> AppResult<(SocketAddr, bridge_auth::VerifiedReconnectOffer)> {
    let payload = serde_json::to_vec(&attempt.query())?;
    exchange(targets, &payload, |bytes| {
        serde_json::from_slice::<bridge_auth::ReconnectOffer>(bytes)
            .ok()
            .and_then(|offer| attempt.verify_offer(&offer))
    })
    .await
}

async fn exchange<T>(
    targets: &DiscoveryTargets,
    payload: &[u8],
    accept: impl FnMut(&[u8]) -> Option<T>,
) -> AppResult<(SocketAddr, T)> {
    match targets {
        DiscoveryTargets::Lan => {
            exchange_with(
                &[SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT))],
                true,
                None,
                payload,
                LAN_DISCOVERY_WAIT,
                LAN_DISCOVERY_SENDS,
                accept,
            )
            .await
        }
        DiscoveryTargets::Unicast(addresses) => {
            exchange_with(
                addresses,
                false,
                Some(addresses),
                payload,
                UNICAST_DISCOVERY_WAIT,
                UNICAST_DISCOVERY_SENDS,
                accept,
            )
            .await
        }
    }
}

/// Sends one query (resending over a lossy path) and returns the first
/// answer `accept` takes. `accept_from: None` reads answers from any source,
/// as a broadcast must; the answer's own authentication then decides.
async fn exchange_with<T>(
    send_to: &[SocketAddr],
    broadcast: bool,
    accept_from: Option<&[SocketAddr]>,
    payload: &[u8],
    wait: Duration,
    sends: usize,
    mut accept: impl FnMut(&[u8]) -> Option<T>,
) -> AppResult<(SocketAddr, T)> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .await
        .map_err(|error| AppError::Network(format!("unable to open discovery socket: {error}")))?;
    if broadcast {
        socket.set_broadcast(true).map_err(|error| {
            AppError::Network(format!("unable to enable discovery broadcast: {error}"))
        })?;
    }
    let deadline = tokio::time::Instant::now() + wait;
    let resend_every = wait / sends.max(1) as u32;
    let mut next_send = tokio::time::Instant::now();
    let mut sent = 0;
    let mut buffer = vec![0u8; 4096];
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(AppError::Timeout(
                "The other device did not answer. Check the code, that Pastey is open on the other device, and, for another network, that both devices are on the same overlay network.".into(),
            ));
        }
        if sent < sends && now >= next_send {
            for target in send_to {
                let _ = socket.send_to(payload, target).await;
            }
            sent += 1;
            next_send = now + resend_every;
        }
        let until = if sent < sends {
            next_send.min(deadline)
        } else {
            deadline
        };
        let Ok(received) = tokio::time::timeout_at(until, socket.recv_from(&mut buffer)).await
        else {
            continue;
        };
        let (size, source) = received
            .map_err(|error| AppError::Network(format!("discovery receive failed: {error}")))?;
        if accept_from
            .is_some_and(|allowed| !allowed.iter().any(|target| target.ip() == source.ip()))
        {
            continue;
        }
        if let Some(found) = accept(&buffer[..size]) {
            return Ok((source, found));
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
        "pair_query" => handle_pair_query(state, socket, value, source).await,
        "reconnect_query" => handle_reconnect_query(state, socket, value, source).await,
        "nearby_beacon" => handle_beacon(state, value, source),
        "join_request" => handle_join_request(state, value, source),
        _ => {}
    }
}

async fn handle_pair_query(
    state: Arc<AppState>,
    socket: &UdpSocket,
    value: Value,
    source: SocketAddr,
) {
    let Ok(query) = serde_json::from_value::<bridge_auth::PairQuery>(value) else {
        return;
    };
    for offer in bridge_auth::answer_pair_query(&state, &query) {
        if let Ok(bytes) = serde_json::to_vec(&offer) {
            let _ = socket.send_to(&bytes, source).await;
        }
    }
}

async fn handle_reconnect_query(
    state: Arc<AppState>,
    socket: &UdpSocket,
    value: Value,
    source: SocketAddr,
) {
    let Ok(query) = serde_json::from_value::<bridge_auth::ReconnectQuery>(value) else {
        return;
    };
    if let Some(offer) = bridge_auth::answer_reconnect_query(&state, &query) {
        if let Ok(bytes) = serde_json::to_vec(&offer) {
            let _ = socket.send_to(&bytes, source).await;
        }
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
    route_join_request(state, request, SocketAddr::new(source.ip(), response_port));
}

/// A request from a device that proves it paired with this one before
/// connects without a prompt; anything else asks this device's user.
fn route_join_request(state: Arc<AppState>, request: NearbyJoinRequest, source: SocketAddr) {
    if request.known.is_none() {
        surface_join_request(state, request, source);
        return;
    }
    let task_state = state.clone();
    state.spawn(async move {
        if !crate::commands::accept_known_device_connect(task_state.clone(), &request, source).await
        {
            surface_join_request(task_state, request, source);
        }
    });
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
    route_join_request(state, request, SocketAddr::new(source.ip(), response_port));
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "ok": true })),
    )
}

fn surface_join_request(state: Arc<AppState>, request: NearbyJoinRequest, source: SocketAddr) {
    if request.device_id == local_device_id(&state) {
        return;
    }
    {
        // A prompt is a human gate, not authentication. Bound how many any
        // one address, and all addresses together, can raise.
        let now = storage::now_ts();
        let mut pending = state.pending_join_requests.lock();
        pending.retain(|_, request| request.expires_at > now);
        if pending.len() >= MAX_PENDING_JOIN_REQUESTS
            || pending
                .values()
                .any(|request| request.source.ip() == source.ip())
        {
            logging::write_transfer_line("[pastey antenna] event=join_request_rate_limited");
            return;
        }
    }
    if !state.outgoing_join_requests.lock().is_empty() {
        logging::write_transfer_line("[pastey antenna] event=simultaneous_join_detected");
    }
    let now = storage::now_ts();
    let expires_at = now + JOIN_REQUEST_TIMEOUT_SECS as i64;
    if !claim_nearby_join_request(
        &mut state.seen_nearby_join_requests.lock(),
        &request.request_id,
        now,
        now + JOIN_REQUEST_REPLAY_TTL_SECS,
    ) {
        logging::write_transfer_line("[pastey antenna] event=join_request_replay_rejected");
        return;
    }
    let pending = PendingJoinRequest {
        request_id: request.request_id.clone(),
        source,
        device_name: request.display_name,
        platform: request.platform,
        app_version: request.app_version,
        received_at: now,
        expires_at,
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
    let _ = state.emit(JOIN_REQUEST_EVENT, &prompt);
    logging::write_transfer_line("[pastey antenna] event=join_request_emitted_to_ui");
}

/// Claims a nearby request id once, as the prompt path does.
pub(crate) fn claim_join_request_id(state: &Arc<AppState>, request_id: &str) -> bool {
    let now = storage::now_ts();
    claim_nearby_join_request(
        &mut state.seen_nearby_join_requests.lock(),
        request_id,
        now,
        now + JOIN_REQUEST_REPLAY_TTL_SECS,
    )
}

fn claim_nearby_join_request(
    seen: &mut HashMap<String, i64>,
    request_id: &str,
    now: i64,
    expires_at: i64,
) -> bool {
    seen.retain(|_, seen_expires_at| *seen_expires_at > now);
    if request_id.trim().is_empty() || request_id.len() > 128 || seen.contains_key(request_id) {
        return false;
    }
    seen.insert(request_id.to_string(), expires_at);
    true
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
        known: false,
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
            known: false,
        };

        let json = serde_json::to_string(&device).unwrap();

        assert!(!json.contains("ip"));
        assert!(!json.contains("port"));
        assert!(!json.contains("endpoint"));
    }

    /// F10 regression: an acceptance carries no code, Bridge id, port or
    /// key; the code is shown on the accepting device and typed here.
    #[test]
    fn nearby_join_response_carries_no_code_or_session_material() {
        let response = NearbyJoinResponse {
            kind: "join_response".into(),
            request_id: "request".into(),
            accepted: true,
            message: None,
            device_name: Some("MacBook Air".into()),
            known_offer: None,
            bridge_label: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        for field in ["room", "code", "port", "key", "expires"] {
            assert!(!json.contains(field), "{field}");
        }
    }

    #[test]
    fn nearby_join_request_id_is_one_use_until_its_expiry() {
        let mut seen = HashMap::new();
        assert!(claim_nearby_join_request(&mut seen, "request-a", 10, 310));
        assert!(!claim_nearby_join_request(&mut seen, "request-a", 40, 340));
        assert!(claim_nearby_join_request(&mut seen, "request-a", 310, 610));
        assert!(!claim_nearby_join_request(&mut seen, "", 310, 610));
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

    #[test]
    fn a_peer_address_is_only_a_private_or_overlay_location() {
        for accepted in [
            "100.101.102.103",
            "192.168.1.20",
            "10.0.0.7",
            "172.20.4.9",
            "home-pc",
            "Windows-PC.tail1234.TS.net",
        ] {
            assert!(validate_peer_address(accepted).is_ok(), "{accepted}");
        }
        assert_eq!(
            validate_peer_address(" Windows-PC.tail1234.ts.net ").unwrap(),
            "windows-pc.tail1234.ts.net"
        );
        for refused in [
            "",
            "8.8.8.8",
            "203.0.113.5",
            "0.0.0.0",
            "255.255.255.255",
            "127.0.0.1",
            "169.254.10.10",
            "224.0.0.1",
            "::1",
            "fd7a:115c:a1e0::1",
            "1.2.3",
            "https://home-pc",
            "home-pc:48392",
            "home-pc/rooms",
            "user@home-pc",
            "home pc",
            "-bad.example",
            "bad..example",
            // Numeric forms a resolver reads as addresses without a lookup.
            "0x08080808",
            "0X7f000001",
            "0x7f.1",
            "0xa.0x0.0x0.0x1",
        ] {
            assert!(validate_peer_address(refused).is_err(), "{refused}");
        }
        // Names that merely contain hex-like or numeric labels remain names.
        for accepted in ["0xbox", "pc-0x1.tail1234.ts.net", "a1.lan"] {
            assert!(validate_peer_address(accepted).is_ok(), "{accepted}");
        }
    }

    /// A Host and an attacker on one LAN, both answering every query.
    async fn lan(
        code: &'static str,
        forged: impl Fn(&bridge_auth::PairQuery, &bridge_auth::PairOffer) -> Vec<bridge_auth::PairOffer>
            + Send
            + 'static,
    ) -> (SocketAddr, String) {
        let host = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let attacker = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let address = host.local_addr().unwrap();
        let identity = crate::device_identity::DeviceIdentity::generate_for_test();
        let transport_key = crate::crypto::encode_key(&crate::crypto::transport_public_key(
            &crate::crypto::generate_transport_secret(),
        ));
        let host_ref = crate::host_identity::HostRef::from_device_id("lan-host").unwrap();
        let genuine_key = transport_key.clone();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 4096];
            while let Ok((size, source)) = host.recv_from(&mut buffer).await {
                let Ok(query) = serde_json::from_slice::<bridge_auth::PairQuery>(&buffer[..size])
                else {
                    continue;
                };
                let offer = bridge_auth::offer_for_test(
                    &query,
                    code,
                    bridge_auth::OfferedRoom {
                        room_id: "genuine-room",
                        port: 40_000,
                        expires_at: 0,
                        transport_public_key: &transport_key,
                        device_name: "Windows",
                        host_ref: host_ref.as_str(),
                    },
                    &identity,
                )
                .unwrap();
                // The attacker answers first, from its own address.
                for fake in forged(&query, &offer) {
                    let _ = attacker
                        .send_to(&serde_json::to_vec(&fake).unwrap(), source)
                        .await;
                }
                let _ = host
                    .send_to(&serde_json::to_vec(&offer).unwrap(), source)
                    .await;
            }
        });
        (address, genuine_key)
    }

    async fn pair_like_a_broadcast(
        code: &str,
        target: SocketAddr,
        wait: Duration,
    ) -> AppResult<(SocketAddr, bridge_auth::VerifiedPairOffer)> {
        let mut attempt = bridge_auth::PairAttempt::new(code);
        let payload = serde_json::to_vec(&attempt.query()).unwrap();
        // Read answers from any source, exactly as the LAN broadcast does.
        exchange_with(&[target], false, None, &payload, wait, 2, |bytes| {
            serde_json::from_slice::<bridge_auth::PairOffer>(bytes)
                .ok()
                .and_then(|offer| attempt.verify_offer(&offer))
        })
        .await
    }

    /// F4 regression: on a LAN anyone may answer, but only an offer whose
    /// key confirmation proves the code is taken. Forged, tampered and
    /// misdirected offers from another device are ignored.
    #[tokio::test]
    async fn a_forged_lan_offer_is_ignored_and_the_genuine_one_accepted() {
        let attacker_identity = crate::device_identity::DeviceIdentity::generate_for_test();
        let (host, genuine_key) = lan("24681357", move |query, genuine| {
            let attacker_key = crate::crypto::encode_key(&[7u8; 32]);
            let guessed = bridge_auth::offer_for_test(
                query,
                "11111111",
                bridge_auth::OfferedRoom {
                    room_id: "attacker-room",
                    port: 1,
                    expires_at: 0,
                    transport_public_key: &attacker_key,
                    device_name: "Windows",
                    host_ref: genuine.host_ref.as_str(),
                },
                &attacker_identity,
            )
            .unwrap();
            vec![
                // A guess at the code.
                guessed,
                // The genuine answer with the attacker's key and port.
                bridge_auth::PairOffer {
                    transport_public_key: attacker_key.clone(),
                    port: 1,
                    ..genuine.clone()
                },
                // The genuine answer pointed at another Bridge id.
                bridge_auth::PairOffer {
                    room_id: "attacker-room".into(),
                    ..genuine.clone()
                },
            ]
        })
        .await;
        let (source, verified) = pair_like_a_broadcast("24681357", host, Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(source, host, "the answer taken is the Host's own");
        assert_eq!(verified.offer.room_id, "genuine-room");
        assert_eq!(verified.offer.transport_public_key, genuine_key);
        assert_eq!(verified.offer.port, 40_000);
    }

    /// With the wrong code nothing verifies, whoever answers.
    #[tokio::test]
    async fn pairing_with_a_wrong_code_finds_nothing_and_times_out_truthfully() {
        let (host, _) = lan("24681357", |_, genuine| vec![genuine.clone()]).await;
        let error = pair_like_a_broadcast("24681358", host, Duration::from_millis(800))
            .await
            .err()
            .unwrap();
        assert!(matches!(error, AppError::Timeout(_)));
    }

    #[tokio::test]
    async fn a_public_or_malformed_address_is_refused_before_any_packet_is_sent() {
        // `localhost` is a valid name, but it resolves outside any private
        // or overlay network, so nothing may be sent to what it resolves to.
        for address in ["8.8.8.8", "http://home-pc", "home-pc:1", "localhost"] {
            assert!(matches!(
                resolve_peer_targets(address).await,
                Err(AppError::InvalidInput(_))
            ));
        }
    }

    struct NoopEventSink;

    impl crate::host_runtime::HostEventSink for NoopEventSink {
        fn emit(&self, _event: crate::host_runtime::HostEvent) -> AppResult<()> {
            Ok(())
        }
    }

    struct NoopTaskSpawner;

    impl crate::host_runtime::RuntimeTaskSpawner for NoopTaskSpawner {
        fn spawn(&self, _task: crate::host_runtime::RuntimeTask) {}
    }

    /// F10 regression: join prompts are a human gate, bounded per address
    /// and in total.
    #[test]
    fn join_request_prompts_are_bounded_per_source_and_in_total() {
        let root =
            std::env::temp_dir().join(format!("pastey-join-prompts-{}", uuid::Uuid::new_v4()));
        let paths = crate::storage::AppPaths::new(root.clone(), root.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let config = crate::config::StoredConfig {
            version: 5,
            default_expiry_minutes: 15,
            inbox_dir: None,
            auto_burn_after_download: false,
            save_received_files_to_inbox: true,
            save_received_images_to_inbox: true,
            transfer_window_override: None,
            dev_tools_enabled: false,
            micro_flow_group_mode: "off".into(),
            shortcut: "test".into(),
            app_secret: crate::crypto::encode_key(&[44u8; 32]),
            device_id: "prompt-host".into(),
            internet: Default::default(),
        };
        let state = Arc::new(
            AppState::new(
                paths,
                config,
                Arc::new(NoopEventSink),
                Arc::new(NoopTaskSpawner),
            )
            .unwrap(),
        );
        let request = |id: &str| NearbyJoinRequest {
            kind: "join_request".into(),
            request_id: id.into(),
            device_id: format!("device-{id}"),
            display_name: "Alice's MacBook".into(),
            platform: "macOS".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            response_port: None,
            known: None,
        };
        let from = |last: u8| SocketAddr::from(([192, 168, 1, last], 5000));
        surface_join_request(state.clone(), request("a"), from(2));
        surface_join_request(state.clone(), request("b"), from(2));
        assert_eq!(
            state.pending_join_requests.lock().len(),
            1,
            "one per address"
        );
        for (index, last) in (3..10).enumerate() {
            surface_join_request(state.clone(), request(&format!("c{index}")), from(last));
        }
        assert_eq!(
            state.pending_join_requests.lock().len(),
            MAX_PENDING_JOIN_REQUESTS
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

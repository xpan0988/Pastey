use std::{path::PathBuf, sync::Arc};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;

use crate::{
    capability_probe::{self, CapabilityProbeMode},
    config, crypto,
    device_profile::{self, ProfileProbeMode},
    diagnostics, discovery,
    error::{AppError, AppResult},
    file_candidates::{
        self, ArtifactTransformClaimRequest, ArtifactTransformClaimResult, CandidatePayloadExecutionRequest, CandidatePayloadLocalResolution,
        FileCandidateExecutionRequest, FileCandidateExecutionResult,
    },
    hello_stdout::{self, HelloStdoutExecutionRequest, HelloStdoutExecutionResult},
    link_benchmark, logging,
    models::{
        AppConfig, BridgeDeliveryContentKind, BridgeDeliveryOutcome, BridgeDeliveryOutcomeStatus,
        BridgeDeliveryTargetKind, BridgePeerLiveness, BridgeSendAggregateStatus,
        BridgeSendOperation, BridgeSendTarget, JoinRequestPrompt, LocalRole, NearbyDevice,
        RoomInfo, RoomItem, RoomStatus, StoredBridgePeerEndpoint, StoredRoom,
    },
    room_control::{
        ReceivedRoomControlEvent, RoomControlDeliveryReceipt, RoomControlSendError,
        RoomControlSessionContext,
    },
    storage, transfer, AppState,
};

const RELEASES_URL: &str = "https://github.com/xpan0988/Pastey/releases";
const DIAGNOSTICS_CACHE_TTL_SECONDS: i64 = 60;
const TEXT_BRIDGE_ROUTE_SCHEMA_VERSION: &str = "pastey-bridge-text-route-v1";
const FILE_BRIDGE_ROUTE_SCHEMA_VERSION: &str = "pastey-bridge-file-route-v1";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformFinalizationDelivery {
    terminal_category: String,
    sent: bool,
}

#[derive(Serialize)]
pub struct FileTransferMetadata {
    path: String,
    display_name: String,
    mime_type: Option<String>,
    size_bytes: u64,
    modified_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BridgeRouteTargetKind {
    LegacyNone,
    SelectedPeer,
    SelectedPeers,
    BroadcastBridge,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ValidatedBridgeRouteTargets {
    target_kind: BridgeRouteTargetKind,
    targets: Vec<ValidatedBridgeRouteTarget>,
    endpoints: Vec<transfer::BridgePeerTransferEndpoint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ValidatedBridgeRouteTarget {
    peer_session_id: String,
    endpoint: Option<transfer::BridgePeerTransferEndpoint>,
    route_error_code: Option<BridgeRouteErrorCode>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BridgeRouteErrorCode {
    NoRouteablePeer,
    UnknownPeer,
    PeerUnrouteable,
    MalformedRoute,
    RouteMismatch,
    RouteExpired,
}

impl BridgeRouteErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::NoRouteablePeer => "no_routeable_peer",
            Self::UnknownPeer => "unknown_peer",
            Self::PeerUnrouteable => "peer_unrouteable",
            Self::MalformedRoute => "malformed_route",
            Self::RouteMismatch => "route_mismatch",
            Self::RouteExpired => "route_expired",
        }
    }
}

fn bridge_route_error(code: BridgeRouteErrorCode, message: impl Into<String>) -> AppError {
    AppError::InvalidInput(format!(
        "[pastey:bridge-route-error code={}] {}",
        code.as_str(),
        message.into()
    ))
}

fn validate_bridge_route_payload(
    bridge_route: Option<&Value>,
    room_id: &str,
    room: &StoredRoom,
    peers: &[StoredBridgePeerEndpoint],
    expected_schema_version: &str,
    content_label: &str,
) -> AppResult<ValidatedBridgeRouteTargets> {
    let Some(route) = bridge_route else {
        return Ok(ValidatedBridgeRouteTargets {
            target_kind: BridgeRouteTargetKind::LegacyNone,
            targets: Vec::new(),
            endpoints: Vec::new(),
        });
    };
    if room.status != RoomStatus::Active {
        return Err(bridge_route_error(
            BridgeRouteErrorCode::RouteExpired,
            format!("Bridge {content_label} route requires an active room."),
        ));
    }
    let route = route.as_object().ok_or_else(|| {
        bridge_route_error(
            BridgeRouteErrorCode::MalformedRoute,
            format!("Bridge {content_label} route must be an object."),
        )
    })?;
    require_exact_bridge_route_fields(
        route,
        &["schemaVersion", "bridgeSessionId", "target"],
        content_label,
    )?;
    let schema_version = bridge_route_string_field(route, "schemaVersion", content_label)?;
    if schema_version != expected_schema_version {
        return Err(bridge_route_error(
            BridgeRouteErrorCode::MalformedRoute,
            format!("Bridge {content_label} route schema version is unsupported."),
        ));
    }
    let bridge_session_id = bridge_route_string_field(route, "bridgeSessionId", content_label)?;
    let expected_bridge_session_id = format!("legacy-room:{room_id}");
    if bridge_session_id != expected_bridge_session_id {
        return Err(bridge_route_error(
            BridgeRouteErrorCode::RouteMismatch,
            format!("Bridge {content_label} route session does not match the current room."),
        ));
    }

    let target = route
        .get("target")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            bridge_route_error(
                BridgeRouteErrorCode::MalformedRoute,
                format!("Bridge {content_label} route target must be an object."),
            )
        })?;
    let target_kind = bridge_route_string_field(target, "kind", content_label)?;

    match target_kind {
        "selected_peer" => {
            require_exact_bridge_route_fields(target, &["kind", "peerSessionId"], content_label)?;
            let peer_session_id =
                bridge_route_string_field(target, "peerSessionId", content_label)?;
            let endpoint = resolve_routeable_bridge_peer(peers, peer_session_id, content_label)?;
            Ok(ValidatedBridgeRouteTargets {
                target_kind: BridgeRouteTargetKind::SelectedPeer,
                targets: vec![ValidatedBridgeRouteTarget {
                    peer_session_id: endpoint.peer_session_id.clone(),
                    endpoint: Some(endpoint.clone()),
                    route_error_code: None,
                }],
                endpoints: vec![endpoint],
            })
        }
        "selected_peers" => {
            require_exact_bridge_route_fields(target, &["kind", "peerSessionIds"], content_label)?;
            let Some(peer_session_ids) = target.get("peerSessionIds").and_then(Value::as_array)
            else {
                return Err(bridge_route_error(
                    BridgeRouteErrorCode::MalformedRoute,
                    format!(
                    "Bridge {content_label} route selected_peers target requires peerSessionIds."
                ),
                ));
            };
            let peer_session_ids = bridge_route_string_array(peer_session_ids, content_label)?;
            if peer_session_ids.len() < 2 {
                return Err(bridge_route_error(
                    BridgeRouteErrorCode::MalformedRoute,
                    format!(
                    "Bridge {content_label} route selected_peers target requires two or more peers."
                ),
                ));
            }
            let unique: std::collections::BTreeSet<_> = peer_session_ids.iter().collect();
            if unique.len() != peer_session_ids.len() {
                return Err(bridge_route_error(
                    BridgeRouteErrorCode::MalformedRoute,
                    format!(
                    "Bridge {content_label} route selected_peers target rejects duplicate peers."
                ),
                ));
            }
            let targets = peer_session_ids
                .iter()
                .map(|peer_session_id| {
                    resolve_known_bridge_peer_target(peers, peer_session_id, content_label)
                })
                .collect::<AppResult<Vec<_>>>()?;
            let endpoints = targets
                .iter()
                .filter_map(|target| target.endpoint.clone())
                .collect::<Vec<_>>();
            Ok(ValidatedBridgeRouteTargets {
                target_kind: BridgeRouteTargetKind::SelectedPeers,
                targets,
                endpoints,
            })
        }
        "broadcast_bridge" => {
            require_exact_bridge_route_fields(target, &["kind", "explicit"], content_label)?;
            if target.get("explicit").and_then(Value::as_bool) != Some(true) {
                return Err(bridge_route_error(
                    BridgeRouteErrorCode::MalformedRoute,
                    format!("Bridge {content_label} route broadcast target must be explicit."),
                ));
            }
            let routeable = peers
                .iter()
                .filter_map(|peer| routeable_endpoint_for_peer(peer).ok())
                .collect::<Vec<_>>();
            if routeable.is_empty() {
                return Err(bridge_route_error(
                    BridgeRouteErrorCode::NoRouteablePeer,
                    format!(
                    "Bridge {content_label} route broadcast target has no current routeable peers."
                ),
                ));
            }
            Ok(ValidatedBridgeRouteTargets {
                target_kind: BridgeRouteTargetKind::BroadcastBridge,
                targets: routeable
                    .iter()
                    .map(|endpoint| ValidatedBridgeRouteTarget {
                        peer_session_id: endpoint.peer_session_id.clone(),
                        endpoint: Some(endpoint.clone()),
                        route_error_code: None,
                    })
                    .collect(),
                endpoints: routeable,
            })
        }
        _ => Err(bridge_route_error(
            BridgeRouteErrorCode::MalformedRoute,
            format!("Bridge {content_label} route target kind is unsupported."),
        )),
    }
}

fn resolve_routeable_bridge_peer(
    peers: &[StoredBridgePeerEndpoint],
    peer_session_id: &str,
    content_label: &str,
) -> AppResult<transfer::BridgePeerTransferEndpoint> {
    let Some(peer) = peers
        .iter()
        .find(|peer| peer.peer_session_id == peer_session_id)
    else {
        return Err(bridge_route_error(
            BridgeRouteErrorCode::UnknownPeer,
            format!(
                "Bridge {content_label} route target contains an unknown current-session peer."
            ),
        ));
    };
    routeable_endpoint_for_peer(peer).map_err(|_| {
        bridge_route_error(
            bridge_route_error_code_for_peer(peer),
            format!("Bridge {content_label} route target is not currently routeable."),
        )
    })
}

fn resolve_known_bridge_peer_target(
    peers: &[StoredBridgePeerEndpoint],
    peer_session_id: &str,
    content_label: &str,
) -> AppResult<ValidatedBridgeRouteTarget> {
    let Some(peer) = peers
        .iter()
        .find(|peer| peer.peer_session_id == peer_session_id)
    else {
        return Err(bridge_route_error(
            BridgeRouteErrorCode::UnknownPeer,
            format!(
                "Bridge {content_label} route target contains an unknown current-session peer."
            ),
        ));
    };

    match routeable_endpoint_for_peer(peer) {
        Ok(endpoint) => Ok(ValidatedBridgeRouteTarget {
            peer_session_id: endpoint.peer_session_id.clone(),
            endpoint: Some(endpoint),
            route_error_code: None,
        }),
        Err(_) => Ok(ValidatedBridgeRouteTarget {
            peer_session_id: peer.peer_session_id.clone(),
            endpoint: None,
            route_error_code: Some(bridge_route_error_code_for_peer(peer)),
        }),
    }
}

fn bridge_route_error_code_for_peer(peer: &StoredBridgePeerEndpoint) -> BridgeRouteErrorCode {
    match peer.liveness {
        BridgePeerLiveness::Left | BridgePeerLiveness::Stale | BridgePeerLiveness::Expired => {
            BridgeRouteErrorCode::RouteExpired
        }
        BridgePeerLiveness::Connected
            if peer.endpoint_host.as_deref().unwrap_or_default().is_empty()
                || peer.endpoint_port.is_none()
                || peer
                    .transport_public_key
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty() =>
        {
            BridgeRouteErrorCode::PeerUnrouteable
        }
        BridgePeerLiveness::Connected
        | BridgePeerLiveness::Reconnecting
        | BridgePeerLiveness::Disconnected => BridgeRouteErrorCode::PeerUnrouteable,
    }
}

fn routeable_endpoint_for_peer(
    peer: &StoredBridgePeerEndpoint,
) -> AppResult<transfer::BridgePeerTransferEndpoint> {
    if peer.liveness != BridgePeerLiveness::Connected {
        return Err(AppError::InvalidInput(
            "Bridge peer is not connected.".into(),
        ));
    }
    let Some(host) = peer
        .endpoint_host
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return Err(AppError::InvalidInput(
            "Bridge peer endpoint is missing.".into(),
        ));
    };
    let Some(port) = peer.endpoint_port else {
        return Err(AppError::InvalidInput(
            "Bridge peer endpoint is missing.".into(),
        ));
    };
    let Some(transport_public_key) = peer
        .transport_public_key
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return Err(AppError::InvalidInput(
            "Bridge peer transport key is missing.".into(),
        ));
    };
    Ok(transfer::BridgePeerTransferEndpoint {
        peer_session_id: peer.peer_session_id.clone(),
        host: host.to_string(),
        port,
        transport_public_key: transport_public_key.to_string(),
    })
}

fn bridge_send_target_for_route(targets: &ValidatedBridgeRouteTargets) -> Option<BridgeSendTarget> {
    match targets.target_kind {
        BridgeRouteTargetKind::LegacyNone => None,
        BridgeRouteTargetKind::SelectedPeer => {
            targets
                .targets
                .first()
                .map(|target| BridgeSendTarget::SelectedPeer {
                    peer_session_ref: target.peer_session_id.clone(),
                })
        }
        BridgeRouteTargetKind::SelectedPeers => Some(BridgeSendTarget::SelectedPeers {
            peer_session_refs: targets
                .targets
                .iter()
                .map(|target| target.peer_session_id.clone())
                .collect(),
        }),
        BridgeRouteTargetKind::BroadcastBridge => {
            Some(BridgeSendTarget::BroadcastBridge { explicit: true })
        }
    }
}

fn bridge_delivery_target_kind(target_kind: BridgeRouteTargetKind) -> BridgeDeliveryTargetKind {
    match target_kind {
        BridgeRouteTargetKind::LegacyNone | BridgeRouteTargetKind::SelectedPeer => {
            BridgeDeliveryTargetKind::SelectedPeer
        }
        BridgeRouteTargetKind::SelectedPeers => BridgeDeliveryTargetKind::SelectedPeers,
        BridgeRouteTargetKind::BroadcastBridge => BridgeDeliveryTargetKind::BroadcastBridge,
    }
}

fn bridge_operation_id(content_label: &str, item_id: &str) -> String {
    format!("bridge-send:{content_label}:{item_id}")
}

fn bridge_operation_timestamp() -> String {
    storage::now_ts().to_string()
}

fn bridge_delivery_outcome(
    operation_id: &str,
    bridge_session_ref: &str,
    peer_session_ref: &str,
    target_kind: BridgeDeliveryTargetKind,
    content_kind: BridgeDeliveryContentKind,
    status: BridgeDeliveryOutcomeStatus,
    error_code: Option<&str>,
) -> BridgeDeliveryOutcome {
    let now = bridge_operation_timestamp();
    BridgeDeliveryOutcome {
        operation_id: operation_id.to_string(),
        bridge_session_ref: bridge_session_ref.to_string(),
        peer_session_ref: peer_session_ref.to_string(),
        target_kind,
        content_kind,
        status,
        error_code: error_code.map(str::to_string),
        created_at: now.clone(),
        updated_at: now,
    }
}

fn bridge_aggregate_status(outcomes: &[BridgeDeliveryOutcome]) -> BridgeSendAggregateStatus {
    let delivered = outcomes
        .iter()
        .filter(|outcome| outcome.status == BridgeDeliveryOutcomeStatus::Delivered)
        .count();
    if delivered == outcomes.len() && !outcomes.is_empty() {
        BridgeSendAggregateStatus::Completed
    } else if delivered > 0 {
        BridgeSendAggregateStatus::Partial
    } else {
        BridgeSendAggregateStatus::Failed
    }
}

fn bridge_send_operation(
    item_id: &str,
    content_label: &str,
    content_kind: BridgeDeliveryContentKind,
    route_targets: &ValidatedBridgeRouteTargets,
    outcomes: Vec<BridgeDeliveryOutcome>,
) -> Option<BridgeSendOperation> {
    let target = bridge_send_target_for_route(route_targets)?;
    let now = bridge_operation_timestamp();
    Some(BridgeSendOperation {
        operation_id: bridge_operation_id(content_label, item_id),
        bridge_session_ref: outcomes
            .first()
            .map(|outcome| outcome.bridge_session_ref.clone())
            .unwrap_or_default(),
        target,
        resolved_peer_session_refs: route_targets
            .targets
            .iter()
            .map(|target| target.peer_session_id.clone())
            .collect(),
        content_kind,
        aggregate_status: bridge_aggregate_status(&outcomes),
        outcomes,
        created_at: now.clone(),
        updated_at: now,
    })
}

fn require_exact_bridge_route_fields(
    object: &serde_json::Map<String, Value>,
    expected: &[&str],
    content_label: &str,
) -> AppResult<()> {
    if object.len() != expected.len() || expected.iter().any(|field| !object.contains_key(*field)) {
        return Err(bridge_route_error(
            BridgeRouteErrorCode::MalformedRoute,
            format!("Bridge {content_label} route contains unsupported or missing fields."),
        ));
    }
    Ok(())
}

fn bridge_route_string_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
    content_label: &str,
) -> AppResult<&'a str> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            bridge_route_error(
                BridgeRouteErrorCode::MalformedRoute,
                format!("Bridge {content_label} route {field} is invalid."),
            )
        })
}

fn bridge_route_string_array(values: &[Value], content_label: &str) -> AppResult<Vec<String>> {
    let mut peer_session_ids = Vec::with_capacity(values.len());
    for value in values {
        let Some(peer_session_id) = value.as_str().map(str::trim).filter(|id| !id.is_empty())
        else {
            return Err(bridge_route_error(
                BridgeRouteErrorCode::MalformedRoute,
                format!(
                "Bridge {content_label} route peerSessionIds must contain only non-empty strings."
            ),
            ));
        };
        peer_session_ids.push(peer_session_id.to_string());
    }
    Ok(peer_session_ids)
}

#[tauri::command]
pub async fn create_room(
    expiry_minutes: u64,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let code = unique_room_code(&state.paths)?;
        let room = storage::create_room(
            &state.paths,
            &master_key,
            &code,
            expiry_minutes,
            LocalRole::Creator,
            None,
            None,
        )?;
        transfer::start_room_server(state.inner().clone(), &room.id).await?;
        storage::room_to_info_with_bridge_peers(&state.paths, room, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn join_room(code: String, state: State<'_, Arc<AppState>>) -> Result<RoomInfo, String> {
    run_async(async move {
        let compact = normalize_code(&code)?;
        let room_code_hash = crypto::hash_code(&compact);
        let (source, discovered) = discovery::discover_room(room_code_hash).await?;
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };

        let room = storage::create_room(
            &state.paths,
            &master_key,
            &compact,
            15,
            LocalRole::Joined,
            Some(discovered.room_id.clone()),
            Some(discovered.expires_at),
        )?;
        transfer::start_room_server(state.inner().clone(), &room.id).await?;
        let response = transfer::announce_join(
            state.inner().clone(),
            &room.id,
            &source.ip().to_string(),
            discovered.port,
        )
        .await?;

        storage::update_room_peer(
            &state.paths,
            &room.id,
            Some(&source.ip().to_string()),
            Some(discovered.port),
            Some(&response.device_name),
            Some(&discovered.transport_public_key),
            crate::models::RoomStatus::Active,
        )?;

        let updated = storage::get_room_by_id(&state.paths, &room.id)?;
        storage::room_to_info_with_bridge_peers(&state.paths, updated, &master_key)
    })
    .await
}

#[tauri::command]
pub fn list_nearby_devices(state: State<'_, Arc<AppState>>) -> Result<Vec<NearbyDevice>, String> {
    Ok(discovery::list_nearby_devices(&state))
}

#[tauri::command]
pub async fn request_nearby_join(
    device_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let (source, response) =
            discovery::request_nearby_join(state.inner().clone(), &device_id).await?;
        if !response.accepted {
            logging::write_transfer_line("[pastey antenna] event=join_rejected");
            return Err(AppError::InvalidInput(
                response
                    .message
                    .unwrap_or_else(|| "Join request rejected.".into()),
            ));
        }

        let room_code = response
            .room_code
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let room_id = response
            .room_id
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let expires_at = response
            .expires_at
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let port = response
            .port
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let transport_public_key = response
            .transport_public_key
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let peer_device_name = response
            .device_name
            .unwrap_or_else(|| "Nearby device".into());

        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let room = storage::create_room(
            &state.paths,
            &master_key,
            &room_code,
            15,
            LocalRole::Joined,
            Some(room_id),
            Some(expires_at),
        )?;
        transfer::start_room_server(state.inner().clone(), &room.id).await?;
        transfer::announce_join(
            state.inner().clone(),
            &room.id,
            &source.ip().to_string(),
            port,
        )
        .await
        .map_err(|_| {
            logging::write_transfer_line("[pastey antenna] event=nearby_unreachable");
            logging::write_transfer_line("[pastey antenna] event=blocked_network_suspected");
            AppError::Network(
                "Device found, but this network may block direct local connections.".into(),
            )
        })?;

        storage::update_room_peer(
            &state.paths,
            &room.id,
            Some(&source.ip().to_string()),
            Some(port),
            Some(&peer_device_name),
            Some(&transport_public_key),
            crate::models::RoomStatus::Active,
        )?;

        logging::write_transfer_line("[pastey antenna] event=join_accepted");
        let updated = storage::get_room_by_id(&state.paths, &room.id)?;
        storage::room_to_info_with_bridge_peers(&state.paths, updated, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn accept_nearby_join(
    request_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let request = state
            .pending_join_requests
            .lock()
            .remove(&request_id)
            .ok_or_else(|| AppError::NotFound("Join request timed out.".into()))?;
        if request.expires_at <= storage::now_ts() {
            return Err(AppError::InvalidInput("Join request timed out.".into()));
        }

        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let code = unique_room_code(&state.paths)?;
        let expiry_minutes = {
            let config = state.config.read();
            config.default_expiry_minutes
        };
        let room = storage::create_room(
            &state.paths,
            &master_key,
            &code,
            expiry_minutes,
            LocalRole::Creator,
            None,
            None,
        )?;
        let port = transfer::start_room_server(state.inner().clone(), &room.id).await?;
        let transport_public_key = state
            .active_servers
            .lock()
            .get(&room.id)
            .map(|server| server.transport_public_key())
            .ok_or_else(|| AppError::Network("Firewall may be blocking Pastey.".into()))?;
        let response = discovery::NearbyJoinResponse {
            kind: "join_response".into(),
            request_id: request.request_id.clone(),
            accepted: true,
            message: None,
            room_id: Some(room.id.clone()),
            room_code: Some(code),
            port: Some(port),
            expires_at: Some(room.expires_at),
            transport_public_key: Some(transport_public_key),
            device_name: Some(transfer::device_name()),
        };
        discovery::send_join_response(&request, &response).await?;
        logging::write_transfer_line("[pastey antenna] event=join_accepted");
        storage::room_to_info_with_bridge_peers(&state.paths, room, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn reject_nearby_join(
    request_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<bool, String> {
    run_async(async move {
        let Some(request) = state.pending_join_requests.lock().remove(&request_id) else {
            return Ok(false);
        };
        let response = discovery::NearbyJoinResponse {
            kind: "join_response".into(),
            request_id: request.request_id.clone(),
            accepted: false,
            message: Some("Join request rejected.".into()),
            room_id: None,
            room_code: None,
            port: None,
            expires_at: None,
            transport_public_key: None,
            device_name: Some(transfer::device_name()),
        };
        discovery::send_join_response(&request, &response).await?;
        logging::write_transfer_line("[pastey antenna] event=join_rejected");
        Ok(true)
    })
    .await
}

#[tauri::command]
pub fn pending_join_requests(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<JoinRequestPrompt>, String> {
    let now = storage::now_ts();
    state
        .pending_join_requests
        .lock()
        .retain(|_, request| request.expires_at > now);
    Ok(state
        .pending_join_requests
        .lock()
        .values()
        .map(discovery::pending_join_prompt)
        .collect())
}

#[tauri::command]
pub fn mark_join_prompt_rendered() -> Result<bool, String> {
    logging::write_transfer_line("[pastey antenna] event=join_prompt_rendered");
    Ok(true)
}

#[tauri::command]
pub async fn list_rooms(state: State<'_, Arc<AppState>>) -> Result<Vec<RoomInfo>, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let rooms = storage::list_rooms(&state.paths)?;
        rooms
            .into_iter()
            .map(|room| storage::room_to_info_with_bridge_peers(&state.paths, room, &master_key))
            .collect()
    })
    .await
}

#[tauri::command]
pub async fn get_room(
    room_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let room = storage::get_room_by_id(&state.paths, &room_id)?;
        storage::room_to_info_with_bridge_peers(&state.paths, room, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn pair_bridge_peer(
    room_id: String,
    peer_session_id: String,
    display_label: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let room = storage::get_room_by_id(&state.paths, &room_id)?;
        let _ = storage::sync_legacy_bridge_peer_endpoint(&state.paths, &room)?;
        storage::pair_bridge_peer(
            &state.paths,
            &room_id,
            &peer_session_id,
            display_label.as_deref(),
        )?;
        let room = storage::get_room_by_id(&state.paths, &room_id)?;
        storage::room_to_info_with_bridge_peers(&state.paths, room, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn revoke_bridge_peer_pairing(
    room_id: String,
    peer_session_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        storage::revoke_bridge_peer_pairing(&state.paths, &room_id, &peer_session_id)?;
        let room = storage::get_room_by_id(&state.paths, &room_id)?;
        storage::room_to_info_with_bridge_peers(&state.paths, room, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn mark_bridge_peer_pairing_rotation_required(
    room_id: String,
    peer_session_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        storage::mark_bridge_peer_pairing_rotation_required(
            &state.paths,
            &room_id,
            &peer_session_id,
        )?;
        let room = storage::get_room_by_id(&state.paths, &room_id)?;
        storage::room_to_info_with_bridge_peers(&state.paths, room, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn list_room_items(
    room_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<RoomItem>, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let items = storage::list_room_items(&state.paths, &room_id)?;
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            match storage::room_item_to_info(&state.paths, &master_key, item) {
                Ok(item) => result.push(item),
                Err(AppError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    })
    .await
}

#[tauri::command]
pub async fn send_text_to_room(
    room_id: String,
    text: String,
    bridge_route: Option<Value>,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomItem, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let room = storage::get_room_by_id(&state.paths, &room_id)?;
        let _ = storage::sync_legacy_bridge_peer_endpoint(&state.paths, &room)?;
        let peers = storage::list_bridge_peer_endpoints(&state.paths, &room_id)?;
        let route_targets = validate_bridge_route_payload(
            bridge_route.as_ref(),
            &room_id,
            &room,
            &peers,
            TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "text",
        )?;
        let item = storage::create_outgoing_text_item(&state.paths, &master_key, &room_id, &text)?;
        let mut bridge_operation = None;
        match route_targets.target_kind {
            BridgeRouteTargetKind::LegacyNone => {
                transfer::send_room_item(state.inner().clone(), &room_id, &item.id).await?;
            }
            BridgeRouteTargetKind::SelectedPeer => {
                let operation_id = bridge_operation_id("text", &item.id);
                let bridge_session_ref = format!("legacy-room:{room_id}");
                let endpoint = route_targets.endpoints.first().cloned().ok_or_else(|| {
                    bridge_route_error(
                        BridgeRouteErrorCode::NoRouteablePeer,
                        "Bridge text route selected_peer target has no resolved endpoint.",
                    )
                })?;
                transfer::send_room_item_to_bridge_peer_endpoint(
                    state.inner().clone(),
                    &room_id,
                    &item.id,
                    endpoint.clone(),
                )
                .await?;
                let outcomes = vec![bridge_delivery_outcome(
                    &operation_id,
                    &bridge_session_ref,
                    &endpoint.peer_session_id,
                    bridge_delivery_target_kind(route_targets.target_kind),
                    BridgeDeliveryContentKind::Text,
                    BridgeDeliveryOutcomeStatus::Delivered,
                    None,
                )];
                bridge_operation = bridge_send_operation(
                    &item.id,
                    "text",
                    BridgeDeliveryContentKind::Text,
                    &route_targets,
                    outcomes,
                );
            }
            BridgeRouteTargetKind::SelectedPeers | BridgeRouteTargetKind::BroadcastBridge => {
                let operation_id = bridge_operation_id("text", &item.id);
                let bridge_session_ref = format!("legacy-room:{room_id}");
                let target_kind = bridge_delivery_target_kind(route_targets.target_kind);
                let mut outcomes = Vec::new();
                for target in route_targets.targets.iter().cloned() {
                    if let Some(endpoint) = target.endpoint {
                        let send_result = transfer::send_room_item_to_bridge_peer_endpoint(
                            state.inner().clone(),
                            &room_id,
                            &item.id,
                            endpoint.clone(),
                        )
                        .await;
                        outcomes.push(bridge_delivery_outcome(
                            &operation_id,
                            &bridge_session_ref,
                            &endpoint.peer_session_id,
                            target_kind.clone(),
                            BridgeDeliveryContentKind::Text,
                            if send_result.is_ok() {
                                BridgeDeliveryOutcomeStatus::Delivered
                            } else {
                                BridgeDeliveryOutcomeStatus::Failed
                            },
                            if send_result.is_ok() {
                                None
                            } else {
                                Some("transport_error")
                            },
                        ));
                    } else {
                        outcomes.push(bridge_delivery_outcome(
                            &operation_id,
                            &bridge_session_ref,
                            &target.peer_session_id,
                            target_kind.clone(),
                            BridgeDeliveryContentKind::Text,
                            BridgeDeliveryOutcomeStatus::Rejected,
                            Some(
                                target
                                    .route_error_code
                                    .unwrap_or(BridgeRouteErrorCode::PeerUnrouteable)
                                    .as_str(),
                            ),
                        ));
                    }
                }
                bridge_operation = bridge_send_operation(
                    &item.id,
                    "text",
                    BridgeDeliveryContentKind::Text,
                    &route_targets,
                    outcomes,
                );
            }
        }
        let stored = storage::get_room_item_by_id(&state.paths, &item.id)?;
        let mut info = storage::room_item_to_info(&state.paths, &master_key, stored)?;
        info.bridge_send_operation = bridge_operation;
        Ok(info)
    })
    .await
}

#[tauri::command]
pub async fn send_file_to_room(
    room_id: String,
    path: String,
    display_name: Option<String>,
    mime_type: Option<String>,
    queue_item_id: Option<String>,
    requested_window: Option<usize>,
    bridge_route: Option<Value>,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomItem, String> {
    run_async(async move {
        let file_path = resolve_user_path(&path)?;
        if !file_path.is_file() {
            return Err(AppError::InvalidInput("selected path is not a file".into()));
        }

        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let room = storage::get_room_by_id(&state.paths, &room_id)?;
        let _ = storage::sync_legacy_bridge_peer_endpoint(&state.paths, &room)?;
        let peers = storage::list_bridge_peer_endpoints(&state.paths, &room_id)?;
        let route_targets = validate_bridge_route_payload(
            bridge_route.as_ref(),
            &room_id,
            &room,
            &peers,
            FILE_BRIDGE_ROUTE_SCHEMA_VERSION,
            "file",
        )?;
        let content_kind = if mime_type
            .as_deref()
            .map(|value| value.starts_with("image/"))
            .unwrap_or(false)
        {
            BridgeDeliveryContentKind::Image
        } else {
            BridgeDeliveryContentKind::File
        };
        let item = storage::create_outgoing_file_item_with_metadata(
            &state.paths,
            &master_key,
            &room_id,
            &file_path,
            display_name,
            mime_type,
        )?;
        let mut bridge_operation = None;
        match route_targets.target_kind {
            BridgeRouteTargetKind::LegacyNone => {
                if let Err(error) = transfer::send_room_file(
                    state.inner().clone(),
                    &room_id,
                    &item.id,
                    &file_path,
                    queue_item_id,
                    requested_window,
                )
                .await
                {
                    let _ = storage::delete_room_item(&state.paths, &item.id);
                    return Err(error);
                }
            }
            BridgeRouteTargetKind::SelectedPeer => {
                let operation_id = bridge_operation_id("file", &item.id);
                let bridge_session_ref = format!("legacy-room:{room_id}");
                let endpoint = route_targets.endpoints.first().cloned().ok_or_else(|| {
                    bridge_route_error(
                        BridgeRouteErrorCode::NoRouteablePeer,
                        "Bridge file route selected_peer target has no resolved endpoint.",
                    )
                })?;
                if let Err(error) = transfer::send_room_file_to_bridge_peer_endpoint(
                    state.inner().clone(),
                    &room_id,
                    &item.id,
                    &file_path,
                    queue_item_id,
                    requested_window,
                    endpoint.clone(),
                )
                .await
                {
                    let _ = storage::delete_room_item(&state.paths, &item.id);
                    return Err(error);
                }
                let outcomes = vec![bridge_delivery_outcome(
                    &operation_id,
                    &bridge_session_ref,
                    &endpoint.peer_session_id,
                    bridge_delivery_target_kind(route_targets.target_kind),
                    content_kind.clone(),
                    BridgeDeliveryOutcomeStatus::Delivered,
                    None,
                )];
                bridge_operation = bridge_send_operation(
                    &item.id,
                    "file",
                    content_kind.clone(),
                    &route_targets,
                    outcomes,
                );
            }
            BridgeRouteTargetKind::SelectedPeers | BridgeRouteTargetKind::BroadcastBridge => {
                let operation_id = bridge_operation_id("file", &item.id);
                let bridge_session_ref = format!("legacy-room:{room_id}");
                let target_kind = bridge_delivery_target_kind(route_targets.target_kind);
                let mut outcomes = Vec::new();
                for target in route_targets.targets.iter().cloned() {
                    if let Some(endpoint) = target.endpoint {
                        let send_result = transfer::send_room_file_to_bridge_peer_endpoint(
                            state.inner().clone(),
                            &room_id,
                            &item.id,
                            &file_path,
                            queue_item_id.clone(),
                            requested_window,
                            endpoint.clone(),
                        )
                        .await;
                        outcomes.push(bridge_delivery_outcome(
                            &operation_id,
                            &bridge_session_ref,
                            &endpoint.peer_session_id,
                            target_kind.clone(),
                            content_kind.clone(),
                            if send_result.is_ok() {
                                BridgeDeliveryOutcomeStatus::Delivered
                            } else {
                                BridgeDeliveryOutcomeStatus::Failed
                            },
                            if send_result.is_ok() {
                                None
                            } else {
                                Some("transport_error")
                            },
                        ));
                    } else {
                        outcomes.push(bridge_delivery_outcome(
                            &operation_id,
                            &bridge_session_ref,
                            &target.peer_session_id,
                            target_kind.clone(),
                            content_kind.clone(),
                            BridgeDeliveryOutcomeStatus::Rejected,
                            Some(
                                target
                                    .route_error_code
                                    .unwrap_or(BridgeRouteErrorCode::PeerUnrouteable)
                                    .as_str(),
                            ),
                        ));
                    }
                }
                bridge_operation = bridge_send_operation(
                    &item.id,
                    "file",
                    content_kind.clone(),
                    &route_targets,
                    outcomes,
                );
            }
        }
        let stored = storage::get_room_item_by_id(&state.paths, &item.id)?;
        let mut info = storage::room_item_to_info(&state.paths, &master_key, stored)?;
        info.bridge_send_operation = bridge_operation;
        Ok(info)
    })
    .await
}

#[tauri::command]
pub async fn send_room_control_event(
    room_id: String,
    event: Value,
    bridge_route: Option<Value>,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomControlDeliveryReceipt, RoomControlSendError> {
    crate::room_control::send_room_control_event(
        state.inner().clone(),
        &room_id,
        event,
        bridge_route,
    )
    .await
    .map_err(RoomControlSendError::from_app_error)
}

#[tauri::command]
pub async fn execute_hello_stdout_capability(
    request: HelloStdoutExecutionRequest,
) -> Result<HelloStdoutExecutionResult, String> {
    hello_stdout::execute_hello_stdout(request).map_err(|error| error.message())
}

#[tauri::command]
pub async fn execute_file_candidate_search_capability(
    request: FileCandidateExecutionRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<FileCandidateExecutionResult, String> {
    let mut store = state.candidate_payload_store.lock();
    file_candidates::execute_file_candidate_search_and_store(request, &state.paths, &mut store)
        .map_err(|error| error.message())
}

#[tauri::command]
pub async fn resolve_candidate_payload_capability(
    request: CandidatePayloadExecutionRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<CandidatePayloadLocalResolution, String> {
    let mut store = state.candidate_payload_store.lock();
    file_candidates::resolve_candidate_payload_for_handoff(request, &mut store)
        .map_err(|error| error.message())
}

#[tauri::command]
pub async fn begin_transform_operation(
    request: ArtifactTransformClaimRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<ArtifactTransformClaimResult, String> {
    let mut store = state.candidate_payload_store.lock();
    let mut authority = state.transform_authority.lock();
    file_candidates::begin_artifact_transform_operation(request, &mut store, &mut authority)
        .map_err(|error| error.message())
}

#[tauri::command]
pub async fn create_transform_consent_prompt(
    room_id: String,
    source_preview_event_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<file_candidates::TransformConsentPromptInfo, String> {
    let seed = crate::room_control::authenticated_transform_preview_consent_seed(&state, &room_id, &source_preview_event_id)
        .map_err(|error| error.message())?;
    let mut authority = state.transform_authority.lock();
    file_candidates::create_pending_transform_consent_prompt(seed, &mut authority)
        .map_err(|error| error.message())
}

#[tauri::command]
pub async fn resolve_transform_consent_prompt(
    room_id: String,
    pending_consent_prompt_id: String,
    decision: String,
    state: State<'_, Arc<AppState>>,
) -> Result<file_candidates::TransformConsentPromptInfo, String> {
    let context = crate::room_control::room_control_session_context(&state, &room_id).map_err(|error| error.message())?;
    let mut authority = state.transform_authority.lock();
    file_candidates::resolve_pending_transform_consent_prompt(
        &pending_consent_prompt_id, &decision, &room_id, &context.peer_session_ref, &context.local_session_ref, &mut authority,
    ).map_err(|error| error.message())
}

#[tauri::command]
pub async fn revalidate_transform_operation(
    request: ArtifactTransformClaimRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<ArtifactTransformClaimResult, String> {
    let mut store = state.candidate_payload_store.lock();
    let mut authority = state.transform_authority.lock();
    file_candidates::revalidate_artifact_transform_operation(&request, &mut store, &mut authority)
        .map_err(|error| error.message())
}

#[tauri::command]
pub async fn abort_transform_operation(
    request: ArtifactTransformClaimRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<ArtifactTransformClaimResult, String> {
    let mut store = state.candidate_payload_store.lock();
    let mut authority = state.transform_authority.lock();
    file_candidates::abort_artifact_transform_operation(&request, &mut store, &mut authority)
        .map_err(|error| error.message())
}

#[tauri::command]
pub async fn finalize_and_send_transform_result(
    request: ArtifactTransformClaimRequest,
    raw_result: Value,
    state: State<'_, Arc<AppState>>,
) -> Result<TransformFinalizationDelivery, String> {
    let outcome = {
        let mut store = state.candidate_payload_store.lock();
        let mut authority = state.transform_authority.lock();
        file_candidates::sanitize_and_finalize_transform_operation(&request, raw_result, &mut store, &mut authority)
            .map_err(|error| error.message())?
    };
    let Some(result) = outcome.result else {
        return Ok(TransformFinalizationDelivery { terminal_category: outcome.terminal_category, sent: false });
    };
    let now = time::OffsetDateTime::now_utc();
    let event = serde_json::json!({
        "schemaVersion": "pastey-room-control-event-v1",
        "eventId": format!("transform-result-{}", uuid::Uuid::new_v4()),
        "kind": "capability_execution_result",
        "roomRef": request.room_ref,
        "sourceDeviceRef": request.target_peer_ref,
        "targetPeerRef": request.source_device_ref,
        "createdAt": now.format(&time::format_description::well_known::Rfc3339).map_err(|_| "Invalid Transform result time.")?,
        "expiresAt": (now + time::Duration::seconds(60)).format(&time::format_description::well_known::Rfc3339).map_err(|_| "Invalid Transform result expiry.")?,
        "previewOnly": false,
        "payload": result,
    });
    crate::room_control::send_authoritative_transform_result_event(state.inner().clone(), &request.room_ref, event, None)
        .await.map_err(|error| error.message())?;
    Ok(TransformFinalizationDelivery { terminal_category: outcome.terminal_category, sent: true })
}

#[tauri::command]
pub async fn get_transform_operation_status(
    request: ArtifactTransformClaimRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<ArtifactTransformClaimResult, String> {
    let authority = state.transform_authority.lock();
    Ok(file_candidates::artifact_transform_operation_status(&request, &authority))
}

#[tauri::command]
pub fn get_room_control_session_context(
    room_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomControlSessionContext, String> {
    crate::room_control::room_control_session_context(&state, &room_id)
        .map_err(|error| error.message())
}

#[tauri::command]
pub fn list_received_room_control_events(
    room_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<ReceivedRoomControlEvent>, String> {
    crate::room_control::list_received_room_control_events(&state, &room_id)
        .map_err(|error| error.message())
}

#[tauri::command]
pub fn write_temp_file(
    file_name: String,
    bytes: Vec<u8>,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let path = storage::write_temp_file(&state.paths, &file_name, &bytes)
        .map_err(|error| error.message())?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn get_file_transfer_metadata(path: String) -> Result<FileTransferMetadata, String> {
    let file_path = resolve_user_path(&path).map_err(|error| error.message())?;
    if !file_path.is_file() {
        return Err(AppError::InvalidInput("selected path is not a file".into()).message());
    }

    let (display_name, mime_type, size_bytes, modified_ms) =
        storage::file_transfer_metadata(&file_path).map_err(|error| error.message())?;
    Ok(FileTransferMetadata {
        path,
        display_name,
        mime_type,
        size_bytes,
        modified_ms,
    })
}

#[tauri::command]
pub fn delete_temp_file(path: String, state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    let file_path = resolve_user_path(&path).map_err(|error| error.message())?;
    storage::delete_temp_file(&state.paths, &file_path).map_err(|error| error.message())
}

#[tauri::command]
pub async fn burn_room(room_id: String, state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    run_async(async move {
        let peer = storage::get_room_by_id(&state.paths, &room_id)
            .ok()
            .and_then(|room| room.peer_host.zip(room.peer_port));
        transfer::cancel_room_transfers(
            state.inner().clone(),
            &room_id,
            "Room burned",
            false,
            Some("receiver_burned_room"),
        )
        .await?;
        let effective_inbox_dir = {
            let config = state.config.read();
            config::effective_inbox_dir(&state.paths, &config)
        };
        let removed = storage::burn_room(&state.paths, &room_id, &effective_inbox_dir)?.is_some();
        let _ = transfer::stop_room_server(state.inner().clone(), &room_id).await;
        if let Some((peer_host, peer_port)) = peer {
            transfer::notify_room_burn_with_peer(&peer_host, peer_port, &room_id).await;
        }
        Ok(removed)
    })
    .await
}

#[tauri::command]
pub async fn leave_room(room_id: String, state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    run_async(async move {
        // Internal legacy disconnect cleanup. This is not a user-facing room
        // lifecycle action; Burn Room is the product-level terminal action.
        let _ = transfer::cancel_room_transfers(
            state.inner().clone(),
            &room_id,
            "Transfer cancelled",
            true,
            Some("peer_disconnected"),
        )
        .await;
        transfer::notify_room_leave(state.inner().clone(), &room_id).await;
        let removed = storage::leave_room(&state.paths, &room_id)?.is_some();
        let _ = transfer::stop_room_server(state.inner().clone(), &room_id).await;
        Ok(removed)
    })
    .await
}

#[tauri::command]
pub async fn cancel_transfer(
    transfer_id: String,
    cancel_source: Option<String>,
    queue_item_id: Option<String>,
    batch_id: Option<String>,
    room_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<bool, String> {
    logging::write_transfer_line(&format!(
        "[pastey transfer][transfer_id={transfer_id}] event=cancel_transfer_command source={} queue_item_id={} batch_id={} room_id={}",
        log_field(cancel_source.as_deref()),
        log_field(queue_item_id.as_deref()),
        log_field(batch_id.as_deref()),
        log_field(room_id.as_deref())
    ));
    run_async(async move {
        transfer::cancel_transfer(state.inner().clone(), &transfer_id, cancel_source).await
    })
    .await
}

#[tauri::command]
pub fn update_transfer_window(
    transfer_id: String,
    requested_window: usize,
    state: State<'_, Arc<AppState>>,
) -> Result<transfer::UpdateTransferWindowResult, String> {
    let result =
        transfer::update_transfer_window(state.inner().clone(), &transfer_id, requested_window)
            .map_err(|error| error.message())?;
    logging::write_transfer_line(&format!(
        "[pastey transfer][transfer_id={transfer_id}] event=update_transfer_window updated={} reason={} requested_window={} previous_window={} effective_window={}",
        result.updated,
        result.reason,
        result.requested_window,
        result.previous_window.map(|value| value.to_string()).unwrap_or_else(|| "none".into()),
        result.effective_window.map(|value| value.to_string()).unwrap_or_else(|| "none".into())
    ));
    Ok(result)
}

#[tauri::command]
pub fn get_config(state: State<'_, Arc<AppState>>) -> Result<AppConfig, String> {
    let config = state.config.read().clone();
    Ok(config::public_config(&state.paths, &config))
}

#[tauri::command]
pub async fn get_device_profile(
    force_refresh: Option<bool>,
    state: State<'_, Arc<AppState>>,
) -> Result<diagnostics::DeviceProfile, String> {
    run_async(async move {
        let force_refresh = force_refresh.unwrap_or(false);
        if let Some(profile) = cached_device_profile(&state, force_refresh) {
            return Ok(profile);
        }

        let _guard = state.diagnostics_refresh.lock().await;
        if let Some(profile) = cached_device_profile(&state, force_refresh) {
            return Ok(profile);
        }

        let config = state.config.read().clone();
        let mode = diagnostics_profile_mode(force_refresh);
        let profile = tauri::async_runtime::spawn_blocking(move || {
            device_profile::local_device_profile_with_mode(&config, mode)
        })
        .await
        .map_err(|error| AppError::InvalidInput(format!("device profile probe failed: {error}")))?;
        state.latest_device_profile.lock().replace(profile.clone());
        Ok(profile)
    })
    .await
}

#[tauri::command]
pub async fn get_device_capabilities(
    force_refresh: Option<bool>,
    probe_mode: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<diagnostics::DeviceCapabilities, String> {
    run_async(async move {
        let force_refresh = force_refresh.unwrap_or(false);
        let capability_mode = diagnostics_capability_mode(force_refresh, probe_mode.as_deref())?;
        if let Some(capabilities) =
            cached_device_capabilities_for_mode(&state, force_refresh, capability_mode)
        {
            return Ok(capabilities);
        }

        let _guard = state.diagnostics_refresh.lock().await;
        if let Some(capabilities) =
            cached_device_capabilities_for_mode(&state, force_refresh, capability_mode)
        {
            return Ok(capabilities);
        }

        let config = state.config.read().clone();
        let profile_mode =
            diagnostics_profile_mode(force_refresh || capability_mode == CapabilityProbeMode::Full);
        let cached_profile =
            cached_profile_for_capability_probe(&state, force_refresh, capability_mode);
        let (profile, capabilities) = tauri::async_runtime::spawn_blocking(move || {
            let profile = cached_profile.unwrap_or_else(|| {
                device_profile::local_device_profile_with_mode(&config, profile_mode)
            });
            let capabilities =
                capability_probe::probe_device_capabilities_with_mode(&profile, capability_mode);
            (profile, capabilities)
        })
        .await
        .map_err(|error| {
            AppError::InvalidInput(format!("device capability probe failed: {error}"))
        })?;
        state.latest_device_profile.lock().replace(profile);
        state
            .latest_device_capabilities
            .lock()
            .replace(capabilities.clone());
        Ok(capabilities)
    })
    .await
}

#[tauri::command]
pub async fn run_loopback_benchmark(
    mode: Option<String>,
    duration_seconds: Option<u64>,
    window_size: Option<usize>,
    state: State<'_, Arc<AppState>>,
) -> Result<diagnostics::LinkBenchmarkResult, String> {
    run_async(async move {
        let mode = diagnostics::BenchmarkMode::from_option(mode.as_deref());
        let result = link_benchmark::run_loopback_benchmark(
            mode,
            duration_seconds,
            window_size,
            link_benchmark::cpu_hint(),
        )
        .await?;
        state
            .latest_benchmark_results
            .lock()
            .insert("loopback".into(), result.clone());
        Ok(result)
    })
    .await
}

#[tauri::command]
pub async fn run_peer_link_benchmark(
    room_id: String,
    mode: Option<String>,
    duration_seconds: Option<u64>,
    window_size: Option<usize>,
    state: State<'_, Arc<AppState>>,
) -> Result<diagnostics::LinkBenchmarkResult, String> {
    run_async(async move {
        let mode = diagnostics::BenchmarkMode::from_option(mode.as_deref());
        let result = link_benchmark::run_peer_link_benchmark(
            state.inner().clone(),
            room_id.clone(),
            mode,
            duration_seconds,
            window_size,
            link_benchmark::cpu_hint(),
        )
        .await?;
        state
            .latest_benchmark_results
            .lock()
            .insert(room_id, result.clone());
        Ok(result)
    })
    .await
}

#[tauri::command]
pub fn get_last_benchmark_results(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<diagnostics::LinkBenchmarkResult>, String> {
    let mut results = state
        .latest_benchmark_results
        .lock()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    results.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(results)
}

#[tauri::command]
pub fn update_config(
    // The frontend must invoke this as `configValue`; Tauri maps that camel-case
    // argument onto this Rust `config_value` parameter.
    config_value: AppConfig,
    state: State<'_, Arc<AppState>>,
) -> Result<AppConfig, String> {
    let mut guard = state.config.write();
    config::update(&state.paths, &mut guard, config_value).map_err(|error| error.message())
}

#[tauri::command]
pub fn reveal_in_folder(path: String, app: AppHandle) -> Result<(), String> {
    let path = resolve_user_path(&path).map_err(|error| error.message())?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_logs_folder(state: State<'_, Arc<AppState>>, app: AppHandle) -> Result<(), String> {
    std::fs::create_dir_all(&state.paths.logs_dir).map_err(|error| error.to_string())?;
    app.opener()
        .open_path(state.paths.logs_dir.display().to_string(), None::<String>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn copy_last_error(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<Option<String>, String> {
    let Some(summary) = logging::latest_error_summary(&state.paths.logs_dir) else {
        return Ok(None);
    };
    app.clipboard()
        .write_text(summary.clone())
        .map_err(|error| error.to_string())?;
    Ok(Some(summary))
}

#[tauri::command]
pub fn check_for_updates(app: AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(RELEASES_URL, None::<String>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn copy_text_to_clipboard(text: String, app: AppHandle) -> Result<(), String> {
    app.clipboard()
        .write_text(text)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn log_frontend_diagnostic(line: String) -> Result<bool, String> {
    let line = normalize_frontend_diagnostic_line(&line)?;
    logging::write_transfer_line(&line);
    Ok(true)
}

fn cached_device_profile(
    state: &Arc<AppState>,
    force_refresh: bool,
) -> Option<diagnostics::DeviceProfile> {
    state
        .latest_device_profile
        .lock()
        .clone()
        .filter(|profile| diagnostics_cache_is_fresh(profile.updated_at, force_refresh))
}

fn cached_device_capabilities(
    state: &Arc<AppState>,
    force_refresh: bool,
) -> Option<diagnostics::DeviceCapabilities> {
    state
        .latest_device_capabilities
        .lock()
        .clone()
        .filter(|capabilities| diagnostics_cache_is_fresh(capabilities.updated_at, force_refresh))
}

fn cached_device_capabilities_for_mode(
    state: &Arc<AppState>,
    force_refresh: bool,
    mode: CapabilityProbeMode,
) -> Option<diagnostics::DeviceCapabilities> {
    cached_device_capabilities(state, force_refresh)
        .filter(|capabilities| capability_cache_satisfies_mode(capabilities, mode))
}

fn capability_cache_satisfies_mode(
    capabilities: &diagnostics::DeviceCapabilities,
    mode: CapabilityProbeMode,
) -> bool {
    match mode {
        CapabilityProbeMode::Quick => true,
        CapabilityProbeMode::Full => !capabilities.runtimes.is_empty(),
    }
}

fn cached_profile_for_capability_probe(
    state: &Arc<AppState>,
    force_refresh: bool,
    mode: CapabilityProbeMode,
) -> Option<diagnostics::DeviceProfile> {
    if should_reuse_cached_profile_for_capability_probe(force_refresh, mode) {
        cached_device_profile(state, false)
    } else {
        None
    }
}

fn should_reuse_cached_profile_for_capability_probe(
    force_refresh: bool,
    mode: CapabilityProbeMode,
) -> bool {
    !force_refresh && mode == CapabilityProbeMode::Quick
}

fn diagnostics_cache_is_fresh(updated_at: i64, force_refresh: bool) -> bool {
    !force_refresh
        && updated_at > 0
        && storage::now_ts() <= updated_at.saturating_add(DIAGNOSTICS_CACHE_TTL_SECONDS)
}

fn diagnostics_profile_mode(force_refresh: bool) -> ProfileProbeMode {
    if force_refresh {
        ProfileProbeMode::Full
    } else {
        ProfileProbeMode::Quick
    }
}

fn diagnostics_capability_mode(
    force_refresh: bool,
    requested_mode: Option<&str>,
) -> AppResult<CapabilityProbeMode> {
    let requested_mode = match requested_mode {
        Some("quick") => Some(CapabilityProbeMode::Quick),
        Some("full") => Some(CapabilityProbeMode::Full),
        Some(mode) => {
            return Err(AppError::InvalidInput(format!(
                "unsupported capability probe mode: {mode}"
            )))
        }
        None => None,
    };
    Ok(if force_refresh {
        CapabilityProbeMode::Full
    } else {
        requested_mode.unwrap_or(CapabilityProbeMode::Quick)
    })
}

fn log_field(value: Option<&str>) -> &str {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("none")
}

fn normalize_frontend_diagnostic_line(line: &str) -> Result<String, String> {
    const MAX_FRONTEND_DIAGNOSTIC_CHARS: usize = 2_000;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("diagnostic log line is empty".into());
    }
    if trimmed.len() > MAX_FRONTEND_DIAGNOSTIC_CHARS {
        return Err("diagnostic log line is too long".into());
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("diagnostic log line must be single-line".into());
    }
    if !is_allowed_frontend_diagnostic_prefix(trimmed) {
        return Err("unsupported frontend diagnostic prefix".into());
    }
    if contains_path_like_sensitive_value(trimmed) {
        return Err("diagnostic log line must not include absolute paths".into());
    }
    Ok(trimmed.to_string())
}

fn is_allowed_frontend_diagnostic_prefix(line: &str) -> bool {
    line.starts_with("[pastey:planner] ")
        || line.starts_with("[pastey:micro-group] ")
        || line.starts_with("[pastey:runtime-window] ")
        || line.starts_with("[pastey:agent-bridge] ")
}

fn contains_path_like_sensitive_value(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("path=")
        || lower.contains("file://")
        || lower.contains("/users/")
        || lower.contains("/volumes/")
        || lower.contains("/tmp/")
        || lower.contains("/private/")
        || lower.contains("\\users\\")
        || lower.contains("c:\\")
        || lower.contains("d:\\")
}

async fn run_async<T>(
    future: impl std::future::Future<Output = AppResult<T>>,
) -> Result<T, String> {
    future.await.map_err(|error| error.message())
}

fn unique_room_code(paths: &storage::AppPaths) -> AppResult<String> {
    for _ in 0..16 {
        let code = crypto::generate_code();
        if !storage::active_room_code_exists(paths, &crypto::hash_code(&code))? {
            return Ok(code);
        }
    }

    Err(AppError::InvalidInput(
        "unable to allocate a unique room code".into(),
    ))
}

fn normalize_code(code: &str) -> AppResult<String> {
    let compact = code.replace('-', "");
    if compact.len() != 8 || !compact.chars().all(|char| char.is_ascii_digit()) {
        return Err(AppError::InvalidInput("enter an 8-digit room code".into()));
    }
    Ok(compact)
}

fn resolve_user_path(input: &str) -> AppResult<PathBuf> {
    if input.starts_with("file://") {
        let url = url::Url::parse(input)?;
        return url
            .to_file_path()
            .map_err(|_| AppError::InvalidInput("invalid file URL".into()));
    }

    Ok(PathBuf::from(input))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bridge_route_room() -> StoredRoom {
        StoredRoom {
            id: "room-1".into(),
            room_code_hash: "hash".into(),
            created_at: 1,
            expires_at: 2,
            status: RoomStatus::Active,
            local_role: LocalRole::Creator,
            peer_device_name: Some("Peer".into()),
            auto_burn_after_expiry: false,
            wrapped_room_code: "wrapped".into(),
            code_nonce: "nonce".into(),
            peer_host: Some("127.0.0.1".into()),
            peer_port: Some(9000),
            peer_transport_public_key: Some("peer-key".into()),
            local_burned_at: None,
            peer_burned_at: None,
        }
    }

    fn matching_bridge_route(schema_version: &str) -> Value {
        json!({
            "schemaVersion": schema_version,
            "bridgeSessionId": "legacy-room:room-1",
            "target": {
                "kind": "selected_peer",
                "peerSessionId": "legacy-room-peer:room-1"
            }
        })
    }

    fn bridge_route_peers() -> Vec<StoredBridgePeerEndpoint> {
        vec![StoredBridgePeerEndpoint {
            room_id: "room-1".into(),
            peer_session_id: "legacy-room-peer:room-1".into(),
            display_name: Some("Peer".into()),
            endpoint_host: Some("127.0.0.1".into()),
            endpoint_port: Some(9000),
            transport_public_key: Some("peer-key".into()),
            liveness: BridgePeerLiveness::Connected,
            join_method: crate::models::BridgePeerJoinMethod::NearbyAccept,
            durable_identity_id: None,
            updated_at: 1,
        }]
    }

    fn assert_route_error_code(
        result: AppResult<impl std::fmt::Debug>,
        code: BridgeRouteErrorCode,
    ) {
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(&format!("code={}", code.as_str())),
            "expected route error code {}, got {error}",
            code.as_str()
        );
    }

    fn second_bridge_route_peer() -> StoredBridgePeerEndpoint {
        StoredBridgePeerEndpoint {
            room_id: "room-1".into(),
            peer_session_id: "legacy-room-peer:room-1:1".into(),
            display_name: Some("Peer 2".into()),
            endpoint_host: Some("127.0.0.2".into()),
            endpoint_port: Some(9001),
            transport_public_key: Some("peer-key-2".into()),
            liveness: BridgePeerLiveness::Connected,
            join_method: crate::models::BridgePeerJoinMethod::NearbyAccept,
            durable_identity_id: None,
            updated_at: 2,
        }
    }

    #[test]
    fn diagnostics_cache_respects_force_refresh_and_ttl() {
        let now = storage::now_ts();

        assert!(now > 0);
        assert!(now <= now.saturating_add(DIAGNOSTICS_CACHE_TTL_SECONDS));
        assert!(diagnostics_cache_is_fresh(now, false));
        assert!(!diagnostics_cache_is_fresh(now, true));
        assert!(!diagnostics_cache_is_fresh(
            now - DIAGNOSTICS_CACHE_TTL_SECONDS - 1,
            false
        ));
    }

    #[test]
    fn diagnostics_normal_load_uses_quick_probe_modes() {
        assert_eq!(diagnostics_profile_mode(false), ProfileProbeMode::Quick);
        assert_eq!(
            diagnostics_capability_mode(false, None).unwrap(),
            CapabilityProbeMode::Quick
        );
        assert_eq!(diagnostics_profile_mode(true), ProfileProbeMode::Full);
        assert_eq!(
            diagnostics_capability_mode(true, None).unwrap(),
            CapabilityProbeMode::Full
        );
    }

    #[test]
    fn diagnostics_can_request_full_capability_probe_without_force_refresh() {
        assert_eq!(
            diagnostics_capability_mode(false, Some("full")).unwrap(),
            CapabilityProbeMode::Full
        );
        assert_eq!(
            diagnostics_capability_mode(false, Some("quick")).unwrap(),
            CapabilityProbeMode::Quick
        );
        assert!(diagnostics_capability_mode(false, Some("unknown")).is_err());
    }

    #[test]
    fn full_capability_probe_rejects_cached_quick_capabilities() {
        let profile = diagnostics::DeviceProfile {
            device_id: "device".into(),
            device_name: "Pastey".into(),
            platform: std::env::consts::OS.into(),
            os_version: None,
            arch: std::env::consts::ARCH.into(),
            cpu_name: None,
            cpu_physical_core_count: None,
            cpu_logical_processor_count: None,
            cpu_core_count: None,
            memory_total_gb: None,
            gpu_names: Vec::new(),
            power_state: diagnostics::PowerState::Unknown,
            battery_percent: None,
            updated_at: storage::now_ts(),
        };
        let quick_capabilities = diagnostics::DeviceCapabilities {
            runtimes: Vec::new(),
            gpu_acceleration: diagnostics::GpuAcceleration {
                cuda_available: false,
                metal_available: false,
                gpu_names: Vec::new(),
                vram_gb: None,
            },
            updated_at: storage::now_ts(),
        };
        let full_capabilities = capability_probe::probe_device_capabilities_with_mode(
            &profile,
            CapabilityProbeMode::Full,
        );

        assert!(!capability_cache_satisfies_mode(
            &quick_capabilities,
            CapabilityProbeMode::Full
        ));
        assert!(capability_cache_satisfies_mode(
            &full_capabilities,
            CapabilityProbeMode::Full
        ));
        assert!(capability_cache_satisfies_mode(
            &quick_capabilities,
            CapabilityProbeMode::Quick
        ));
    }

    #[test]
    fn forced_capability_refresh_does_not_reuse_cached_quick_profile() {
        assert!(should_reuse_cached_profile_for_capability_probe(
            false,
            CapabilityProbeMode::Quick
        ));
        assert!(!should_reuse_cached_profile_for_capability_probe(
            false,
            CapabilityProbeMode::Full
        ));
        assert!(!should_reuse_cached_profile_for_capability_probe(
            true,
            CapabilityProbeMode::Quick
        ));
    }

    #[test]
    fn bridge_route_payload_accepts_matching_selected_peer_text_file_and_legacy_no_route() {
        let room = bridge_route_room();
        let peers = bridge_route_peers();
        let text_route = matching_bridge_route(TEXT_BRIDGE_ROUTE_SCHEMA_VERSION);
        let file_route = matching_bridge_route(FILE_BRIDGE_ROUTE_SCHEMA_VERSION);

        assert_eq!(
            validate_bridge_route_payload(
                Some(&text_route),
                "room-1",
                &room,
                &peers,
                TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
                "text"
            )
            .unwrap()
            .endpoints,
            vec![transfer::BridgePeerTransferEndpoint {
                peer_session_id: "legacy-room-peer:room-1".into(),
                host: "127.0.0.1".into(),
                port: 9000,
                transport_public_key: "peer-key".into(),
            }]
        );
        assert_eq!(
            validate_bridge_route_payload(
                Some(&file_route),
                "room-1",
                &room,
                &peers,
                FILE_BRIDGE_ROUTE_SCHEMA_VERSION,
                "file"
            )
            .unwrap()
            .endpoints,
            vec![transfer::BridgePeerTransferEndpoint {
                peer_session_id: "legacy-room-peer:room-1".into(),
                host: "127.0.0.1".into(),
                port: 9000,
                transport_public_key: "peer-key".into(),
            }]
        );
        assert!(validate_bridge_route_payload(
            None,
            "room-1",
            &room,
            &peers,
            FILE_BRIDGE_ROUTE_SCHEMA_VERSION,
            "file"
        )
        .unwrap()
        .endpoints
        .is_empty());
    }

    #[test]
    fn bridge_route_payload_resolves_explicit_broadcast_for_data_delivery() {
        let room = bridge_route_room();
        let peers = bridge_route_peers();
        let broadcast = json!({
            "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "bridgeSessionId": "legacy-room:room-1",
            "target": {
                "kind": "broadcast_bridge",
                "explicit": true
            }
        });

        let targets = validate_bridge_route_payload(
            Some(&broadcast),
            "room-1",
            &room,
            &peers,
            TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "text",
        )
        .unwrap();
        assert_eq!(targets.target_kind, BridgeRouteTargetKind::BroadcastBridge);
        assert_eq!(targets.endpoints.len(), 1);
        assert_eq!(
            bridge_send_target_for_route(&targets),
            Some(BridgeSendTarget::BroadcastBridge { explicit: true })
        );
    }

    #[test]
    fn bridge_route_payload_validates_selected_peers_against_endpoint_table() {
        let room = bridge_route_room();
        let mut peers = bridge_route_peers();
        peers.push(second_bridge_route_peer());
        let selected_peers = json!({
            "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "bridgeSessionId": "legacy-room:room-1",
            "target": {
                "kind": "selected_peers",
                "peerSessionIds": ["legacy-room-peer:room-1", "legacy-room-peer:room-1:1"]
            }
        });

        let targets = validate_bridge_route_payload(
            Some(&selected_peers),
            "room-1",
            &room,
            &peers,
            TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "text",
        )
        .unwrap();
        assert_eq!(targets.target_kind, BridgeRouteTargetKind::SelectedPeers);
        assert_eq!(targets.endpoints.len(), 2);
        assert_eq!(
            targets.endpoints[0].peer_session_id,
            "legacy-room-peer:room-1"
        );
        assert_eq!(
            targets.endpoints[1].peer_session_id,
            "legacy-room-peer:room-1:1"
        );
        assert_eq!(
            bridge_send_target_for_route(&targets),
            Some(BridgeSendTarget::SelectedPeers {
                peer_session_refs: vec![
                    "legacy-room-peer:room-1".into(),
                    "legacy-room-peer:room-1:1".into(),
                ],
            })
        );
    }

    #[test]
    fn bridge_route_payload_selected_peers_keeps_known_stale_targets_as_rejected_outcomes() {
        let room = bridge_route_room();
        let mut peers = bridge_route_peers();
        let mut stale_peer = second_bridge_route_peer();
        stale_peer.liveness = BridgePeerLiveness::Stale;
        stale_peer.endpoint_host = None;
        stale_peer.endpoint_port = None;
        stale_peer.transport_public_key = None;
        peers.push(stale_peer);
        let selected_peers = json!({
            "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "bridgeSessionId": "legacy-room:room-1",
            "target": {
                "kind": "selected_peers",
                "peerSessionIds": ["legacy-room-peer:room-1", "legacy-room-peer:room-1:1"]
            }
        });

        let targets = validate_bridge_route_payload(
            Some(&selected_peers),
            "room-1",
            &room,
            &peers,
            TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "text",
        )
        .unwrap();
        assert_eq!(targets.target_kind, BridgeRouteTargetKind::SelectedPeers);
        assert_eq!(targets.targets.len(), 2);
        assert_eq!(targets.endpoints.len(), 1);
        assert_eq!(
            targets.targets[1].route_error_code,
            Some(BridgeRouteErrorCode::RouteExpired)
        );
        assert_eq!(
            bridge_send_target_for_route(&targets),
            Some(BridgeSendTarget::SelectedPeers {
                peer_session_refs: vec![
                    "legacy-room-peer:room-1".into(),
                    "legacy-room-peer:room-1:1".into(),
                ],
            })
        );

        let operation_id = bridge_operation_id("text", "item-1");
        let bridge_session_ref = "legacy-room:room-1";
        let outcomes = vec![
            bridge_delivery_outcome(
                &operation_id,
                bridge_session_ref,
                "legacy-room-peer:room-1",
                BridgeDeliveryTargetKind::SelectedPeers,
                BridgeDeliveryContentKind::Text,
                BridgeDeliveryOutcomeStatus::Delivered,
                None,
            ),
            bridge_delivery_outcome(
                &operation_id,
                bridge_session_ref,
                "legacy-room-peer:room-1:1",
                BridgeDeliveryTargetKind::SelectedPeers,
                BridgeDeliveryContentKind::Text,
                BridgeDeliveryOutcomeStatus::Rejected,
                Some(BridgeRouteErrorCode::RouteExpired.as_str()),
            ),
        ];
        let operation = bridge_send_operation(
            "item-1",
            "text",
            BridgeDeliveryContentKind::Text,
            &targets,
            outcomes,
        )
        .unwrap();
        assert_eq!(
            operation.aggregate_status,
            BridgeSendAggregateStatus::Partial
        );
        assert_eq!(
            operation.resolved_peer_session_refs,
            vec![
                "legacy-room-peer:room-1".to_string(),
                "legacy-room-peer:room-1:1".to_string(),
            ]
        );
    }

    #[test]
    fn bridge_route_payload_rejects_mismatch_unknown_malformed_and_unsupported_authority_fields_with_codes(
    ) {
        let room = bridge_route_room();
        let peers = bridge_route_peers();
        let cases = [
            (
                BridgeRouteErrorCode::RouteMismatch,
                json!({
                    "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
                    "bridgeSessionId": "legacy-room:other",
                    "target": {
                        "kind": "selected_peer",
                        "peerSessionId": "legacy-room-peer:room-1"
                    }
                }),
            ),
            (
                BridgeRouteErrorCode::UnknownPeer,
                json!({
                    "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
                    "bridgeSessionId": "legacy-room:room-1",
                    "target": {
                        "kind": "selected_peer",
                        "peerSessionId": "legacy-room-peer:unknown"
                    }
                }),
            ),
            (
                BridgeRouteErrorCode::MalformedRoute,
                json!({
                    "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
                    "bridgeSessionId": "legacy-room:room-1",
                    "target": {
                        "kind": "broadcast_bridge",
                        "explicit": false
                    }
                }),
            ),
            (
                BridgeRouteErrorCode::MalformedRoute,
                json!({
                    "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
                    "bridgeSessionId": "legacy-room:room-1",
                    "target": {
                        "kind": "selected_peers",
                        "peerSessionIds": ["legacy-room-peer:room-1"]
                    }
                }),
            ),
            (
                BridgeRouteErrorCode::MalformedRoute,
                json!({
                    "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
                    "bridgeSessionId": "legacy-room:room-1"
                }),
            ),
            (
                BridgeRouteErrorCode::MalformedRoute,
                json!({
                    "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
                    "bridgeSessionId": "legacy-room:room-1",
                    "target": {
                        "kind": "selected_peer",
                        "peerSessionId": "legacy-room-peer:room-1"
                    },
                    "trust": true
                }),
            ),
        ];

        for (code, route) in cases {
            assert_route_error_code(
                validate_bridge_route_payload(
                    Some(&route),
                    "room-1",
                    &room,
                    &peers,
                    TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
                    "text",
                ),
                code,
            );
        }
    }

    #[test]
    fn bridge_route_payload_rejects_inactive_disconnected_and_stale_peers_with_codes() {
        let route = matching_bridge_route(FILE_BRIDGE_ROUTE_SCHEMA_VERSION);
        let room = bridge_route_room();
        let mut disconnected_peers = bridge_route_peers();
        disconnected_peers[0].liveness = BridgePeerLiveness::Disconnected;
        assert_route_error_code(
            validate_bridge_route_payload(
                Some(&route),
                "room-1",
                &room,
                &disconnected_peers,
                FILE_BRIDGE_ROUTE_SCHEMA_VERSION,
                "file",
            ),
            BridgeRouteErrorCode::PeerUnrouteable,
        );

        let mut missing_endpoint = bridge_route_peers();
        missing_endpoint[0].endpoint_host = None;
        assert_route_error_code(
            validate_bridge_route_payload(
                Some(&route),
                "room-1",
                &room,
                &missing_endpoint,
                FILE_BRIDGE_ROUTE_SCHEMA_VERSION,
                "file",
            ),
            BridgeRouteErrorCode::PeerUnrouteable,
        );

        let mut stale_peer = bridge_route_peers();
        stale_peer[0].liveness = BridgePeerLiveness::Stale;
        assert_route_error_code(
            validate_bridge_route_payload(
                Some(&route),
                "room-1",
                &room,
                &stale_peer,
                FILE_BRIDGE_ROUTE_SCHEMA_VERSION,
                "file",
            ),
            BridgeRouteErrorCode::RouteExpired,
        );

        let mut stale = bridge_route_room();
        stale.status = RoomStatus::PeerLeft;
        let peers = bridge_route_peers();
        assert_route_error_code(
            validate_bridge_route_payload(
                Some(&route),
                "room-1",
                &stale,
                &peers,
                FILE_BRIDGE_ROUTE_SCHEMA_VERSION,
                "file",
            ),
            BridgeRouteErrorCode::RouteExpired,
        );
    }

    #[test]
    fn durable_identity_marker_does_not_change_route_validation_or_broadcast_resolution() {
        let route = matching_bridge_route(FILE_BRIDGE_ROUTE_SCHEMA_VERSION);
        let room = bridge_route_room();
        let mut disconnected_peers = bridge_route_peers();
        disconnected_peers[0].durable_identity_id = Some("paired-device:one".into());
        disconnected_peers[0].liveness = BridgePeerLiveness::Disconnected;
        assert_route_error_code(
            validate_bridge_route_payload(
                Some(&route),
                "room-1",
                &room,
                &disconnected_peers,
                FILE_BRIDGE_ROUTE_SCHEMA_VERSION,
                "file",
            ),
            BridgeRouteErrorCode::PeerUnrouteable,
        );

        let mut peers = bridge_route_peers();
        let mut paired_stale = second_bridge_route_peer();
        paired_stale.durable_identity_id = Some("paired-device:two".into());
        paired_stale.liveness = BridgePeerLiveness::Stale;
        paired_stale.endpoint_host = None;
        paired_stale.endpoint_port = None;
        paired_stale.transport_public_key = None;
        peers.push(paired_stale);
        let broadcast = json!({
            "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "bridgeSessionId": "legacy-room:room-1",
            "target": {
                "kind": "broadcast_bridge",
                "explicit": true
            }
        });
        let targets = validate_bridge_route_payload(
            Some(&broadcast),
            "room-1",
            &room,
            &peers,
            TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "text",
        )
        .unwrap();
        assert_eq!(targets.endpoints.len(), 1);
        assert_eq!(
            targets.endpoints[0].peer_session_id,
            "legacy-room-peer:room-1"
        );
    }

    #[test]
    fn bridge_route_payload_does_not_fall_back_to_arbitrary_peer_when_validation_fails() {
        let room = bridge_route_room();
        let mut peers = bridge_route_peers();
        peers.push(second_bridge_route_peer());
        let unknown_selected_peer = json!({
            "schemaVersion": TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
            "bridgeSessionId": "legacy-room:room-1",
            "target": {
                "kind": "selected_peer",
                "peerSessionId": "legacy-room-peer:unknown"
            }
        });

        assert_route_error_code(
            validate_bridge_route_payload(
                Some(&unknown_selected_peer),
                "room-1",
                &room,
                &peers,
                TEXT_BRIDGE_ROUTE_SCHEMA_VERSION,
                "text",
            ),
            BridgeRouteErrorCode::UnknownPeer,
        );
    }

    #[test]
    fn frontend_diagnostic_log_accepts_known_prefixes() {
        let line = "[pastey:micro-group] event=planned room_id=room group_id=group children=2 requested_window=1";

        assert_eq!(normalize_frontend_diagnostic_line(line).unwrap(), line);
        let agent_bridge = "[pastey:agent-bridge] {\"category\":\"agent_bridge\",\"eventKind\":\"peer_allowed_once\",\"roomRefShort\":\"room..short\"}";
        assert_eq!(
            normalize_frontend_diagnostic_line(agent_bridge).unwrap(),
            agent_bridge
        );
    }

    #[test]
    fn frontend_diagnostic_log_rejects_unknown_prefix_and_paths() {
        assert!(normalize_frontend_diagnostic_line("[pastey queue] event=nope").is_err());
        assert!(normalize_frontend_diagnostic_line(
            "[pastey:planner] event=launch_summary path=/Users/example/secret.txt"
        )
        .is_err());
        assert!(normalize_frontend_diagnostic_line(
            "[pastey:runtime-window] event=summary display_name=C:\\Users\\me\\secret.txt"
        )
        .is_err());
    }
}

use std::{path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;
use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};

use crate::{
    bridge_plan::{
        self, ActivityKind, BridgePlan, BridgePlanActivity, BridgePlanApproval, BridgePlanRecords,
        BridgePlanResultSummary, BridgePlanRevision, BridgePlanState, RevisionState,
    },
    capability_probe::{self, CapabilityProbeMode},
    config, crypto,
    device_profile::{self, ProfileProbeMode},
    diagnostics, discovery,
    error::{AppError, AppResult},
    file_candidates::{self, BridgePlanSearchRequest},
    link_benchmark, logging,
    models::{
        AppConfig, BridgeDeliveryContentKind, BridgeDeliveryOutcome, BridgeDeliveryOutcomeStatus,
        BridgeDeliveryTargetKind, BridgePeerLiveness, BridgeSendAggregateStatus,
        BridgeSendOperation, BridgeSendTarget, JoinRequestPrompt, LocalRole, NearbyDevice,
        RoomInfo, RoomItem, RoomStatus, StoredBridgePeerEndpoint, StoredRoom,
    },
    room_control::{
        ReceivedRoomControlEvent, RoomControlDeliveryReceipt, RoomControlSessionContext,
    },
    storage, transfer, AppState,
};

const RELEASES_URL: &str = "https://github.com/xpan0988/Pastey/releases";
const DIAGNOSTICS_CACHE_TTL_SECONDS: i64 = 60;
const TEXT_BRIDGE_ROUTE_SCHEMA_VERSION: &str = "pastey-bridge-text-route-v1";
const FILE_BRIDGE_ROUTE_SCHEMA_VERSION: &str = "pastey-bridge-file-route-v1";

const BRIDGE_PLAN_APPROVAL_TTL_SECONDS: i64 = 24 * 60 * 60;
const BRIDGE_PLAN_CONTROL_LIFETIME_SECONDS: i64 = 120;

/// Requests a fresh, selected-peer capability observation over the existing
/// current-session Room Control channel. Delivery is not availability: the
/// response remains a separate non-authorizing fact.
#[tauri::command]
pub async fn refresh_selected_peer_capabilities(
    room_id: String,
    bridge_route: Option<Value>,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomControlDeliveryReceipt, String> {
    let state = state.inner().clone();
    let context = crate::room_control::room_control_session_context(&state, &room_id)
        .map_err(|error| error.message())?;
    let event = crate::room_control::peer_capability_event(
        "peer_capability.query",
        serde_json::json!({
            "schemaVersion": crate::peer_capabilities::PEER_CAPABILITY_SCHEMA,
            "peerSessionId": context.peer_route_ref,
        }),
        &context,
    )
    .map_err(|error| error.message())?;
    crate::room_control::log_peer_capability("query_dispatch", None, None);
    match crate::room_control::send_room_control_event(state, &room_id, event, bridge_route).await {
        Ok(receipt) => {
            crate::room_control::log_peer_capability("query_delivered", None, None);
            Ok(receipt)
        }
        Err(error) => {
            crate::room_control::log_peer_capability(
                "query_rejected",
                None,
                Some("delivery_failed"),
            );
            Err(error.message())
        }
    }
}

/// Returns only the fresh observation bound to the currently selected peer
/// session. Missing, restarted, reconnected, or purged state is Unknown.
#[tauri::command]
pub fn selected_peer_transform_availability(
    room_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<crate::peer_capabilities::SelectedPeerTransformAvailability, String> {
    let context = crate::room_control::room_control_session_context(state.inner(), &room_id)
        .map_err(|error| error.message())?;
    Ok(state.peer_capabilities.lock().selected_transform(
        &room_id,
        &context.peer_route_ref,
        &context.peer_observation_ref,
        storage::now_ts(),
    ))
}

/// A local production-backend fact for the requesting-device executor choice.
/// It is a capability observation only and is never persisted as authority.
#[tauri::command]
pub fn local_transform_availability() -> crate::peer_capabilities::SelectedPeerTransformAvailability
{
    let projection = crate::peer_capabilities::local_projection("local".into(), storage::now_ts());
    let capability = &projection.capabilities[0];
    crate::peer_capabilities::SelectedPeerTransformAvailability {
        peer_session_id: "local".into(),
        status: if capability.available {
            "available"
        } else {
            "unavailable"
        },
        available: capability.available,
        reason: if capability.available {
            "available"
        } else {
            "platform_unsupported"
        },
        accepted_input_media_types: capability.accepted_input_media_types.clone(),
        output_media_type: Some(capability.output_media_type.clone()),
    }
}

/// A selected-peer fact is only composition/admission input. This does not
/// issue any execution authority; receiver-local Transform execution still
/// revalidates its one-use grant, candidate, media type, and backend.
fn require_selected_peer_transform_available(
    state: &Arc<AppState>,
    room_id: &str,
    context: &RoomControlSessionContext,
) -> Result<(), String> {
    let observation = state.peer_capabilities.lock().selected_transform(
        room_id,
        &context.peer_route_ref,
        &context.peer_observation_ref,
        storage::now_ts(),
    );
    if observation.available {
        Ok(())
    } else {
        Err(observation.reason.into())
    }
}

fn bridge_plan_control_event(
    kind: &str,
    payload: Value,
    context: &RoomControlSessionContext,
) -> AppResult<Value> {
    let now = OffsetDateTime::now_utc();
    Ok(serde_json::json!({
        "schemaVersion": "pastey-room-control-event-v1",
        "eventId": format!("bridge-plan-event-{}", uuid::Uuid::new_v4()),
        "kind": kind,
        "protocolFamily": "bridge_plan",
        "roomRef": context.room_id,
        "sourceDeviceRef": context.local_session_ref,
        "targetPeerRef": context.peer_session_ref,
        "createdAt": now.format(&Rfc3339).map_err(|_| AppError::InvalidInput("Unable to format Bridge Plan event time.".into()))?,
        "expiresAt": (now + Duration::seconds(BRIDGE_PLAN_CONTROL_LIFETIME_SECONDS)).format(&Rfc3339).map_err(|_| AppError::InvalidInput("Unable to format Bridge Plan event time.".into()))?,
        "previewOnly": false,
        "payload": payload,
    }))
}

/// Emits only opaque Bridge Plan correlation identifiers.  It deliberately
/// excludes transport keys, private paths, and protocol bodies so two-device
/// failures can be correlated without turning the event log into authority.
fn log_bridge_plan_control(event: &Value, stage: &str) {
    let field = |object: &Value, name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 256)
            .unwrap_or("unknown")
            .to_string()
    };
    let payload = event.get("payload").unwrap_or(event);
    logging::write_transfer_line(&format!(
        "[pastey bridge-plan-control] stage={stage} kind={} event_id={} bridge_id={} plan_id={} revision_id={} approval_id={} source_session={} target_session={}",
        field(event, "kind"),
        field(event, "eventId"),
        field(payload, "bridgeId"),
        field(payload, "planId"),
        field(payload, "revisionId"),
        field(payload, "approvalId"),
        field(event, "sourceDeviceRef"),
        field(event, "targetPeerRef"),
    ));
}

fn log_bridge_plan_search_attempt(
    stage: &str,
    room_id: &str,
    attempt_id: &str,
    code: &str,
    candidate_count: Option<usize>,
) {
    logging::write_transfer_line(&format!(
        "[pastey bridge-plan-search] stage={stage} bridge_id={room_id} attempt_id={attempt_id} platform={} code={code} candidate_count={}",
        std::env::consts::OS,
        candidate_count.map(|count| count.to_string()).unwrap_or_else(|| "unknown".into()),
    ));
}

/// Emits only correlation identifiers and a bounded execution stage for the
/// receiver-owned Transfer path. Private candidate paths and transfer payloads
/// must not enter diagnostic logs.
fn log_bridge_plan_transfer_attempt(stage: &str, room_id: &str, attempt_id: &str, code: &str) {
    logging::write_transfer_line(&format!(
        "[pastey bridge-plan-transfer] stage={stage} bridge_id={room_id} attempt_id={attempt_id} platform={} code={code}",
        std::env::consts::OS,
    ));
}

/// Pipeline diagnostics deliberately carry only correlation ids and bounded
/// stage/code values. They never include an app-private handoff path, bytes,
/// object reference, or transport credential.
pub(crate) fn log_pipeline_handoff(
    stage: &str,
    bridge_id: &str,
    attempt_id: &str,
    step_id: &str,
    code: &str,
) {
    logging::write_transfer_line(&format!(
        "[pastey pipeline-handoff] stage={stage} bridge_id={bridge_id} attempt_id={attempt_id} step_id={step_id} platform={} code={code}",
        std::env::consts::OS,
    ));
}

fn bridge_plan_transfer_failure_code(error: &AppError) -> &'static str {
    let message = error.message();
    if message.contains("candidate changed") {
        "candidate_changed"
    } else if message.contains("candidate") {
        "candidate_unavailable"
    } else if message.contains("route target") || message.contains("routeable") {
        "route_unavailable"
    } else if message.contains("outgoing") || message.contains("master key") {
        "outgoing_item_failed"
    } else {
        "file_send_failed"
    }
}

/// Renderer-provided intent for a file Search. Device bindings, immutable
/// revision shape, and authority stay Host-owned.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileSearchBridgePlanRequest {
    pub room_id: String,
    pub original_user_goal: String,
    pub filename_hint: String,
    pub extensions: Vec<String>,
    pub safe_scopes: Vec<String>,
    #[serde(default)]
    pub transfer_to_requester: bool,
    #[serde(default)]
    pub transfer_destination: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileTransformAlternativeBridgePlanRequest {
    pub room_id: String,
    pub original_user_goal: String,
    pub filename_hint: String,
    pub extensions: Vec<String>,
    pub safe_scopes: Vec<String>,
    pub transform_intent: String,
    pub transform_execution_device: String,
    #[serde(default)]
    pub transfer_to_requester: bool,
    #[serde(default)]
    pub transfer_destination: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectFileTransferBridgePlanRequest {
    pub room_id: String,
    pub original_user_goal: String,
    pub source_path: String,
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

/// Creates the first immutable revision for a Bridge workspace.  The caller
/// supplies only reviewed product semantics; the Host recomputes the semantic
/// hash and owns every durable state transition.
pub fn create_bridge_plan(
    mut revision: BridgePlanRevision,
    state: State<'_, Arc<AppState>>,
) -> Result<BridgePlanRecords, String> {
    let now = storage::now_ts();
    revision.revision_hash =
        bridge_plan::canonical_revision_hash(&revision).map_err(|error| error.message())?;
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let plan = BridgePlan {
        plan_id: revision.plan_id.clone(),
        bridge_id: revision.bridge_id.clone(),
        requesting_device_ref: revision.requesting_device_ref.clone(),
        created_at: now,
    };
    store
        .create_plan(&plan, BridgePlanState::Draft)
        .map_err(|error| error.message())?;
    store
        .transition_plan(&plan.plan_id, BridgePlanState::Open)
        .map_err(|error| error.message())?;
    store
        .append_revision(&revision, RevisionState::Proposed, now)
        .map_err(|error| error.message())?;
    store
        .transition_revision(&revision.revision_id, RevisionState::Available)
        .map_err(|error| error.message())?;
    store
        .append_activity(&BridgePlanActivity {
            activity_id: format!("plan-created-{}", uuid::Uuid::new_v4()),
            bridge_id: revision.bridge_id.clone(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            attempt_id: None,
            step_id: None,
            kind: ActivityKind::RevisionProposed,
            occurred_at: now,
            summary: "Plan ready for complete review.".into(),
        })
        .map_err(|error| error.message())?;
    store
        .list_bridge(&revision.bridge_id)
        .map_err(|error| error.message())
}

/// Creates the supported file Search plan from a bounded natural-language
/// advisory. This is the renderer entry point; arbitrary Bridge Plan revisions
/// are not constructed in TypeScript.
#[tauri::command]
pub fn create_file_search_bridge_plan(
    request: FileSearchBridgePlanRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<BridgePlanRecords, String> {
    let context = crate::room_control::room_control_session_context(&state, &request.room_id)
        .map_err(|error| error.message())?;
    let mut revision = bridge_plan::build_file_plan_revision(
        request.room_id,
        context.local_session_ref,
        context.peer_session_ref.clone(),
        request.original_user_goal,
        request.filename_hint,
        request.extensions,
        request.safe_scopes,
        request.transfer_to_requester
            || request.transfer_destination.as_deref() == Some("selected_device"),
    )
    .map_err(|error| error.message())?;
    if request.transfer_destination.as_deref() == Some("selected_device") {
        for step in &mut revision.steps {
            if let bridge_plan::BridgePlanStep::Transfer { destination, .. } = step {
                *destination = bridge_plan::TransferDestination::UserSelectedLocation {
                    device_ref: context.peer_session_ref.clone(),
                    user_visible_location_scope: bridge_plan::SafeLocationDescription::from(
                        "Pastey Shared",
                    ),
                };
            }
        }
        revision.revision_hash =
            bridge_plan::canonical_revision_hash(&revision).map_err(|error| error.message())?;
    }
    create_bridge_plan(revision, state)
}

/// Creates a live supported Search -> Transform plan. The request must carry
/// the reviewed Bridge-role executor; Rust resolves it to the current session,
/// validates that Host's capability fact, and constructs the immutable flow.
#[tauri::command]
pub fn create_file_transform_bridge_plan(
    request: FileTransformAlternativeBridgePlanRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<BridgePlanRecords, String> {
    let context = crate::room_control::room_control_session_context(&state, &request.room_id)
        .map_err(|error| error.message())?;
    let transform_execution_device = match request.transform_execution_device.as_str() {
        "selected_device" => {
            require_selected_peer_transform_available(state.inner(), &request.room_id, &context)?;
            context.peer_session_ref.clone()
        }
        "requesting_device" => {
            if !crate::peer_capabilities::local_projection("local".into(), storage::now_ts())
                .capabilities[0]
                .available
            {
                return Err("This device cannot execute Extract readable text with its current secure Transform backend.".into());
            }
            context.local_session_ref.clone()
        }
        _ => return Err("Transform execution device is not part of this current Bridge.".into()),
    };
    let mut revision = bridge_plan::build_file_transform_revision(
        request.room_id,
        context.local_session_ref,
        context.peer_session_ref.clone(),
        request.original_user_goal,
        request.filename_hint,
        request.extensions,
        request.safe_scopes,
        request.transform_intent,
        transform_execution_device,
        request.transfer_to_requester,
    )
    .map_err(|error| error.message())?;
    if request.transfer_destination.as_deref() == Some("selected_device") {
        for step in &mut revision.steps {
            if let bridge_plan::BridgePlanStep::Transfer { destination, .. } = step {
                *destination = bridge_plan::TransferDestination::UserSelectedLocation {
                    device_ref: context.peer_session_ref.clone(),
                    user_visible_location_scope: bridge_plan::SafeLocationDescription::from(
                        "Pastey Shared",
                    ),
                };
            }
        }
        revision.revision_hash =
            bridge_plan::canonical_revision_hash(&revision).map_err(|error| error.message())?;
    }
    create_bridge_plan(revision, state)
}

/// Withdraws an unapproved renderer-visible revision after its draft semantics
/// change. The Host verifies current requester/session ownership and performs
/// the durable transition atomically; an approved revision cannot be edited.
#[tauri::command]
pub fn withdraw_bridge_plan_revision(
    room_id: String,
    revision_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<BridgePlanRecords, String> {
    let context = crate::room_control::room_control_session_context(&state, &room_id)
        .map_err(|error| error.message())?;
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let record = store
        .get_revision(&revision_id)
        .map_err(|error| error.message())?;
    if record.revision.bridge_id != room_id
        || record.revision.requesting_device_ref != context.local_session_ref
        || record.revision.selected_device_ref != context.peer_session_ref
    {
        return Err("Bridge Plan revision is not owned by this current requester session.".into());
    }
    store
        .withdraw_unapproved_revision(&revision_id)
        .map_err(|error| error.message())?;
    store.list_bridge(&room_id).map_err(|error| error.message())
}

/// Creates a requester-originated Transfer revision. The path received from
/// the file picker is captured into process-local Rust state and is never
/// stored in, serialized with, or returned from the immutable Plan.
#[tauri::command]
pub fn create_direct_file_transfer_bridge_plan(
    request: DirectFileTransferBridgePlanRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<BridgePlanRecords, String> {
    let context = crate::room_control::room_control_session_context(&state, &request.room_id)
        .map_err(|error| error.message())?;
    let source = file_candidates::capture_bridge_plan_requester_file(request.source_path.into())
        .map_err(|error| error.message())?;
    let revision = bridge_plan::build_direct_file_transfer_revision(
        request.room_id,
        context.local_session_ref,
        context.peer_session_ref,
        request.original_user_goal,
    )
    .map_err(|error| error.message())?;
    let revision_id = revision.revision_id.clone();
    let records = create_bridge_plan(revision, state.clone())?;
    state
        .bridge_plan_requester_sources
        .lock()
        .insert(revision_id, source);
    Ok(records)
}

/// Creates an explicit fallback revision only after a live Transform attempt
/// reports that the selected device cannot execute it. The original immutable
/// revision and its attempt history remain intact; this revision is unapproved
/// and must receive complete-plan review again.
#[tauri::command]
pub fn propose_bridge_plan_transform_fallback(
    revision_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<BridgePlanRecords, String> {
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let base = store
        .get_revision(&revision_id)
        .map_err(|error| error.message())?;
    let transform_index = base
        .revision
        .steps
        .iter()
        .position(|step| matches!(step, bridge_plan::BridgePlanStep::Transform { .. }))
        .ok_or_else(|| "This revision has no Transform step to replace.".to_string())?;
    let mut alternative = base.revision.clone();
    let transform = alternative.steps.remove(transform_index);
    let bridge_plan::BridgePlanStep::Transform { input_slots, .. } = transform else {
        unreachable!()
    };
    let selected_input = input_slots
        .into_iter()
        .next()
        .ok_or_else(|| "This Transform has no approved input.".to_string())?;
    for step in &mut alternative.steps {
        if let bridge_plan::BridgePlanStep::Transfer {
            depends_on,
            input_slots,
            source,
            ..
        } = step
        {
            if depends_on
                .iter()
                .any(|dependency| dependency == "transform")
            {
                *depends_on = vec!["search".into()];
                *input_slots = vec![selected_input.clone()];
                *source = bridge_plan::ObjectSelectionRule::FromSlot {
                    slot_id: selected_input.slot_id.clone(),
                };
            }
        }
    }
    if !alternative
        .steps
        .iter()
        .any(|step| matches!(step, bridge_plan::BridgePlanStep::Transfer { .. }))
    {
        if let Some(bridge_plan::BridgePlanStep::Search { selection, .. }) = alternative
            .steps
            .iter_mut()
            .find(|step| matches!(step, bridge_plan::BridgePlanStep::Search { .. }))
        {
            *selection = None;
            alternative.search_selection_mode = bridge_plan::SearchSelectionMode::Staged;
        }
    }
    alternative
        .presentation
        .step_explanations
        .retain(|entry| entry.step_id != "transform");
    alternative.presentation.title = bridge_plan::GeneratedUserVisibleText::from_semantic(
        "Alternative file plan without local processing",
    );
    alternative.presentation.natural_language_plan = bridge_plan::GeneratedUserVisibleText::from_semantic("The selected device cannot perform the requested processing for the selected file. This alternative keeps the original file flow and removes that processing step.");
    alternative.expected_outcome = bridge_plan::GeneratedUserVisibleText::from_semantic(
        "The original file is handled without the unavailable processing step.",
    );
    alternative.revision_id = format!("revision-{}", uuid::Uuid::new_v4());
    alternative.revision_number = base.revision.revision_number + 1;
    alternative.revision_hash.clear();
    alternative.alternative = Some(bridge_plan::AlternativeProposal {
        based_on_revision_id: base.revision.revision_id.clone(),
        change_explanation: bridge_plan::GeneratedUserVisibleText::from_semantic("The selected device cannot execute the approved Transform for this file. The proposed revision removes only that Transform; all remaining reviewed devices, locations, and transfers stay unchanged."),
    });
    let alternative = store
        .append_alternative_revision(&base.revision.revision_id, alternative, storage::now_ts())
        .map_err(|error| error.message())?;
    store
        .transition_revision(
            &alternative.revision_id,
            bridge_plan::RevisionState::Available,
        )
        .map_err(|error| error.message())?;
    store.append_activity(&BridgePlanActivity {
        activity_id: format!("transform-alternative-proposed-{}", uuid::Uuid::new_v4()),
        bridge_id: alternative.bridge_id.clone(), plan_id: alternative.plan_id.clone(), revision_id: alternative.revision_id.clone(), attempt_id: None, step_id: None,
        kind: ActivityKind::RevisionProposed, occurred_at: storage::now_ts(),
        summary: "A new unapproved alternative was proposed because the selected device cannot execute the requested Transform.".into(),
    }).map_err(|error| error.message())?;
    store
        .list_bridge(&alternative.bridge_id)
        .map_err(|error| error.message())
}

#[tauri::command]
pub fn list_bridge_plan_workspace(
    room_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<BridgePlanRecords, String> {
    bridge_plan::BridgePlanStore::new(&state.paths)
        .list_bridge(&room_id)
        .map_err(|error| error.message())
}

/// Records the one requester approval for an exact immutable revision. A
/// current Bridge session is the receiver's bounded, ephemeral consent
/// boundary; this command never creates an attempt or execution authority.
#[tauri::command]
pub fn approve_bridge_plan(
    revision_id: String,
    approval_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<BridgePlanRecords, String> {
    let now = storage::now_ts();
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let record = store
        .get_revision(&revision_id)
        .map_err(|error| error.message())?;
    if record.state != RevisionState::Available {
        return Err("This plan revision is not available for approval.".into());
    }
    let revision = record.revision;
    let approval = BridgePlanApproval {
        approval_id,
        plan_id: revision.plan_id.clone(),
        revision_id: revision.revision_id.clone(),
        revision_hash: revision.revision_hash.clone(),
        bridge_id: revision.bridge_id.clone(),
        requester_device_ref: revision.requesting_device_ref.clone(),
        selected_device_ref: revision.selected_device_ref.clone(),
        receiver_required: false,
        expires_at: now + BRIDGE_PLAN_APPROVAL_TTL_SECONDS,
    };
    store
        .create_approval(&approval, now)
        .map_err(|error| error.message())?;
    store
        .append_activity(&BridgePlanActivity {
            activity_id: format!("plan-approved-{}", uuid::Uuid::new_v4()),
            bridge_id: revision.bridge_id.clone(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            attempt_id: None,
            step_id: None,
            kind: ActivityKind::ApprovalCreated,
            occurred_at: now,
            summary: "Requester approved the complete plan for the current Bridge session.".into(),
        })
        .map_err(|error| error.message())?;
    store
        .list_bridge(&revision.bridge_id)
        .map_err(|error| error.message())
}

/// Sends the immutable approved revision to the current selected peer so its
/// Host can validate and bind the bounded plan before execution. This is data
/// for current-session validation, not a receiver approval.
#[tauri::command]
pub async fn bind_bridge_plan_to_session(
    approval_id: String,
    bridge_route: Option<Value>,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomControlDeliveryReceipt, String> {
    let state = state.inner().clone();
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let approval = store
        .get_approval(&approval_id)
        .map_err(|error| error.message())?;
    if approval.state != bridge_plan::ApprovalState::Valid {
        return Err("This plan is not currently approved.".into());
    }
    let revision = store
        .get_revision(&approval.approval.revision_id)
        .map_err(|error| error.message())?;
    let payload = bridge_plan::review_request_payload(&approval.approval, &revision.revision)
        .map_err(|error| error.message())?;
    let context =
        crate::room_control::room_control_session_context(&state, &approval.approval.bridge_id)
            .map_err(|error| error.message())?;
    if context.local_session_ref != approval.approval.requester_device_ref
        || context.peer_session_ref != approval.approval.selected_device_ref
    {
        return Err(
            "The selected device session changed before this plan could be reviewed.".into(),
        );
    }
    let event = bridge_plan_control_event("bridge_plan.review_request", payload, &context)
        .map_err(|error| error.message())?;
    log_bridge_plan_control(&event, "review_request_dispatch");
    let receipt = match crate::room_control::send_room_control_event(
        state.clone(),
        &context.room_id,
        event.clone(),
        bridge_route,
    )
    .await
    {
        Ok(receipt) => receipt,
        Err(error) => {
            log_bridge_plan_control(&event, "review_request_delivery_failed");
            return Err(error.message());
        }
    };
    log_bridge_plan_control(&event, "review_request_delivered");
    bridge_plan::record_outbound_protocol_event(
        &state.paths,
        "bridge_plan.review_request",
        &event,
        storage::now_ts(),
    )
    .map_err(|error| error.message())?;
    log_bridge_plan_control(&event, "review_request_correlated");
    Ok(receipt)
}

/// Starts the single attempt bound to a consumed approval, then tells the
/// selected receiver to derive its own local authority. A retry can resend the
/// exact attempt-start event only while its authority remains live on A.
#[tauri::command]
pub async fn start_bridge_plan_attempt(
    approval_id: String,
    attempt_id: String,
    bridge_route: Option<Value>,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomControlDeliveryReceipt, String> {
    let state = state.inner().clone();
    let now = storage::now_ts();
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let attempt = store
        .create_attempt_from_approval(&attempt_id, &approval_id, now)
        .map_err(|error| error.message())?;
    store
        .transition_attempt(&attempt.attempt_id, bridge_plan::AttemptState::Running, now)
        .map_err(|error| error.message())?;
    let search_step = attempt
        .graph_projection
        .nodes
        .iter()
        .find(|node| matches!(node.operation, bridge_plan::StepOperation::Search))
        .map(|node| node.step_id.clone());
    let is_requester_direct_transfer = search_step.is_none();
    let (step_id, summary, event_kind, payload) = if let Some(search_step) = search_step {
        store
            .transition_step(
                &attempt.attempt_id,
                &search_step,
                bridge_plan::StepExecutionState::Authorized,
                now,
            )
            .map_err(|error| error.message())?;
        (
            search_step,
            "Approved plan started on the selected device.",
            "bridge_plan.attempt_start",
            bridge_plan::attempt_start_payload(&state.paths, &attempt, now)
                .map_err(|error| error.message())?,
        )
    } else {
        let transfer = attempt
            .graph_projection
            .nodes
            .iter()
            .find(|node| matches!(node.operation, bridge_plan::StepOperation::Transfer))
            .ok_or_else(|| "This plan has no supported first step.".to_string())?;
        store
            .transition_step(
                &attempt.attempt_id,
                &transfer.step_id,
                bridge_plan::StepExecutionState::Authorized,
                now,
            )
            .map_err(|error| error.message())?;
        (
            transfer.step_id.clone(),
            "Approved direct Transfer started on the requesting device.",
            "bridge_plan.transfer_start",
            bridge_plan::transfer_start_payload(
                &state.paths,
                &attempt.bridge_id,
                &attempt.attempt_id,
                now,
            )
            .map_err(|error| error.message())?,
        )
    };
    store
        .append_activity(&BridgePlanActivity {
            activity_id: format!("attempt-started-{}", uuid::Uuid::new_v4()),
            bridge_id: attempt.bridge_id.clone(),
            plan_id: attempt.plan_id.clone(),
            revision_id: attempt.revision_id.clone(),
            attempt_id: Some(attempt.attempt_id.clone()),
            step_id: Some(step_id),
            kind: ActivityKind::AttemptStarted,
            occurred_at: now,
            summary: summary.into(),
        })
        .map_err(|error| error.message())?;
    let context = crate::room_control::room_control_session_context(&state, &attempt.bridge_id)
        .map_err(|error| error.message())?;
    if context.local_session_ref
        != payload
            .get("requesterDeviceRef")
            .and_then(Value::as_str)
            .unwrap_or_default()
        || context.peer_session_ref
            != payload
                .get("receiverDeviceRef")
                .and_then(Value::as_str)
                .unwrap_or_default()
    {
        return Err("The selected device session changed before this attempt could start.".into());
    }
    let event = bridge_plan_control_event(event_kind, payload, &context)
        .map_err(|error| error.message())?;
    log_bridge_plan_control(&event, "attempt_start_dispatch");
    let receipt = match crate::room_control::send_room_control_event(
        state.clone(),
        &context.room_id,
        event.clone(),
        bridge_route,
    )
    .await
    {
        Ok(receipt) => receipt,
        Err(error) => {
            log_bridge_plan_control(&event, "attempt_start_delivery_failed");
            return Err(error.message());
        }
    };
    log_bridge_plan_control(&event, "attempt_start_delivered");
    bridge_plan::record_outbound_protocol_event(&state.paths, event_kind, &event, now)
        .map_err(|error| error.message())?;
    log_bridge_plan_control(&event, "attempt_start_correlated");
    if is_requester_direct_transfer {
        execute_direct_bridge_plan_transfer_attempt_inner(
            state.clone(),
            attempt.bridge_id.clone(),
            attempt.attempt_id.clone(),
        )
        .await?;
    }
    Ok(receipt)
}

/// Sends the requester-selected bounded Search candidate back to the selected
/// device. The receiver validates it against its original private result set
/// and keeps the backing object local to the attempt.
#[tauri::command]
pub async fn select_bridge_plan_search_candidate(
    room_id: String,
    attempt_id: String,
    candidate_id: String,
    bridge_route: Option<Value>,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomControlDeliveryReceipt, String> {
    let state = state.inner().clone();
    let context = crate::room_control::room_control_session_context(&state, &room_id)
        .map_err(|error| error.message())?;
    let payload =
        bridge_plan::search_selection_payload(&state.paths, &room_id, &attempt_id, &candidate_id)
            .map_err(|error| error.message())?;
    if context.local_session_ref
        != payload
            .get("requesterDeviceRef")
            .and_then(Value::as_str)
            .unwrap_or_default()
        || context.peer_session_ref
            != payload
                .get("receiverDeviceRef")
                .and_then(Value::as_str)
                .unwrap_or_default()
    {
        return Err(
            "The selected device session changed before this candidate could be selected.".into(),
        );
    }
    let event = bridge_plan_control_event("bridge_plan.search_selection", payload, &context)
        .map_err(|error| error.message())?;
    let receipt = crate::room_control::send_room_control_event(
        state.clone(),
        &room_id,
        event,
        bridge_route.clone(),
    )
    .await
    .map_err(|error| error.message())?;
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let attempt = store
        .list_attempt(&attempt_id)
        .map_err(|error| error.message())?;
    let revision = store
        .get_revision(&attempt.attempt.revision_id)
        .map_err(|error| error.message())?
        .revision;
    if revision.steps.iter().any(|step| {
        matches!(
            step,
            bridge_plan::BridgePlanStep::Transfer {
                destination: bridge_plan::TransferDestination::PipelineHandoff { .. },
                ..
            }
        )
    }) {
        start_bridge_plan_transfer_attempt_inner(state, room_id, attempt_id, bridge_route).await?;
    } else if revision
        .steps
        .iter()
        .any(|step| matches!(step, bridge_plan::BridgePlanStep::Transform { .. }))
    {
        start_bridge_plan_transform_attempt_inner(state, room_id, attempt_id, bridge_route).await?;
    } else if revision
        .steps
        .iter()
        .any(|step| matches!(step, bridge_plan::BridgePlanStep::Transfer { .. }))
    {
        start_bridge_plan_transfer_attempt_inner(state, room_id, attempt_id, bridge_route).await?;
    }
    Ok(receipt)
}

pub(crate) async fn start_bridge_plan_transfer_attempt_inner(
    state: Arc<AppState>,
    room_id: String,
    attempt_id: String,
    bridge_route: Option<Value>,
) -> Result<RoomControlDeliveryReceipt, String> {
    let now = storage::now_ts();
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let attempt = store
        .list_attempt(&attempt_id)
        .map_err(|error| error.message())?;
    if attempt.attempt.bridge_id != room_id || attempt.state != bridge_plan::AttemptState::Running {
        return Err("This approved plan is not ready to transfer a selected file.".into());
    }
    let transfer_node = attempt
        .attempt
        .graph_projection
        .nodes
        .iter()
        .find(|node| matches!(node.operation, bridge_plan::StepOperation::Transfer))
        .ok_or_else(|| "This plan has no supported Transfer step.".to_string())?;
    let transfer = attempt
        .steps
        .iter()
        .find(|step| step.step_id == transfer_node.step_id)
        .ok_or_else(|| "This plan has no durable Transfer state.".to_string())?;
    if transfer.state != bridge_plan::StepExecutionState::Eligible {
        return Err("Choose a completed Search result before starting Transfer.".into());
    }
    let payload = bridge_plan::transfer_start_payload(&state.paths, &room_id, &attempt_id, now)
        .map_err(|error| error.message())?;
    let context = crate::room_control::room_control_session_context(&state, &room_id)
        .map_err(|error| error.message())?;
    if context.local_session_ref
        != payload
            .get("requesterDeviceRef")
            .and_then(Value::as_str)
            .unwrap_or_default()
        || context.peer_session_ref
            != payload
                .get("receiverDeviceRef")
                .and_then(Value::as_str)
                .unwrap_or_default()
    {
        return Err("The selected device session changed before Transfer could start.".into());
    }
    let event = bridge_plan_control_event("bridge_plan.transfer_start", payload, &context)
        .map_err(|error| error.message())?;
    log_bridge_plan_control(&event, "transfer_start_dispatch");
    let receipt = match crate::room_control::send_room_control_event(
        state.clone(),
        &room_id,
        event.clone(),
        bridge_route,
    )
    .await
    {
        Ok(receipt) => receipt,
        Err(error) => {
            log_bridge_plan_control(&event, "transfer_start_delivery_failed");
            return Err(error.message());
        }
    };
    log_bridge_plan_control(&event, "transfer_start_delivered");
    bridge_plan::record_outbound_protocol_event(
        &state.paths,
        "bridge_plan.transfer_start",
        &event,
        now,
    )
    .map_err(|error| error.message())?;
    log_bridge_plan_control(&event, "transfer_start_correlated");
    let refreshed = store
        .list_attempt(&attempt_id)
        .map_err(|error| error.message())?;
    if refreshed
        .steps
        .iter()
        .find(|step| step.step_id == transfer.step_id)
        .is_some_and(|step| step.state == bridge_plan::StepExecutionState::Eligible)
    {
        store
            .transition_step(
                &attempt_id,
                &transfer.step_id,
                bridge_plan::StepExecutionState::Authorized,
                now,
            )
            .map_err(|error| error.message())?;
    }
    Ok(receipt)
}

/// Executes the requester-owned half of an approved direct Transfer. The
/// source was captured when its immutable revision was created and is checked
/// again here; no renderer path or receiver authority is accepted.
/// Host-owned continuation for requester-local direct Transfers. This is
/// called automatically by the single plan start, never as a renderer-driven
/// second step.
pub(crate) async fn execute_direct_bridge_plan_transfer_attempt_inner(
    state: Arc<AppState>,
    room_id: String,
    attempt_id: String,
) -> Result<bool, String> {
    let now = storage::now_ts();
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let attempt = store
        .list_attempt(&attempt_id)
        .map_err(|error| error.message())?;
    if attempt.attempt.bridge_id != room_id || attempt.state != bridge_plan::AttemptState::Running {
        return Err("This direct Transfer plan is not running.".into());
    }
    let revision = store
        .get_revision(&attempt.attempt.revision_id)
        .map_err(|error| error.message())?
        .revision;
    let transfer = revision
        .steps
        .iter()
        .find(|step| matches!(step, bridge_plan::BridgePlanStep::Transfer { .. }))
        .ok_or_else(|| "This plan has no Transfer step.".to_string())?;
    let bridge_plan::BridgePlanStep::Transfer {
        step_id,
        source,
        destination,
        ..
    } = transfer
    else {
        unreachable!()
    };
    if !matches!(
        source,
        bridge_plan::ObjectSelectionRule::FutureUserSelection { .. }
    ) || !matches!(destination, bridge_plan::TransferDestination::SelectedDevice { device_ref } if device_ref == &revision.selected_device_ref)
    {
        return Err("This plan is not a supported requester Transfer.".into());
    }
    let context = crate::room_control::room_control_session_context(&state, &room_id)
        .map_err(|error| error.message())?;
    if context.local_session_ref != revision.requesting_device_ref
        || context.peer_session_ref != revision.selected_device_ref
    {
        return Err("The selected device session changed before Transfer could run.".into());
    }
    let private_file = state
        .bridge_plan_requester_sources
        .lock()
        .get(&revision.revision_id)
        .cloned()
        .ok_or_else(|| {
            "The selected local file is unavailable after restart or cancellation.".to_string()
        })?;
    let metadata = std::fs::symlink_metadata(&private_file.path)
        .map_err(|_| "The selected local file is unavailable.".to_string())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != private_file.size_bytes
    {
        return Err("The selected local file changed before Transfer started.".into());
    }
    store
        .transition_step(
            &attempt_id,
            step_id,
            bridge_plan::StepExecutionState::Running,
            now,
        )
        .map_err(|error| error.message())?;
    let peers = storage::list_bridge_peer_endpoints(&state.paths, &room_id)
        .map_err(|error| error.message())?;
    let endpoint = resolve_routeable_bridge_peer(&peers, &context.peer_route_ref, "Transfer")
        .map_err(|error| error.message())?;
    let master_key = {
        let config = state.config.read();
        config::master_key(&config).map_err(|error| error.message())?
    };
    let item = storage::create_outgoing_file_item_with_metadata(
        &state.paths,
        &master_key,
        &room_id,
        &private_file.path,
        Some(private_file.display_name.clone()),
        Some(private_file.mime_type.clone()),
    )
    .map_err(|error| error.message())?;
    let sent = transfer::send_room_file_to_bridge_peer_endpoint(
        state.clone(),
        &room_id,
        &item.id,
        &private_file.path,
        Some(format!("bridge-plan-transfer-{attempt_id}")),
        None,
        endpoint,
    )
    .await;
    match sent {
        Ok(()) => {
            let completed_at = storage::now_ts();
            store
                .transition_step(
                    &attempt_id,
                    step_id,
                    bridge_plan::StepExecutionState::Completed,
                    completed_at,
                )
                .map_err(|error| error.message())?;
            store
                .transition_attempt(
                    &attempt_id,
                    bridge_plan::AttemptState::Completed,
                    completed_at,
                )
                .map_err(|error| error.message())?;
            store
                .append_result(&BridgePlanResultSummary {
                    result_id: format!("direct-transfer-result-{}", uuid::Uuid::new_v4()),
                    bridge_id: room_id.clone(),
                    plan_id: attempt.attempt.plan_id.clone(),
                    revision_id: revision.revision_id.clone(),
                    attempt_id: attempt_id.clone(),
                    step_id: step_id.clone(),
                    status: bridge_plan::GeneratedUserVisibleText::from_semantic("completed"),
                    summary: "Transfer completed to the selected device.".into(),
                    produced_object_description: Some(
                        bridge_plan::GeneratedUserVisibleText::from_semantic(
                            "One reviewed file was transferred.",
                        ),
                    ),
                    created_at: completed_at,
                })
                .map_err(|error| error.message())?;
            store
                .append_activity(&BridgePlanActivity {
                    activity_id: format!("direct-transfer-completed-{}", uuid::Uuid::new_v4()),
                    bridge_id: room_id.clone(),
                    plan_id: attempt.attempt.plan_id.clone(),
                    revision_id: revision.revision_id.clone(),
                    attempt_id: Some(attempt_id.clone()),
                    step_id: Some(step_id.clone()),
                    kind: ActivityKind::AttemptCompleted,
                    occurred_at: completed_at,
                    summary: "Direct Transfer completed to the selected device.".into(),
                })
                .map_err(|error| error.message())?;
            state
                .bridge_plan_requester_sources
                .lock()
                .remove(&revision.revision_id);
            Ok(true)
        }
        Err(error) => {
            let failed_at = storage::now_ts();
            let _ = store.transition_step(
                &attempt_id,
                step_id,
                bridge_plan::StepExecutionState::Failed,
                failed_at,
            );
            let _ =
                store.transition_attempt(&attempt_id, bridge_plan::AttemptState::Failed, failed_at);
            let _ = store.append_activity(&BridgePlanActivity {
                activity_id: format!("direct-transfer-failed-{}", uuid::Uuid::new_v4()),
                bridge_id: room_id.clone(),
                plan_id: attempt.attempt.plan_id.clone(),
                revision_id: revision.revision_id.clone(),
                attempt_id: Some(attempt_id.clone()),
                step_id: Some(step_id.clone()),
                kind: ActivityKind::AttemptFailed,
                occurred_at: failed_at,
                summary: "Direct Transfer could not complete.".into(),
            });
            Err(error.message())
        }
    }
}

pub(crate) async fn start_bridge_plan_transform_attempt_inner(
    state: Arc<AppState>,
    room_id: String,
    attempt_id: String,
    bridge_route: Option<Value>,
) -> Result<RoomControlDeliveryReceipt, String> {
    let now = storage::now_ts();
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let attempt = store
        .list_attempt(&attempt_id)
        .map_err(|error| error.message())?;
    if attempt.attempt.bridge_id != room_id || attempt.state != bridge_plan::AttemptState::Running {
        return Err("This approved plan is not ready to process a selected file.".into());
    }
    let transform_node = attempt
        .attempt
        .graph_projection
        .nodes
        .iter()
        .find(|node| matches!(node.operation, bridge_plan::StepOperation::Transform))
        .ok_or_else(|| "This plan has no supported Transform step.".to_string())?;
    let transform = attempt
        .steps
        .iter()
        .find(|step| step.step_id == transform_node.step_id)
        .ok_or_else(|| "This plan has no durable Transform state.".to_string())?;
    if transform.state != bridge_plan::StepExecutionState::Eligible {
        return Err("Choose a completed Search result before starting Transform.".into());
    }
    let payload = bridge_plan::transform_start_payload(&state.paths, &room_id, &attempt_id, now)
        .map_err(|error| error.message())?;
    let context = crate::room_control::room_control_session_context(&state, &room_id)
        .map_err(|error| error.message())?;
    if transform_node.execution_device_ref != context.peer_session_ref {
        return Err("This Transform requires a private pipeline handoff before it can start; the current executor cannot receive it through the final-delivery path.".into());
    }
    if context.local_session_ref
        != payload
            .get("requesterDeviceRef")
            .and_then(Value::as_str)
            .unwrap_or_default()
        || context.peer_session_ref
            != payload
                .get("receiverDeviceRef")
                .and_then(Value::as_str)
                .unwrap_or_default()
    {
        return Err("The selected device session changed before Transform could start.".into());
    }
    let event = bridge_plan_control_event("bridge_plan.transform_start", payload, &context)
        .map_err(|error| error.message())?;
    log_bridge_plan_control(&event, "transform_start_dispatch");
    let receipt = match crate::room_control::send_room_control_event(
        state.clone(),
        &room_id,
        event.clone(),
        bridge_route,
    )
    .await
    {
        Ok(receipt) => receipt,
        Err(error) => {
            log_bridge_plan_control(&event, "transform_start_delivery_failed");
            return Err(error.message());
        }
    };
    log_bridge_plan_control(&event, "transform_start_delivered");
    bridge_plan::record_outbound_protocol_event(
        &state.paths,
        "bridge_plan.transform_start",
        &event,
        now,
    )
    .map_err(|error| error.message())?;
    log_bridge_plan_control(&event, "transform_start_correlated");
    let refreshed = store
        .list_attempt(&attempt_id)
        .map_err(|error| error.message())?;
    if refreshed
        .steps
        .iter()
        .find(|step| step.step_id == transform.step_id)
        .is_some_and(|step| step.state == bridge_plan::StepExecutionState::Eligible)
    {
        store
            .transition_step(
                &attempt_id,
                &transform.step_id,
                bridge_plan::StepExecutionState::Authorized,
                now,
            )
            .map_err(|error| error.message())?;
    }
    Ok(receipt)
}

/// Internal Host-owned continuation used after authenticated attempt-start.
/// Room Control calls this boundary automatically.
pub(crate) async fn execute_bridge_plan_search_attempt_inner(
    state: Arc<AppState>,
    room_id: String,
    attempt_id: String,
    bridge_route: Option<Value>,
) -> Result<(), String> {
    let now = storage::now_ts();
    log_bridge_plan_search_attempt(
        "search_attempt_started",
        &room_id,
        &attempt_id,
        "started",
        None,
    );
    let grant = match bridge_plan::consume_search_execution_grant(
        &state.paths,
        &state.bridge_plan_protocol_authority.lock(),
        &room_id,
        &attempt_id,
        now,
    ) {
        Ok(grant) => grant,
        Err(error) => {
            log_bridge_plan_search_attempt(
                "search_execution_failed",
                &room_id,
                &attempt_id,
                "grant_unavailable",
                None,
            );
            return Err(error.message());
        }
    };
    log_bridge_plan_search_attempt(
        "search_grant_consumed",
        &room_id,
        &attempt_id,
        "consumed",
        None,
    );
    let context =
        crate::room_control::room_control_session_context(&state, &room_id).map_err(|error| {
            log_bridge_plan_search_attempt(
                "search_execution_failed",
                &room_id,
                &attempt_id,
                "room_control_context_unavailable",
                None,
            );
            error.message()
        })?;
    if context.local_session_ref != grant.receiver_device_ref
        || context.peer_session_ref != grant.requester_device_ref
    {
        log_bridge_plan_search_attempt(
            "search_execution_failed",
            &room_id,
            &attempt_id,
            "requester_session_changed",
            None,
        );
        return Err("The requester session changed before Search could run.".into());
    }
    let send = |kind: &str, payload: Value| {
        let event = bridge_plan_control_event(kind, payload, &context)?;
        Ok::<_, AppError>(event)
    };
    let ack = send(
        "bridge_plan.attempt_ack",
        bridge_plan::attempt_update_payload(&grant, "bridge_plan.attempt_ack", None, None)
            .map_err(|error| error.message())?,
    )
    .map_err(|error| error.message())?;
    crate::room_control::send_room_control_event(
        state.clone(),
        &room_id,
        ack,
        bridge_route.clone(),
    )
    .await
    .map_err(|error| {
        log_bridge_plan_search_attempt(
            "search_execution_failed",
            &room_id,
            &attempt_id,
            "attempt_ack_delivery_failed",
            None,
        );
        error.message()
    })?;
    let progress = send(
        "bridge_plan.step_progress",
        bridge_plan::attempt_update_payload(&grant, "bridge_plan.step_progress", None, None)
            .map_err(|error| error.message())?,
    )
    .map_err(|error| error.message())?;
    crate::room_control::send_room_control_event(
        state.clone(),
        &room_id,
        progress,
        bridge_route.clone(),
    )
    .await
    .map_err(|error| {
        log_bridge_plan_search_attempt(
            "search_execution_failed",
            &room_id,
            &attempt_id,
            "progress_delivery_failed",
            None,
        );
        error.message()
    })?;
    let created = OffsetDateTime::now_utc();
    let request = BridgePlanSearchRequest {
        request_id: format!("bridge-plan-request-{}", grant.attempt_id),
        room_ref: room_id.clone(),
        requester_device_ref: grant.requester_device_ref.clone(),
        receiver_device_ref: grant.receiver_device_ref.clone(),
        filename_hint: grant.filename_hint.clone(),
        extensions: grant.extensions.clone(),
        safe_scope_labels: grant.safe_scope_labels.clone(),
        expires_at: (created + Duration::seconds(BRIDGE_PLAN_CONTROL_LIFETIME_SECONDS))
            .format(&Rfc3339)
            .map_err(|error| error.to_string())?,
    };
    let result = {
        let mut candidates = state.bridge_plan_candidate_store.lock();
        file_candidates::execute_bridge_plan_search_and_store(
            request,
            &state.paths,
            &mut candidates,
        )
        .map_err(|error| {
            log_bridge_plan_search_attempt(
                "search_execution_failed",
                &room_id,
                &attempt_id,
                "search_request_invalid",
                None,
            );
            error.message()
        })?
    };
    log_bridge_plan_search_attempt(
        if result.status == "completed" {
            "search_execution_completed"
        } else {
            "search_execution_failed"
        },
        &room_id,
        &attempt_id,
        result.error_code.as_deref().unwrap_or("completed"),
        Some(result.candidates.len()),
    );
    let (kind, payload) = if result.status == "completed" {
        (
            "bridge_plan.step_result",
            bridge_plan::attempt_search_result_payload(&grant, &result)
                .map_err(|error| error.message())?,
        )
    } else {
        (
            "bridge_plan.step_failed",
            bridge_plan::attempt_update_payload(
                &grant,
                "bridge_plan.step_failed",
                None,
                result.error_code.as_deref().or(Some("search_failed")),
            )
            .map_err(|error| error.message())?,
        )
    };
    let terminal = send(kind, payload).map_err(|error| error.message())?;
    crate::room_control::send_room_control_event(state, &room_id, terminal, bridge_route)
        .await
        .map_err(|error| {
            log_bridge_plan_search_attempt(
                "search_execution_failed",
                &room_id,
                &attempt_id,
                "result_delivery_failed",
                Some(result.candidates.len()),
            );
            error.message()
        })?;
    log_bridge_plan_search_attempt(
        "search_result_delivered",
        &room_id,
        &attempt_id,
        result.error_code.as_deref().unwrap_or("completed"),
        Some(result.candidates.len()),
    );
    Ok(())
}

/// Executes one receiver-local Transform after consuming the grant derived
/// from the authenticated complete-plan binding. The generated file is kept
/// in receiver-private ephemeral storage; no path or ObjectRef crosses Room
/// Control, and unsupported input or intent fails with a product-safe result.
pub(crate) async fn execute_bridge_plan_transform_attempt_inner(
    state: Arc<AppState>,
    room_id: String,
    attempt_id: String,
    bridge_route: Option<Value>,
) -> Result<bool, String> {
    let grant = bridge_plan::consume_transform_execution_grant(
        &state.paths,
        &state.bridge_plan_protocol_authority.lock(),
        &room_id,
        &attempt_id,
        storage::now_ts(),
    )
    .map_err(|error| error.message())?;
    let context = crate::room_control::room_control_session_context(&state, &room_id)
        .map_err(|error| error.message())?;
    if context.local_session_ref != grant.receiver_device_ref
        || context.peer_session_ref != grant.requester_device_ref
    {
        return Err("The requester session changed before Transform could run.".into());
    }
    let send = |kind: &str, payload: Value| -> Result<_, String> {
        bridge_plan_control_event(kind, payload, &context).map_err(|error| error.message())
    };
    for (kind, payload) in [
        (
            "bridge_plan.attempt_ack",
            bridge_plan::transform_update_payload(&grant, "bridge_plan.attempt_ack", None, None),
        ),
        (
            "bridge_plan.step_progress",
            bridge_plan::transform_update_payload(&grant, "bridge_plan.step_progress", None, None),
        ),
    ] {
        let event = send(kind, payload.map_err(|error| error.message())?)?;
        crate::room_control::send_room_control_event(
            state.clone(),
            &room_id,
            event,
            bridge_route.clone(),
        )
        .await
        .map_err(|error| error.message())?;
    }
    let transformed = {
        let mut candidates = state.bridge_plan_candidate_store.lock();
        file_candidates::transform_bridge_plan_selected_file(
            &mut candidates,
            &state.paths,
            &room_id,
            &grant.requester_device_ref,
            &grant.receiver_device_ref,
            &grant.attempt_id,
            &grant.candidate_id,
            &grant.intent,
        )
    };
    let (kind, payload, succeeded, failure_message) = match transformed {
        Ok(output) => {
            state
                .bridge_plan_protocol_authority
                .lock()
                .retain_transform_output(&room_id, &attempt_id, output)
                .map_err(|error| error.message())?;
            (
                "bridge_plan.step_result",
                bridge_plan::transform_update_payload(
                    &grant,
                    "bridge_plan.step_result",
                    Some("The selected file was processed and remains on the selected device."),
                    None,
                ),
                true,
                None,
            )
        }
        Err(error) => {
            let unsupported_input = error
                .message()
                .contains("cannot process this file with the requested Transform");
            (
                "bridge_plan.step_failed",
                bridge_plan::transform_update_payload(
                    &grant,
                    "bridge_plan.step_failed",
                    None,
                    Some(if unsupported_input {
                        "transform_input_unsupported"
                    } else {
                        "transform_unavailable_or_failed"
                    }),
                ),
                false,
                Some(if unsupported_input {
                    "The selected file type is not supported by the approved Transform."
                } else {
                    "The approved Transform could not be completed on the selected device."
                }),
            )
        }
    };
    let terminal = send(kind, payload.map_err(|error| error.message())?)?;
    crate::room_control::send_room_control_event(state, &room_id, terminal, bridge_route)
        .await
        .map_err(|error| error.message())?;
    if !succeeded {
        return Err(failure_message
            .unwrap_or("The approved Transform could not be completed on the selected device.")
            .into());
    }
    Ok(true)
}

/// Continues a cross-device object flow after encrypted PipelinePrivate
/// finalization. The input is obtained only from the receiver-private store;
/// no control event or renderer value supplies a path or object reference.
pub(crate) async fn execute_pipeline_transform_after_handoff(
    state: Arc<AppState>,
    metadata: crate::models::PipelineHandoffMetadata,
) -> Result<(), String> {
    let now = storage::now_ts();
    log_pipeline_handoff(
        "pipeline_dependency_activated",
        &metadata.bridge_id,
        &metadata.attempt_id,
        &metadata.step_id,
        "started",
    );
    let context = crate::room_control::room_control_session_context(&state, &metadata.bridge_id)
        .map_err(|error| error.message())?;
    if context.local_session_ref != metadata.destination_device_ref
        || context.peer_session_ref != metadata.source_device_ref
        || !crate::peer_capabilities::local_projection("local".into(), now).capabilities[0]
            .available
    {
        return Err(
            "Pipeline Transform is no longer available in this current Bridge session.".into(),
        );
    }
    let store = bridge_plan::BridgePlanStore::new(&state.paths);
    let revision = store
        .get_revision(&metadata.revision_id)
        .map_err(|error| error.message())?
        .revision;
    let transform = revision
        .steps
        .iter()
        .find_map(|step| match step {
            bridge_plan::BridgePlanStep::Transform {
                step_id,
                depends_on,
                intent,
                execution_device_ref,
                ..
            } if depends_on
                .iter()
                .any(|dependency| dependency == &metadata.step_id)
                && execution_device_ref == &metadata.destination_device_ref =>
            {
                Some((step_id.clone(), intent.as_str().to_owned()))
            }
            _ => None,
        })
        .ok_or_else(|| "Pipeline handoff has no eligible local Transform.".to_string())?;
    let attempt = store
        .list_attempt(&metadata.attempt_id)
        .map_err(|error| error.message())?;
    let state_for = |step_id: &str| {
        attempt
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .map(|step| step.state.clone())
    };
    if state_for(&metadata.step_id) == Some(bridge_plan::StepExecutionState::Authorized) {
        store
            .transition_step(
                &metadata.attempt_id,
                &metadata.step_id,
                bridge_plan::StepExecutionState::Running,
                now,
            )
            .map_err(|error| error.message())?;
        store
            .transition_step(
                &metadata.attempt_id,
                &metadata.step_id,
                bridge_plan::StepExecutionState::Completed,
                now,
            )
            .map_err(|error| error.message())?;
    }
    let refreshed = store
        .list_attempt(&metadata.attempt_id)
        .map_err(|error| error.message())?;
    if refreshed
        .steps
        .iter()
        .find(|step| step.step_id == transform.0)
        .is_some_and(|step| step.state == bridge_plan::StepExecutionState::Pending)
    {
        store
            .transition_step(
                &metadata.attempt_id,
                &transform.0,
                bridge_plan::StepExecutionState::Eligible,
                now,
            )
            .map_err(|error| error.message())?;
    }
    store
        .transition_step(
            &metadata.attempt_id,
            &transform.0,
            bridge_plan::StepExecutionState::Authorized,
            now,
        )
        .map_err(|error| error.message())?;
    store
        .transition_step(
            &metadata.attempt_id,
            &transform.0,
            bridge_plan::StepExecutionState::Running,
            now,
        )
        .map_err(|error| error.message())?;
    // Consume only after every session, capability, revision, and state
    // transition precondition has passed. From here the private root is
    // deterministically cleaned immediately after Transform returns.
    let input = state
        .bridge_plan_protocol_authority
        .lock()
        .consume_pipeline_input(&metadata)
        .map_err(|error| error.message())?;
    let transformed = {
        let mut candidates = state.bridge_plan_candidate_store.lock();
        file_candidates::transform_bridge_plan_private_file(
            &mut candidates,
            &state.paths,
            &metadata.bridge_id,
            &metadata.destination_device_ref,
            &input,
            &transform.1,
        )
    };
    file_candidates::cleanup_bridge_plan_private_pipeline_file(&input);
    let output = transformed.map_err(|error| error.message())?;
    state
        .bridge_plan_protocol_authority
        .lock()
        .retain_transform_output(&metadata.bridge_id, &metadata.attempt_id, output)
        .map_err(|error| error.message())?;
    log_pipeline_handoff(
        "pipeline_step_result_delivered",
        &metadata.bridge_id,
        &metadata.attempt_id,
        &metadata.step_id,
        "transform_completed",
    );
    store
        .transition_step(
            &metadata.attempt_id,
            &transform.0,
            bridge_plan::StepExecutionState::Completed,
            storage::now_ts(),
        )
        .map_err(|error| error.message())?;
    // A final delivery to the same requesting execution device is not another
    // network handoff. Materialize only the transformed private output in the
    // normal Inbox; the pipeline source was already removed above.
    if let Some(final_transfer_id) = revision.steps.iter().find_map(|step| match step {
        bridge_plan::BridgePlanStep::Transfer { step_id, depends_on, destination, .. }
            if depends_on.iter().any(|dependency| dependency == &transform.0)
                && matches!(destination, bridge_plan::TransferDestination::RequestingDevice { device_ref } if device_ref == &metadata.destination_device_ref) => Some(step_id.clone()),
        _ => None,
    }) {
        let refreshed = store.list_attempt(&metadata.attempt_id).map_err(|error| error.message())?;
        if refreshed.steps.iter().find(|step| step.step_id == final_transfer_id).is_some_and(|step| step.state == bridge_plan::StepExecutionState::Pending) {
            store.transition_step(&metadata.attempt_id, &final_transfer_id, bridge_plan::StepExecutionState::Eligible, storage::now_ts()).map_err(|error| error.message())?;
        }
        store.transition_step(&metadata.attempt_id, &final_transfer_id, bridge_plan::StepExecutionState::Authorized, storage::now_ts()).map_err(|error| error.message())?;
        store.transition_step(&metadata.attempt_id, &final_transfer_id, bridge_plan::StepExecutionState::Running, storage::now_ts()).map_err(|error| error.message())?;
        let private_output = state.bridge_plan_protocol_authority.lock().consume_transform_output(&metadata.bridge_id, &metadata.attempt_id).map_err(|error| error.message())?;
        let destination_dir = { let config = state.config.read(); config::received_item_destination_dir(&state.paths, &config, Some(&private_output.mime_type)) };
        std::fs::create_dir_all(&destination_dir).map_err(|_| "Could not create the final Inbox location.".to_string())?;
        let final_path = storage::next_inbox_path_excluding(&destination_dir, Some(&private_output.display_name), &[]).map_err(|error| error.to_string())?;
        if std::fs::copy(&private_output.path, &final_path).map_err(|_| "Could not deliver the transformed result.".to_string())? != private_output.size_bytes {
            return Err("Final transformed delivery was incomplete.".into());
        }
        // This is the explicitly reviewed final-delivery step.  Register only
        // the transformed output after the copy succeeds; the preceding
        // PipelinePrivate input was never an Inbox item.
        let master_key = {
            let config = state.config.read();
            config::master_key(&config).map_err(|error| error.message())?
        };
        storage::persist_incoming_file_item_metadata(
            &state.paths,
            &master_key,
            &metadata.bridge_id,
            &format!("bridge-plan-final-{}", uuid::Uuid::new_v4()),
            private_output.size_bytes,
            Some(private_output.display_name.clone()),
            Some(private_output.mime_type.clone()),
            storage::now_ts(),
            Some(final_path.display().to_string()),
        )
        .map_err(|error| error.message())?;
        file_candidates::cleanup_bridge_plan_private_pipeline_file(&private_output);
        store.transition_step(&metadata.attempt_id, &final_transfer_id, bridge_plan::StepExecutionState::Completed, storage::now_ts()).map_err(|error| error.message())?;
        store.transition_attempt(&metadata.attempt_id, bridge_plan::AttemptState::Completed, storage::now_ts()).map_err(|error| error.message())?;
    }
    Ok(())
}

/// Executes one receiver-local Transfer. The selected candidate is resolved
/// only in Rust after the authenticated transfer-start grant is consumed; no
/// path or private object reference enters this product path.
pub(crate) async fn execute_bridge_plan_transfer_attempt_inner(
    state: Arc<AppState>,
    room_id: String,
    attempt_id: String,
    bridge_route: Option<Value>,
) -> Result<bool, String> {
    let now = storage::now_ts();
    log_bridge_plan_transfer_attempt("transfer_attempt_started", &room_id, &attempt_id, "started");
    let grant = bridge_plan::consume_transfer_execution_grant(
        &state.paths,
        &state.bridge_plan_protocol_authority.lock(),
        &room_id,
        &attempt_id,
        now,
    )
    .map_err(|error| {
        log_bridge_plan_transfer_attempt(
            "transfer_execution_failed",
            &room_id,
            &attempt_id,
            "grant_unavailable",
        );
        error.message()
    })?;
    log_bridge_plan_transfer_attempt("transfer_grant_consumed", &room_id, &attempt_id, "consumed");
    let context = crate::room_control::room_control_session_context(&state, &room_id)
        .map_err(|error| error.message())?;
    if context.local_session_ref != grant.receiver_device_ref
        || context.peer_session_ref != grant.requester_device_ref
    {
        return Err("The requester session changed before Transfer could run.".into());
    }
    let send = |kind: &str, payload: Value| -> Result<_, String> {
        bridge_plan_control_event(kind, payload, &context).map_err(|error| error.message())
    };
    let ack = send(
        "bridge_plan.attempt_ack",
        bridge_plan::transfer_update_payload(&grant, "bridge_plan.attempt_ack", None, None)
            .map_err(|error| error.message())?,
    )?;
    crate::room_control::send_room_control_event(
        state.clone(),
        &room_id,
        ack,
        bridge_route.clone(),
    )
    .await
    .map_err(|error| {
        log_bridge_plan_transfer_attempt(
            "transfer_execution_failed",
            &room_id,
            &attempt_id,
            "transfer_ack_delivery_failed",
        );
        error.message()
    })?;
    log_bridge_plan_transfer_attempt("transfer_ack_delivered", &room_id, &attempt_id, "accepted");
    let progress = send(
        "bridge_plan.step_progress",
        bridge_plan::transfer_update_payload(&grant, "bridge_plan.step_progress", None, None)
            .map_err(|error| error.message())?,
    )?;
    crate::room_control::send_room_control_event(
        state.clone(),
        &room_id,
        progress,
        bridge_route.clone(),
    )
    .await
    .map_err(|error| {
        log_bridge_plan_transfer_attempt(
            "transfer_execution_failed",
            &room_id,
            &attempt_id,
            "progress_delivery_failed",
        );
        error.message()
    })?;

    let transfer_result: Result<(), AppError> = async {
        let private_file = match grant.generated_file.clone() {
            Some(output) => output,
            None => {
                let mut candidates = state.bridge_plan_candidate_store.lock();
                file_candidates::resolve_bridge_plan_selected_file(
                    &mut candidates,
                    &room_id,
                    &grant.requester_device_ref,
                    &grant.receiver_device_ref,
                    &grant.attempt_id,
                    &grant.candidate_id,
                )
                .map_err(|error| {
                    log_bridge_plan_transfer_attempt(
                        "transfer_execution_failed",
                        &room_id,
                        &attempt_id,
                        bridge_plan_transfer_failure_code(&error),
                    );
                    error
                })?
            }
        };
        log_bridge_plan_transfer_attempt(
            "transfer_candidate_resolved",
            &room_id,
            &attempt_id,
            "resolved",
        );
        match &grant.destination {
            bridge_plan::TransferDestination::RequestingDevice { .. } => {
                let peers = storage::list_bridge_peer_endpoints(&state.paths, &room_id)?;
                let endpoint =
                    resolve_routeable_bridge_peer(&peers, &context.peer_route_ref, "Transfer")
                        .map_err(|error| {
                            log_bridge_plan_transfer_attempt(
                                "transfer_execution_failed",
                                &room_id,
                                &attempt_id,
                                "route_unavailable",
                            );
                            error
                        })?;
                log_bridge_plan_transfer_attempt(
                    "transfer_route_resolved",
                    &room_id,
                    &attempt_id,
                    "resolved",
                );
                log_pipeline_handoff(
                    "pipeline_source_resolved",
                    &room_id,
                    &attempt_id,
                    &grant.step_id,
                    "resolved",
                );
                let master_key = {
                    let config = state.config.read();
                    config::master_key(&config)?
                };
                let item = storage::create_outgoing_file_item_with_metadata(
                    &state.paths,
                    &master_key,
                    &room_id,
                    &private_file.path,
                    Some(private_file.display_name.clone()),
                    Some(private_file.mime_type.clone()),
                )
                .map_err(|error| {
                    log_bridge_plan_transfer_attempt(
                        "transfer_execution_failed",
                        &room_id,
                        &attempt_id,
                        "outgoing_item_failed",
                    );
                    error
                })?;
                log_bridge_plan_transfer_attempt(
                    "transfer_item_created",
                    &room_id,
                    &attempt_id,
                    "created",
                );
                if item.size_bytes != private_file.size_bytes {
                    return Err(AppError::InvalidInput(
                        "The selected file changed before Transfer started.".into(),
                    ));
                }
                transfer::send_room_file_to_bridge_peer_endpoint(
                    state.clone(),
                    &room_id,
                    &item.id,
                    &private_file.path,
                    Some(format!("bridge-plan-transfer-{}", grant.attempt_id)),
                    None,
                    endpoint,
                )
                .await
                .map(|()| {
                    log_bridge_plan_transfer_attempt(
                        "transfer_bytes_completed",
                        &room_id,
                        &attempt_id,
                        "completed",
                    );
                })
            }
            bridge_plan::TransferDestination::PipelineHandoff { device_ref }
                if device_ref == &grant.requester_device_ref =>
            {
                log_pipeline_handoff(
                    "pipeline_handoff_start",
                    &room_id,
                    &attempt_id,
                    &grant.step_id,
                    "started",
                );
                log_bridge_plan_transfer_attempt(
                    "pipeline_handoff_start",
                    &room_id,
                    &attempt_id,
                    "started",
                );
                let peers = storage::list_bridge_peer_endpoints(&state.paths, &room_id)?;
                let endpoint = resolve_routeable_bridge_peer(
                    &peers,
                    &context.peer_route_ref,
                    "Pipeline handoff",
                )?;
                let master_key = {
                    let config = state.config.read();
                    config::master_key(&config)?
                };
                let item = storage::create_outgoing_file_item_with_metadata(
                    &state.paths,
                    &master_key,
                    &room_id,
                    &private_file.path,
                    Some(private_file.display_name.clone()),
                    Some(private_file.mime_type.clone()),
                )?;
                log_pipeline_handoff(
                    "pipeline_source_resolved",
                    &room_id,
                    &attempt_id,
                    &grant.step_id,
                    "resolved",
                );
                if item.size_bytes != private_file.size_bytes {
                    return Err(AppError::InvalidInput(
                        "Pipeline source changed before handoff.".into(),
                    ));
                }
                let metadata = crate::models::PipelineHandoffMetadata {
                    bridge_id: grant.bridge_id.clone(),
                    plan_id: grant.plan_id.clone(),
                    revision_id: grant.revision_id.clone(),
                    revision_hash: grant.revision_hash.clone(),
                    attempt_id: grant.attempt_id.clone(),
                    step_id: grant.step_id.clone(),
                    source_device_ref: grant.receiver_device_ref.clone(),
                    destination_device_ref: grant.requester_device_ref.clone(),
                    media_type: private_file.mime_type.clone(),
                };
                log_bridge_plan_transfer_attempt(
                    "pipeline_transfer_created",
                    &room_id,
                    &attempt_id,
                    "created",
                );
                log_pipeline_handoff(
                    "pipeline_transfer_created",
                    &room_id,
                    &attempt_id,
                    &grant.step_id,
                    "created",
                );
                let send_result = transfer::send_room_file_to_bridge_peer_endpoint_with_landing(
                    state.clone(),
                    &room_id,
                    &item.id,
                    &private_file.path,
                    Some(format!("bridge-plan-pipeline-{}", grant.attempt_id)),
                    None,
                    endpoint,
                    Some(metadata),
                )
                .await;
                // The current encrypted binary sender obtains its per-transfer
                // wrapped key from a normal room-item record.  That record is
                // merely transient transport bookkeeping for PipelinePrivate,
                // never a delivery/history item, so delete it on either
                // success or failure once the sender has stopped using it.
                let _ = storage::delete_room_item(&state.paths, &item.id);
                send_result.map(|()| {
                    log_bridge_plan_transfer_attempt(
                        "pipeline_bytes_completed",
                        &room_id,
                        &attempt_id,
                        "completed",
                    );
                    log_pipeline_handoff(
                        "pipeline_bytes_completed",
                        &room_id,
                        &attempt_id,
                        &grant.step_id,
                        "completed",
                    );
                })
            }
            bridge_plan::TransferDestination::UserSelectedLocation {
                device_ref,
                user_visible_location_scope,
            } if device_ref == &grant.receiver_device_ref
                && user_visible_location_scope.as_str() == "Pastey Shared" =>
            {
                let root = state.paths.app_data_dir.join("shared");
                std::fs::create_dir_all(&root)?;
                let root = std::fs::canonicalize(root)?;
                let name = sanitize_filename::sanitize(&private_file.display_name);
                let destination = root.join(format!("bridge-plan-{}-{name}", uuid::Uuid::new_v4()));
                if std::fs::copy(&private_file.path, &destination)? != private_file.size_bytes {
                    return Err(AppError::InvalidInput(
                        "The approved location copy was incomplete.".into(),
                    ));
                }
                Ok(())
            }
            _ => Err(AppError::InvalidInput(
                "This approved Transfer destination is unavailable on this device.".into(),
            )),
        }
    }
    .await;
    let success_summary = match &grant.destination {
        bridge_plan::TransferDestination::UserSelectedLocation { .. } => {
            "Transfer saved in the approved Pastey Shared location on the selected device."
        }
        _ => "Transfer completed to the requesting device.",
    };
    let (kind, payload) = match transfer_result {
        Ok(()) => (
            "bridge_plan.step_result",
            bridge_plan::transfer_update_payload(
                &grant,
                "bridge_plan.step_result",
                Some(success_summary),
                None,
            ),
        ),
        Err(ref error) => (
            "bridge_plan.step_failed",
            bridge_plan::transfer_update_payload(
                &grant,
                "bridge_plan.step_failed",
                None,
                Some(bridge_plan_transfer_failure_code(&error)),
            ),
        ),
    };
    let terminal = send(kind, payload.map_err(|error| error.message())?)?;
    crate::room_control::send_room_control_event(state, &room_id, terminal, bridge_route)
        .await
        .map_err(|error| {
            log_bridge_plan_transfer_attempt(
                "transfer_execution_failed",
                &room_id,
                &attempt_id,
                "result_delivery_failed",
            );
            error.message()
        })?;
    if transfer_result.is_err() {
        log_bridge_plan_transfer_attempt(
            "transfer_execution_failed",
            &room_id,
            &attempt_id,
            "failed",
        );
        return Err("The approved Transfer could not be completed.".into());
    }
    log_bridge_plan_transfer_attempt(
        "transfer_result_delivered",
        &room_id,
        &attempt_id,
        "completed",
    );
    Ok(true)
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
    run_async(async move { burn_bridge_scope(state.inner().clone(), &room_id, true, true).await })
        .await
}

/// Purges all ephemeral execution authority for one Bridge. Lock acquisition
/// is deliberately stable so no pre-Burn private binding can interleave with
/// the individual stores' purges.
pub(crate) fn purge_bridge_runtime_authority(
    state: &Arc<AppState>,
    room_id: &str,
) -> AppResult<()> {
    state.room_control.lock().purge_room(room_id);

    let mut first_error = None;
    let mut candidates = state.bridge_plan_candidate_store.lock();
    // Direct requester sources are never durable. Clearing the small
    // process-local map on Burn is conservative across all Bridges and
    // prevents any later retry from reusing a pre-Burn file binding.
    state.bridge_plan_requester_sources.lock().clear();
    let bridge_plan_protocol_authority = state.bridge_plan_protocol_authority.lock();
    let mut peer_capabilities = state.peer_capabilities.lock();
    if let Err(error) = candidates.purge_room(room_id) {
        first_error.get_or_insert(error);
    }
    bridge_plan_protocol_authority.purge_bridge(room_id);
    peer_capabilities.purge_room(room_id);

    first_error.map_or(Ok(()), Err)
}

/// Receiver-host-owned, cross-layer terminal cleanup. Authority is cut off
/// first; later failures retain the tombstone and cannot reopen the Bridge.
pub(crate) async fn burn_bridge_scope(
    state: Arc<AppState>,
    room_id: &str,
    stop_server: bool,
    notify_peer: bool,
) -> AppResult<bool> {
    let peer = storage::get_room_by_id(&state.paths, room_id)
        .ok()
        .and_then(|room| room.peer_host.zip(room.peer_port));
    if !storage::cut_off_bridge_authority(&state.paths, room_id)? {
        return Ok(false);
    }
    let mut cleanup_error = purge_bridge_runtime_authority(&state, room_id).err();
    if stop_server {
        if let Err(error) = transfer::stop_room_server(state.clone(), room_id).await {
            cleanup_error.get_or_insert(error);
        }
    }
    if let Err(error) = transfer::cancel_room_transfers(
        state.clone(),
        room_id,
        "Room burned",
        false,
        Some("receiver_burned_room"),
    )
    .await
    {
        cleanup_error.get_or_insert(error);
    }
    let effective_inbox_dir = {
        let config = state.config.read();
        config::effective_inbox_dir(&state.paths, &config)
    };
    if let Err(error) = storage::finalize_burned_room(&state.paths, room_id, &effective_inbox_dir) {
        cleanup_error.get_or_insert(error);
    }
    if notify_peer {
        if let Some((peer_host, peer_port)) = peer {
            let _ = transfer::notify_room_burn_with_peer(&peer_host, peer_port, room_id).await;
        }
    }
    if let Some(error) = cleanup_error {
        return Err(error);
    }
    Ok(true)
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
        assert!(normalize_frontend_diagnostic_line(
            "[pastey:agent-bridge] event=summary url=file:///Users/pastey-secret/Documents/private.pdf"
        )
        .is_err());
    }
}

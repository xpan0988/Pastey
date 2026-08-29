use std::{sync::Arc, time::Duration};

use serde::Deserialize;
use tokio::{sync::oneshot, time::sleep};

use crate::{
    discovery,
    error::{AppError, AppResult},
    host_identity::HostRef,
    host_runtime::{DiscoveryHandle, HostRuntime as AppState},
    logging,
    models::{BridgePeerLiveness, RoomStatus, StoredBridgePeerEndpoint, StoredRoom},
    room_control, storage, transfer,
};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);
const LIVENESS_TIMEOUT: Duration = Duration::from_millis(900);
const DISCONNECTED_AFTER_ATTEMPTS: u8 = 2;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeLivenessProbe {
    transport_public_key: String,
}

pub async fn start(state: Arc<AppState>) {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    {
        let mut handle = state.bridge_lifecycle_handle.lock();
        if handle.is_some() {
            return;
        }
        *handle = Some(DiscoveryHandle {
            shutdown: shutdown_tx,
        });
    }

    if let Err(error) = bootstrap_room_servers(state.clone()).await {
        logging::write_error_line(&format!(
            "[pastey bridge-lifecycle] event=bootstrap_failed error={:?}",
            error.message()
        ));
    }

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            _ = sleep(RECONCILE_INTERVAL) => {
                if let Err(error) = reconcile_once(state.clone()).await {
                    logging::write_error_line(&format!(
                        "[pastey bridge-lifecycle] event=reconcile_failed error={:?}",
                        error.message()
                    ));
                }
            }
        }
    }
}

async fn bootstrap_room_servers(state: Arc<AppState>) -> AppResult<()> {
    for room in storage::list_rooms(&state.paths)? {
        if room.status == RoomStatus::Active {
            transfer::start_room_server(state.clone(), &room.id).await?;
        }
    }
    Ok(())
}

async fn reconcile_once(state: Arc<AppState>) -> AppResult<()> {
    invalidate_unreachable_sessions(state.clone()).await?;
    reconnect_preserved_bridges(state).await
}

async fn invalidate_unreachable_sessions(state: Arc<AppState>) -> AppResult<()> {
    for room in storage::list_rooms(&state.paths)? {
        if room.status != RoomStatus::Active {
            continue;
        }
        for peer in storage::list_bridge_peer_endpoints(&state.paths, &room.id)? {
            if peer.liveness != BridgePeerLiveness::Connected {
                continue;
            }
            if probe_exact_peer(&room, &peer).await {
                continue;
            }
            if storage::mark_bridge_peer_reconnecting(
                &state.paths,
                &room.id,
                &peer.peer_session_id,
            )? {
                state
                    .bridge_reconnect_rotations
                    .lock()
                    .insert(room.id.clone());
                state
                    .bridge_reconnect_attempts
                    .lock()
                    .insert(room.id.clone(), 0);
                room_control::clear_room_control_state(&state, &room.id);
                logging::write_transfer_line(&format!(
                    "[pastey bridge-lifecycle] event=peer_session_invalidated room_id={} peer_session_id={} reason=liveness_probe_failed",
                    room.id, peer.peer_session_id
                ));
            }
        }
    }
    Ok(())
}

async fn probe_exact_peer(room: &StoredRoom, peer: &StoredBridgePeerEndpoint) -> bool {
    let (Some(host), Some(port), Some(expected_key)) = (
        peer.endpoint_host.as_deref(),
        peer.endpoint_port,
        peer.transport_public_key.as_deref(),
    ) else {
        return false;
    };
    let client = match reqwest::Client::builder().timeout(LIVENESS_TIMEOUT).build() {
        Ok(client) => client,
        Err(_) => return false,
    };
    let response = match client
        .post(format!(
            "http://{host}:{port}/rooms/{}/diagnostics/ping",
            room.id
        ))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        _ => return false,
    };
    response
        .json::<BridgeLivenessProbe>()
        .await
        .is_ok_and(|probe| probe.transport_public_key == expected_key)
}

async fn reconnect_preserved_bridges(state: Arc<AppState>) -> AppResult<()> {
    for room in storage::list_rooms(&state.paths)? {
        if room.status != RoomStatus::Active {
            continue;
        }
        let peers = storage::list_bridge_peer_endpoints(&state.paths, &room.id)?;
        if peers.is_empty()
            || peers
                .iter()
                .any(|peer| peer.liveness == BridgePeerLiveness::Connected)
        {
            continue;
        }
        storage::mark_bridge_reconnect_started(&state.paths, &room.id)?;
        if state.bridge_reconnect_rotations.lock().remove(&room.id) {
            let _ = transfer::stop_room_server(state.clone(), &room.id).await;
        }
        transfer::start_room_server(state.clone(), &room.id).await?;

        match reconnect_room(state.clone(), &room).await {
            Ok(()) => {
                state.bridge_reconnect_attempts.lock().remove(&room.id);
                logging::write_transfer_line(&format!(
                    "[pastey bridge-lifecycle] event=reconnected room_id={}",
                    room.id
                ));
            }
            Err(error) => {
                let attempts = {
                    let mut attempts = state.bridge_reconnect_attempts.lock();
                    let entry = attempts.entry(room.id.clone()).or_default();
                    *entry = entry.saturating_add(1);
                    *entry
                };
                if attempts >= DISCONNECTED_AFTER_ATTEMPTS {
                    storage::mark_bridge_reconnect_failed(&state.paths, &room.id)?;
                }
                logging::write_transfer_line(&format!(
                    "[pastey bridge-lifecycle] event=reconnect_pending room_id={} attempt={} reason={:?}",
                    room.id,
                    attempts,
                    error.message()
                ));
            }
        }
    }
    Ok(())
}

async fn reconnect_room(state: Arc<AppState>, room: &StoredRoom) -> AppResult<()> {
    let (source, discovered) = discovery::discover_room(
        room.room_code_hash.clone(),
        Some(discovery::local_device_id(&state)),
        Some(room.id.clone()),
    )
    .await?;
    if discovered.room_id != room.id {
        return Err(AppError::InvalidInput(
            "Discovered Bridge identity does not match the preserved Bridge.".into(),
        ));
    }
    let response = transfer::announce_join(
        state.clone(),
        &room.id,
        &source.ip().to_string(),
        discovered.port,
    )
    .await?;
    let peer_host_ref = response
        .host_ref
        .as_deref()
        .map(|value| HostRef::parse_peer(value, &state.local_host_ref))
        .transpose()?;
    storage::update_room_peer(
        &state.paths,
        &room.id,
        Some(&source.ip().to_string()),
        Some(discovered.port),
        Some(&response.device_name),
        Some(&discovered.transport_public_key),
        RoomStatus::Active,
    )?;
    if let Some(host_ref) = peer_host_ref.as_ref() {
        storage::bind_legacy_room_peer_host_ref(&state.paths, &room.id, host_ref.as_str())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_policy_is_bounded_before_disconnected() {
        assert_eq!(DISCONNECTED_AFTER_ATTEMPTS, 2);
        assert!(LIVENESS_TIMEOUT < RECONCILE_INTERVAL);
    }
}

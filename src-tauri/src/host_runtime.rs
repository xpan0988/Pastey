use serde::{Deserialize, Serialize};

use crate::{error::AppResult, room_control, AppState};

/// UI-independent process-local services owned by one Pastey Host process.
/// Tauri's `AppState` contains this value, but the services do not depend on
/// windows, renderer state, or Layer 5 Plan authority.
#[derive(Default)]
pub struct HostRuntimeState {
    pub developer_terminal: crate::developer_terminal::DeveloperTerminalService,
}

impl HostRuntimeState {
    pub fn purge_room(&self, room_id: &str) {
        self.developer_terminal.purge_room(room_id);
    }

    pub fn shutdown_all(&self) {
        self.developer_terminal.shutdown_all();
    }
}

/// A logical Host identity token for Developer Mode v0. It is deliberately a
/// distinct type from a Layer 4 route/session reference. In v0 it is derived
/// from the Host's current transport identity; a later durable HostRef contract
/// may replace that derivation without changing terminal grant semantics.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct HostRef(pub String);

/// Exact current-session reachability binding. Route/session liveness is input
/// to admission, never admission or authority by itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSessionBinding {
    pub room_id: String,
    pub controller_host: HostRef,
    pub target_host: HostRef,
    pub controller_session_ref: String,
    pub target_session_ref: String,
    pub peer_route_ref: String,
    pub binding_ref: String,
}

impl HostSessionBinding {
    pub fn new(
        room_id: &str,
        controller_session_ref: &str,
        target_session_ref: &str,
        peer_route_ref: &str,
    ) -> Self {
        let controller_host = host_ref(controller_session_ref);
        let target_host = host_ref(target_session_ref);
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pastey-developer-host-session-binding-v0\0");
        hasher.update(room_id.as_bytes());
        hasher.update(controller_host.0.as_bytes());
        hasher.update(target_host.0.as_bytes());
        hasher.update(controller_session_ref.as_bytes());
        hasher.update(target_session_ref.as_bytes());
        Self {
            room_id: room_id.to_string(),
            controller_host,
            target_host,
            controller_session_ref: controller_session_ref.to_string(),
            target_session_ref: target_session_ref.to_string(),
            peer_route_ref: peer_route_ref.to_string(),
            binding_ref: format!("host-session-binding:{}", hasher.finalize().to_hex()),
        }
    }
}

fn host_ref(session_ref: &str) -> HostRef {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pastey-developer-host-ref-v0\0");
    hasher.update(session_ref.as_bytes());
    HostRef(format!("host:{}", hasher.finalize().to_hex()))
}

pub fn current_controller_binding(
    state: &std::sync::Arc<AppState>,
    room_id: &str,
    peer_session_id: &str,
) -> AppResult<HostSessionBinding> {
    let context =
        room_control::room_control_session_context_for_peer(state, room_id, peer_session_id)?;
    Ok(HostSessionBinding::new(
        room_id,
        &context.local_session_ref,
        &context.peer_session_ref,
        &context.peer_route_ref,
    ))
}

pub fn current_target_binding(
    state: &std::sync::Arc<AppState>,
    room_id: &str,
    controller_peer_session_id: &str,
) -> AppResult<HostSessionBinding> {
    let context = room_control::room_control_session_context_for_peer(
        state,
        room_id,
        controller_peer_session_id,
    )?;
    Ok(HostSessionBinding::new(
        room_id,
        &context.peer_session_ref,
        &context.local_session_ref,
        &context.peer_route_ref,
    ))
}

pub fn inbound_controller_binding(
    room_id: &str,
    controller_session_ref: &str,
    target_session_ref: &str,
    peer_route_ref: &str,
) -> HostSessionBinding {
    HostSessionBinding::new(
        room_id,
        controller_session_ref,
        target_session_ref,
        peer_route_ref,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_identity_and_current_session_binding_are_distinct() {
        let binding = HostSessionBinding::new("room", "controller-session", "host-session", "peer");
        assert_ne!(binding.controller_host.0, binding.controller_session_ref);
        assert_ne!(binding.target_host.0, binding.target_session_ref);
        assert!(binding.binding_ref.starts_with("host-session-binding:"));
    }

    #[test]
    fn stale_session_changes_binding_without_route_authority() {
        let first = HostSessionBinding::new("room", "controller-a", "host-a", "peer");
        let second = HostSessionBinding::new("room", "controller-a", "host-b", "peer");
        assert_ne!(first.binding_ref, second.binding_ref);
        assert_ne!(first.target_host, second.target_host);
    }
}

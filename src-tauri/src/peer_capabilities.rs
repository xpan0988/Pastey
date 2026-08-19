//! Current-session Host capability observations.
//!
//! Facts answer only whether one exact capability is currently implemented by
//! a Host. They do not approve a step, select a Host, rewrite topology, move an
//! object, or carry an ObjectRef.
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub(crate) const PEER_CAPABILITY_SCHEMA: &str = "pastey-peer-capabilities-v2";
const MAX_CAPABILITIES: usize = 16;
const MAX_MEDIA_TYPES: usize = 16;
const MAX_PAYLOAD_BYTES: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HostCapabilityFact {
    pub(crate) capability_id: String,
    pub(crate) available: bool,
    pub(crate) accepted_input_media_types: Vec<String>,
    pub(crate) effect: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PeerCapabilityProjection {
    pub(crate) schema_version: String,
    pub(crate) peer_session_id: String,
    pub(crate) observed_at: i64,
    pub(crate) capabilities: Vec<HostCapabilityFact>,
}

#[derive(Default)]
pub(crate) struct PeerCapabilityStore {
    projections: HashMap<(String, String, String), PeerCapabilityProjection>,
}

pub(crate) fn local_projection(
    peer_session_id: String,
    observed_at: i64,
) -> PeerCapabilityProjection {
    PeerCapabilityProjection {
        schema_version: PEER_CAPABILITY_SCHEMA.into(),
        peer_session_id,
        observed_at,
        // Pastey Core currently projects no concrete Transform or Execute
        // implementations. The current-session transport remains ready for a
        // later Agent-owned registry without treating framework support as an
        // available capability.
        capabilities: Vec::new(),
    }
}

impl PeerCapabilityStore {
    pub(crate) fn observe(
        &mut self,
        room_id: &str,
        expected_peer_session_id: &str,
        expected_peer_observation_ref: &str,
        projection: PeerCapabilityProjection,
        _received_at: i64,
    ) -> AppResult<()> {
        validate_projection(&projection)?;
        if projection.peer_session_id != expected_peer_session_id {
            return Err(AppError::InvalidInput(
                "Peer capability session mismatch.".into(),
            ));
        }
        self.projections.insert(
            (
                room_id.into(),
                expected_peer_session_id.into(),
                expected_peer_observation_ref.into(),
            ),
            projection,
        );
        Ok(())
    }

    pub(crate) fn purge_room(&mut self, room_id: &str) {
        self.projections
            .retain(|(stored_room, _, _), _| stored_room != room_id);
    }

    #[cfg(test)]
    pub(crate) fn projection(
        &self,
        room_id: &str,
        peer_session_id: &str,
        peer_observation_ref: &str,
    ) -> Option<&PeerCapabilityProjection> {
        self.projections.get(&(
            room_id.into(),
            peer_session_id.into(),
            peer_observation_ref.into(),
        ))
    }
}

pub(crate) fn validate_projection(projection: &PeerCapabilityProjection) -> AppResult<()> {
    if projection.schema_version != PEER_CAPABILITY_SCHEMA
        || projection.peer_session_id.is_empty()
        || projection.peer_session_id.len() > 256
        || projection.observed_at <= 0
        || projection.capabilities.len() > MAX_CAPABILITIES
        || serde_json::to_vec(projection)?.len() > MAX_PAYLOAD_BYTES
    {
        return Err(AppError::InvalidInput(
            "Invalid peer capability projection.".into(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for capability in &projection.capabilities {
        if !seen.insert(capability.capability_id.as_str())
            || capability.capability_id.is_empty()
            || capability.capability_id.len() > 128
            || capability.accepted_input_media_types.len() > MAX_MEDIA_TYPES
            || capability
                .accepted_input_media_types
                .iter()
                .any(|media| !media.contains('/'))
            || capability.effect.is_empty()
            || capability.effect.len() > 128
            || match (capability.available, capability.unavailable_reason.as_ref()) {
                (true, None) => false,
                (false, Some(reason)) if !reason.is_empty() && reason.len() <= 128 => false,
                _ => true,
            }
        {
            return Err(AppError::InvalidInput(
                "Invalid peer capability fact.".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_projection_is_empty_non_authorizing_and_valid() {
        let projection = local_projection("peer".into(), 1);
        assert!(projection.capabilities.is_empty());
        assert!(validate_projection(&projection).is_ok());
        let json = serde_json::to_string(&projection).unwrap();
        let decoded: PeerCapabilityProjection = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, projection);
        assert!(decoded.capabilities.is_empty());
        for forbidden in [
            "objectRef",
            "path",
            "authority",
            "deviceSelection",
            "topology",
        ] {
            assert!(!json.contains(forbidden));
        }
    }

    #[test]
    fn empty_projection_is_received_and_stored_without_fabricating_a_fact() {
        let projection = local_projection("peer".into(), 10);
        let mut store = PeerCapabilityStore::default();
        store
            .observe("room", "peer", "observation", projection, 10)
            .unwrap();

        let stored = store.projection("room", "peer", "observation").unwrap();
        assert!(stored.capabilities.is_empty());
        assert_eq!(stored.peer_session_id, "peer");
    }

    #[test]
    fn generic_transport_accepts_bounded_facts_without_granting_authority() {
        let mut projection = local_projection("peer".into(), 10);
        projection.capabilities.push(HostCapabilityFact {
            capability_id: "future_agent_capability".into(),
            available: false,
            accepted_input_media_types: vec!["text/plain".into()],
            effect: "future_agent_owned_effect".into(),
            unavailable_reason: Some("agent_not_installed".into()),
        });
        assert!(validate_projection(&projection).is_ok());
        let mut store = PeerCapabilityStore::default();
        store
            .observe("room", "peer", "observation", projection, 10)
            .unwrap();
        store.purge_room("room");
    }
}

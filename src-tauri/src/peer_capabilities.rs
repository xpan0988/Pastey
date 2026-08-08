//! Current-session peer capability observations.
//!
//! These facts are deliberately ephemeral and non-authorizing. They bind a
//! reply to the Room Control session that delivered it; they are not durable
//! pairing metadata, a route, consent, or an execution grant.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    error::{AppError, AppResult},
    room_control::log_peer_capability,
};

pub(crate) const PEER_CAPABILITY_SCHEMA: &str = "pastey-peer-capabilities-v1";
pub(crate) const CAPABILITY_ID: &str = "extract_readable_text_v1";
const MAX_CAPABILITIES: usize = 1;
const MAX_MEDIA_TYPES: usize = 4;
const MAX_PAYLOAD_BYTES: usize = 2048;
pub(crate) const MAX_FACT_AGE_SECONDS: i64 = 120;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransformCapabilityFact {
    pub(crate) capability_id: String,
    pub(crate) available: bool,
    pub(crate) accepted_input_media_types: Vec<String>,
    pub(crate) output_media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PeerCapabilityProjection {
    pub(crate) schema_version: String,
    pub(crate) peer_session_id: String,
    pub(crate) observed_at: i64,
    pub(crate) capabilities: Vec<TransformCapabilityFact>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectedPeerTransformAvailability {
    pub(crate) peer_session_id: String,
    pub(crate) status: &'static str,
    pub(crate) available: bool,
    pub(crate) reason: &'static str,
    pub(crate) accepted_input_media_types: Vec<String>,
    pub(crate) output_media_type: Option<String>,
}

#[derive(Default)]
pub(crate) struct PeerCapabilityStore {
    projections: HashMap<(String, String, String), ObservedProjection>,
}

struct ObservedProjection {
    projection: PeerCapabilityProjection,
    received_at: i64,
}

pub(crate) fn local_projection(
    peer_session_id: String,
    observed_at: i64,
) -> PeerCapabilityProjection {
    let available = cfg!(unix);
    PeerCapabilityProjection {
        schema_version: PEER_CAPABILITY_SCHEMA.into(),
        peer_session_id,
        observed_at,
        capabilities: vec![TransformCapabilityFact {
            capability_id: CAPABILITY_ID.into(),
            available,
            accepted_input_media_types: vec![
                "text/plain".into(),
                "text/markdown".into(),
                "application/json".into(),
                "text/csv".into(),
            ],
            output_media_type: "text/plain".into(),
            unavailable_reason: (!available).then(|| "platform_unsupported".into()),
        }],
    }
}

impl PeerCapabilityStore {
    pub(crate) fn observe(
        &mut self,
        room_id: &str,
        expected_peer_session_id: &str,
        expected_peer_observation_ref: &str,
        projection: PeerCapabilityProjection,
        received_at: i64,
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
            ObservedProjection {
                projection,
                received_at,
            },
        );
        Ok(())
    }

    pub(crate) fn selected_transform(
        &self,
        room_id: &str,
        peer_session_id: &str,
        peer_observation_ref: &str,
        now: i64,
    ) -> SelectedPeerTransformAvailability {
        let Some(observed) = self.projections.get(&(
            room_id.into(),
            peer_session_id.into(),
            peer_observation_ref.into(),
        )) else {
            log_peer_capability(
                "projection_lookup_miss",
                None,
                Some("missing_or_route_changed"),
            );
            return unknown(peer_session_id);
        };
        if observed.received_at <= 0 || now - observed.received_at > MAX_FACT_AGE_SECONDS {
            log_peer_capability("projection_lookup_miss", None, Some("stale"));
            return unknown(peer_session_id);
        }
        let Some(capability) = observed
            .projection
            .capabilities
            .iter()
            .find(|fact| fact.capability_id == CAPABILITY_ID)
        else {
            log_peer_capability("projection_lookup_miss", None, Some("capability_missing"));
            return unknown(peer_session_id);
        };
        if capability.available {
            log_peer_capability("projection_lookup_hit", Some(true), Some("available"));
            SelectedPeerTransformAvailability {
                peer_session_id: peer_session_id.into(),
                status: "available",
                available: true,
                reason: "Available on the selected device.",
                accepted_input_media_types: capability.accepted_input_media_types.clone(),
                output_media_type: Some(capability.output_media_type.clone()),
            }
        } else {
            log_peer_capability(
                "projection_lookup_hit",
                Some(false),
                capability.unavailable_reason.as_deref(),
            );
            SelectedPeerTransformAvailability {
                peer_session_id: peer_session_id.into(),
                status: "unavailable",
                available: false,
                reason: "The selected device cannot run readable-text Transform.",
                accepted_input_media_types: capability.accepted_input_media_types.clone(),
                output_media_type: Some(capability.output_media_type.clone()),
            }
        }
    }

    pub(crate) fn purge_room(&mut self, room_id: &str) {
        self.projections
            .retain(|(stored_room, _, _), _| stored_room != room_id);
    }
}

fn unknown(peer_session_id: &str) -> SelectedPeerTransformAvailability {
    SelectedPeerTransformAvailability {
        peer_session_id: peer_session_id.into(),
        status: "unknown",
        available: false,
        reason: "Checking selected device capability…",
        accepted_input_media_types: Vec::new(),
        output_media_type: None,
    }
}

pub(crate) fn validate_projection(projection: &PeerCapabilityProjection) -> AppResult<()> {
    if projection.schema_version != PEER_CAPABILITY_SCHEMA
        || projection.peer_session_id.is_empty()
        || projection.peer_session_id.len() > 256
        || projection.observed_at <= 0
        || projection.capabilities.len() != MAX_CAPABILITIES
        || serde_json::to_vec(projection)?.len() > MAX_PAYLOAD_BYTES
    {
        return Err(AppError::InvalidInput(
            "Invalid peer capability projection.".into(),
        ));
    }
    let capability = &projection.capabilities[0];
    if capability.capability_id != CAPABILITY_ID
        || capability.accepted_input_media_types.len() != MAX_MEDIA_TYPES
        || capability.output_media_type != "text/plain"
        || capability.accepted_input_media_types.as_slice()
            != [
                "text/plain",
                "text/markdown",
                "application/json",
                "text/csv",
            ]
    {
        return Err(AppError::InvalidInput(
            "Unsupported peer capability projection.".into(),
        ));
    }
    match (
        capability.available,
        capability.unavailable_reason.as_deref(),
    ) {
        (true, None)
        | (
            false,
            Some("platform_unsupported" | "backend_unavailable" | "capability_unavailable"),
        ) => Ok(()),
        _ => Err(AppError::InvalidInput(
            "Invalid peer capability availability.".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_projection_matches_current_staging_platform() {
        assert_eq!(
            local_projection("peer".into(), 1).capabilities[0].available,
            cfg!(unix)
        );
    }

    #[test]
    fn projection_is_session_bound_and_burn_purgeable() {
        let mut store = PeerCapabilityStore::default();
        let projection = local_projection("peer-one".into(), 1);
        store
            .observe("room", "peer-one", "endpoint-one", projection, 100)
            .unwrap();
        assert_eq!(
            store
                .selected_transform("room", "peer-one", "endpoint-one", 100)
                .status,
            if cfg!(unix) {
                "available"
            } else {
                "unavailable"
            }
        );
        assert_eq!(
            store
                .selected_transform("room", "peer-two", "endpoint-two", 100)
                .status,
            "unknown"
        );
        assert_eq!(
            store
                .selected_transform("room", "peer-one", "endpoint-changed", 100)
                .status,
            "unknown"
        );
        store.purge_room("room");
        assert_eq!(
            store
                .selected_transform("room", "peer-one", "endpoint-one", 100)
                .status,
            "unknown"
        );
        let projection = local_projection("peer-one".into(), 100);
        store
            .observe("room", "peer-one", "endpoint-one", projection, 100)
            .unwrap();
        assert_eq!(
            store
                .selected_transform(
                    "room",
                    "peer-one",
                    "endpoint-one",
                    100 + MAX_FACT_AGE_SECONDS + 1,
                )
                .status,
            "unknown"
        );
        assert!(store
            .observe(
                "room",
                "windows-selected-session",
                "endpoint-one",
                local_projection("requester-session".into(), 100),
                100,
            )
            .is_err());
    }

    #[test]
    fn malformed_and_unknown_facts_fail_closed() {
        let mut projection = local_projection("peer".into(), 1);
        projection.capabilities[0].capability_id = "other".into();
        assert!(validate_projection(&projection).is_err());
        let mut projection = local_projection("peer".into(), 1);
        projection
            .capabilities
            .push(projection.capabilities[0].clone());
        assert!(validate_projection(&projection).is_err());
        let mut projection = local_projection("peer".into(), 1);
        projection.capabilities[0].output_media_type = "x".repeat(MAX_PAYLOAD_BYTES);
        assert!(validate_projection(&projection).is_err());
    }

    #[test]
    fn restart_starts_unknown_and_platform_reason_is_factual() {
        assert_eq!(
            PeerCapabilityStore::default()
                .selected_transform("room", "peer", "endpoint", 1)
                .status,
            "unknown"
        );
        let fact = &local_projection("peer".into(), 1).capabilities[0];
        if cfg!(unix) {
            assert!(fact.available);
            assert_eq!(fact.unavailable_reason, None);
        } else {
            assert!(!fact.available);
            assert_eq!(
                fact.unavailable_reason.as_deref(),
                Some("platform_unsupported")
            );
        }
    }
}

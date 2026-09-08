//! Current-session Host capability observations.
//!
//! Facts answer only whether one exact capability is currently implemented by
//! a Host. They do not approve a step, select a Host, rewrite topology, move an
//! object, or carry an ObjectRef.
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    capability_probe::{self, KnownCapabilityProbeResult},
    diagnostics::DiagnosticState,
    error::{AppError, AppResult},
    host_runtime::HostRuntime,
    worker_provider_config::WorkerProviderHealthStateV1,
};

pub(crate) const PEER_CAPABILITY_SCHEMA: &str = "pastey-peer-capabilities-v2";
const MAX_CAPABILITIES: usize = 16;
const MAX_MEDIA_TYPES: usize = 16;
const MAX_PAYLOAD_BYTES: usize = 4096;

pub(crate) const MANAGED_PROVIDER_CAPABILITY: &str = "pastey.managed.provider";
pub(crate) const MANAGED_RUNTIME_CAPABILITY: &str = "pastey.managed.runtime";
pub(crate) const EXECUTION_WORLD_CAPABILITY: &str = "pastey.managed.execution_world";
pub(crate) const MANAGED_EXECUTION_CAPABILITY: &str = "pastey.managed.execution";

const REASON_NOT_CONFIGURED: &str = "not_configured";
const REASON_UNKNOWN: &str = "unknown";
const REASON_PLAN_BINDING_REQUIRED: &str = "plan_process_binding_required";
const REASON_PROVIDER_UNAVAILABLE: &str = "provider_unavailable";
const REASON_RUNTIME_UNAVAILABLE: &str = "runtime_unavailable";
const REASON_EXECUTION_WORLD_UNAVAILABLE: &str = "execution_world_unavailable";
const REASON_SYSTEM_PROBE_UNAVAILABLE: &str = "system_probe_unavailable";
const REASON_SYSTEM_PROBE_UNSUPPORTED: &str = "system_probe_unsupported";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostCapabilityFact {
    pub capability_id: String,
    pub available: bool,
    pub accepted_input_media_types: Vec<String>,
    pub effect: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
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

#[cfg(test)]
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

/// Projects current Host-owned managed readiness through the existing bounded
/// capability channel. These are observations only: an exact Plan still owns
/// Host selection, approval, admission, process binding, and effect authority.
pub(crate) fn local_diagnostic_projection(
    state: &HostRuntime,
    peer_session_id: String,
    observed_at: i64,
) -> PeerCapabilityProjection {
    let provider = match state
        .worker_provider_configs
        .selected_managed_worker_metadata()
    {
        Ok(None) => capability(
            MANAGED_PROVIDER_CAPABILITY,
            false,
            Some(REASON_NOT_CONFIGURED),
        ),
        Ok(Some(metadata)) => match metadata.health {
            WorkerProviderHealthStateV1::Healthy => {
                capability(MANAGED_PROVIDER_CAPABILITY, true, None)
            }
            WorkerProviderHealthStateV1::Unknown => {
                capability(MANAGED_PROVIDER_CAPABILITY, false, Some(REASON_UNKNOWN))
            }
            WorkerProviderHealthStateV1::Unhealthy => capability(
                MANAGED_PROVIDER_CAPABILITY,
                false,
                Some(REASON_PROVIDER_UNAVAILABLE),
            ),
        },
        Err(_) => capability(
            MANAGED_PROVIDER_CAPABILITY,
            false,
            Some(REASON_PROVIDER_UNAVAILABLE),
        ),
    };

    let runtime = match state.managed_runtime_configs.selected_for_managed_execute() {
        Ok(None) => capability(
            MANAGED_RUNTIME_CAPABILITY,
            false,
            Some(REASON_NOT_CONFIGURED),
        ),
        Ok(Some(_)) => capability(MANAGED_RUNTIME_CAPABILITY, true, None),
        Err(_) => capability(
            MANAGED_RUNTIME_CAPABILITY,
            false,
            Some(REASON_RUNTIME_UNAVAILABLE),
        ),
    };
    let execution_world_available = state.execution_worlds.platform_availability().available;
    let execution_world = capability(
        EXECUTION_WORLD_CAPABILITY,
        execution_world_available,
        (!execution_world_available).then_some(REASON_EXECUTION_WORLD_UNAVAILABLE),
    );
    let managed_execution = if provider.unavailable_reason.as_deref() == Some(REASON_NOT_CONFIGURED)
        || runtime.unavailable_reason.as_deref() == Some(REASON_NOT_CONFIGURED)
    {
        capability(
            MANAGED_EXECUTION_CAPABILITY,
            false,
            Some(REASON_NOT_CONFIGURED),
        )
    } else if provider.unavailable_reason.as_deref() == Some(REASON_PROVIDER_UNAVAILABLE)
        || runtime.unavailable_reason.as_deref() == Some(REASON_RUNTIME_UNAVAILABLE)
    {
        capability(
            MANAGED_EXECUTION_CAPABILITY,
            false,
            if provider.unavailable_reason.as_deref() == Some(REASON_PROVIDER_UNAVAILABLE) {
                Some(REASON_PROVIDER_UNAVAILABLE)
            } else {
                Some(REASON_RUNTIME_UNAVAILABLE)
            },
        )
    } else if !execution_world_available {
        capability(
            MANAGED_EXECUTION_CAPABILITY,
            false,
            Some(REASON_EXECUTION_WORLD_UNAVAILABLE),
        )
    } else {
        capability(
            MANAGED_EXECUTION_CAPABILITY,
            false,
            Some(REASON_PLAN_BINDING_REQUIRED),
        )
    };

    PeerCapabilityProjection {
        schema_version: PEER_CAPABILITY_SCHEMA.into(),
        peer_session_id,
        observed_at,
        capabilities: vec![provider, runtime, execution_world, managed_execution],
    }
}

/// Adds exact requested system-probe observations to the existing managed
/// readiness projection. The request has already been reduced to canonical
/// semantic IDs; the Host alone chooses the fixed probe implementation.
pub(crate) fn local_diagnostic_projection_with_system_probes(
    state: &HostRuntime,
    peer_session_id: String,
    observed_at: i64,
    capability_ids: &[String],
) -> AppResult<PeerCapabilityProjection> {
    let capability_ids = capability_probe::normalize_known_capability_request(capability_ids)?;
    let mut projection = local_diagnostic_projection(state, peer_session_id, observed_at);
    append_system_probe_facts(
        &mut projection,
        &capability_ids,
        capability_probe::probe_known_capability,
    )?;
    validate_projection(&projection)?;
    Ok(projection)
}

/// Validates and deduplicates the capability-only part of a Room Control
/// query. It intentionally has no command, executable, argument, or shell
/// field to deserialize.
pub(crate) fn normalize_system_probe_request(capability_ids: &[String]) -> AppResult<Vec<String>> {
    capability_probe::normalize_known_capability_request(capability_ids)
}

fn append_system_probe_facts(
    projection: &mut PeerCapabilityProjection,
    capability_ids: &[String],
    probe: impl Fn(&str) -> KnownCapabilityProbeResult,
) -> AppResult<()> {
    for capability_id in capability_ids {
        let fact = match probe(capability_id) {
            KnownCapabilityProbeResult::Available => system_probe_fact(capability_id, true, None),
            KnownCapabilityProbeResult::Unavailable => {
                system_probe_fact(capability_id, false, Some(REASON_SYSTEM_PROBE_UNAVAILABLE))
            }
            KnownCapabilityProbeResult::Unsupported => {
                system_probe_fact(capability_id, false, Some(REASON_SYSTEM_PROBE_UNSUPPORTED))
            }
        };
        projection.capabilities.push(fact);
    }
    Ok(())
}

fn system_probe_fact(
    capability_id: &str,
    available: bool,
    unavailable_reason: Option<&str>,
) -> HostCapabilityFact {
    HostCapabilityFact {
        capability_id: capability_id.into(),
        available,
        accepted_input_media_types: Vec::new(),
        effect: "system_probe_observation".into(),
        unavailable_reason: unavailable_reason.map(str::to_string),
    }
}

fn capability(
    capability_id: &str,
    available: bool,
    unavailable_reason: Option<&str>,
) -> HostCapabilityFact {
    HostCapabilityFact {
        capability_id: capability_id.into(),
        available,
        accepted_input_media_types: Vec::new(),
        effect: "readiness_observation".into(),
        unavailable_reason: unavailable_reason.map(str::to_string),
    }
}

impl PeerCapabilityProjection {
    /// Converts one known capability fact to renderer-safe diagnostics without
    /// making callers interpret protocol reason codes.
    pub(crate) fn diagnostic_state(&self, capability_id: &str) -> DiagnosticState {
        let Some(fact) = self
            .capabilities
            .iter()
            .find(|fact| fact.capability_id == capability_id)
        else {
            return DiagnosticState::Unknown;
        };
        if fact.available {
            return DiagnosticState::Available;
        }
        match fact.unavailable_reason.as_deref() {
            Some(REASON_NOT_CONFIGURED) => DiagnosticState::NotConfigured,
            Some(REASON_UNKNOWN | REASON_PLAN_BINDING_REQUIRED) => DiagnosticState::Unknown,
            Some(_) => DiagnosticState::Unavailable,
            None => DiagnosticState::Unknown,
        }
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

    pub(crate) fn projection(
        &self,
        room_id: &str,
        peer_session_id: &str,
        peer_observation_ref: &str,
    ) -> Option<PeerCapabilityProjection> {
        self.projections
            .get(&(
                room_id.into(),
                peer_session_id.into(),
                peer_observation_ref.into(),
            ))
            .cloned()
    }

    pub(crate) fn remove_projection(
        &mut self,
        room_id: &str,
        peer_session_id: &str,
        peer_observation_ref: &str,
    ) {
        self.projections.remove(&(
            room_id.into(),
            peer_session_id.into(),
            peer_observation_ref.into(),
        ));
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
    fn replaced_session_cannot_reuse_an_old_capability_observation() {
        let projection = local_projection("old-session".into(), 10);
        let mut store = PeerCapabilityStore::default();
        store
            .observe("room", "old-session", "old-route", projection.clone(), 10)
            .unwrap();

        assert!(store
            .projection("room", "new-session", "new-route")
            .is_none());
        assert!(store
            .observe("room", "new-session", "new-route", projection, 11)
            .is_err());
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

    #[test]
    fn diagnostic_states_keep_not_configured_unavailable_and_unknown_distinct() {
        let mut projection = local_projection("peer".into(), 10);
        projection.capabilities = vec![
            capability(
                MANAGED_PROVIDER_CAPABILITY,
                false,
                Some(REASON_NOT_CONFIGURED),
            ),
            capability(
                MANAGED_RUNTIME_CAPABILITY,
                false,
                Some(REASON_PLAN_BINDING_REQUIRED),
            ),
            capability(
                EXECUTION_WORLD_CAPABILITY,
                false,
                Some(REASON_EXECUTION_WORLD_UNAVAILABLE),
            ),
        ];

        assert_eq!(
            projection.diagnostic_state(MANAGED_PROVIDER_CAPABILITY),
            DiagnosticState::NotConfigured
        );
        assert_eq!(
            projection.diagnostic_state(MANAGED_RUNTIME_CAPABILITY),
            DiagnosticState::Unknown
        );
        assert_eq!(
            projection.diagnostic_state(EXECUTION_WORLD_CAPABILITY),
            DiagnosticState::Unavailable
        );
        assert_eq!(
            projection.diagnostic_state(MANAGED_EXECUTION_CAPABILITY),
            DiagnosticState::Unknown
        );
    }

    #[test]
    fn known_system_probe_observations_preserve_readiness_and_distinguish_unavailable() {
        let mut projection = local_projection("peer".into(), 10);
        projection.capabilities.push(capability(
            MANAGED_RUNTIME_CAPABILITY,
            false,
            Some(REASON_NOT_CONFIGURED),
        ));
        append_system_probe_facts(
            &mut projection,
            &["runtime.python".into(), "runtime.node".into()],
            |capability_id| match capability_id {
                "runtime.python" => KnownCapabilityProbeResult::Available,
                "runtime.node" => KnownCapabilityProbeResult::Unavailable,
                _ => KnownCapabilityProbeResult::Unsupported,
            },
        )
        .unwrap();

        assert_eq!(projection.capabilities.len(), 3);
        assert_eq!(
            projection
                .capabilities
                .iter()
                .find(|fact| fact.capability_id == "runtime.python")
                .unwrap()
                .available,
            true
        );
        assert_eq!(
            projection
                .capabilities
                .iter()
                .find(|fact| fact.capability_id == "runtime.node")
                .unwrap()
                .unavailable_reason
                .as_deref(),
            Some(REASON_SYSTEM_PROBE_UNAVAILABLE)
        );
        assert_eq!(
            projection.diagnostic_state(MANAGED_RUNTIME_CAPABILITY),
            DiagnosticState::NotConfigured
        );
    }

    #[test]
    fn absent_system_probe_observation_remains_unknown_not_unavailable() {
        let projection = local_projection("peer".into(), 10);
        assert_eq!(
            projection.diagnostic_state("runtime.python"),
            DiagnosticState::Unknown
        );
    }

    #[test]
    fn locally_unsupported_system_probe_is_a_distinct_bounded_observation() {
        let mut projection = local_projection("peer".into(), 10);
        append_system_probe_facts(
            &mut projection,
            &["runtime.powershell".into()],
            |_capability_id| KnownCapabilityProbeResult::Unsupported,
        )
        .unwrap();
        assert_eq!(projection.capabilities.len(), 1);
        assert_eq!(
            projection.capabilities[0].unavailable_reason.as_deref(),
            Some(REASON_SYSTEM_PROBE_UNSUPPORTED)
        );
    }
}

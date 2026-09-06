use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

const HOST_REF_PREFIX: &str = "host:v1:";
const PARTICIPANT_REF_PREFIX: &str = "plan-participant:v1:";

/// Core-owned logical identity for one Pastey Host.
///
/// A HostRef is deliberately independent of Bridge rooms, transport keys,
/// peer routes, liveness, capabilities, and display-only paired identities.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct HostRef(String);

impl HostRef {
    /// Derives the stable local HostRef from the persistent installation
    /// identity. The raw device id is never exposed as the HostRef.
    pub fn from_device_id(device_id: &str) -> AppResult<Self> {
        let device_id = device_id.trim();
        if device_id.is_empty() {
            return Err(AppError::InvalidInput(
                "Host identity is unavailable.".into(),
            ));
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pastey-host-ref-v1\0");
        hasher.update(device_id.as_bytes());
        Ok(Self(format!(
            "{HOST_REF_PREFIX}{}",
            hasher.finalize().to_hex()
        )))
    }

    pub fn parse(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        let digest = value
            .strip_prefix(HOST_REF_PREFIX)
            .ok_or_else(|| AppError::InvalidInput("Invalid HostRef contract version.".into()))?;
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::InvalidInput("Invalid HostRef encoding.".into()));
        }
        Ok(Self(value))
    }

    pub fn parse_peer(value: impl Into<String>, local_host_ref: &Self) -> AppResult<Self> {
        let peer = Self::parse(value)?;
        if &peer == local_host_ref {
            return Err(AppError::InvalidInput(
                "Peer HostRef must differ from the local HostRef.".into(),
            ));
        }
        Ok(peer)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identity of a Host's participation in one immutable logical Plan.
///
/// This is not a requester/selected-device role. The same Host receives a
/// different participant reference in a different Plan.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PlanParticipantRef(String);

impl PlanParticipantRef {
    pub fn for_host(plan_id: &str, host_ref: &HostRef) -> AppResult<Self> {
        let plan_id = plan_id.trim();
        if plan_id.is_empty() {
            return Err(AppError::InvalidInput(
                "Plan identity is unavailable.".into(),
            ));
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pastey-plan-participant-ref-v1\0");
        hasher.update(plan_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(host_ref.as_str().as_bytes());
        Ok(Self(format!(
            "{PARTICIPANT_REF_PREFIX}{}",
            hasher.finalize().to_hex()
        )))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanParticipant {
    pub participant_ref: PlanParticipantRef,
    pub host_ref: HostRef,
}

/// Role-neutral participant set used natively by Plan v2 and kept separate
/// from BridgePlanRevision v1 so the compatibility hash cannot change.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PlanParticipants(Vec<PlanParticipant>);

impl PlanParticipants {
    #[allow(dead_code)] // Native v2 Composer/outbound construction is not exposed yet.
    pub fn new(plan_id: &str, hosts: impl IntoIterator<Item = HostRef>) -> AppResult<Self> {
        let mut by_host = BTreeMap::new();
        for host_ref in hosts {
            let participant_ref = PlanParticipantRef::for_host(plan_id, &host_ref)?;
            if by_host
                .insert(
                    host_ref.clone(),
                    PlanParticipant {
                        participant_ref,
                        host_ref,
                    },
                )
                .is_some()
            {
                return Err(AppError::InvalidInput(
                    "A Host may participate only once in a Plan.".into(),
                ));
            }
        }
        if by_host.is_empty() {
            return Err(AppError::InvalidInput(
                "A Plan must have at least one Host participant.".into(),
            ));
        }
        Ok(Self(by_host.into_values().collect()))
    }

    pub fn as_slice(&self) -> &[PlanParticipant] {
        &self.0
    }
}

/// Fresh process generation for the durable local Host.
///
/// This is deliberately not a Bridge session, peer route, or transport
/// binding. Restarting the HostRuntime creates a different reference while
/// retaining the same durable HostRef.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalRuntimeRef {
    host_ref: HostRef,
    generation_ref: String,
}

impl LocalRuntimeRef {
    pub(crate) fn fresh(host_ref: HostRef) -> Self {
        Self {
            host_ref,
            generation_ref: format!("local-runtime:v1:{}", uuid::Uuid::new_v4()),
        }
    }

    pub fn host_ref(&self) -> &HostRef {
        &self.host_ref
    }

    pub fn generation_ref(&self) -> &str {
        &self.generation_ref
    }

    pub fn validate_current(&self, current: &Self) -> AppResult<()> {
        if self != current {
            return Err(AppError::InvalidInput(
                "Local runtime reference is stale or mismatched.".into(),
            ));
        }
        Ok(())
    }
}

/// Bridge-scoped validity for direct execution on the current HostRuntime.
/// It carries no peer, route, or session identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalExecutionFreshness {
    pub bridge_id: String,
    pub local_runtime_ref: LocalRuntimeRef,
    pub expires_at: i64,
}

impl LocalExecutionFreshness {
    pub fn new(
        bridge_id: &str,
        local_runtime_ref: LocalRuntimeRef,
        expires_at: i64,
    ) -> AppResult<Self> {
        if bridge_id.trim().is_empty() || expires_at <= 0 {
            return Err(AppError::InvalidInput(
                "Local execution freshness is incomplete.".into(),
            ));
        }
        Ok(Self {
            bridge_id: bridge_id.to_string(),
            local_runtime_ref,
            expires_at,
        })
    }

    pub fn validate_current(&self, current: &Self, now: i64) -> AppResult<()> {
        if self.expires_at <= now || current.expires_at <= now {
            return Err(AppError::InvalidInput(
                "Local execution freshness has expired.".into(),
            ));
        }
        self.local_runtime_ref
            .validate_current(&current.local_runtime_ref)?;
        if self != current {
            return Err(AppError::InvalidInput(
                "Local execution freshness is stale or mismatched.".into(),
            ));
        }
        Ok(())
    }
}

/// Runtime freshness used by shared managed admission and execution code.
/// Remote serialization remains the exact HostSessionBinding representation;
/// the local representation contains no synthetic peer/session fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum HostExecutionFreshness {
    Local(LocalExecutionFreshness),
    Remote(HostSessionBinding),
}

impl HostExecutionFreshness {
    pub fn bridge_id(&self) -> &str {
        match self {
            Self::Local(freshness) => &freshness.bridge_id,
            Self::Remote(binding) => &binding.bridge_id,
        }
    }

    pub fn local_host_ref(&self) -> &HostRef {
        match self {
            Self::Local(freshness) => freshness.local_runtime_ref.host_ref(),
            Self::Remote(binding) => &binding.local_host_ref,
        }
    }

    pub fn expires_at(&self) -> i64 {
        match self {
            Self::Local(freshness) => freshness.expires_at,
            Self::Remote(binding) => binding.expires_at,
        }
    }

    pub fn authority_ref(&self) -> &str {
        match self {
            Self::Local(freshness) => freshness.local_runtime_ref.generation_ref(),
            Self::Remote(binding) => &binding.binding_ref,
        }
    }

    pub fn as_remote(&self) -> Option<&HostSessionBinding> {
        match self {
            Self::Local(_) => None,
            Self::Remote(binding) => Some(binding),
        }
    }

    pub fn validate_current(&self, current: &Self, now: i64) -> AppResult<()> {
        match (self, current) {
            (Self::Local(captured), Self::Local(current)) => {
                captured.validate_current(current, now)
            }
            (Self::Remote(captured), Self::Remote(current)) => {
                captured.validate_current(current, now)
            }
            _ => Err(AppError::InvalidInput(
                "Host execution freshness kind is stale or mismatched.".into(),
            )),
        }
    }
}

impl From<HostSessionBinding> for HostExecutionFreshness {
    fn from(binding: HostSessionBinding) -> Self {
        Self::Remote(binding)
    }
}

impl From<&HostSessionBinding> for HostExecutionFreshness {
    fn from(binding: &HostSessionBinding) -> Self {
        Self::Remote(binding.clone())
    }
}

impl From<&HostExecutionFreshness> for HostExecutionFreshness {
    fn from(freshness: &HostExecutionFreshness) -> Self {
        freshness.clone()
    }
}

/// Exact, expiring association between logical Hosts and one current Layer 4
/// Bridge/session route. It is evidence for later admission checks, never
/// consent, capability, or Layer 5 authority by itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostSessionBinding {
    pub bridge_id: String,
    pub local_host_ref: HostRef,
    pub peer_host_ref: HostRef,
    pub local_session_ref: String,
    pub peer_session_ref: String,
    pub peer_route_ref: String,
    pub expires_at: i64,
    pub binding_ref: String,
    /// Symmetric, non-authoritative correlation for the two endpoints of an
    /// exact Bridge session. Unlike `binding_ref`, this deliberately excludes
    /// direction and route so opposite sides can compare the same wire value.
    #[serde(default)]
    pub session_pair_ref: String,
}

impl HostSessionBinding {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bridge_id: &str,
        local_host_ref: HostRef,
        peer_host_ref: HostRef,
        local_session_ref: &str,
        peer_session_ref: &str,
        peer_route_ref: &str,
        expires_at: i64,
    ) -> AppResult<Self> {
        if bridge_id.trim().is_empty()
            || local_session_ref.trim().is_empty()
            || peer_session_ref.trim().is_empty()
            || peer_route_ref.trim().is_empty()
            || expires_at <= 0
        {
            return Err(AppError::InvalidInput(
                "Host session binding is incomplete.".into(),
            ));
        }
        if local_host_ref == peer_host_ref {
            return Err(AppError::InvalidInput(
                "Host session binding identities must be distinct.".into(),
            ));
        }

        let session_pair_ref = session_pair_ref(
            bridge_id,
            &local_host_ref,
            local_session_ref,
            &peer_host_ref,
            peer_session_ref,
        );
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pastey-host-session-binding-v1\0");
        for value in [
            bridge_id,
            local_host_ref.as_str(),
            peer_host_ref.as_str(),
            local_session_ref,
            peer_session_ref,
            peer_route_ref,
        ] {
            hasher.update(value.as_bytes());
            hasher.update(b"\0");
        }
        hasher.update(&expires_at.to_le_bytes());

        Ok(Self {
            bridge_id: bridge_id.to_string(),
            local_host_ref,
            peer_host_ref,
            local_session_ref: local_session_ref.to_string(),
            peer_session_ref: peer_session_ref.to_string(),
            peer_route_ref: peer_route_ref.to_string(),
            expires_at,
            binding_ref: format!("host-session-binding:v1:{}", hasher.finalize().to_hex()),
            session_pair_ref,
        })
    }

    /// Validates a previously captured binding against a freshly resolved
    /// current binding. Exact equality deliberately makes reconnect/restart,
    /// route replacement, identity mismatch, expiry, and Burn fail closed.
    pub fn validate_current(&self, current: &Self, now: i64) -> AppResult<()> {
        if self.expires_at <= now || current.expires_at <= now {
            return Err(AppError::InvalidInput(
                "Host session binding has expired.".into(),
            ));
        }
        if self != current {
            return Err(AppError::InvalidInput(
                "Host session binding is stale or mismatched.".into(),
            ));
        }
        Ok(())
    }
}

fn session_pair_ref(
    bridge_id: &str,
    first_host_ref: &HostRef,
    first_session_ref: &str,
    second_host_ref: &HostRef,
    second_session_ref: &str,
) -> String {
    let mut endpoints = [
        (first_host_ref.as_str(), first_session_ref),
        (second_host_ref.as_str(), second_session_ref),
    ];
    endpoints.sort_unstable();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pastey-host-session-pair-ref-v1\0");
    hasher.update(bridge_id.as_bytes());
    hasher.update(b"\0");
    for (host_ref, session_ref) in endpoints {
        hasher.update(host_ref.as_bytes());
        hasher.update(b"\0");
        hasher.update(session_ref.as_bytes());
        hasher.update(b"\0");
    }
    format!("host-session-pair:v1:{}", hasher.finalize().to_hex())
}

/// Role-neutral projection for a legacy Plan device token. It is deliberately
/// outside BridgePlanRevision v1 and therefore cannot affect its hash or wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPlanParticipantProjection {
    pub participant_ref: PlanParticipantRef,
    pub legacy_device_ref: String,
    pub host_ref: Option<HostRef>,
}

pub fn project_legacy_plan_participants(
    plan_id: &str,
    device_refs: impl IntoIterator<Item = String>,
    resolved_hosts: &BTreeMap<String, HostRef>,
) -> AppResult<Vec<LegacyPlanParticipantProjection>> {
    let mut seen = HashSet::new();
    let mut claimed_hosts = HashSet::new();
    let mut projections = Vec::new();
    for legacy_device_ref in device_refs {
        if !seen.insert(legacy_device_ref.clone()) {
            continue;
        }
        let host_ref = resolved_hosts.get(&legacy_device_ref).cloned();
        if let Some(host_ref) = &host_ref {
            if !claimed_hosts.insert(host_ref.clone()) {
                return Err(AppError::InvalidInput(
                    "A HostRef cannot be claimed by multiple Plan participants.".into(),
                ));
            }
        }
        let participant_ref = if let Some(host_ref) = &host_ref {
            PlanParticipantRef::for_host(plan_id, host_ref)?
        } else {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"pastey-legacy-plan-participant-ref-v1\0");
            hasher.update(plan_id.as_bytes());
            hasher.update(b"\0");
            hasher.update(legacy_device_ref.as_bytes());
            PlanParticipantRef(format!(
                "{PARTICIPANT_REF_PREFIX}{}",
                hasher.finalize().to_hex()
            ))
        };
        projections.push(LegacyPlanParticipantProjection {
            participant_ref,
            legacy_device_ref,
            host_ref,
        });
    }
    Ok(projections)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(device: &str) -> HostRef {
        HostRef::from_device_id(device).unwrap()
    }

    fn binding(peer_device: &str, peer_session: &str, route: &str) -> HostSessionBinding {
        HostSessionBinding::new(
            "bridge",
            host("local"),
            host(peer_device),
            "local-session",
            peer_session,
            route,
            100,
        )
        .unwrap()
    }

    #[test]
    fn host_ref_is_durable_and_not_a_transport_or_pairing_token() {
        let first = host("installation-id");
        let second = host("installation-id");
        assert_eq!(first, second);
        assert!(first.as_str().starts_with(HOST_REF_PREFIX));
        assert!(!first.as_str().contains("room-session"));
        assert!(!first.as_str().contains("paired-device"));
        assert!(HostRef::parse(first.as_str()).is_ok());
        assert!(HostRef::parse("room-session:temporary").is_err());
        assert!(HostRef::parse_peer(first.as_str(), &first).is_err());
    }

    #[test]
    fn duplicate_host_ref_claims_are_rejected() {
        let duplicate = host("same-host");
        assert!(PlanParticipants::new("plan", [duplicate.clone(), duplicate.clone()]).is_err());

        let resolved = BTreeMap::from([
            ("legacy-a".to_string(), duplicate.clone()),
            ("legacy-b".to_string(), duplicate),
        ]);
        assert!(project_legacy_plan_participants(
            "plan",
            ["legacy-a".to_string(), "legacy-b".to_string()],
            &resolved,
        )
        .is_err());
    }

    #[test]
    fn plan_participant_refs_are_role_neutral_and_plan_scoped() {
        let alpha = host("alpha");
        let beta = host("beta");
        let first = PlanParticipants::new("plan-a", [alpha.clone(), beta.clone()]).unwrap();
        let second = PlanParticipants::new("plan-b", [alpha, beta]).unwrap();
        assert_eq!(first.as_slice().len(), 2);
        assert_ne!(
            first.as_slice()[0].participant_ref,
            second.as_slice()[0].participant_ref
        );
    }

    #[test]
    fn host_session_binding_contains_no_trust_or_admission_claim() {
        let encoded = serde_json::to_value(binding("peer", "peer-session", "route")).unwrap();
        let object = encoded.as_object().unwrap();
        for forbidden in [
            "trusted",
            "trust",
            "admitted",
            "admission",
            "approved",
            "capabilities",
            "authority",
            "grant",
        ] {
            assert!(!object.contains_key(forbidden));
        }
        assert_eq!(object.len(), 9);
    }

    #[test]
    fn opposite_session_bindings_remain_directional_but_share_wire_correlation() {
        let alpha = host("alpha");
        let beta = host("beta");
        let alpha_view = HostSessionBinding::new(
            "bridge",
            alpha.clone(),
            beta.clone(),
            "alpha-session",
            "beta-session",
            "route-to-beta",
            100,
        )
        .unwrap();
        let beta_view = HostSessionBinding::new(
            "bridge",
            beta,
            alpha,
            "beta-session",
            "alpha-session",
            "route-to-alpha",
            100,
        )
        .unwrap();

        assert_ne!(alpha_view.binding_ref, beta_view.binding_ref);
        assert_eq!(alpha_view.session_pair_ref, beta_view.session_pair_ref);
        assert!(alpha_view.validate_current(&beta_view, 1).is_err());
    }

    #[test]
    fn session_pair_correlation_is_bridge_and_session_exact_but_route_neutral() {
        let captured = binding("peer", "peer-session", "route-a");
        let route_changed = binding("peer", "peer-session", "route-b");
        let session_changed = binding("peer", "new-peer-session", "route-a");
        let other_bridge = HostSessionBinding::new(
            "other-bridge",
            host("local"),
            host("peer"),
            "local-session",
            "peer-session",
            "route-a",
            100,
        )
        .unwrap();

        assert_eq!(captured.session_pair_ref, route_changed.session_pair_ref);
        assert_ne!(captured.binding_ref, route_changed.binding_ref);
        assert_ne!(captured.session_pair_ref, session_changed.session_pair_ref);
        assert_ne!(captured.session_pair_ref, other_bridge.session_pair_ref);
        assert!(captured.validate_current(&session_changed, 1).is_err());
    }

    #[test]
    fn local_runtime_freshness_is_host_and_process_exact_without_peer_fields() {
        let local = host("local");
        let captured = LocalRuntimeRef::fresh(local.clone());
        let restarted = LocalRuntimeRef::fresh(local);
        let captured = LocalExecutionFreshness::new("bridge", captured, 100).unwrap();
        let restarted = LocalExecutionFreshness::new("bridge", restarted, 100).unwrap();

        assert_ne!(
            captured.local_runtime_ref.generation_ref(),
            restarted.local_runtime_ref.generation_ref()
        );
        assert!(captured.validate_current(&restarted, 1).is_err());
        let encoded = serde_json::to_value(&captured).unwrap();
        let object = encoded.as_object().unwrap();
        for forbidden in [
            "peerHostRef",
            "localSessionRef",
            "peerSessionRef",
            "peerRouteRef",
            "sessionPairRef",
        ] {
            assert!(!object.contains_key(forbidden));
        }
    }

    #[test]
    fn identity_or_session_mismatch_fails_closed() {
        let captured = binding("peer-a", "peer-session", "route");
        assert!(captured
            .validate_current(&binding("peer-b", "peer-session", "route"), 1)
            .is_err());
        assert!(captured
            .validate_current(&binding("peer-a", "other-session", "route"), 1)
            .is_err());
    }

    #[test]
    fn disconnect_reconnect_replaces_the_binding() {
        let disconnected = binding("peer", "session-a", "route-a");
        let reconnected = binding("peer", "session-b", "route-b");
        assert_ne!(disconnected.binding_ref, reconnected.binding_ref);
        assert!(disconnected.validate_current(&reconnected, 1).is_err());
        assert!(reconnected.validate_current(&reconnected, 1).is_ok());
    }

    #[test]
    fn expired_binding_models_restart_or_burn_as_unavailable() {
        let captured = binding("peer", "session", "route");
        assert!(captured.validate_current(&captured, 100).is_err());
    }
}

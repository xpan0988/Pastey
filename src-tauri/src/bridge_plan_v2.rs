//! Native participant-based Bridge Plan schema and protocol v2.
//!
//! This module is deliberately parallel to `bridge_plan` v1. No v1 value is
//! deserialized, projected, re-hashed, or accepted as v2 authority. V2 keeps
//! logical Host topology in the immutable Plan and uses Layer 4 bindings only
//! when delivering exact Host-bound work.

#![allow(dead_code)] // The v2 Core/protocol seam is intentionally not exposed through the v1 UI.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    bridge_plan::StepOperation,
    error::{AppError, AppResult},
    host_admission::{
        HostAdmissionDecision, HostAdmissionRequestV2, HostAdmissionService,
        ManagedPrimitiveAvailabilityV1,
    },
    host_identity::{
        HostRef, HostSessionBinding, PlanParticipant, PlanParticipantRef, PlanParticipants,
    },
    storage::AppPaths,
};

pub(crate) const PLAN_SCHEMA_VERSION: &str = "bridge-plan-v2";
pub(crate) const PROTOCOL_VERSION: &str = "pastey-bridge-plan-protocol-v2";
const HASH_VERSION: &str = "bridge-plan-revision-hash-v2";
const MAX_ID_LEN: usize = 128;
const MAX_TEXT_LEN: usize = 1_024;
const MAX_STEPS: usize = 64;
const MAX_PARTICIPANTS: usize = 32;
const MAX_ROOTS: usize = 64;
const MAX_DEPENDENCIES: usize = 64;
const MAX_LIFETIME: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedObjectRevisionV2 {
    pub(crate) logical_object_id: String,
    pub(crate) revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlanRootV2 {
    pub(crate) root_id: String,
    pub(crate) object: ManagedObjectRevisionV2,
    pub(crate) host: PlanParticipantRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum PlanStepV2 {
    Search {
        step_id: String,
        depends_on: Vec<String>,
        host: PlanParticipantRef,
        output: ManagedObjectRevisionV2,
        query: String,
        safe_scope_labels: Vec<String>,
    },
    Transform {
        step_id: String,
        depends_on: Vec<String>,
        host: PlanParticipantRef,
        input: ManagedObjectRevisionV2,
        output: ManagedObjectRevisionV2,
        modification_intent: String,
    },
    Transfer {
        step_id: String,
        depends_on: Vec<String>,
        source: PlanParticipantRef,
        destination: PlanParticipantRef,
        input: ManagedObjectRevisionV2,
        output: ManagedObjectRevisionV2,
    },
    Execute {
        step_id: String,
        depends_on: Vec<String>,
        host: PlanParticipantRef,
        target: ManagedObjectRevisionV2,
        execution_intent: String,
    },
}

impl PlanStepV2 {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Search { step_id, .. }
            | Self::Transform { step_id, .. }
            | Self::Transfer { step_id, .. }
            | Self::Execute { step_id, .. } => step_id,
        }
    }

    pub(crate) fn dependencies(&self) -> &[String] {
        match self {
            Self::Search { depends_on, .. }
            | Self::Transform { depends_on, .. }
            | Self::Transfer { depends_on, .. }
            | Self::Execute { depends_on, .. } => depends_on,
        }
    }

    pub(crate) fn operation(&self) -> StepOperation {
        match self {
            Self::Search { .. } => StepOperation::Search,
            Self::Transform { .. } => StepOperation::Transform,
            Self::Transfer { .. } => StepOperation::Transfer,
            Self::Execute { .. } => StepOperation::Execute,
        }
    }

    pub(crate) fn binds_participant(&self, participant: &PlanParticipantRef) -> bool {
        match self {
            Self::Search { host, .. }
            | Self::Transform { host, .. }
            | Self::Execute { host, .. } => host == participant,
            Self::Transfer {
                source,
                destination,
                ..
            } => source == participant || destination == participant,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlanRevisionV2 {
    pub(crate) schema_version: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_number: u32,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) requester: PlanParticipantRef,
    pub(crate) participants: PlanParticipants,
    pub(crate) roots: Vec<PlanRootV2>,
    pub(crate) original_user_goal: String,
    pub(crate) expected_outcome: String,
    pub(crate) steps: Vec<PlanStepV2>,
}

#[derive(Serialize)]
struct SemanticRevisionV2<'a> {
    schema_version: &'a str,
    plan_id: &'a str,
    revision_id: &'a str,
    revision_number: u32,
    bridge_id: &'a str,
    requester: &'a PlanParticipantRef,
    participants: &'a PlanParticipants,
    roots: &'a [PlanRootV2],
    original_user_goal: &'a str,
    expected_outcome: &'a str,
    steps: &'a [PlanStepV2],
}

impl<'a> From<&'a PlanRevisionV2> for SemanticRevisionV2<'a> {
    fn from(revision: &'a PlanRevisionV2) -> Self {
        Self {
            schema_version: &revision.schema_version,
            plan_id: &revision.plan_id,
            revision_id: &revision.revision_id,
            revision_number: revision.revision_number,
            bridge_id: &revision.bridge_id,
            requester: &revision.requester,
            participants: &revision.participants,
            roots: &revision.roots,
            original_user_goal: &revision.original_user_goal,
            expected_outcome: &revision.expected_outcome,
            steps: &revision.steps,
        }
    }
}

#[derive(Clone)]
struct ObjectState {
    revision: u64,
    host: PlanParticipantRef,
    producer: Option<String>,
}

pub(crate) fn validate_revision(revision: &PlanRevisionV2) -> AppResult<()> {
    if revision.schema_version != PLAN_SCHEMA_VERSION {
        return invalid("Bridge Plan v2 requires its exact schema version.");
    }
    id(&revision.plan_id, "plan id")?;
    id(&revision.revision_id, "revision id")?;
    id(&revision.bridge_id, "bridge id")?;
    text(&revision.original_user_goal, "original user goal")?;
    text(&revision.expected_outcome, "expected outcome")?;
    if revision.revision_number == 0 {
        return invalid("Bridge Plan v2 revision number must be positive.");
    }
    let participants = revision.participants.as_slice();
    if participants.is_empty() || participants.len() > MAX_PARTICIPANTS {
        return invalid("Bridge Plan v2 has an invalid participant count.");
    }
    let mut by_ref = BTreeMap::new();
    let mut hosts = HashSet::new();
    for participant in participants {
        let expected = PlanParticipantRef::for_host(&revision.plan_id, &participant.host_ref)?;
        if participant.participant_ref != expected {
            return invalid("Bridge Plan v2 participant does not match its Plan and HostRef.");
        }
        if by_ref
            .insert(
                participant.participant_ref.clone(),
                participant.host_ref.clone(),
            )
            .is_some()
            || !hosts.insert(participant.host_ref.clone())
        {
            return invalid("Bridge Plan v2 contains a duplicate participant or HostRef claim.");
        }
    }
    if !by_ref.contains_key(&revision.requester) {
        return invalid("Bridge Plan v2 requester is not a Plan participant.");
    }
    if revision.roots.len() > MAX_ROOTS
        || revision.steps.is_empty()
        || revision.steps.len() > MAX_STEPS
    {
        return invalid("Bridge Plan v2 has an invalid root or step count.");
    }

    let mut root_ids = HashSet::new();
    let mut objects = HashMap::<String, ObjectState>::new();
    for root in &revision.roots {
        id(&root.root_id, "root id")?;
        validate_object(&root.object)?;
        participant(&by_ref, &root.host)?;
        if !root_ids.insert(root.root_id.as_str()) {
            return invalid("Bridge Plan v2 contains duplicate root ids.");
        }
        if objects
            .insert(
                root.object.logical_object_id.clone(),
                ObjectState {
                    revision: root.object.revision,
                    host: root.host.clone(),
                    producer: None,
                },
            )
            .is_some()
        {
            return invalid("Bridge Plan v2 roots claim the same logical object twice.");
        }
    }

    let mut step_ids = HashSet::<String>::new();
    for step in &revision.steps {
        id(step.id(), "step id")?;
        if !step_ids.insert(step.id().to_string()) {
            return invalid("Bridge Plan v2 contains duplicate step ids.");
        }
        if step.dependencies().len() > MAX_DEPENDENCIES
            || step.dependencies().iter().collect::<HashSet<_>>().len() != step.dependencies().len()
        {
            return invalid("Bridge Plan v2 has invalid step dependencies.");
        }
        for dependency in step.dependencies() {
            id(dependency, "step dependency")?;
            if dependency == step.id() || !step_ids.contains(dependency) {
                return invalid("Bridge Plan v2 dependencies must name preceding steps.");
            }
        }

        match step {
            PlanStepV2::Search {
                host,
                output,
                query,
                safe_scope_labels,
                ..
            } => {
                participant(&by_ref, host)?;
                validate_object(output)?;
                text(query, "Search query")?;
                if output.revision != 1 || safe_scope_labels.is_empty() {
                    return invalid(
                        "Bridge Plan v2 Search must establish revision 1 in reviewed scopes.",
                    );
                }
                for scope in safe_scope_labels {
                    text(scope, "Search scope")?;
                }
                if objects
                    .insert(
                        output.logical_object_id.clone(),
                        ObjectState {
                            revision: output.revision,
                            host: host.clone(),
                            producer: Some(step.id().to_string()),
                        },
                    )
                    .is_some()
                {
                    return invalid("Bridge Plan v2 Search output already exists.");
                }
            }
            PlanStepV2::Transform {
                host,
                input,
                output,
                modification_intent,
                ..
            } => {
                participant(&by_ref, host)?;
                validate_object(input)?;
                validate_object(output)?;
                text(modification_intent, "Transform intent")?;
                let state = current_object(&objects, input)?;
                require_dependency(step, &state.producer)?;
                if &state.host != host {
                    return invalid("Bridge Plan v2 Transform would move an object implicitly.");
                }
                if output.logical_object_id != input.logical_object_id
                    || output.revision
                        != input.revision.checked_add(1).ok_or_else(|| {
                            AppError::InvalidInput("Bridge Plan v2 revision overflowed.".into())
                        })?
                {
                    return invalid("Bridge Plan v2 Transform must produce exact N+1 in place.");
                }
                objects.insert(
                    output.logical_object_id.clone(),
                    ObjectState {
                        revision: output.revision,
                        host: host.clone(),
                        producer: Some(step.id().to_string()),
                    },
                );
            }
            PlanStepV2::Transfer {
                source,
                destination,
                input,
                output,
                ..
            } => {
                participant(&by_ref, source)?;
                participant(&by_ref, destination)?;
                validate_object(input)?;
                validate_object(output)?;
                let state = current_object(&objects, input)?;
                require_dependency(step, &state.producer)?;
                if &state.host != source || source == destination {
                    return invalid("Bridge Plan v2 Transfer source or destination is invalid.");
                }
                if output != input {
                    return invalid(
                        "Bridge Plan v2 Transfer must preserve the exact logical revision.",
                    );
                }
                objects.insert(
                    output.logical_object_id.clone(),
                    ObjectState {
                        revision: output.revision,
                        host: destination.clone(),
                        producer: Some(step.id().to_string()),
                    },
                );
            }
            PlanStepV2::Execute {
                host,
                target,
                execution_intent,
                ..
            } => {
                participant(&by_ref, host)?;
                validate_object(target)?;
                text(execution_intent, "Execute intent")?;
                let state = current_object(&objects, target)?;
                require_dependency(step, &state.producer)?;
                if &state.host != host {
                    return invalid("Bridge Plan v2 Execute would consume at the wrong Host.");
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn canonical_revision_hash(revision: &PlanRevisionV2) -> AppResult<String> {
    validate_revision(revision)?;
    let canonical = canonical_json(&serde_json::to_value(SemanticRevisionV2::from(revision))?);
    Ok(format!(
        "{HASH_VERSION}:{}",
        blake3::hash(format!("{HASH_VERSION}\0{canonical}").as_bytes()).to_hex()
    ))
}

pub(crate) fn seal_revision(mut revision: PlanRevisionV2) -> AppResult<PlanRevisionV2> {
    if !revision.revision_hash.is_empty() {
        return invalid("Bridge Plan v2 sealing requires an unhashed revision.");
    }
    revision.revision_hash = canonical_revision_hash(&revision)?;
    Ok(revision)
}

pub(crate) fn verify_sealed_revision(revision: &PlanRevisionV2) -> AppResult<()> {
    if !revision
        .revision_hash
        .starts_with("bridge-plan-revision-hash-v2:")
        || canonical_revision_hash(revision)? != revision.revision_hash
    {
        return invalid("Bridge Plan v2 semantic hash does not match its immutable revision.");
    }
    Ok(())
}

pub(crate) fn participant_for_ref<'a>(
    revision: &'a PlanRevisionV2,
    participant_ref: &PlanParticipantRef,
) -> Option<&'a PlanParticipant> {
    revision
        .participants
        .as_slice()
        .iter()
        .find(|participant| &participant.participant_ref == participant_ref)
}

pub(crate) fn requester_host(revision: &PlanRevisionV2) -> AppResult<&HostRef> {
    participant_for_ref(revision, &revision.requester)
        .map(|participant| &participant.host_ref)
        .ok_or_else(|| AppError::InvalidInput("Bridge Plan v2 requester is unavailable.".into()))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlanApprovalV2 {
    pub(crate) approval_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) requester: PlanParticipantRef,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReviewRequestV2 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
    pub(crate) correlation_id: String,
    pub(crate) request_nonce: String,
    pub(crate) sender: PlanParticipantRef,
    pub(crate) target: PlanParticipantRef,
    pub(crate) approval: PlanApprovalV2,
    pub(crate) revision: PlanRevisionV2,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AttemptStartV2 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
    pub(crate) correlation_id: String,
    pub(crate) request_nonce: String,
    pub(crate) attempt_id: String,
    pub(crate) approval_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) sender: PlanParticipantRef,
    pub(crate) target: PlanParticipantRef,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AcceptedAttemptV2 {
    pub(crate) attempt_id: String,
    pub(crate) admission_ref: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AttemptStartDecisionV2 {
    Accepted(AcceptedAttemptV2),
    Denied(HostAdmissionDecision),
}

pub(crate) struct BridgePlanV2Store<'a> {
    paths: &'a AppPaths,
}

impl<'a> BridgePlanV2Store<'a> {
    pub(crate) fn new(paths: &'a AppPaths) -> Self {
        Self { paths }
    }

    pub(crate) fn record_review(
        &self,
        review: &ReviewRequestV2,
        current_binding: &HostSessionBinding,
        now: i64,
    ) -> AppResult<()> {
        validate_review(review, now)?;
        ensure_active_bridge(self.paths, &review.revision.bridge_id)?;
        validate_review_binding(review, current_binding, now)?;
        let mut conn = connection(self.paths)?;
        let tx = conn.transaction()?;
        let revision_json = serde_json::to_string(&review.revision)?;
        let approval_json = serde_json::to_string(&review.approval)?;
        tx.execute(
            "INSERT INTO bridge_plan_v2_revisions (revision_id, plan_id, bridge_id, revision_number, revision_hash, requester_participant_ref, created_at, revision_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![review.revision.revision_id, review.revision.plan_id, review.revision.bridge_id, review.revision.revision_number, review.revision.revision_hash, review.revision.requester.as_str(), now, revision_json],
        )?;
        tx.execute(
            "INSERT INTO bridge_plan_v2_approvals (approval_id, plan_id, revision_id, bridge_id, revision_hash, requester_participant_ref, expires_at, state, created_at, approval_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'valid', ?8, ?9)",
            params![review.approval.approval_id, review.approval.plan_id, review.approval.revision_id, review.approval.bridge_id, review.approval.revision_hash, review.approval.requester.as_str(), review.approval.expires_at, now, approval_json],
        )?;
        tx.execute(
            "INSERT INTO bridge_plan_v2_protocol_reviews (bridge_id, message_id, correlation_id, request_nonce, approval_id, plan_id, revision_id, revision_hash, sender_participant_ref, target_participant_ref, expires_at, review_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![review.revision.bridge_id, review.message_id, review.correlation_id, review.request_nonce, review.approval.approval_id, review.revision.plan_id, review.revision.revision_id, review.revision.revision_hash, review.sender.as_str(), review.target.as_str(), review.approval.expires_at, serde_json::to_string(review)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Read-only coordinator projection used solely to derive a Host-owned
    /// whole-Plan availability snapshot before attempt authority is accepted.
    pub(crate) fn reviewed_revision_for_start(
        &self,
        start: &AttemptStartV2,
        now: i64,
    ) -> AppResult<PlanRevisionV2> {
        validate_attempt_start(start, now)?;
        let review: ReviewRequestV2 = connection(self.paths)?
            .query_row(
                "SELECT review_json FROM bridge_plan_v2_protocol_reviews
                 WHERE bridge_id = ?1 AND correlation_id = ?2",
                params![start.bridge_id, start.correlation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .ok_or_else(|| {
                AppError::InvalidInput("Bridge Plan v2 review correlation is unavailable.".into())
            })?;
        if start.request_nonce != review.request_nonce
            || start.approval_id != review.approval.approval_id
            || start.plan_id != review.revision.plan_id
            || start.revision_id != review.revision.revision_id
            || start.revision_hash != review.revision.revision_hash
            || start.bridge_id != review.revision.bridge_id
            || start.sender != review.sender
            || start.target != review.target
            || start.expires_at > review.approval.expires_at
        {
            return invalid("Bridge Plan v2 attempt does not match the exact reviewed Plan.");
        }
        verify_sealed_revision(&review.revision)?;
        Ok(review.revision)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn accept_attempt_start(
        &self,
        start: &AttemptStartV2,
        captured_binding: &HostSessionBinding,
        current_binding: &HostSessionBinding,
        admission_service: &HostAdmissionService,
        now: i64,
    ) -> AppResult<AttemptStartDecisionV2> {
        self.accept_attempt_start_with_availability(
            start,
            captured_binding,
            current_binding,
            admission_service,
            ManagedPrimitiveAvailabilityV1::unavailable(),
            now,
        )
    }

    /// Core-only Step 8 admission attachment. The protocol-facing path above
    /// always supplies an unavailable snapshot and therefore retains the
    /// Phase 4 whole-Plan denial. No availability fact is read from wire data.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn accept_attempt_start_with_availability(
        &self,
        start: &AttemptStartV2,
        captured_binding: &HostSessionBinding,
        current_binding: &HostSessionBinding,
        admission_service: &HostAdmissionService,
        availability: ManagedPrimitiveAvailabilityV1,
        now: i64,
    ) -> AppResult<AttemptStartDecisionV2> {
        validate_attempt_start(start, now)?;
        ensure_active_bridge(self.paths, &start.bridge_id)?;
        let conn = connection(self.paths)?;
        let stored = conn
            .query_row(
                "SELECT reviews.review_json, approvals.approval_json, approvals.state, revisions.revision_json
                 FROM bridge_plan_v2_protocol_reviews AS reviews
                 JOIN bridge_plan_v2_approvals AS approvals ON approvals.approval_id = reviews.approval_id
                 JOIN bridge_plan_v2_revisions AS revisions ON revisions.revision_id = reviews.revision_id
                 WHERE reviews.bridge_id = ?1 AND reviews.correlation_id = ?2",
                params![start.bridge_id, start.correlation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::InvalidInput("Bridge Plan v2 review correlation is unavailable.".into()))?;
        let review: ReviewRequestV2 = serde_json::from_str(&stored.0)?;
        let approval: PlanApprovalV2 = serde_json::from_str(&stored.1)?;
        let revision: PlanRevisionV2 = serde_json::from_str(&stored.3)?;
        if stored.2 != "valid" || approval != review.approval || revision != review.revision {
            return invalid("Bridge Plan v2 stored approval or revision is unavailable.");
        }
        if start.request_nonce != review.request_nonce
            || start.approval_id != review.approval.approval_id
            || start.plan_id != review.revision.plan_id
            || start.revision_id != review.revision.revision_id
            || start.revision_hash != review.revision.revision_hash
            || start.bridge_id != review.revision.bridge_id
            || start.sender != review.sender
            || start.target != review.target
            || start.expires_at > review.approval.expires_at
        {
            return invalid("Bridge Plan v2 attempt does not match the exact reviewed Plan.");
        }

        // Claim the authenticated v2 event before admission. A stale-binding
        // denial requires a fresh event/nonce and cannot be replayed after a
        // reconnect. V1 replay keys live in separate tables and namespaces.
        conn.execute(
            "INSERT INTO bridge_plan_v2_protocol_messages (bridge_id, message_id, request_nonce, correlation_id, received_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![start.bridge_id, start.message_id, start.request_nonce, start.correlation_id, now],
        )?;

        let request = HostAdmissionRequestV2 {
            approval_id: start.approval_id.clone(),
            plan_id: start.plan_id.clone(),
            revision_id: start.revision_id.clone(),
            revision_hash: start.revision_hash.clone(),
            host_ref: captured_binding.local_host_ref.clone(),
            participant_ref: start.target.clone(),
            protocol_correlation_id: start.correlation_id.clone(),
            session_binding: captured_binding.clone(),
        };
        let decision = admission_service.evaluate_v2_with_availability(
            &review.revision,
            &review.approval,
            &request,
            current_binding,
            availability,
            now,
        )?;
        let Some(admission) = decision.admitted() else {
            return Ok(AttemptStartDecisionV2::Denied(decision));
        };
        conn.execute(
            "INSERT INTO bridge_plan_v2_attempts (attempt_id, bridge_id, approval_id, plan_id, revision_id, revision_hash, target_participant_ref, correlation_id, session_binding_ref, admission_ref, expires_at, state, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'accepted', ?12)",
            params![start.attempt_id, start.bridge_id, start.approval_id, start.plan_id, start.revision_id, start.revision_hash, start.target.as_str(), start.correlation_id, captured_binding.binding_ref, admission.admission_ref, start.expires_at, now],
        )?;
        Ok(AttemptStartDecisionV2::Accepted(AcceptedAttemptV2 {
            attempt_id: start.attempt_id.clone(),
            admission_ref: admission.admission_ref.clone(),
        }))
    }

    #[cfg(test)]
    fn attempt_state(&self, attempt_id: &str) -> AppResult<Option<String>> {
        Ok(connection(self.paths)?
            .query_row(
                "SELECT state FROM bridge_plan_v2_attempts WHERE attempt_id = ?1",
                [attempt_id],
                |row| row.get(0),
            )
            .optional()?)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProtocolMetadataV2 {
    pub(crate) replay_id: String,
}

pub(crate) fn protocol_metadata(
    kind: &str,
    payload: &serde_json::Map<String, Value>,
    expected_bridge: &str,
    now: i64,
) -> AppResult<ProtocolMetadataV2> {
    let payload = Value::Object(payload.clone());
    match kind {
        "bridge_plan.v2.review_request" => {
            let review: ReviewRequestV2 = serde_json::from_value(payload)?;
            validate_review(&review, now)?;
            if review.revision.bridge_id != expected_bridge {
                return invalid("Bridge Plan v2 review crossed Bridge scope.");
            }
            Ok(ProtocolMetadataV2 {
                replay_id: format!("v2:review:{}", review.message_id),
            })
        }
        "bridge_plan.v2.attempt_start" => {
            let start: AttemptStartV2 = serde_json::from_value(payload)?;
            validate_attempt_start(&start, now)?;
            if start.bridge_id != expected_bridge {
                return invalid("Bridge Plan v2 attempt crossed Bridge scope.");
            }
            Ok(ProtocolMetadataV2 {
                replay_id: format!("v2:start:{}", start.message_id),
            })
        }
        _ => Ok(ProtocolMetadataV2 {
            replay_id: crate::native_v2_orchestration::protocol_replay_id(
                kind,
                payload,
                expected_bridge,
                now,
            )?,
        }),
    }
}

pub(crate) fn init_schema(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_revisions (
            revision_id TEXT PRIMARY KEY, plan_id TEXT NOT NULL, bridge_id TEXT NOT NULL,
            revision_number INTEGER NOT NULL, revision_hash TEXT NOT NULL,
            requester_participant_ref TEXT NOT NULL, created_at INTEGER NOT NULL,
            revision_json TEXT NOT NULL,
            UNIQUE(plan_id, revision_number), UNIQUE(plan_id, revision_hash)
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_approvals (
            approval_id TEXT PRIMARY KEY, plan_id TEXT NOT NULL, revision_id TEXT NOT NULL,
            bridge_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
            requester_participant_ref TEXT NOT NULL, expires_at INTEGER NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('valid','revoked','expired','burned')),
            created_at INTEGER NOT NULL, approval_json TEXT NOT NULL,
            FOREIGN KEY(revision_id) REFERENCES bridge_plan_v2_revisions(revision_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_protocol_reviews (
            bridge_id TEXT NOT NULL, message_id TEXT NOT NULL, correlation_id TEXT NOT NULL,
            request_nonce TEXT NOT NULL, approval_id TEXT NOT NULL, plan_id TEXT NOT NULL,
            revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
            sender_participant_ref TEXT NOT NULL, target_participant_ref TEXT NOT NULL,
            expires_at INTEGER NOT NULL, review_json TEXT NOT NULL,
            PRIMARY KEY(bridge_id, message_id),
            UNIQUE(bridge_id, correlation_id), UNIQUE(bridge_id, request_nonce),
            FOREIGN KEY(approval_id) REFERENCES bridge_plan_v2_approvals(approval_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_protocol_messages (
            bridge_id TEXT NOT NULL, message_id TEXT NOT NULL, request_nonce TEXT NOT NULL,
            correlation_id TEXT NOT NULL, received_at INTEGER NOT NULL,
            PRIMARY KEY(bridge_id, message_id), UNIQUE(bridge_id, request_nonce)
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_attempts (
            attempt_id TEXT PRIMARY KEY, bridge_id TEXT NOT NULL, approval_id TEXT NOT NULL,
            plan_id TEXT NOT NULL, revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
            target_participant_ref TEXT NOT NULL, correlation_id TEXT NOT NULL,
            session_binding_ref TEXT NOT NULL, admission_ref TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('accepted','interrupted','burned')),
            created_at INTEGER NOT NULL,
            UNIQUE(bridge_id, correlation_id, target_participant_ref),
            FOREIGN KEY(approval_id) REFERENCES bridge_plan_v2_approvals(approval_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_managed_step_claims (
            attempt_id TEXT NOT NULL, step_id TEXT NOT NULL,
            operation TEXT NOT NULL CHECK(operation IN ('transform','execute')),
            context_ref TEXT NOT NULL UNIQUE, envelope_ref TEXT NOT NULL UNIQUE,
            run_control_ref TEXT NOT NULL UNIQUE, evidence_head TEXT,
            state TEXT NOT NULL CHECK(state IN ('claimed','completed','failed','interrupted')),
            claimed_at INTEGER NOT NULL, completed_at INTEGER,
            PRIMARY KEY(attempt_id, step_id),
            FOREIGN KEY(attempt_id) REFERENCES bridge_plan_v2_attempts(attempt_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_transform_results (
            attempt_id TEXT NOT NULL, step_id TEXT NOT NULL,
            logical_object_id TEXT NOT NULL, input_revision INTEGER NOT NULL,
            output_revision INTEGER NOT NULL, host_ref TEXT NOT NULL,
            content_digest TEXT NOT NULL, seal_ref TEXT NOT NULL,
            evidence_head TEXT NOT NULL, result_json TEXT NOT NULL,
            completed_at INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, step_id),
            UNIQUE(logical_object_id, output_revision, host_ref),
            FOREIGN KEY(attempt_id, step_id) REFERENCES bridge_plan_v2_managed_step_claims(attempt_id, step_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_execute_results (
            attempt_id TEXT NOT NULL, step_id TEXT NOT NULL,
            result_ref TEXT NOT NULL UNIQUE, host_ref TEXT NOT NULL,
            input_logical_object_id TEXT NOT NULL, input_revision INTEGER NOT NULL,
            result_schema_ref TEXT NOT NULL, result_digest TEXT NOT NULL,
            evidence_head TEXT NOT NULL, result_json TEXT NOT NULL,
            completed_at INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, step_id),
            FOREIGN KEY(attempt_id, step_id) REFERENCES bridge_plan_v2_managed_step_claims(attempt_id, step_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_worker_attempts (
            attempt_id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL, provider_generation INTEGER NOT NULL,
            provider_config_digest TEXT NOT NULL, provider_model TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN
                ('accepted','running','waiting','completed','failed','interrupted','cancelled')),
            failure_code TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            FOREIGN KEY(attempt_id) REFERENCES bridge_plan_v2_attempts(attempt_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_v2_worker_dispatches (
            attempt_id TEXT NOT NULL, step_id TEXT NOT NULL,
            operation TEXT NOT NULL CHECK(operation IN ('transform','execute')),
            state TEXT NOT NULL CHECK(state IN
                ('dispatching','completed','failed','interrupted','cancelled')),
            failure_code TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, step_id),
            FOREIGN KEY(attempt_id) REFERENCES bridge_plan_v2_attempts(attempt_id) ON DELETE CASCADE
        );
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_revision_immutable
        BEFORE UPDATE ON bridge_plan_v2_revisions
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan v2 revision is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_approval_authority_immutable
        BEFORE UPDATE OF plan_id, revision_id, bridge_id, revision_hash,
            requester_participant_ref, expires_at, created_at, approval_json
        ON bridge_plan_v2_approvals
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan v2 approval authority is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_approval_state_guard
        BEFORE UPDATE OF state ON bridge_plan_v2_approvals
        WHEN NOT (OLD.state = 'valid' AND NEW.state IN ('revoked','expired','burned'))
        BEGIN SELECT RAISE(ABORT, 'Illegal Bridge Plan v2 approval transition'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_attempt_authority_immutable
        BEFORE UPDATE OF bridge_id, approval_id, plan_id, revision_id, revision_hash,
            target_participant_ref, correlation_id, session_binding_ref, admission_ref,
            expires_at, created_at
        ON bridge_plan_v2_attempts
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan v2 attempt authority is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_attempt_state_guard
        BEFORE UPDATE OF state ON bridge_plan_v2_attempts
        WHEN NOT (OLD.state = 'accepted' AND NEW.state IN ('interrupted','burned'))
        BEGIN SELECT RAISE(ABORT, 'Illegal Bridge Plan v2 attempt transition'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_managed_claim_authority_immutable
        BEFORE UPDATE OF attempt_id, step_id, operation, context_ref, envelope_ref,
            run_control_ref, claimed_at
        ON bridge_plan_v2_managed_step_claims
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan v2 managed step claim is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_managed_claim_state_guard
        BEFORE UPDATE OF state ON bridge_plan_v2_managed_step_claims
        WHEN NOT (OLD.state = 'claimed' AND NEW.state IN ('completed','failed','interrupted'))
        BEGIN SELECT RAISE(ABORT, 'Illegal Bridge Plan v2 managed claim transition'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_worker_attempt_authority_immutable
        BEFORE UPDATE OF attempt_id, provider_id, provider_generation,
            provider_config_digest, provider_model, created_at
        ON bridge_plan_v2_worker_attempts
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan v2 Worker binding is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_worker_attempt_state_guard
        BEFORE UPDATE OF state ON bridge_plan_v2_worker_attempts
        WHEN NOT (
            (OLD.state = 'accepted' AND NEW.state IN
                ('running','waiting','completed','failed','interrupted','cancelled')) OR
            (OLD.state = 'running' AND NEW.state IN
                ('waiting','completed','failed','interrupted','cancelled')) OR
            (OLD.state = 'waiting' AND NEW.state IN
                ('running','completed','failed','interrupted','cancelled'))
        )
        BEGIN SELECT RAISE(ABORT, 'Illegal Bridge Plan v2 Worker attempt transition'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_worker_attempt_terminal_guard
        BEFORE UPDATE ON bridge_plan_v2_worker_attempts
        WHEN OLD.state IN ('completed','failed','interrupted','cancelled')
        BEGIN SELECT RAISE(ABORT, 'Terminal Bridge Plan v2 Worker attempt is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_worker_dispatch_authority_immutable
        BEFORE UPDATE OF attempt_id, step_id, operation, created_at
        ON bridge_plan_v2_worker_dispatches
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan v2 Worker dispatch is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_worker_dispatch_state_guard
        BEFORE UPDATE OF state ON bridge_plan_v2_worker_dispatches
        WHEN NOT (OLD.state = 'dispatching' AND NEW.state IN
            ('completed','failed','interrupted','cancelled'))
        BEGIN SELECT RAISE(ABORT, 'Illegal Bridge Plan v2 Worker dispatch transition'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_v2_worker_dispatch_terminal_guard
        BEFORE UPDATE ON bridge_plan_v2_worker_dispatches
        WHEN OLD.state IN ('completed','failed','interrupted','cancelled')
        BEGIN SELECT RAISE(ABORT, 'Terminal Bridge Plan v2 Worker dispatch is immutable'); END;
        "#,
    )?;
    crate::native_v2_orchestration::init_schema(conn)?;
    Ok(())
}

pub(crate) fn reconcile_startup(paths: &AppPaths) -> AppResult<usize> {
    let conn = connection(paths)?;
    let attempts = conn.execute(
        "UPDATE bridge_plan_v2_attempts SET state = 'interrupted' WHERE state = 'accepted'",
        [],
    )?;
    conn.execute(
        "UPDATE bridge_plan_v2_managed_step_claims SET state = 'interrupted' WHERE state = 'claimed'",
        [],
    )?;
    conn.execute(
        "UPDATE bridge_plan_v2_worker_attempts SET state = 'interrupted',
         failure_code = 'host_restarted', updated_at = ?1
         WHERE state IN ('accepted','running','waiting')",
        [crate::storage::now_ts()],
    )?;
    conn.execute(
        "UPDATE bridge_plan_v2_worker_dispatches SET state = 'interrupted',
         failure_code = 'host_restarted', updated_at = ?1
         WHERE state = 'dispatching'",
        [crate::storage::now_ts()],
    )?;
    Ok(attempts
        + crate::native_v2_orchestration::reconcile_startup(paths, crate::storage::now_ts())?)
}

pub(crate) fn delete_bridge_records(tx: &Transaction<'_>, bridge_id: &str) -> AppResult<()> {
    crate::native_v2_orchestration::delete_bridge_records(tx, bridge_id)?;
    tx.execute(
        "DELETE FROM bridge_plan_v2_managed_step_claims WHERE attempt_id IN (SELECT attempt_id FROM bridge_plan_v2_attempts WHERE bridge_id = ?1)",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM bridge_plan_v2_attempts WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM bridge_plan_v2_protocol_messages WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM bridge_plan_v2_protocol_reviews WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM bridge_plan_v2_approvals WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM bridge_plan_v2_revisions WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    Ok(())
}

fn validate_review(review: &ReviewRequestV2, now: i64) -> AppResult<()> {
    if review.protocol_version != PROTOCOL_VERSION {
        return invalid("Bridge Plan protocol v2 requires its exact protocol version.");
    }
    for (value, label) in [
        (&review.message_id, "message id"),
        (&review.correlation_id, "correlation id"),
        (&review.request_nonce, "request nonce"),
    ] {
        id(value, label)?;
    }
    verify_sealed_revision(&review.revision)?;
    validate_approval(&review.approval, &review.revision, now)?;
    if review.sender != review.revision.requester
        || review.target == review.sender
        || participant_for_ref(&review.revision, &review.target).is_none()
    {
        return invalid("Bridge Plan protocol v2 review participants are invalid.");
    }
    Ok(())
}

fn validate_review_binding(
    review: &ReviewRequestV2,
    binding: &HostSessionBinding,
    now: i64,
) -> AppResult<()> {
    if binding.expires_at <= now || binding.bridge_id != review.revision.bridge_id {
        return invalid("Bridge Plan v2 review session binding is unavailable.");
    }
    let sender = participant_for_ref(&review.revision, &review.sender)
        .ok_or_else(|| AppError::InvalidInput("Bridge Plan v2 sender is unavailable.".into()))?;
    let target = participant_for_ref(&review.revision, &review.target)
        .ok_or_else(|| AppError::InvalidInput("Bridge Plan v2 target is unavailable.".into()))?;
    if sender.host_ref != binding.peer_host_ref || target.host_ref != binding.local_host_ref {
        return invalid("Bridge Plan v2 participants do not match the current Host session.");
    }
    Ok(())
}

fn validate_approval(
    approval: &PlanApprovalV2,
    revision: &PlanRevisionV2,
    now: i64,
) -> AppResult<()> {
    id(&approval.approval_id, "approval id")?;
    if approval.plan_id != revision.plan_id
        || approval.revision_id != revision.revision_id
        || approval.revision_hash != revision.revision_hash
        || approval.bridge_id != revision.bridge_id
        || approval.requester != revision.requester
    {
        return invalid("Bridge Plan v2 approval does not match the exact revision.");
    }
    if approval.expires_at <= now || approval.expires_at > now + MAX_LIFETIME {
        return invalid("Bridge Plan v2 approval expiry is invalid.");
    }
    Ok(())
}

fn validate_attempt_start(start: &AttemptStartV2, now: i64) -> AppResult<()> {
    if start.protocol_version != PROTOCOL_VERSION {
        return invalid("Bridge Plan attempt uses the wrong protocol version.");
    }
    for (value, label) in [
        (&start.message_id, "message id"),
        (&start.correlation_id, "correlation id"),
        (&start.request_nonce, "request nonce"),
        (&start.attempt_id, "attempt id"),
        (&start.approval_id, "approval id"),
        (&start.plan_id, "plan id"),
        (&start.revision_id, "revision id"),
        (&start.bridge_id, "bridge id"),
    ] {
        id(value, label)?;
    }
    if start.expires_at <= now || start.expires_at > now + MAX_LIFETIME {
        return invalid("Bridge Plan v2 attempt expiry is invalid.");
    }
    Ok(())
}

fn current_object<'a>(
    objects: &'a HashMap<String, ObjectState>,
    expected: &ManagedObjectRevisionV2,
) -> AppResult<&'a ObjectState> {
    let state = objects.get(&expected.logical_object_id).ok_or_else(|| {
        AppError::InvalidInput(
            "Bridge Plan v2 input has no managed-object root or producer.".into(),
        )
    })?;
    if state.revision != expected.revision {
        return Err(AppError::InvalidInput(
            "Bridge Plan v2 step does not consume the exact current logical revision.".into(),
        ));
    }
    Ok(state)
}

fn require_dependency(step: &PlanStepV2, producer: &Option<String>) -> AppResult<()> {
    if let Some(producer) = producer {
        if !step.dependencies().iter().any(|value| value == producer) {
            return invalid("Bridge Plan v2 input producer is not an explicit dependency.");
        }
    }
    Ok(())
}

fn participant(
    participants: &BTreeMap<PlanParticipantRef, HostRef>,
    value: &PlanParticipantRef,
) -> AppResult<()> {
    if !participants.contains_key(value) {
        return invalid("Bridge Plan v2 step references a participant outside the Plan.");
    }
    Ok(())
}

fn validate_object(object: &ManagedObjectRevisionV2) -> AppResult<()> {
    id(&object.logical_object_id, "logical object id")?;
    if object.revision == 0 {
        return invalid("Bridge Plan v2 logical revisions must be positive.");
    }
    Ok(())
}

fn id(value: &str, label: &str) -> AppResult<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_ID_LEN
        || value.chars().any(|character| character.is_control())
    {
        return Err(AppError::InvalidInput(format!(
            "Bridge Plan v2 {label} is invalid."
        )));
    }
    Ok(())
}

fn text(value: &str, label: &str) -> AppResult<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_TEXT_LEN
        || value.chars().any(|character| character.is_control())
    {
        return Err(AppError::InvalidInput(format!(
            "Bridge Plan v2 {label} is invalid."
        )));
    }
    Ok(())
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("JSON string serialization"),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let sorted = values.iter().collect::<BTreeMap<_, _>>();
            format!(
                "{{{}}}",
                sorted
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("JSON key serialization"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn ensure_active_bridge(paths: &AppPaths, bridge_id: &str) -> AppResult<()> {
    let conn = connection(paths)?;
    let active: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM rooms WHERE id = ?1 AND status != 'burned') AND NOT EXISTS(SELECT 1 FROM burned_bridges WHERE room_id = ?1)",
        [bridge_id],
        |row| row.get(0),
    )?;
    if active == 0 {
        return invalid("Bridge Plan v2 Bridge is unavailable or burned.");
    }
    Ok(())
}

fn connection(paths: &AppPaths) -> AppResult<Connection> {
    let conn = Connection::open(&paths.db_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])?;
    Ok(conn)
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{host_admission::HostAdmissionDenialCode, models::LocalRole, storage};

    const NOW: i64 = 10_000;

    fn host(value: &str) -> HostRef {
        HostRef::from_device_id(value).unwrap()
    }

    fn paths(label: &str) -> AppPaths {
        let root = std::env::temp_dir().join(format!("{label}-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(root.clone(), root.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        storage::create_room(
            &paths,
            &crate::crypto::random_key(),
            "123456",
            5,
            LocalRole::Creator,
            Some("bridge-v2".into()),
            Some(NOW + 3_600),
        )
        .unwrap();
        paths
    }

    fn participant(revision: &PlanRevisionV2, host_ref: &HostRef) -> PlanParticipantRef {
        revision
            .participants
            .as_slice()
            .iter()
            .find(|participant| &participant.host_ref == host_ref)
            .unwrap()
            .participant_ref
            .clone()
    }

    fn transfer_revision() -> PlanRevisionV2 {
        let plan_id = "plan-v2";
        let requester_host = host("requester");
        let source_host = host("source");
        let destination_host = host("destination");
        let participants = PlanParticipants::new(
            plan_id,
            [
                requester_host.clone(),
                source_host.clone(),
                destination_host,
            ],
        )
        .unwrap();
        let requester = PlanParticipantRef::for_host(plan_id, &requester_host).unwrap();
        let source = PlanParticipantRef::for_host(plan_id, &source_host).unwrap();
        let destination = participants
            .as_slice()
            .iter()
            .find(|value| value.host_ref != requester_host && value.host_ref != source_host)
            .unwrap()
            .participant_ref
            .clone();
        let object = ManagedObjectRevisionV2 {
            logical_object_id: "managed-root-a".into(),
            revision: 7,
        };
        seal_revision(PlanRevisionV2 {
            schema_version: PLAN_SCHEMA_VERSION.into(),
            plan_id: plan_id.into(),
            revision_id: "revision-v2".into(),
            revision_number: 1,
            revision_hash: String::new(),
            bridge_id: "bridge-v2".into(),
            requester,
            participants,
            roots: vec![PlanRootV2 {
                root_id: "root-a".into(),
                object: object.clone(),
                host: source.clone(),
            }],
            original_user_goal: "Move the already managed object.".into(),
            expected_outcome: "The exact revision is present on the destination Host.".into(),
            steps: vec![PlanStepV2::Transfer {
                step_id: "transfer-a".into(),
                depends_on: Vec::new(),
                source,
                destination,
                input: object.clone(),
                output: object,
            }],
        })
        .unwrap()
    }

    fn all_four_revision() -> PlanRevisionV2 {
        let mut revision = transfer_revision();
        let (source, destination) = match &revision.steps[0] {
            PlanStepV2::Transfer {
                source,
                destination,
                ..
            } => (source.clone(), destination.clone()),
            _ => unreachable!(),
        };
        let revision_one = ManagedObjectRevisionV2 {
            logical_object_id: "search-result-object".into(),
            revision: 1,
        };
        let revision_two = ManagedObjectRevisionV2 {
            logical_object_id: revision_one.logical_object_id.clone(),
            revision: 2,
        };
        revision.roots.clear();
        revision.steps = vec![
            PlanStepV2::Search {
                step_id: "search".into(),
                depends_on: Vec::new(),
                host: source.clone(),
                output: revision_one.clone(),
                query: "Find the approved object.".into(),
                safe_scope_labels: vec!["documents".into()],
            },
            PlanStepV2::Transform {
                step_id: "transform".into(),
                depends_on: vec!["search".into()],
                host: source.clone(),
                input: revision_one,
                output: revision_two.clone(),
                modification_intent: "Apply the reviewed modification.".into(),
            },
            PlanStepV2::Transfer {
                step_id: "transfer".into(),
                depends_on: vec!["transform".into()],
                source,
                destination: destination.clone(),
                input: revision_two.clone(),
                output: revision_two.clone(),
            },
            PlanStepV2::Execute {
                step_id: "execute".into(),
                depends_on: vec!["transfer".into()],
                host: destination,
                target: revision_two,
                execution_intent: "Run the reviewed action.".into(),
            },
        ];
        revision.revision_hash.clear();
        seal_revision(revision).unwrap()
    }

    fn review_for(revision: &PlanRevisionV2, target: PlanParticipantRef) -> ReviewRequestV2 {
        ReviewRequestV2 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: "review-message".into(),
            correlation_id: "correlation-v2".into(),
            request_nonce: "review-nonce".into(),
            sender: revision.requester.clone(),
            target,
            approval: PlanApprovalV2 {
                approval_id: "approval-v2".into(),
                plan_id: revision.plan_id.clone(),
                revision_id: revision.revision_id.clone(),
                revision_hash: revision.revision_hash.clone(),
                bridge_id: revision.bridge_id.clone(),
                requester: revision.requester.clone(),
                expires_at: NOW + 600,
            },
            revision: revision.clone(),
        }
    }

    fn start_for(review: &ReviewRequestV2) -> AttemptStartV2 {
        AttemptStartV2 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: "start-message".into(),
            correlation_id: review.correlation_id.clone(),
            request_nonce: review.request_nonce.clone(),
            attempt_id: "attempt-v2".into(),
            approval_id: review.approval.approval_id.clone(),
            plan_id: review.revision.plan_id.clone(),
            revision_id: review.revision.revision_id.clone(),
            revision_hash: review.revision.revision_hash.clone(),
            bridge_id: review.revision.bridge_id.clone(),
            sender: review.sender.clone(),
            target: review.target.clone(),
            expires_at: NOW + 500,
        }
    }

    fn binding(revision: &PlanRevisionV2, local: &HostRef, suffix: &str) -> HostSessionBinding {
        HostSessionBinding::new(
            &revision.bridge_id,
            local.clone(),
            requester_host(revision).unwrap().clone(),
            &format!("local-session-{suffix}"),
            &format!("requester-session-{suffix}"),
            &format!("route-{suffix}"),
            NOW + 600,
        )
        .unwrap()
    }

    fn record_review(
        store: &BridgePlanV2Store<'_>,
        review: &ReviewRequestV2,
        local_host: &HostRef,
    ) {
        store
            .record_review(
                review,
                &binding(&review.revision, local_host, "review"),
                NOW,
            )
            .unwrap();
    }

    fn denial_code(decision: AttemptStartDecisionV2) -> HostAdmissionDenialCode {
        match decision {
            AttemptStartDecisionV2::Denied(HostAdmissionDecision::Deny(denial)) => denial.code,
            other => panic!("expected denial, got {other:?}"),
        }
    }

    #[test]
    fn v1_and_v2_schema_and_hash_domains_are_explicitly_separate() {
        let v2 = transfer_revision();
        assert!(v2
            .revision_hash
            .starts_with("bridge-plan-revision-hash-v2:"));
        assert!(
            serde_json::from_value::<crate::bridge_plan::BridgePlanRevision>(
                serde_json::to_value(&v2).unwrap()
            )
            .is_err()
        );

        let v1 = crate::bridge_plan::build_file_search_revision(
            "bridge".into(),
            "requester-session".into(),
            "selected-session".into(),
            "Find a PDF".into(),
            "report.pdf".into(),
            vec!["pdf".into()],
            vec!["documents".into()],
        )
        .unwrap();
        assert!(v1
            .revision_hash
            .starts_with("bridge-plan-revision-hash-v1:"));
        assert!(
            serde_json::from_value::<PlanRevisionV2>(serde_json::to_value(v1).unwrap()).is_err()
        );
    }

    #[test]
    fn participant_host_claim_must_match_the_plan_scoped_ref() {
        let revision = transfer_revision();
        let mut encoded = serde_json::to_value(&revision).unwrap();
        encoded["participants"][0]["hostRef"] = serde_json::to_value(host("impostor")).unwrap();
        let mismatched: PlanRevisionV2 = serde_json::from_value(encoded).unwrap();
        assert!(validate_revision(&mismatched).is_err());
    }

    #[test]
    fn participant_refs_are_unique_and_conflicting_host_mappings_are_rejected() {
        let revision = transfer_revision();

        let mut duplicate = serde_json::to_value(&revision).unwrap();
        let first = duplicate["participants"][0].clone();
        duplicate["participants"]
            .as_array_mut()
            .unwrap()
            .push(first);
        let duplicate: PlanRevisionV2 = serde_json::from_value(duplicate).unwrap();
        assert!(validate_revision(&duplicate).is_err());

        let mut conflicting = serde_json::to_value(&revision).unwrap();
        conflicting["participants"][1]["participantRef"] =
            conflicting["participants"][0]["participantRef"].clone();
        let conflicting: PlanRevisionV2 = serde_json::from_value(conflicting).unwrap();
        assert!(validate_revision(&conflicting).is_err());
    }

    #[test]
    fn managed_roots_require_an_explicit_plan_scoped_host_location() {
        let revision = transfer_revision();

        let mut missing_host = serde_json::to_value(&revision).unwrap();
        missing_host["roots"][0]
            .as_object_mut()
            .unwrap()
            .remove("host");
        assert!(serde_json::from_value::<PlanRevisionV2>(missing_host).is_err());

        for inferred_fact in ["routeRef", "capabilityFacts", "objectPresent"] {
            let mut inferred = serde_json::to_value(&revision).unwrap();
            inferred["roots"][0][inferred_fact] = Value::Bool(true);
            assert!(serde_json::from_value::<PlanRevisionV2>(inferred).is_err());
        }

        let mut cross_plan = revision.clone();
        cross_plan.roots[0].host =
            PlanParticipantRef::for_host("other-plan", &host("source")).unwrap();
        assert!(validate_revision(&cross_plan).is_err());
    }

    #[test]
    fn generic_root_needs_no_search_and_only_exact_transfer_changes_location() {
        let revision = transfer_revision();
        assert!(validate_revision(&revision).is_ok());
        assert_eq!(revision.roots[0].object.revision, 7);
        assert!(matches!(revision.steps[0], PlanStepV2::Transfer { .. }));

        let mut wrong_revision = revision.clone();
        if let PlanStepV2::Transfer { output, .. } = &mut wrong_revision.steps[0] {
            output.revision += 1;
        }
        assert!(validate_revision(&wrong_revision).is_err());

        let mut wrong_source = revision.clone();
        if let PlanStepV2::Transfer { source, .. } = &mut wrong_source.steps[0] {
            *source = revision.requester.clone();
        }
        assert!(validate_revision(&wrong_source).is_err());
    }

    #[test]
    fn hidden_transform_or_execute_movement_is_rejected() {
        let mut revision = transfer_revision();
        let (destination, object) = match &revision.steps[0] {
            PlanStepV2::Transfer {
                destination,
                output,
                ..
            } => (destination.clone(), output.clone()),
            _ => unreachable!(),
        };
        revision.steps.push(PlanStepV2::Execute {
            step_id: "execute-a".into(),
            depends_on: vec!["transfer-a".into()],
            host: destination,
            target: object,
            execution_intent: "Inspect the exact transferred revision.".into(),
        });
        revision.revision_hash.clear();
        assert!(validate_revision(&revision).is_ok());

        if let PlanStepV2::Execute { host, .. } = &mut revision.steps[1] {
            *host = revision.roots[0].host.clone();
        }
        assert!(validate_revision(&revision).is_err());
    }

    #[test]
    fn all_four_primitives_have_explicit_host_and_revision_topology() {
        let mut revision = all_four_revision();
        assert!(verify_sealed_revision(&revision).is_ok());

        if let PlanStepV2::Transfer { depends_on, .. } = &mut revision.steps[2] {
            depends_on.clear();
        }
        assert!(validate_revision(&revision).is_err());
    }

    #[test]
    fn stale_latest_and_ambiguous_revision_lineage_are_rejected() {
        let revision = all_four_revision();

        let mut stale_transform = revision.clone();
        if let PlanStepV2::Transform { input, .. } = &mut stale_transform.steps[1] {
            input.revision += 1;
        }
        assert!(validate_revision(&stale_transform).is_err());

        let mut skipped_revision = revision.clone();
        if let PlanStepV2::Transform { output, .. } = &mut skipped_revision.steps[1] {
            output.revision += 1;
        }
        assert!(validate_revision(&skipped_revision).is_err());

        let mut stale_execute = revision.clone();
        if let PlanStepV2::Execute { target, .. } = &mut stale_execute.steps[3] {
            target.revision -= 1;
        }
        assert!(validate_revision(&stale_execute).is_err());

        let mut latest = serde_json::to_value(&revision).unwrap();
        latest["steps"][1]["input"]["revision"] = Value::String("latest".into());
        assert!(serde_json::from_value::<PlanRevisionV2>(latest).is_err());

        let mut ambiguous_roots = transfer_revision();
        let mut duplicate = ambiguous_roots.roots[0].clone();
        duplicate.root_id = "root-b".into();
        ambiguous_roots.roots.push(duplicate);
        assert!(validate_revision(&ambiguous_roots).is_err());
    }

    #[test]
    fn native_protocol_requires_exact_current_binding_and_host_admission() {
        let paths = paths("pastey-v2-admission");
        let revision = transfer_revision();
        let local_host = host("source");
        let target = participant(&revision, &local_host);
        let review = review_for(&revision, target);
        let store = BridgePlanV2Store::new(&paths);
        assert!(store
            .record_review(
                &review,
                &binding(&revision, &host("destination"), "wrong-review-host"),
                NOW,
            )
            .is_err());
        record_review(&store, &review, &local_host);
        let start = start_for(&review);
        let captured = binding(&revision, &local_host, "current");
        let decision = store
            .accept_attempt_start(
                &start,
                &captured,
                &captured,
                &HostAdmissionService::new(local_host),
                NOW,
            )
            .unwrap();
        let AttemptStartDecisionV2::Accepted(accepted) = decision else {
            panic!("expected admission-backed acceptance");
        };
        assert!(accepted.admission_ref.starts_with("host-admission:v2:"));
        assert_eq!(
            store.attempt_state(&start.attempt_id).unwrap().as_deref(),
            Some("accepted")
        );
        std::fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn protocol_v2_metadata_is_typed_bridge_scoped_and_versioned() {
        let revision = transfer_revision();
        let review = review_for(&revision, participant(&revision, &host("source")));
        let payload = serde_json::to_value(&review).unwrap();
        let payload = payload.as_object().unwrap();
        let metadata =
            protocol_metadata("bridge_plan.v2.review_request", payload, "bridge-v2", NOW).unwrap();
        assert_eq!(metadata.replay_id, "v2:review:review-message");
        assert!(
            protocol_metadata("bridge_plan.review_request", payload, "bridge-v2", NOW,).is_err()
        );
        assert!(protocol_metadata(
            "bridge_plan.v2.review_request",
            payload,
            "other-bridge",
            NOW,
        )
        .is_err());
    }

    #[test]
    fn stale_or_wrong_session_binding_denies_and_consumes_the_replay_nonce() {
        let paths = paths("pastey-v2-stale-binding");
        let revision = transfer_revision();
        let local_host = host("source");
        let review = review_for(&revision, participant(&revision, &local_host));
        let store = BridgePlanV2Store::new(&paths);
        record_review(&store, &review, &local_host);
        let start = start_for(&review);
        let captured = binding(&revision, &local_host, "old");
        let current = binding(&revision, &local_host, "reconnected");
        assert_eq!(
            denial_code(
                store
                    .accept_attempt_start(
                        &start,
                        &captured,
                        &current,
                        &HostAdmissionService::new(local_host.clone()),
                        NOW,
                    )
                    .unwrap()
            ),
            HostAdmissionDenialCode::SessionMismatch
        );
        assert!(store
            .accept_attempt_start(
                &start,
                &current,
                &current,
                &HostAdmissionService::new(local_host),
                NOW,
            )
            .is_err());
        std::fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn host_mismatch_and_admission_correlation_fail_closed() {
        let paths = paths("pastey-v2-correlation");
        let revision = transfer_revision();
        let local_host = host("source");
        let review = review_for(&revision, participant(&revision, &local_host));
        let store = BridgePlanV2Store::new(&paths);
        record_review(&store, &review, &local_host);

        let mut wrong_correlation = start_for(&review);
        wrong_correlation.correlation_id = "other-correlation".into();
        let captured = binding(&revision, &local_host, "exact");
        assert!(store
            .accept_attempt_start(
                &wrong_correlation,
                &captured,
                &captured,
                &HostAdmissionService::new(local_host.clone()),
                NOW,
            )
            .is_err());

        let wrong_local = host("destination");
        let wrong_binding = binding(&revision, &wrong_local, "wrong-host");
        assert_eq!(
            denial_code(
                store
                    .accept_attempt_start(
                        &start_for(&review),
                        &wrong_binding,
                        &wrong_binding,
                        &HostAdmissionService::new(wrong_local),
                        NOW,
                    )
                    .unwrap()
            ),
            HostAdmissionDenialCode::HostMismatch
        );
        std::fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn sender_target_and_cross_plan_participant_substitution_are_rejected() {
        let paths = paths("pastey-v2-participant-substitution");
        let revision = transfer_revision();
        let local_host = host("source");
        let target = participant(&revision, &local_host);
        let store = BridgePlanV2Store::new(&paths);

        let mut sender_substitution = review_for(&revision, target.clone());
        sender_substitution.sender = participant(&revision, &host("destination"));
        assert!(store
            .record_review(
                &sender_substitution,
                &binding(&revision, &local_host, "sender-substitution"),
                NOW,
            )
            .is_err());

        let mut target_substitution = review_for(&revision, target.clone());
        target_substitution.target = revision.requester.clone();
        assert!(store
            .record_review(
                &target_substitution,
                &binding(&revision, &local_host, "target-substitution"),
                NOW,
            )
            .is_err());

        let review = review_for(&revision, target);
        record_review(&store, &review, &local_host);
        let current = binding(&revision, &local_host, "exact");
        let mut substituted_start = start_for(&review);
        substituted_start.sender = participant(&revision, &host("destination"));
        assert!(store
            .accept_attempt_start(
                &substituted_start,
                &current,
                &current,
                &HostAdmissionService::new(local_host.clone()),
                NOW,
            )
            .is_err());
        substituted_start = start_for(&review);
        substituted_start.target = participant(&revision, &host("destination"));
        assert!(store
            .accept_attempt_start(
                &substituted_start,
                &current,
                &current,
                &HostAdmissionService::new(local_host),
                NOW,
            )
            .is_err());

        let mut cross_plan = revision.clone();
        if let PlanStepV2::Transfer { source, .. } = &mut cross_plan.steps[0] {
            *source = PlanParticipantRef::for_host("other-plan", &host("source")).unwrap();
        }
        assert!(validate_revision(&cross_plan).is_err());
        std::fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn v1_and_v2_replay_namespaces_do_not_collide() {
        let paths = paths("pastey-v2-replay-isolation");
        let conn = Connection::open(&paths.db_path).unwrap();
        conn.execute(
            "INSERT INTO bridge_plan_protocol_reviews (bridge_id, direction, approval_id, plan_id, revision_id, revision_hash, requester_device_ref, receiver_device_ref, correlation_id, request_nonce, search_step_digest, review_expires_at, revision_json) VALUES ('bridge-v2', 'inbound', 'v1-approval', 'v1-plan', 'v1-revision', 'bridge-plan-revision-hash-v1:test', 'requester-session', 'receiver-session', 'correlation-v2', 'review-nonce', 'digest', ?1, '{}')",
            [NOW + 600],
        )
        .unwrap();
        let revision = transfer_revision();
        let review = review_for(&revision, participant(&revision, &host("source")));
        let store = BridgePlanV2Store::new(&paths);
        let start = start_for(&review);
        let binding = binding(&revision, &host("source"), "replay-isolation");

        // A matching v1 correlation and nonce cannot satisfy a v2 start.
        assert!(store
            .accept_attempt_start(
                &start,
                &binding,
                &binding,
                &HostAdmissionService::new(host("source")),
                NOW,
            )
            .is_err());

        record_review(&store, &review, &host("source"));
        let counts: (i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM bridge_plan_protocol_reviews WHERE correlation_id = 'correlation-v2' AND request_nonce = 'review-nonce'), (SELECT COUNT(*) FROM bridge_plan_v2_protocol_reviews WHERE correlation_id = 'correlation-v2' AND request_nonce = 'review-nonce')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (1, 1));

        let payload = serde_json::to_value(&review).unwrap();
        let metadata = protocol_metadata(
            "bridge_plan.v2.review_request",
            payload.as_object().unwrap(),
            "bridge-v2",
            NOW,
        )
        .unwrap();
        assert_eq!(metadata.replay_id, "v2:review:review-message");
        assert_ne!(metadata.replay_id, "bridge-plan-review:review-nonce");
        std::fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn unsupported_semantics_fail_the_whole_plan_before_any_attempt() {
        let mut transform_plan = all_four_revision();
        transform_plan.steps.truncate(2);
        transform_plan.revision_hash.clear();
        transform_plan = seal_revision(transform_plan).unwrap();

        let mut execute_plan = transfer_revision();
        let destination = match &execute_plan.steps[0] {
            PlanStepV2::Transfer { destination, .. } => destination.clone(),
            _ => unreachable!(),
        };
        execute_plan.steps.push(PlanStepV2::Execute {
            step_id: "execute-a".into(),
            depends_on: vec!["transfer-a".into()],
            host: destination,
            target: execute_plan.roots[0].object.clone(),
            execution_intent: "Run the approved target.".into(),
        });
        execute_plan.revision_hash.clear();
        execute_plan = seal_revision(execute_plan).unwrap();

        for (label, revision) in [
            ("pastey-v2-whole-plan-transform", transform_plan),
            ("pastey-v2-whole-plan-execute", execute_plan),
        ] {
            let paths = paths(label);
            let local_host = host("source");
            let review = review_for(&revision, participant(&revision, &local_host));
            let store = BridgePlanV2Store::new(&paths);
            record_review(&store, &review, &local_host);
            let binding = binding(&revision, &local_host, "unsupported");
            assert_eq!(
                denial_code(
                    store
                        .accept_attempt_start(
                            &start_for(&review),
                            &binding,
                            &binding,
                            &HostAdmissionService::new(local_host),
                            NOW,
                        )
                        .unwrap()
                ),
                HostAdmissionDenialCode::UnsupportedOperation
            );
            assert!(store.attempt_state("attempt-v2").unwrap().is_none());

            let conn = Connection::open(&paths.db_path).unwrap();
            let v1_attempts: i64 = conn
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM bridge_plan_protocol_attempts) + (SELECT COUNT(*) FROM bridge_plan_protocol_transfer_steps)",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(v1_attempts, 0);

            let authorities = crate::bridge_plan::ProtocolSearchAuthorityStore::default();
            assert!(crate::bridge_plan::consume_search_execution_grant(
                &paths,
                &authorities,
                "bridge-v2",
                "attempt-v2",
                NOW,
            )
            .is_err());
            assert!(crate::bridge_plan::consume_transfer_execution_grant(
                &paths,
                &authorities,
                "bridge-v2",
                "attempt-v2",
                NOW,
            )
            .is_err());
            std::fs::remove_dir_all(paths.app_data_dir).unwrap();
        }
    }

    #[test]
    fn restart_interrupts_and_burn_removes_v2_bindings_but_not_local_hostref() {
        let paths = paths("pastey-v2-restart-burn");
        let revision = transfer_revision();
        let local_host = host("source");
        let review = review_for(&revision, participant(&revision, &local_host));
        let store = BridgePlanV2Store::new(&paths);
        record_review(&store, &review, &local_host);
        let binding = binding(&revision, &local_host, "restart");
        store
            .accept_attempt_start(
                &start_for(&review),
                &binding,
                &binding,
                &HostAdmissionService::new(local_host.clone()),
                NOW,
            )
            .unwrap();
        assert_eq!(reconcile_startup(&paths).unwrap(), 1);
        assert_eq!(
            store.attempt_state("attempt-v2").unwrap().as_deref(),
            Some("interrupted")
        );

        storage::cut_off_bridge_authority(&paths, "bridge-v2").unwrap();
        storage::finalize_burned_room(&paths, "bridge-v2", &paths.inbox_dir).unwrap();
        let conn = Connection::open(&paths.db_path).unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM bridge_plan_v2_revisions) + (SELECT COUNT(*) FROM bridge_plan_v2_protocol_reviews) + (SELECT COUNT(*) FROM bridge_plan_v2_attempts)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        assert_eq!(local_host, host("source"));
        std::fs::remove_dir_all(paths.app_data_dir).unwrap();
    }
}

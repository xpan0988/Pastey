//! Host-local admission for exact approved managed work.
//!
//! Admission is deliberately downstream of requester approval and current
//! session resolution, but upstream of any attempt/step grant or effect. It
//! does not make Layer 4 identity, liveness, capability facts, or object
//! presence authoritative.

#![allow(dead_code)] // V1 compatibility APIs are not all reached outside focused tests.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    bridge_plan::{
        self, ApprovalState, BridgePlanStep, BridgePlanStore, RevisionState, StepOperation,
    },
    bridge_plan_v2::{
        self, participant_for_ref, requester_host, PlanApprovalV2, PlanRevisionV2, PlanStepV2,
    },
    error::AppResult,
    host_identity::{HostRef, HostSessionBinding, PlanParticipantRef},
    storage::AppPaths,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAdmissionRequestV2 {
    pub approval_id: String,
    pub plan_id: String,
    pub revision_id: String,
    pub revision_hash: String,
    pub host_ref: HostRef,
    pub participant_ref: PlanParticipantRef,
    pub protocol_correlation_id: String,
    pub session_binding: HostSessionBinding,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAdmissionRequest {
    pub approval_id: String,
    pub plan_id: String,
    pub revision_id: String,
    pub revision_hash: String,
    pub host_ref: HostRef,
    pub participant_ref: PlanParticipantRef,
    pub session_binding: HostSessionBinding,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmittedHostWork {
    pub step_id: String,
    pub operation: StepOperation,
    pub semantic_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAdmissionConstraints {
    pub allowed_step_ids: Vec<String>,
    pub allowed_operations: Vec<StepOperation>,
    pub expires_at: i64,
    pub requires_current_session: bool,
    pub modification_authority: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAdmission {
    pub admission_ref: String,
    pub approval_id: String,
    pub plan_id: String,
    pub revision_id: String,
    pub revision_hash: String,
    pub host_ref: HostRef,
    pub participant_ref: PlanParticipantRef,
    pub session_binding_ref: String,
    pub work: Vec<AdmittedHostWork>,
    pub constraints: HostAdmissionConstraints,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostAdmissionDenialCode {
    ApprovalUnavailable,
    ApprovalMismatch,
    PlanMismatch,
    HostMismatch,
    SessionMismatch,
    NoHostBoundWork,
    UnsupportedOperation,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAdmissionDenial {
    pub code: HostAdmissionDenialCode,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "result")]
pub enum HostAdmissionDecision {
    Admit(Box<HostAdmission>),
    Deny(HostAdmissionDenial),
}

impl HostAdmissionDecision {
    pub fn admitted(&self) -> Option<&HostAdmission> {
        match self {
            Self::Admit(admission) => Some(admission),
            Self::Deny(_) => None,
        }
    }
}

/// Stateless local policy boundary owned by one HostRuntime.
///
/// The caller must supply both the captured binding being admitted and a
/// freshly resolved current binding. The service derives Host-bound work from
/// the stored immutable revision; callers cannot supply or expand the work.
#[derive(Clone, Debug)]
pub struct HostAdmissionService {
    local_host_ref: HostRef,
}

/// Core-owned availability snapshot for native v2 managed semantics. This is
/// deliberately not serializable and cannot be supplied by Layer 4, a Worker,
/// the renderer, capability observations, or the Plan itself.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ManagedPrimitiveAvailabilityV1 {
    transform_hosts: BTreeSet<HostRef>,
    execute_hosts: BTreeSet<HostRef>,
}

impl ManagedPrimitiveAvailabilityV1 {
    pub(crate) fn unavailable() -> Self {
        Self::default()
    }

    pub(crate) fn verified_attachment(host_ref: HostRef, transform: bool, execute: bool) -> Self {
        Self {
            transform_hosts: transform.then_some(host_ref.clone()).into_iter().collect(),
            execute_hosts: execute.then_some(host_ref).into_iter().collect(),
        }
    }

    pub(crate) fn supports(&self, revision: &PlanRevisionV2, step: &PlanStepV2) -> bool {
        let host_ref = match step {
            PlanStepV2::Transform { host, .. } | PlanStepV2::Execute { host, .. } => {
                let Some(participant) = participant_for_ref(revision, host) else {
                    return false;
                };
                &participant.host_ref
            }
            PlanStepV2::Search { .. } | PlanStepV2::Transfer { .. } => return true,
        };
        match step.operation() {
            StepOperation::Search | StepOperation::Transfer => true,
            StepOperation::Transform => self.transform_hosts.contains(host_ref),
            StepOperation::Execute => self.execute_hosts.contains(host_ref),
        }
    }
}

impl HostAdmissionService {
    pub fn new(local_host_ref: HostRef) -> Self {
        Self { local_host_ref }
    }

    pub fn evaluate(
        &self,
        paths: &AppPaths,
        request: &HostAdmissionRequest,
        current_binding: &HostSessionBinding,
        now: i64,
    ) -> AppResult<HostAdmissionDecision> {
        if request.host_ref != self.local_host_ref
            || request.session_binding.local_host_ref != self.local_host_ref
            || current_binding.local_host_ref != self.local_host_ref
        {
            return Ok(deny(
                HostAdmissionDenialCode::HostMismatch,
                "The requested work is not bound to this Host.",
            ));
        }
        if request
            .session_binding
            .validate_current(current_binding, now)
            .is_err()
        {
            return Ok(deny(
                HostAdmissionDenialCode::SessionMismatch,
                "The Host session binding is stale or mismatched.",
            ));
        }

        let store = BridgePlanStore::new(paths);
        let approval = match store.get_approval(&request.approval_id) {
            Ok(approval) => approval,
            Err(_) => {
                return Ok(deny(
                    HostAdmissionDenialCode::ApprovalUnavailable,
                    "The exact requester approval is unavailable.",
                ))
            }
        };
        if approval.state != ApprovalState::Valid {
            return Ok(deny(
                HostAdmissionDenialCode::ApprovalUnavailable,
                "The exact requester approval is not valid.",
            ));
        }
        if approval.approval.expires_at <= now {
            return Ok(deny(
                HostAdmissionDenialCode::Expired,
                "The exact requester approval has expired.",
            ));
        }
        if approval.approval.approval_id != request.approval_id
            || approval.approval.plan_id != request.plan_id
            || approval.approval.revision_id != request.revision_id
            || approval.approval.revision_hash != request.revision_hash
            || approval.approval.bridge_id != request.session_binding.bridge_id
        {
            return Ok(deny(
                HostAdmissionDenialCode::ApprovalMismatch,
                "The admission request does not match the exact requester approval.",
            ));
        }

        let revision = match store.get_revision(&request.revision_id) {
            Ok(revision) => revision,
            Err(_) => {
                return Ok(deny(
                    HostAdmissionDenialCode::PlanMismatch,
                    "The exact immutable Plan revision is unavailable.",
                ))
            }
        };
        if revision.state != RevisionState::Available
            || revision.revision.plan_id != request.plan_id
            || revision.revision.revision_id != request.revision_id
            || revision.revision.revision_hash != request.revision_hash
            || revision.revision.bridge_id != request.session_binding.bridge_id
            || bridge_plan::canonical_revision_hash(&revision.revision)? != request.revision_hash
        {
            return Ok(deny(
                HostAdmissionDenialCode::PlanMismatch,
                "The admission request does not match the immutable Plan revision.",
            ));
        }

        let (local_device_ref, peer_device_ref) = if revision.revision.requesting_device_ref
            == request.session_binding.local_session_ref
            && revision.revision.selected_device_ref == request.session_binding.peer_session_ref
        {
            (
                revision.revision.requesting_device_ref.as_str(),
                revision.revision.selected_device_ref.as_str(),
            )
        } else if revision.revision.selected_device_ref == request.session_binding.local_session_ref
            && revision.revision.requesting_device_ref == request.session_binding.peer_session_ref
        {
            (
                revision.revision.selected_device_ref.as_str(),
                revision.revision.requesting_device_ref.as_str(),
            )
        } else {
            return Ok(deny(
                HostAdmissionDenialCode::SessionMismatch,
                "The immutable Plan participants do not match the current Host session.",
            ));
        };

        let resolved_hosts = BTreeMap::from([
            (local_device_ref.to_string(), self.local_host_ref.clone()),
            (
                peer_device_ref.to_string(),
                request.session_binding.peer_host_ref.clone(),
            ),
        ]);
        let participants =
            bridge_plan::legacy_participant_projection(&revision.revision, &resolved_hosts)?;
        let local_participant = participants
            .iter()
            .find(|participant| participant.host_ref.as_ref() == Some(&self.local_host_ref));
        if local_participant.map(|participant| &participant.participant_ref)
            != Some(&request.participant_ref)
        {
            return Ok(deny(
                HostAdmissionDenialCode::HostMismatch,
                "The Plan participant does not identify this Host.",
            ));
        }

        let work = revision
            .revision
            .steps
            .iter()
            .filter(|step| step.execution_device() == local_device_ref)
            .map(admitted_work)
            .collect::<AppResult<Vec<_>>>()?;
        if work.is_empty() {
            return Ok(deny(
                HostAdmissionDenialCode::NoHostBoundWork,
                "The immutable Plan contains no work bound to this Host.",
            ));
        }
        if work.iter().any(|item| {
            matches!(
                item.operation,
                StepOperation::Transform | StepOperation::Execute
            )
        }) {
            return Ok(deny(
                HostAdmissionDenialCode::UnsupportedOperation,
                "This Host has no managed Transform or Execute implementation.",
            ));
        }

        let expires_at = approval
            .approval
            .expires_at
            .min(request.session_binding.expires_at);
        if expires_at <= now {
            return Ok(deny(
                HostAdmissionDenialCode::Expired,
                "The Host admission window has expired.",
            ));
        }
        let constraints = HostAdmissionConstraints {
            allowed_step_ids: work.iter().map(|item| item.step_id.clone()).collect(),
            allowed_operations: work.iter().map(|item| item.operation.clone()).collect(),
            expires_at,
            requires_current_session: true,
            modification_authority: false,
        };
        let admission_ref = admission_ref(request, &work, &constraints)?;
        Ok(HostAdmissionDecision::Admit(Box::new(HostAdmission {
            admission_ref,
            approval_id: request.approval_id.clone(),
            plan_id: request.plan_id.clone(),
            revision_id: request.revision_id.clone(),
            revision_hash: request.revision_hash.clone(),
            host_ref: self.local_host_ref.clone(),
            participant_ref: request.participant_ref.clone(),
            session_binding_ref: request.session_binding.binding_ref.clone(),
            work,
            constraints,
        })))
    }

    /// Native v2 admission. The immutable revision and requester approval are
    /// supplied by the receiver-owned protocol store after exact review/start
    /// correlation. Layer 4 contributes only the captured and current session
    /// bindings; it cannot select work or imply admission.
    pub(crate) fn evaluate_v2(
        &self,
        revision: &PlanRevisionV2,
        approval: &PlanApprovalV2,
        request: &HostAdmissionRequestV2,
        current_binding: &HostSessionBinding,
        now: i64,
    ) -> AppResult<HostAdmissionDecision> {
        self.evaluate_v2_with_availability(
            revision,
            approval,
            request,
            current_binding,
            ManagedPrimitiveAvailabilityV1::unavailable(),
            now,
        )
    }

    /// Step 8 Core attachment. The availability value is process-local and
    /// must be derived from verified Host enforcement, never protocol input.
    pub(crate) fn evaluate_v2_with_availability(
        &self,
        revision: &PlanRevisionV2,
        approval: &PlanApprovalV2,
        request: &HostAdmissionRequestV2,
        current_binding: &HostSessionBinding,
        availability: ManagedPrimitiveAvailabilityV1,
        now: i64,
    ) -> AppResult<HostAdmissionDecision> {
        if request.host_ref != self.local_host_ref
            || request.session_binding.local_host_ref != self.local_host_ref
            || current_binding.local_host_ref != self.local_host_ref
        {
            return Ok(deny(
                HostAdmissionDenialCode::HostMismatch,
                "The v2 work is not bound to this Host.",
            ));
        }
        if request
            .session_binding
            .validate_current(current_binding, now)
            .is_err()
        {
            return Ok(deny(
                HostAdmissionDenialCode::SessionMismatch,
                "The v2 Host session binding is stale or mismatched.",
            ));
        }
        if bridge_plan_v2::verify_sealed_revision(revision).is_err()
            || revision.plan_id != request.plan_id
            || revision.revision_id != request.revision_id
            || revision.revision_hash != request.revision_hash
            || revision.bridge_id != request.session_binding.bridge_id
        {
            return Ok(deny(
                HostAdmissionDenialCode::PlanMismatch,
                "The v2 admission request does not match the immutable Plan.",
            ));
        }
        if approval.approval_id != request.approval_id
            || approval.plan_id != request.plan_id
            || approval.revision_id != request.revision_id
            || approval.revision_hash != request.revision_hash
            || approval.bridge_id != revision.bridge_id
            || approval.requester != revision.requester
        {
            return Ok(deny(
                HostAdmissionDenialCode::ApprovalMismatch,
                "The v2 admission request does not match requester approval.",
            ));
        }
        if approval.expires_at <= now {
            return Ok(deny(
                HostAdmissionDenialCode::Expired,
                "The v2 requester approval has expired.",
            ));
        }
        let local_participant = participant_for_ref(revision, &request.participant_ref);
        if local_participant.map(|participant| &participant.host_ref) != Some(&self.local_host_ref)
        {
            return Ok(deny(
                HostAdmissionDenialCode::HostMismatch,
                "The v2 Plan participant does not identify this Host.",
            ));
        }
        if requester_host(revision)? != &request.session_binding.peer_host_ref {
            return Ok(deny(
                HostAdmissionDenialCode::SessionMismatch,
                "The current Layer 4 peer is not the approved v2 requester Host.",
            ));
        }
        if request.protocol_correlation_id.trim().is_empty() {
            return Ok(deny(
                HostAdmissionDenialCode::ApprovalMismatch,
                "The v2 protocol correlation is unavailable.",
            ));
        }

        // Unsupported semantics fail the complete Plan, including Hosts whose
        // own local fragment would otherwise contain only Search or Transfer.
        if revision
            .steps
            .iter()
            .any(|step| !availability.supports(revision, step))
        {
            return Ok(deny(
                HostAdmissionDenialCode::UnsupportedOperation,
                "This HostRuntime cannot safely provide every primitive required by the v2 Plan.",
            ));
        }
        let work = revision
            .steps
            .iter()
            .filter(|step| step.binds_participant(&request.participant_ref))
            .map(admitted_work_v2)
            .collect::<AppResult<Vec<_>>>()?;
        if work.is_empty() {
            return Ok(deny(
                HostAdmissionDenialCode::NoHostBoundWork,
                "The v2 Plan contains no work bound to this Host.",
            ));
        }
        let expires_at = approval.expires_at.min(request.session_binding.expires_at);
        if expires_at <= now {
            return Ok(deny(
                HostAdmissionDenialCode::Expired,
                "The v2 Host admission window has expired.",
            ));
        }
        let constraints = HostAdmissionConstraints {
            allowed_step_ids: work.iter().map(|item| item.step_id.clone()).collect(),
            allowed_operations: work.iter().map(|item| item.operation.clone()).collect(),
            expires_at,
            requires_current_session: true,
            modification_authority: work
                .iter()
                .any(|item| item.operation == StepOperation::Transform),
        };
        let admission_ref = admission_ref_v2(request, &work, &constraints)?;
        Ok(HostAdmissionDecision::Admit(Box::new(HostAdmission {
            admission_ref,
            approval_id: request.approval_id.clone(),
            plan_id: request.plan_id.clone(),
            revision_id: request.revision_id.clone(),
            revision_hash: request.revision_hash.clone(),
            host_ref: self.local_host_ref.clone(),
            participant_ref: request.participant_ref.clone(),
            session_binding_ref: request.session_binding.binding_ref.clone(),
            work,
            constraints,
        })))
    }
}

fn admitted_work(step: &BridgePlanStep) -> AppResult<AdmittedHostWork> {
    let semantic = serde_json::to_vec(step)?;
    Ok(AdmittedHostWork {
        step_id: step.id().to_string(),
        operation: step.operation(),
        semantic_digest: format!("host-work:v1:{}", blake3::hash(&semantic).to_hex()),
    })
}

fn admitted_work_v2(step: &PlanStepV2) -> AppResult<AdmittedHostWork> {
    let semantic = serde_json::to_vec(step)?;
    Ok(AdmittedHostWork {
        step_id: step.id().to_string(),
        operation: step.operation(),
        semantic_digest: format!("host-work:v2:{}", blake3::hash(&semantic).to_hex()),
    })
}

fn admission_ref(
    request: &HostAdmissionRequest,
    work: &[AdmittedHostWork],
    constraints: &HostAdmissionConstraints,
) -> AppResult<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pastey-host-admission-v1\0");
    for value in [
        request.approval_id.as_str(),
        request.plan_id.as_str(),
        request.revision_id.as_str(),
        request.revision_hash.as_str(),
        request.host_ref.as_str(),
        request.session_binding.binding_ref.as_str(),
    ] {
        hasher.update(value.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(&serde_json::to_vec(work)?);
    hasher.update(&serde_json::to_vec(constraints)?);
    Ok(format!("host-admission:v1:{}", hasher.finalize().to_hex()))
}

fn admission_ref_v2(
    request: &HostAdmissionRequestV2,
    work: &[AdmittedHostWork],
    constraints: &HostAdmissionConstraints,
) -> AppResult<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pastey-host-admission-v2\0");
    for value in [
        request.approval_id.as_str(),
        request.plan_id.as_str(),
        request.revision_id.as_str(),
        request.revision_hash.as_str(),
        request.host_ref.as_str(),
        request.participant_ref.as_str(),
        request.protocol_correlation_id.as_str(),
        request.session_binding.binding_ref.as_str(),
    ] {
        hasher.update(value.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(&serde_json::to_vec(work)?);
    hasher.update(&serde_json::to_vec(constraints)?);
    Ok(format!("host-admission:v2:{}", hasher.finalize().to_hex()))
}

fn deny(code: HostAdmissionDenialCode, summary: &str) -> HostAdmissionDecision {
    HostAdmissionDecision::Deny(HostAdmissionDenial {
        code,
        summary: summary.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bridge_plan::{BridgePlan, BridgePlanApproval, BridgePlanState, RevisionState},
        models::LocalRole,
        storage,
    };

    fn host(value: &str) -> HostRef {
        HostRef::from_device_id(value).unwrap()
    }

    fn paths(label: &str) -> AppPaths {
        let root = std::env::temp_dir().join(format!("{label}-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(root.clone(), root.join("logs"));
        paths.ensure_directories().unwrap();
        paths
    }

    fn persist_approval(
        paths: &AppPaths,
        revision: &crate::bridge_plan::BridgePlanRevision,
        approval_id: &str,
        now: i64,
    ) {
        storage::init_database(paths).unwrap();
        storage::create_room(
            paths,
            &crate::crypto::random_key(),
            "123456",
            5,
            LocalRole::Creator,
            Some(revision.bridge_id.clone()),
            Some(now + 600),
        )
        .unwrap();
        let store = BridgePlanStore::new(paths);
        store
            .create_plan(
                &BridgePlan {
                    plan_id: revision.plan_id.clone(),
                    bridge_id: revision.bridge_id.clone(),
                    requesting_device_ref: revision.requesting_device_ref.clone(),
                    created_at: now,
                },
                BridgePlanState::Draft,
            )
            .unwrap();
        store
            .append_revision(revision, RevisionState::Proposed, now)
            .unwrap();
        store
            .transition_plan(&revision.plan_id, BridgePlanState::Open)
            .unwrap();
        store
            .transition_revision(&revision.revision_id, RevisionState::Available)
            .unwrap();
        store
            .create_approval(
                &BridgePlanApproval {
                    approval_id: approval_id.into(),
                    plan_id: revision.plan_id.clone(),
                    revision_id: revision.revision_id.clone(),
                    revision_hash: revision.revision_hash.clone(),
                    bridge_id: revision.bridge_id.clone(),
                    requester_device_ref: revision.requesting_device_ref.clone(),
                    selected_device_ref: revision.selected_device_ref.clone(),
                    expires_at: now + 600,
                },
                now,
            )
            .unwrap();
    }

    fn fixture() -> (
        AppPaths,
        HostAdmissionService,
        HostAdmissionRequest,
        HostSessionBinding,
        i64,
    ) {
        let now = storage::now_ts();
        let paths = paths("pastey-host-admission");
        let revision = crate::bridge_plan::build_file_search_revision(
            "bridge".into(),
            "requester-session".into(),
            "selected-session".into(),
            "Find a PDF".into(),
            "report.pdf".into(),
            vec!["pdf".into()],
            vec!["documents".into()],
        )
        .unwrap();
        persist_approval(&paths, &revision, "approval", now);
        let local = host("selected-host");
        let peer = host("requester-host");
        let binding = HostSessionBinding::new(
            "bridge",
            local.clone(),
            peer,
            "selected-session",
            "requester-session",
            "peer-route",
            now + 600,
        )
        .unwrap();
        let request = HostAdmissionRequest {
            approval_id: "approval".into(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            host_ref: local.clone(),
            participant_ref: PlanParticipantRef::for_host(&revision.plan_id, &local).unwrap(),
            session_binding: binding.clone(),
        };
        (
            paths,
            HostAdmissionService::new(local),
            request,
            binding,
            now,
        )
    }

    fn denial_code(decision: HostAdmissionDecision) -> HostAdmissionDenialCode {
        match decision {
            HostAdmissionDecision::Deny(denial) => denial.code,
            HostAdmissionDecision::Admit(_) => panic!("expected denial"),
        }
    }

    #[test]
    fn exact_approved_host_work_is_admitted_with_bounded_constraints() {
        let (paths, service, request, current, now) = fixture();
        let decision = service.evaluate(&paths, &request, &current, now).unwrap();
        let admission = decision.admitted().unwrap();
        assert_eq!(admission.work.len(), 1);
        assert_eq!(admission.work[0].operation, StepOperation::Search);
        assert_eq!(
            admission.constraints.allowed_step_ids,
            vec![admission.work[0].step_id.clone()]
        );
        assert!(admission.constraints.requires_current_session);
        assert!(!admission.constraints.modification_authority);
        assert!(admission.admission_ref.starts_with("host-admission:v1:"));
        let encoded = serde_json::to_value(admission).unwrap();
        for forbidden in ["capabilities", "grant", "trusted", "routeable"] {
            assert!(encoded.get(forbidden).is_none());
        }
        let _ = std::fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn approval_plan_and_participant_mismatches_deny_without_authority() {
        let (paths, service, request, current, now) = fixture();

        let mut wrong_approval = request.clone();
        wrong_approval.approval_id = "other-approval".into();
        assert_eq!(
            denial_code(
                service
                    .evaluate(&paths, &wrong_approval, &current, now)
                    .unwrap()
            ),
            HostAdmissionDenialCode::ApprovalUnavailable
        );

        let mut wrong_plan = request.clone();
        wrong_plan.revision_hash = "bridge-plan-revision-hash-v1:wrong".into();
        assert_eq!(
            denial_code(
                service
                    .evaluate(&paths, &wrong_plan, &current, now)
                    .unwrap()
            ),
            HostAdmissionDenialCode::ApprovalMismatch
        );

        let mut wrong_participant = request.clone();
        wrong_participant.participant_ref =
            PlanParticipantRef::for_host(&request.plan_id, &host("other-host")).unwrap();
        assert_eq!(
            denial_code(
                service
                    .evaluate(&paths, &wrong_participant, &current, now)
                    .unwrap()
            ),
            HostAdmissionDenialCode::HostMismatch
        );
        let _ = std::fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn stale_or_reconnected_session_binding_denies_admission() {
        let (paths, service, request, _current, now) = fixture();
        let reconnected = HostSessionBinding::new(
            "bridge",
            request.host_ref.clone(),
            request.session_binding.peer_host_ref.clone(),
            "selected-session",
            "new-requester-session",
            "new-peer-route",
            now + 600,
        )
        .unwrap();
        assert_eq!(
            denial_code(
                service
                    .evaluate(&paths, &request, &reconnected, now)
                    .unwrap()
            ),
            HostAdmissionDenialCode::SessionMismatch
        );
        let _ = std::fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn host_ref_and_object_presence_cannot_replace_host_bound_work() {
        let now = storage::now_ts();
        let paths = paths("pastey-host-admission-no-work");
        let revision = crate::bridge_plan::build_direct_file_transfer_revision(
            "bridge".into(),
            "requester-session".into(),
            "selected-session".into(),
            "Send a file".into(),
        )
        .unwrap();
        persist_approval(&paths, &revision, "approval", now);
        let local = host("selected-host");
        let binding = HostSessionBinding::new(
            "bridge",
            local.clone(),
            host("requester-host"),
            "selected-session",
            "requester-session",
            "peer-route",
            now + 600,
        )
        .unwrap();
        let request = HostAdmissionRequest {
            approval_id: "approval".into(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            host_ref: local.clone(),
            participant_ref: PlanParticipantRef::for_host(&revision.plan_id, &local).unwrap(),
            session_binding: binding.clone(),
        };
        assert_eq!(
            denial_code(
                HostAdmissionService::new(local)
                    .evaluate(&paths, &request, &binding, now)
                    .unwrap()
            ),
            HostAdmissionDenialCode::NoHostBoundWork
        );
        let _ = std::fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn framework_only_transform_work_is_denied_before_any_execution_authority() {
        let now = storage::now_ts();
        let paths = paths("pastey-host-admission-transform-denied");
        let revision = crate::bridge_plan::build_composed_file_revision(
            "bridge".into(),
            "requester-session".into(),
            "selected-session".into(),
            "Find and modify a file".into(),
            vec![
                crate::bridge_plan::ComposedFilePlanBlock::Search {
                    execution_device_ref: "selected-session".into(),
                    filename_hint: "example.txt".into(),
                    extensions: vec!["txt".into()],
                    safe_scope_labels: vec!["documents".into()],
                },
                crate::bridge_plan::ComposedFilePlanBlock::Transform {
                    execution_device_ref: "selected-session".into(),
                    target_revision: crate::bridge_plan::LogicalObjectRevision {
                        logical_object_id: "selected_file".into(),
                        revision: 1,
                    },
                    modification_intent: "Apply the reviewed change".into(),
                },
            ],
        )
        .unwrap();
        persist_approval(&paths, &revision, "approval", now);
        let local = host("selected-host");
        let binding = HostSessionBinding::new(
            "bridge",
            local.clone(),
            host("requester-host"),
            "selected-session",
            "requester-session",
            "peer-route",
            now + 600,
        )
        .unwrap();
        let request = HostAdmissionRequest {
            approval_id: "approval".into(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            host_ref: local.clone(),
            participant_ref: PlanParticipantRef::for_host(&revision.plan_id, &local).unwrap(),
            session_binding: binding.clone(),
        };
        assert_eq!(
            denial_code(
                HostAdmissionService::new(local)
                    .evaluate(&paths, &request, &binding, now)
                    .unwrap()
            ),
            HostAdmissionDenialCode::UnsupportedOperation
        );
        let _ = std::fs::remove_dir_all(paths.app_data_dir);
    }
}

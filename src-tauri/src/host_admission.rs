//! Host-local admission for exact approved managed work.
//!
//! Admission is deliberately downstream of requester approval and current
//! session resolution, but upstream of any attempt/step grant or effect. It
//! does not make Layer 4 identity, liveness, capability facts, or object
//! presence authoritative.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    bridge_plan::StepOperation,
    bridge_plan_v2::{
        self, participant_for_ref, requester_host, PlanApprovalV2, PlanRevisionV2, PlanStepV2,
    },
    error::AppResult,
    host_identity::{HostExecutionFreshness, HostRef, PlanParticipantRef},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostAdmissionRequestV2 {
    pub attempt_id: String,
    pub approval_id: String,
    pub plan_id: String,
    pub revision_id: String,
    pub revision_hash: String,
    pub host_ref: HostRef,
    pub participant_ref: PlanParticipantRef,
    pub protocol_correlation_id: String,
    pub execution_freshness: HostExecutionFreshness,
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
    codex_transform_hosts: BTreeSet<HostRef>,
    execute_hosts: BTreeSet<HostRef>,
}

impl ManagedPrimitiveAvailabilityV1 {
    pub(crate) fn unavailable() -> Self {
        Self::default()
    }

    pub(crate) fn verified_attachment(host_ref: HostRef, transform: bool, execute: bool) -> Self {
        Self::verified_attachment_with_codex(host_ref, transform, false, execute)
    }

    /// Host-private readiness result for the one supported specialist. This
    /// is deliberately a fixed field, not a backend registry.
    pub(crate) fn verified_attachment_with_codex(
        host_ref: HostRef,
        transform: bool,
        codex_transform: bool,
        execute: bool,
    ) -> Self {
        Self {
            transform_hosts: transform.then_some(host_ref.clone()).into_iter().collect(),
            codex_transform_hosts: codex_transform
                .then_some(host_ref.clone())
                .into_iter()
                .collect(),
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
            StepOperation::Transform => {
                if step.requires_codex_specialist() {
                    self.codex_transform_hosts.contains(host_ref)
                } else {
                    self.transform_hosts.contains(host_ref)
                }
            }
            StepOperation::Execute => self.execute_hosts.contains(host_ref),
        }
    }
}

impl HostAdmissionService {
    pub fn new(local_host_ref: HostRef) -> Self {
        Self { local_host_ref }
    }

    /// Step 8 Core attachment. The availability value is process-local and
    /// must be derived from verified Host enforcement, never protocol input.
    pub(crate) fn evaluate_v2_with_availability(
        &self,
        revision: &PlanRevisionV2,
        approval: &PlanApprovalV2,
        request: &HostAdmissionRequestV2,
        current_freshness: impl Into<HostExecutionFreshness>,
        availability: ManagedPrimitiveAvailabilityV1,
        now: i64,
    ) -> AppResult<HostAdmissionDecision> {
        let current_freshness = current_freshness.into();
        if request.host_ref != self.local_host_ref
            || request.execution_freshness.local_host_ref() != &self.local_host_ref
            || current_freshness.local_host_ref() != &self.local_host_ref
        {
            return Ok(deny(
                HostAdmissionDenialCode::HostMismatch,
                "The v2 work is not bound to this Host.",
            ));
        }
        if request
            .execution_freshness
            .validate_current(&current_freshness, now)
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
            || revision.bridge_id != request.execution_freshness.bridge_id()
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
        let requester_host = requester_host(revision)?;
        match &request.execution_freshness {
            HostExecutionFreshness::Local(_) => {
                if &request.participant_ref != &revision.requester
                    || requester_host != &self.local_host_ref
                {
                    return Ok(deny(
                        HostAdmissionDenialCode::SessionMismatch,
                        "Local admission is not bound to the exact requester participant.",
                    ));
                }
            }
            HostExecutionFreshness::Remote(binding) => {
                if requester_host != &binding.peer_host_ref
                    || request.participant_ref == revision.requester
                {
                    return Ok(deny(
                        HostAdmissionDenialCode::SessionMismatch,
                        "The current Layer 4 peer is not the approved v2 requester Host.",
                    ));
                }
            }
        }
        if request.attempt_id.trim().is_empty() || request.protocol_correlation_id.trim().is_empty()
        {
            return Ok(deny(
                HostAdmissionDenialCode::ApprovalMismatch,
                "The v2 attempt or protocol correlation is unavailable.",
            ));
        }

        // The sealed revision above is the whole-Plan semantic authority. This
        // process-local snapshot proves availability only for work admitted here.
        let local_steps = revision
            .steps
            .iter()
            .filter(|step| step.binds_participant(&request.participant_ref))
            .collect::<Vec<_>>();
        if local_steps.is_empty() {
            return Ok(deny(
                HostAdmissionDenialCode::NoHostBoundWork,
                "The v2 Plan contains no work bound to this Host.",
            ));
        }
        if local_steps
            .iter()
            .any(|step| !availability.supports(revision, step))
        {
            return Ok(deny(
                HostAdmissionDenialCode::UnsupportedOperation,
                "This HostRuntime cannot safely provide a primitive required by its v2 Plan fragment.",
            ));
        }
        let work = local_steps
            .into_iter()
            .map(admitted_work_v2)
            .collect::<AppResult<Vec<_>>>()?;
        let expires_at = approval
            .expires_at
            .min(request.execution_freshness.expires_at());
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
            // Compatibility field in the frozen admission/effect contract. For
            // local work it contains the opaque LocalRuntime freshness authority
            // ref; remote work continues to contain HostSessionBinding.binding_ref.
            session_binding_ref: request.execution_freshness.authority_ref().to_string(),
            work,
            constraints,
        })))
    }
}

fn admitted_work_v2(step: &PlanStepV2) -> AppResult<AdmittedHostWork> {
    let semantic = serde_json::to_vec(step)?;
    Ok(AdmittedHostWork {
        step_id: step.id().to_string(),
        operation: step.operation(),
        semantic_digest: format!("host-work:v2:{}", blake3::hash(&semantic).to_hex()),
    })
}

fn admission_ref_v2(
    request: &HostAdmissionRequestV2,
    work: &[AdmittedHostWork],
    constraints: &HostAdmissionConstraints,
) -> AppResult<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pastey-host-admission-v2-attempt-bound-v1\0");
    for value in [
        request.attempt_id.as_str(),
        request.approval_id.as_str(),
        request.plan_id.as_str(),
        request.revision_id.as_str(),
        request.revision_hash.as_str(),
        request.host_ref.as_str(),
        request.participant_ref.as_str(),
        request.protocol_correlation_id.as_str(),
        request.execution_freshness.authority_ref(),
    ] {
        hasher.update(value.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(&serde_json::to_vec(work)?);
    hasher.update(&serde_json::to_vec(constraints)?);
    Ok(format!(
        "host-admission:v2-attempt-bound:{}",
        hasher.finalize().to_hex()
    ))
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
        bridge_plan_v2::{seal_revision, ManagedObjectRevisionV2, PlanRootV2, PLAN_SCHEMA_VERSION},
        host_identity::{HostSessionBinding, PlanParticipants},
    };

    fn host(value: &str) -> HostRef {
        HostRef::from_device_id(value).unwrap()
    }

    fn denial_code(decision: HostAdmissionDecision) -> HostAdmissionDenialCode {
        match decision {
            HostAdmissionDecision::Deny(denial) => denial.code,
            HostAdmissionDecision::Admit(_) => panic!("expected denial"),
        }
    }

    struct V2AdmissionFixture {
        revision: PlanRevisionV2,
        approval: PlanApprovalV2,
        transform_host: HostRef,
        execute_host: HostRef,
        execute_participant: PlanParticipantRef,
        transform_binding: HostSessionBinding,
        execute_binding: HostSessionBinding,
        transform_request: HostAdmissionRequestV2,
        execute_request: HostAdmissionRequestV2,
        now: i64,
    }

    fn v2_admission_fixture(search_transfer_only: bool) -> V2AdmissionFixture {
        let now = 10_000;
        let plan_id = if search_transfer_only {
            "plan-v2-search-transfer"
        } else {
            "plan-v2-heterogeneous"
        };
        let requester_host = host("requester-v2");
        let transform_host = host("transform-v2");
        let execute_host = host("execute-v2");
        let participants = PlanParticipants::new(
            plan_id,
            [
                requester_host.clone(),
                transform_host.clone(),
                execute_host.clone(),
            ],
        )
        .unwrap();
        let requester = PlanParticipantRef::for_host(plan_id, &requester_host).unwrap();
        let transform_participant = PlanParticipantRef::for_host(plan_id, &transform_host).unwrap();
        let execute_participant = PlanParticipantRef::for_host(plan_id, &execute_host).unwrap();
        let input = ManagedObjectRevisionV2 {
            logical_object_id: "managed-project".into(),
            revision: 1,
        };
        let output = ManagedObjectRevisionV2 {
            logical_object_id: input.logical_object_id.clone(),
            revision: 2,
        };
        let (roots, steps) = if search_transfer_only {
            (
                Vec::new(),
                vec![
                    PlanStepV2::Search {
                        step_id: "search-b".into(),
                        depends_on: Vec::new(),
                        host: transform_participant.clone(),
                        output: input.clone(),
                        query: "project.txt".into(),
                        safe_scope_labels: vec!["documents".into()],
                    },
                    PlanStepV2::Transfer {
                        step_id: "transfer-b-c".into(),
                        depends_on: vec!["search-b".into()],
                        source: transform_participant.clone(),
                        destination: execute_participant.clone(),
                        input: input.clone(),
                        output: input.clone(),
                    },
                ],
            )
        } else {
            (
                vec![PlanRootV2 {
                    root_id: "project-root".into(),
                    object: input.clone(),
                    host: transform_participant.clone(),
                }],
                vec![
                    PlanStepV2::Transform {
                        step_id: "transform-b".into(),
                        depends_on: Vec::new(),
                        host: transform_participant.clone(),
                        input: input.clone(),
                        output: output.clone(),
                        modification_intent: "Apply the reviewed change.".into(),
                        worker_capability_requirement: None,
                    },
                    PlanStepV2::Transfer {
                        step_id: "transfer-b-c".into(),
                        depends_on: vec!["transform-b".into()],
                        source: transform_participant.clone(),
                        destination: execute_participant.clone(),
                        input: output.clone(),
                        output: output.clone(),
                    },
                    PlanStepV2::Execute {
                        step_id: "execute-c".into(),
                        depends_on: vec!["transfer-b-c".into()],
                        host: execute_participant.clone(),
                        target: output,
                        execution_intent: "Run the reviewed validation.".into(),
                    },
                ],
            )
        };
        let revision = seal_revision(PlanRevisionV2 {
            schema_version: PLAN_SCHEMA_VERSION.into(),
            plan_id: plan_id.into(),
            revision_id: format!("revision-{plan_id}"),
            revision_number: 1,
            revision_hash: String::new(),
            bridge_id: "bridge-v2-admission".into(),
            requester: requester.clone(),
            participants,
            roots,
            original_user_goal: "Use exact authored Host placement.".into(),
            expected_outcome: "Each Host admits only its exact fragment.".into(),
            steps,
        })
        .unwrap();
        let approval = PlanApprovalV2 {
            approval_id: format!("approval-{plan_id}"),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: revision.bridge_id.clone(),
            requester,
            expires_at: now + 600,
        };
        let transform_binding = HostSessionBinding::new(
            &revision.bridge_id,
            transform_host.clone(),
            requester_host.clone(),
            "transform-session",
            "requester-transform-session",
            "requester-transform-route",
            now + 600,
        )
        .unwrap();
        let execute_binding = HostSessionBinding::new(
            &revision.bridge_id,
            execute_host.clone(),
            requester_host.clone(),
            "execute-session",
            "requester-execute-session",
            "requester-execute-route",
            now + 600,
        )
        .unwrap();
        let transform_request = HostAdmissionRequestV2 {
            attempt_id: "attempt-transform".into(),
            approval_id: approval.approval_id.clone(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            host_ref: transform_host.clone(),
            participant_ref: transform_participant.clone(),
            protocol_correlation_id: "correlation-transform".into(),
            execution_freshness: transform_binding.clone().into(),
        };
        let execute_request = HostAdmissionRequestV2 {
            attempt_id: "attempt-execute".into(),
            approval_id: approval.approval_id.clone(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            host_ref: execute_host.clone(),
            participant_ref: execute_participant.clone(),
            protocol_correlation_id: "correlation-execute".into(),
            execution_freshness: execute_binding.clone().into(),
        };
        V2AdmissionFixture {
            revision,
            approval,
            transform_host,
            execute_host,
            execute_participant,
            transform_binding,
            execute_binding,
            transform_request,
            execute_request,
            now,
        }
    }

    #[test]
    fn heterogeneous_v2_hosts_admit_only_their_exact_managed_fragments() {
        let fixture = v2_admission_fixture(false);
        let transform_decision = HostAdmissionService::new(fixture.transform_host.clone())
            .evaluate_v2_with_availability(
                &fixture.revision,
                &fixture.approval,
                &fixture.transform_request,
                &fixture.transform_binding,
                ManagedPrimitiveAvailabilityV1::verified_attachment(
                    fixture.transform_host.clone(),
                    true,
                    false,
                ),
                fixture.now,
            )
            .unwrap();
        let transform_admission = transform_decision.admitted().unwrap();
        assert_eq!(
            transform_admission
                .work
                .iter()
                .map(|item| item.operation.clone())
                .collect::<Vec<_>>(),
            vec![StepOperation::Transform, StepOperation::Transfer]
        );

        let execute_decision = HostAdmissionService::new(fixture.execute_host.clone())
            .evaluate_v2_with_availability(
                &fixture.revision,
                &fixture.approval,
                &fixture.execute_request,
                &fixture.execute_binding,
                ManagedPrimitiveAvailabilityV1::verified_attachment(
                    fixture.execute_host.clone(),
                    false,
                    true,
                ),
                fixture.now,
            )
            .unwrap();
        let execute_admission = execute_decision.admitted().unwrap();
        assert_eq!(
            execute_admission
                .work
                .iter()
                .map(|item| item.operation.clone())
                .collect::<Vec<_>>(),
            vec![StepOperation::Transfer, StepOperation::Execute]
        );
    }

    #[test]
    fn v2_transform_host_rejects_missing_local_transform_availability() {
        let fixture = v2_admission_fixture(false);
        let decision = HostAdmissionService::new(fixture.transform_host.clone())
            .evaluate_v2_with_availability(
                &fixture.revision,
                &fixture.approval,
                &fixture.transform_request,
                &fixture.transform_binding,
                ManagedPrimitiveAvailabilityV1::verified_attachment(
                    fixture.transform_host,
                    false,
                    true,
                ),
                fixture.now,
            )
            .unwrap();
        assert_eq!(
            denial_code(decision),
            HostAdmissionDenialCode::UnsupportedOperation
        );
    }

    #[test]
    fn v2_execute_host_rejects_missing_local_execute_availability() {
        let fixture = v2_admission_fixture(false);
        let decision = HostAdmissionService::new(fixture.execute_host.clone())
            .evaluate_v2_with_availability(
                &fixture.revision,
                &fixture.approval,
                &fixture.execute_request,
                &fixture.execute_binding,
                ManagedPrimitiveAvailabilityV1::verified_attachment(
                    fixture.execute_host,
                    true,
                    false,
                ),
                fixture.now,
            )
            .unwrap();
        assert_eq!(
            denial_code(decision),
            HostAdmissionDenialCode::UnsupportedOperation
        );
    }

    #[test]
    fn another_hosts_v2_availability_cannot_satisfy_local_work() {
        let fixture = v2_admission_fixture(false);
        let decision = HostAdmissionService::new(fixture.transform_host.clone())
            .evaluate_v2_with_availability(
                &fixture.revision,
                &fixture.approval,
                &fixture.transform_request,
                &fixture.transform_binding,
                ManagedPrimitiveAvailabilityV1::verified_attachment(
                    fixture.execute_host,
                    true,
                    true,
                ),
                fixture.now,
            )
            .unwrap();
        assert_eq!(
            denial_code(decision),
            HostAdmissionDenialCode::UnsupportedOperation
        );
    }

    #[test]
    fn v2_host_cannot_request_admission_for_another_hosts_participant() {
        let fixture = v2_admission_fixture(false);
        let mut substituted = fixture.transform_request.clone();
        substituted.participant_ref = fixture.execute_participant;
        let decision = HostAdmissionService::new(fixture.transform_host.clone())
            .evaluate_v2_with_availability(
                &fixture.revision,
                &fixture.approval,
                &substituted,
                &fixture.transform_binding,
                ManagedPrimitiveAvailabilityV1::verified_attachment(
                    fixture.transform_host,
                    true,
                    true,
                ),
                fixture.now,
            )
            .unwrap();
        assert_eq!(denial_code(decision), HostAdmissionDenialCode::HostMismatch);
    }

    #[test]
    fn v2_search_transfer_fragment_needs_no_managed_primitive_availability() {
        let fixture = v2_admission_fixture(true);
        let decision = HostAdmissionService::new(fixture.transform_host)
            .evaluate_v2_with_availability(
                &fixture.revision,
                &fixture.approval,
                &fixture.transform_request,
                &fixture.transform_binding,
                ManagedPrimitiveAvailabilityV1::unavailable(),
                fixture.now,
            )
            .unwrap();
        let admission = decision.admitted().unwrap();
        assert_eq!(
            admission
                .work
                .iter()
                .map(|item| item.operation.clone())
                .collect::<Vec<_>>(),
            vec![StepOperation::Search, StepOperation::Transfer]
        );
        assert!(!admission.constraints.modification_authority);
    }

    #[test]
    fn v2_admission_reference_is_attempt_bound() {
        let fixture = v2_admission_fixture(false);
        let service = HostAdmissionService::new(fixture.transform_host.clone());
        let first = service
            .evaluate_v2_with_availability(
                &fixture.revision,
                &fixture.approval,
                &fixture.transform_request,
                &fixture.transform_binding,
                ManagedPrimitiveAvailabilityV1::verified_attachment(
                    fixture.transform_host.clone(),
                    true,
                    true,
                ),
                fixture.now,
            )
            .unwrap();
        let mut substituted = fixture.transform_request.clone();
        substituted.attempt_id = "attempt-transform-substituted".into();
        let second = service
            .evaluate_v2_with_availability(
                &fixture.revision,
                &fixture.approval,
                &substituted,
                &fixture.transform_binding,
                ManagedPrimitiveAvailabilityV1::verified_attachment(
                    fixture.transform_host,
                    true,
                    true,
                ),
                fixture.now,
            )
            .unwrap();

        let first = first.admitted().unwrap();
        let second = second.admitted().unwrap();
        assert!(first
            .admission_ref
            .starts_with("host-admission:v2-attempt-bound:"));
        assert_ne!(first.admission_ref, second.admission_ref);
    }
}
